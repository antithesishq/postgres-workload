use std::collections::HashMap;

use serde_json::json;

use crate::utils::{Ctx, DbPools, RwMode, disable_logging, max_retries, rw_mode};

use super::query_gen;
use super::schema_gen::{
    collect_samples, create_table_ddl, generate_indexes, generate_tables, seed_table_sql,
};
use super::{equivalent_queries, query_to_sql};

/// Check if a query is valid by running EXPLAIN. Uses the provided client
/// (no second connection).
fn validate_query(client: &mut postgres::Client, sql: &str) -> Option<bool> {
    let explain_sql = format!("EXPLAIN {sql}");
    match client.query(&explain_sql, &[]) {
        Ok(_) => Some(true),
        Err(e) if e.code().is_some() => Some(false),
        Err(_) => None,
    }
}

fn hash_query_sql(sql: &str, n_cols: usize) -> String {
    let ordinals: Vec<String> = (1..=n_cols).map(|i| i.to_string()).collect();
    let order_by = ordinals.join(", ");
    format!(
        "SELECT md5(COALESCE(string_agg(r::text, E'\\n'), '')) \
         FROM (SELECT * FROM ({sql}) AS _inner ORDER BY {order_by}) r"
    )
}

fn hash_query_result(client: &mut postgres::Client, sql: &str, n_cols: usize) -> Option<String> {
    let hash_sql = hash_query_sql(sql, n_cols);
    match client.query_one(&hash_sql, &[]) {
        Ok(row) => {
            let hash: String = row.get(0);
            Some(hash)
        }
        Err(e) => {
            println!("egglog: error hashing query result: {e:?}\n  query: {sql}");
            None
        }
    }
}

fn count_query_result(client: &mut postgres::Client, sql: &str) -> Option<i64> {
    let count_sql = format!("SELECT COUNT(*) FROM ({sql}) AS _cnt");
    match client.query_one(&count_sql, &[]) {
        Ok(row) => {
            let count: i64 = row.get(0);
            Some(count)
        }
        Err(e) => {
            println!("egglog: error counting query result: {e:?}\n  query: {sql}");
            None
        }
    }
}

fn discover_egglog_schema(
    client: &mut postgres::Client,
) -> Option<Vec<super::schema_gen::TableMeta>> {
    let table_rows = client
        .query(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = current_schema() AND table_name LIKE 'egglog_%' \
             ORDER BY table_name",
            &[],
        )
        .ok()?;

    if table_rows.is_empty() {
        return None;
    }

    let mut tables = Vec::new();
    for trow in &table_rows {
        let tname: String = trow.get(0);

        let col_rows = client
            .query(
                "SELECT column_name, data_type, is_nullable \
                 FROM information_schema.columns \
                 WHERE table_schema = current_schema() AND table_name = $1 \
                 ORDER BY ordinal_position",
                &[&tname],
            )
            .ok()?;

        // Get primary key columns
        let pk_rows = client
            .query(
                "SELECT kcu.column_name \
                 FROM information_schema.table_constraints tc \
                 JOIN information_schema.key_column_usage kcu \
                   ON tc.constraint_name = kcu.constraint_name \
                  AND tc.table_schema = kcu.table_schema \
                 WHERE tc.table_schema = current_schema() \
                   AND tc.table_name = $1 \
                   AND tc.constraint_type = 'PRIMARY KEY'",
                &[&tname],
            )
            .ok()?;
        let pk_cols: Vec<String> = pk_rows.iter().map(|r| r.get(0)).collect();

        let mut columns = Vec::new();
        for crow in &col_rows {
            let col_name: String = crow.get(0);
            let data_type: String = crow.get(1);
            let is_nullable: String = crow.get(2);

            let pg_type = match data_type.as_str() {
                "bigint" => super::schema_gen::PgType::Int8,
                "text" | "character varying" => super::schema_gen::PgType::Text,
                "boolean" => super::schema_gen::PgType::Boolean,
                other => {
                    println!(
                        "antigres [egglog READ] unknown column type '{other}' in {tname}.{col_name}, skipping table"
                    );
                    columns.clear();
                    break;
                }
            };

            columns.push(super::schema_gen::ColumnMeta {
                name: col_name.clone(),
                pg_type,
                is_primary_key: pk_cols.contains(&col_name),
                is_not_null: is_nullable == "NO",
                is_unique: false,
                has_default: None,
                has_check: None,
                foreign_key: None,
            });
        }

        if columns.is_empty() {
            continue;
        }

        tables.push(super::schema_gen::TableMeta {
            name: tname,
            columns,
        });
    }

    if tables.is_empty() {
        None
    } else {
        Some(tables)
    }
}

pub fn setup(pools: &DbPools, ctx: &mut Ctx) {
    if rw_mode() == RwMode::Read {
        println!("antigres [egglog] READ mode: discovering schema from existing tables...");
        loop {
            if let Some(mut client) = pools.get_connection() {
                if let Some(tables) = discover_egglog_schema(&mut client) {
                    // Verify at least one table has data
                    let has_data = tables.iter().any(|t| {
                        client
                            .query_one(&format!("SELECT COUNT(*) > 0 FROM {}", t.name), &[])
                            .map(|row| row.get::<_, bool>(0))
                            .unwrap_or(false)
                    });

                    if has_data {
                        println!(
                            "antigres [egglog] READ mode: discovered {} tables with data",
                            tables.len()
                        );
                        let mut all_samples = HashMap::new();
                        for table in &tables {
                            let table_samples = collect_samples(&mut client, table);
                            all_samples.extend(table_samples);
                        }
                        ctx.egglog_tables = Some(std::sync::Arc::new(tables));
                        ctx.egglog_samples = Some(std::sync::Arc::new(all_samples));
                        return;
                    }
                }
            }
            println!("antigres [egglog] READ mode: waiting for egglog tables with data...");
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
    }

    let start = std::time::Instant::now();

    for attempt in 1..=max_retries() {
        println!(
            "antigres [egglog] setup: attempt {attempt}/{} (elapsed: {:?})",
            max_retries(),
            start.elapsed()
        );

        let mut client = match pools.get_connection() {
            Some(c) => c,
            None => {
                println!("antigres [egglog] setup: failed to get connection, giving up");
                return;
            }
        };

        disable_logging(&mut client);

        println!("antigres [egglog] setup: generating random table schemas...");
        let tables = generate_tables(&[]);
        let table_names: Vec<String> = tables.iter().map(|t| t.name.clone()).collect();
        println!(
            "antigres [egglog] setup: generated {} tables: {table_names:?}",
            tables.len()
        );

        let mut ok = true;
        for table in &tables {
            let ddl =
                create_table_ddl(table).replace("CREATE TABLE ", "CREATE TABLE IF NOT EXISTS ");
            if let Err(e) = client.execute(ddl.as_str(), &[]) {
                println!(
                    "antigres [egglog] setup: error creating table {}: {e:?}",
                    table.name
                );
                ok = false;
                break;
            }
            println!("antigres [egglog] setup: created table '{}'", table.name);

            for idx_sql in generate_indexes(table) {
                let idx_sql = idx_sql.replace("CREATE INDEX ", "CREATE INDEX IF NOT EXISTS ");
                if let Err(e) = client.execute(idx_sql.as_str(), &[]) {
                    println!("antigres [egglog] setup: error creating index: {e:?}");
                }
            }
        }

        if !ok {
            continue;
        }

        println!("antigres [egglog] setup: seeding tables...");
        let mut all_pk_values: HashMap<String, Vec<String>> = HashMap::new();
        let mut total_inserted = 0u64;

        for table in &tables {
            let (insert_sql, _n_rows) = seed_table_sql(table, &all_pk_values);
            let safe_sql = format!("{insert_sql} ON CONFLICT DO NOTHING");
            match client.execute(safe_sql.as_str(), &[]) {
                Ok(n) => {
                    total_inserted += n;
                    println!(
                        "antigres [egglog] setup: seeded '{}' with {n} rows",
                        table.name
                    );
                }
                Err(e) => {
                    println!(
                        "antigres [egglog] setup: error seeding {}: {e:?} (continuing)",
                        table.name
                    );
                }
            }

            if let Some(pk) = table.pk_column() {
                let sql = format!("SELECT {}::text FROM {}", pk.name, table.name);
                if let Ok(rows) = client.query(&sql, &[]) {
                    let vals: Vec<String> = rows
                        .iter()
                        .map(|row| {
                            let raw: String = row.get(0);
                            pk.pg_type.sql_literal(&raw)
                        })
                        .collect();
                    let key = format!("{}.{}", table.name, pk.name);
                    all_pk_values.insert(key, vals);
                }
            }
        }

        if total_inserted < 5 {
            println!(
                "antigres [egglog] setup: only {total_inserted} rows inserted, need at least 5, retrying"
            );
            continue;
        }

        println!(
            "antigres [egglog] setup: created {} tables ({}) with {total_inserted} total rows",
            tables.len(),
            table_names.join(", ")
        );

        println!("antigres [egglog] setup: collecting sample values...");
        let mut all_samples = HashMap::new();
        for table in &tables {
            let table_samples = collect_samples(&mut client, table);
            all_samples.extend(table_samples);
        }

        ctx.egglog_tables = Some(std::sync::Arc::new(tables));
        ctx.egglog_samples = Some(std::sync::Arc::new(all_samples));

        println!(
            "antigres [egglog] setup: completed successfully (elapsed: {:?})",
            start.elapsed()
        );
        antithesis_sdk::assert_reachable!(
            "Antithesis Postgres workload (egglog): setup completed successfully",
            &json!({"tables": table_names, "total_inserted": total_inserted})
        );
        return;
    }
    println!(
        "antigres [egglog] setup: FAILED after {} retries (elapsed: {:?})",
        max_retries(),
        start.elapsed()
    );
}

pub fn parallel_action(pools: &DbPools, ctx: &Ctx) {
    let tables_arc = match &ctx.egglog_tables {
        Some(t) => t,
        None => {
            println!("egglog parallel_action: TABLES not initialized");
            return;
        }
    };
    let samples_arc = match &ctx.egglog_samples {
        Some(s) => s,
        None => {
            println!("egglog parallel_action: SAMPLES not initialized");
            return;
        }
    };
    let tables = tables_arc.as_ref();
    let samples = samples_arc.as_ref();

    if tables.is_empty() {
        return;
    }

    antithesis_sdk::assert_reachable!(
        "Antithesis Postgres workload (egglog): started parallel driver",
        &json!({})
    );

    if antithesis_sdk::random::get_random() % 250 < 5 {
        if let Some(mut client) = pools.get_connection() {
            disable_logging(&mut client);
        }
    }

    let mut client = match pools.get_connection() {
        Some(c) => c,
        None => return,
    };

    let mut query = None;
    let mut original_sql = String::new();
    let mut original_hash = String::new();
    let mut n_cols = 0usize;

    for _ in 0..50 {
        let q = if antithesis_sdk::random::get_random() % 100 < 30 {
            query_gen::generate_random_aggregate_query(tables, samples)
        } else {
            query_gen::generate_random_query(tables, samples)
        };
        let schema = q.compute_schema();
        n_cols = schema.fields().len();
        let sql = query_to_sql(&q);

        match validate_query(&mut client, &sql) {
            Some(true) => {}
            Some(false) => continue,
            None => {
                // Connection-level error; get a fresh connection and skip this query
                client = match pools.get_connection() {
                    Some(c) => c,
                    None => return,
                };
                continue;
            }
        }

        let count = match count_query_result(&mut client, &sql) {
            Some(c) => c,
            None => {
                client = match pools.get_connection() {
                    Some(c) => c,
                    None => return,
                };
                continue;
            }
        };

        if count == 0 {
            continue;
        }

        let hash = match hash_query_result(&mut client, &sql, n_cols) {
            Some(h) => h,
            None => {
                client = match pools.get_connection() {
                    Some(c) => c,
                    None => return,
                };
                continue;
            }
        };

        original_sql = sql;
        original_hash = hash;
        query = Some(q);
        break;
    }

    let query = match query {
        Some(q) => q,
        None => {
            antithesis_sdk::assert_unreachable!(
                "Antithesis Postgres workload (egglog): no non-empty query found after 50 attempts",
                &json!({})
            );
            return;
        }
    };

    antithesis_sdk::assert_reachable!(
        "Antithesis Postgres workload (egglog): found non-empty query",
        &json!({"sql": original_sql, "hash": original_hash})
    );

    let variants = match equivalent_queries(&query, 5) {
        Ok(v) => v,
        Err(e) => {
            println!("egglog: error generating variants: {e}");
            return;
        }
    };

    antithesis_sdk::assert_reachable!(
        "Antithesis Postgres workload (egglog): generated equivalent variants",
        &json!({"n_variants": variants.len(), "original_sql": original_sql})
    );

    for variant in &variants {
        let variant_sql = query_to_sql(variant);

        // Validate the variant SQL is executable before hashing.
        // E-graph extraction can produce queries that reference out-of-scope
        // columns (e.g. columns from an Aggregate's input in a HAVING position).
        match validate_query(&mut client, &variant_sql) {
            Some(true) => {}
            Some(false) => continue,
            None => {
                client = match pools.get_connection() {
                    Some(c) => c,
                    None => return,
                };
                continue;
            }
        }

        let variant_hash = match hash_query_result(&mut client, &variant_sql, n_cols) {
            Some(h) => h,
            None => {
                client = match pools.get_connection() {
                    Some(c) => c,
                    None => return,
                };
                match hash_query_result(&mut client, &variant_sql, n_cols) {
                    Some(h) => h,
                    None => continue,
                }
            }
        };

        antithesis_sdk::assert_always!(
            original_hash == variant_hash,
            "Antithesis Postgres workload (egglog): equivalent queries must return identical results (transformed: {variant_sql})",
            &json!({
                "original_sql": original_sql,
                "variant_sql": variant_sql,
                "original_hash": original_hash,
                "variant_hash": variant_hash
            })
        );
    }

    ctx.record_op("egglog", "query_equivalence");
    antithesis_sdk::assert_reachable!(
        "Antithesis Postgres workload (egglog): completed parallel driver",
        &json!({"n_variants": variants.len()})
    );
}
