use crate::utils::{
    Ctx, DbPools, RwMode, disable_logging, err_is_serialization_failure, max_retries,
    max_serialization_retries, rw_mode, wait_for_table,
};
use serde_json::json;
use uuid::Uuid;
use xxhash_rust::xxh32::xxh32;

/// Compute a deterministic 32-bit hash of a text string, stored as i64.
fn hash_text(data: &str) -> i64 {
    xxh32(data.as_bytes(), 0) as i64
}

fn data1_table_name() -> String {
    std::env::var("CKSUM_DATA1_TABLE_NAME").unwrap_or_else(|_| "cksum_data1".to_string())
}

fn data2_table_name() -> String {
    std::env::var("CKSUM_DATA2_TABLE_NAME").unwrap_or_else(|_| "cksum_data2".to_string())
}

fn data3_table_name() -> String {
    std::env::var("CKSUM_DATA3_TABLE_NAME").unwrap_or_else(|_| "cksum_data3".to_string())
}

fn cksums_table_name() -> String {
    std::env::var("CKSUMS_TABLE_NAME").unwrap_or_else(|_| "cksums".to_string())
}

fn data_table_names() -> [String; 3] {
    [data1_table_name(), data2_table_name(), data3_table_name()]
}

const SEED_ROWS: i64 = 20;

pub fn setup(pools: &DbPools) {
    if rw_mode() == RwMode::Read {
        wait_for_table(pools, &cksums_table_name());
        return;
    }

    let start = std::time::Instant::now();
    let cksums = cksums_table_name();
    let tables = data_table_names();
    let mut attempt = 0u64;

    loop {
        attempt += 1;
        println!(
            "antigres [cksum] setup: attempt {attempt} (elapsed: {:?})",
            start.elapsed()
        );

        let mut client = match pools.get_connection() {
            Some(c) => c,
            None => {
                println!("antigres [cksum] setup: failed to get connection, giving up");
                return;
            }
        };

        let mut tx = match client
            .build_transaction()
            .isolation_level(postgres::IsolationLevel::RepeatableRead)
            .start()
        {
            Ok(tx) => tx,
            Err(e) => {
                println!("antigres [cksum] setup: error starting txn: {e:?}");
                continue;
            }
        };

        let table_exists: bool = match tx.query_one(
            r#"
            SELECT EXISTS (
               SELECT FROM information_schema.tables
               WHERE table_schema = current_schema()
                 AND table_name = $1
            )
            "#,
            &[&cksums],
        ) {
            Ok(row) => row.get(0),
            Err(e) => {
                println!("antigres [cksum] setup: error checking table existence: {e:?}");
                false
            }
        };

        if table_exists {
            println!(
                "antigres [cksum] setup: table '{cksums}' already exists, done (elapsed: {:?})",
                start.elapsed()
            );
            return;
        }

        println!(
            "antigres [cksum] setup: creating data tables {:?}...",
            tables
        );
        let mut setup_failed = false;
        for table in &tables {
            match tx.execute(
                &format!("CREATE TABLE {table} (id SERIAL8 PRIMARY KEY, data TEXT NOT NULL, hash INT8 NOT NULL)"),
                &[],
            ) {
                Ok(_) => {
                    println!("antigres [cksum] setup: created table '{table}'");
                }
                Err(e) => {
                    println!("antigres [cksum] setup: error creating '{table}': {e:?}");
                    setup_failed = true;
                    break;
                }
            }
        }
        if setup_failed {
            continue;
        }

        println!("antigres [cksum] setup: creating checksums table '{cksums}'...");
        match tx.execute(
            &format!(
                "CREATE TABLE {cksums} (table_name TEXT PRIMARY KEY, sum INT8 NOT NULL, count INT8 NOT NULL)"
            ),
            &[],
        ) {
            Ok(_) => {}
            Err(e) => {
                println!("antigres [cksum] setup: error creating '{cksums}': {e:?}");
                continue;
            }
        }

        // Seed each data table with random data and compute initial checksums
        println!("antigres [cksum] setup: seeding tables with {SEED_ROWS} rows each...");
        let mut seed_failed = false;
        for table in &tables {
            let mut data_values: Vec<String> = Vec::with_capacity(SEED_ROWS as usize);
            let mut hash_values: Vec<i64> = Vec::with_capacity(SEED_ROWS as usize);
            for _ in 0..SEED_ROWS {
                let s = Uuid::new_v4().to_string();
                let h = hash_text(&s);
                data_values.push(s);
                hash_values.push(h);
            }

            let placeholders: Vec<String> = (0..SEED_ROWS as i32)
                .map(|i| {
                    let d = i * 2 + 1;
                    let h = i * 2 + 2;
                    format!("(${d}, ${h})")
                })
                .collect();
            let insert_sql = format!(
                "INSERT INTO {table} (data, hash) VALUES {}",
                placeholders.join(", ")
            );

            let mut param_refs: Vec<&(dyn postgres::types::ToSql + Sync)> = Vec::new();
            for i in 0..SEED_ROWS as usize {
                param_refs.push(&data_values[i] as &(dyn postgres::types::ToSql + Sync));
                param_refs.push(&hash_values[i] as &(dyn postgres::types::ToSql + Sync));
            }

            match tx.execute(&insert_sql, &param_refs) {
                Ok(_) => {
                    println!("antigres [cksum] setup: seeded '{table}' with {SEED_ROWS} rows");
                }
                Err(e) => {
                    println!("antigres [cksum] setup: error seeding '{table}': {e:?}");
                    seed_failed = true;
                    break;
                }
            }

            let row = match tx.query_one(
                &format!("SELECT COALESCE(SUM(hash), 0)::int8, COUNT(*) FROM {table}"),
                &[],
            ) {
                Ok(row) => row,
                Err(e) => {
                    println!(
                        "antigres [cksum] setup: error computing checksum for '{table}': {e:?}"
                    );
                    seed_failed = true;
                    break;
                }
            };
            let sum: i64 = row.get(0);
            let count: i64 = row.get(1);

            match tx.execute(
                &format!("INSERT INTO {cksums} (table_name, sum, count) VALUES ($1, $2, $3)"),
                &[&table.as_str(), &sum, &count],
            ) {
                Ok(_) => {
                    println!(
                        "antigres [cksum] setup: recorded checksum for '{table}' (sum={sum}, count={count})"
                    );
                }
                Err(e) => {
                    println!(
                        "antigres [cksum] setup: error inserting checksum for '{table}': {e:?}"
                    );
                    seed_failed = true;
                    break;
                }
            }
        }
        if seed_failed {
            continue;
        }

        if let Ok(extra_sql) = std::env::var("CKSUM_EXTRA_SETUP_SQL") {
            println!("antigres [cksum] setup: running CKSUM_EXTRA_SETUP_SQL...");
            match tx.batch_execute(&extra_sql) {
                Ok(_) => {
                    println!("antigres [cksum] setup: extra SQL completed");
                }
                Err(e) => {
                    println!("antigres [cksum] setup: error running extra SQL: {e:?}");
                    continue;
                }
            }
        }

        println!("antigres [cksum] setup: committing...");
        match tx.commit() {
            Ok(_) => {
                println!(
                    "antigres [cksum] setup: committed successfully (elapsed: {:?})",
                    start.elapsed()
                );
                antithesis_sdk::assert_reachable!(
                    "Antithesis Postgres workload: created initial cksum test table.",
                    &json!({"name": cksums})
                );
                return;
            }
            Err(e) => {
                println!("antigres [cksum] setup: error committing: {e:?}");
                continue;
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ActionKind {
    Add,
    Shuffle,
    Delete,
    Move,
}

fn random_action() -> ActionKind {
    let r = antithesis_sdk::random::get_random() % 1000;
    match r {
        0..=349 => ActionKind::Add,
        350..=599 => ActionKind::Shuffle,
        600..=799 => ActionKind::Delete,
        _ => ActionKind::Move,
    }
}

fn pick_random_table(tables: &[String]) -> usize {
    antithesis_sdk::random::get_random() as usize % tables.len()
}

fn pick_two_different_tables(tables: &[String]) -> (usize, usize) {
    let a = pick_random_table(tables);
    let mut b = pick_random_table(tables);
    while b == a {
        b = pick_random_table(tables);
    }
    (a, b)
}

fn do_add(
    tx: &mut postgres::Transaction,
    tables: &[String],
    cksums: &str,
) -> Result<(), postgres::Error> {
    let idx = pick_random_table(tables);
    let table = &tables[idx];
    let uuid_str = Uuid::new_v4().to_string();
    let hash = hash_text(&uuid_str);

    tx.execute(
        &format!("INSERT INTO {table} (data, hash) VALUES ($1, $2)"),
        &[&uuid_str, &hash],
    )?;

    tx.execute(
        &format!("UPDATE {cksums} SET sum = sum + $1, count = count + $2 WHERE table_name = $3"),
        &[&hash, &1i64, &table.as_str()],
    )?;
    Ok(())
}

fn do_delete(
    tx: &mut postgres::Transaction,
    tables: &[String],
    cksums: &str,
) -> Result<(), postgres::Error> {
    let idx = pick_random_table(tables);
    let table = &tables[idx];

    let rows = tx.query(
        &format!("DELETE FROM {table} WHERE id = (SELECT id FROM {table} ORDER BY RANDOM() LIMIT 1) RETURNING hash"),
        &[],
    )?;

    if rows.is_empty() {
        return Ok(());
    }

    let hash: i64 = rows[0].get(0);

    tx.execute(
        &format!("UPDATE {cksums} SET sum = sum + $1, count = count + $2 WHERE table_name = $3"),
        &[&(-hash), &(-1i64), &table.as_str()],
    )?;
    Ok(())
}

fn do_move(
    tx: &mut postgres::Transaction,
    tables: &[String],
    cksums: &str,
) -> Result<(), postgres::Error> {
    let (src_idx, dst_idx) = pick_two_different_tables(tables);
    let src = &tables[src_idx];
    let dst = &tables[dst_idx];

    let rows = tx.query(
        &format!("DELETE FROM {src} WHERE id = (SELECT id FROM {src} ORDER BY RANDOM() LIMIT 1) RETURNING data, hash"),
        &[],
    )?;

    if rows.is_empty() {
        return Ok(());
    }

    let data: String = rows[0].get(0);
    let hash: i64 = rows[0].get(1);
    tx.execute(
        &format!("INSERT INTO {dst} (data, hash) VALUES ($1, $2)"),
        &[&data, &hash],
    )?;

    tx.execute(
        &format!("UPDATE {cksums} SET sum = sum + $1, count = count + $2 WHERE table_name = $3"),
        &[&(-hash), &(-1i64), &src.as_str()],
    )?;
    tx.execute(
        &format!("UPDATE {cksums} SET sum = sum + $1, count = count + $2 WHERE table_name = $3"),
        &[&hash, &1i64, &dst.as_str()],
    )?;
    Ok(())
}

fn do_shuffle(
    tx: &mut postgres::Transaction,
    tables: &[String],
    cksums: &str,
) -> Result<(), postgres::Error> {
    let (idx_a, idx_b) = pick_two_different_tables(tables);
    let table_a = &tables[idx_a];
    let table_b = &tables[idx_b];

    let rows_a = tx.query(
        &format!("SELECT id, data, hash FROM {table_a} ORDER BY RANDOM() LIMIT 1"),
        &[],
    )?;
    if rows_a.is_empty() {
        return Ok(());
    }

    let rows_b = tx.query(
        &format!("SELECT id, data, hash FROM {table_b} ORDER BY RANDOM() LIMIT 1"),
        &[],
    )?;
    if rows_b.is_empty() {
        return Ok(());
    }

    let id_a: i64 = rows_a[0].get(0);
    let data_a: String = rows_a[0].get(1);
    let hash_a: i64 = rows_a[0].get(2);
    let id_b: i64 = rows_b[0].get(0);
    let data_b: String = rows_b[0].get(1);
    let hash_b: i64 = rows_b[0].get(2);

    // Swap: set table_a's row to data_b, table_b's row to data_a
    tx.execute(
        &format!("UPDATE {table_a} SET data = $1, hash = $2 WHERE id = $3"),
        &[&data_b, &hash_b, &id_a],
    )?;
    tx.execute(
        &format!("UPDATE {table_b} SET data = $1, hash = $2 WHERE id = $3"),
        &[&data_a, &hash_a, &id_b],
    )?;

    // table_a lost hash_a, gained hash_b
    let delta_a = -hash_a + hash_b;
    if delta_a != 0 {
        tx.execute(
            &format!(
                "UPDATE {cksums} SET sum = sum + $1, count = count + $2 WHERE table_name = $3"
            ),
            &[&delta_a, &0i64, &table_a.as_str()],
        )?;
    }
    // table_b lost hash_b, gained hash_a
    let delta_b = -hash_b + hash_a;
    if delta_b != 0 {
        tx.execute(
            &format!(
                "UPDATE {cksums} SET sum = sum + $1, count = count + $2 WHERE table_name = $3"
            ),
            &[&delta_b, &0i64, &table_b.as_str()],
        )?;
    }
    Ok(())
}

pub fn parallel_action(pools: &DbPools, ctx: &Ctx) {
    let tables = data_table_names();
    let cksums = cksums_table_name();

    antithesis_sdk::assert_reachable!(
        "Antithesis Postgres workload (cksum): started parallel driver",
        &json!({"tables": tables.to_vec(), "cksums": cksums})
    );

    if antithesis_sdk::random::get_random() % 250 < 5 {
        if let Some(mut client) = pools.get_connection() {
            disable_logging(&mut client);
        }
    }

    let n_txns = antithesis_sdk::random::get_random() % 6 + 5;

    for _ in 0..n_txns {
        // Generate 1-5 random actions for this transaction
        let n_actions = antithesis_sdk::random::get_random() % 5 + 1;
        let actions: Vec<ActionKind> = (0..n_actions).map(|_| random_action()).collect();
        let mut serialization_retries = 0;
        let mut retries = 0;

        loop {
            retries += 1;
            if retries > max_retries() {
                println!("cksum: max retries reached, skipping txn");
                break;
            }

            let mut client = match pools.get_connection() {
                Some(c) => c,
                None => break,
            };

            let mut tx = match client
                .build_transaction()
                .isolation_level(postgres::IsolationLevel::RepeatableRead)
                .start()
            {
                Ok(tx) => tx,
                Err(e) => {
                    println!("cksum: cannot start transaction: {e:?}");
                    continue;
                }
            };

            let mut action_failed = false;

            for action in &actions {
                let result = match action {
                    ActionKind::Add => do_add(&mut tx, &tables, &cksums),
                    ActionKind::Delete => do_delete(&mut tx, &tables, &cksums),
                    ActionKind::Move => do_move(&mut tx, &tables, &cksums),
                    ActionKind::Shuffle => do_shuffle(&mut tx, &tables, &cksums),
                };

                match result {
                    Ok(()) => {}
                    Err(e) if err_is_serialization_failure(&e) => {
                        serialization_retries += 1;
                        action_failed = true;
                        break;
                    }
                    Err(e) => {
                        println!("cksum: error executing {action:?}: {e:?}");
                        action_failed = true;
                        break;
                    }
                }
            }

            if action_failed {
                if serialization_retries >= max_serialization_retries() {
                    println!("cksum: max serialization retries reached, skipping txn");
                    break;
                }
                continue;
            }

            match tx.commit() {
                Ok(_) => {
                    ctx.record_op("cksum", "action");
                    break;
                }
                Err(e) if err_is_serialization_failure(&e) => {
                    serialization_retries += 1;
                    if serialization_retries >= max_serialization_retries() {
                        println!("cksum: max serialization retries reached, skipping txn");
                        break;
                    }
                    continue;
                }
                Err(e) => {
                    println!("cksum: error committing transaction: {e:?}");
                    continue;
                }
            }
        }
    }

    antithesis_sdk::assert_reachable!(
        "Antithesis Postgres workload (cksum): completed parallel driver",
        &json!({"tables": tables.to_vec(), "cksums": cksums})
    );
}

pub fn validate(pools: &DbPools) {
    let tables = data_table_names();
    let cksums = cksums_table_name();

    let unions: Vec<String> = tables
        .iter()
        .map(|table| {
            format!(
                "SELECT
                    '{table}'::text AS table_name,
                    (SELECT COALESCE(SUM(hash), 0)::int8 FROM {table}) AS actual_sum,
                    (SELECT COUNT(*)::int8 FROM {table}) AS actual_count,
                    c.sum AS expected_sum,
                    c.count AS expected_count
                FROM {cksums} c
                WHERE c.table_name = '{table}'"
            )
        })
        .collect();
    let sql = unions.join(" UNION ALL ");

    for _ in 0..max_retries() {
        let mut client = match pools.get_connection() {
            Some(c) => c,
            None => return,
        };

        let rows = match client.query(&sql, &[]) {
            Ok(rows) => rows,
            Err(_) => continue,
        };

        antithesis_sdk::assert_always!(
            rows.len() == tables.len(),
            "Antithesis Postgres workload (cksum): all tables are present in validation results",
            &json!({
                "expected_tables": tables.to_vec(),
                "actual_count": rows.len(),
                "expected_count": tables.len()
            })
        );

        for row in &rows {
            let table: String = row.get(0);
            let actual_sum: i64 = row.get(1);
            let actual_count: i64 = row.get(2);
            let expected_sum: i64 = row.get(3);
            let expected_count: i64 = row.get(4);

            antithesis_sdk::assert_always!(
                actual_sum == expected_sum,
                "Antithesis Postgres workload (cksum): checksum is always consistent",
                &json!({
                    "table": table,
                    "actual_sum": actual_sum,
                    "expected_sum": expected_sum
                })
            );

            antithesis_sdk::assert_always!(
                actual_count == expected_count,
                "Antithesis Postgres workload (cksum): row count is always consistent",
                &json!({
                    "table": table,
                    "actual_count": actual_count,
                    "expected_count": expected_count
                })
            );
        }

        antithesis_sdk::assert_reachable!(
            "Antithesis Postgres workload (cksum): completed validation",
            &json!({"tables": tables.to_vec(), "cksums": cksums})
        );

        return;
    }
}
