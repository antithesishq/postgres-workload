use std::sync::atomic::{AtomicUsize, Ordering};

use crate::utils::{
    Ctx, DbPools, RwMode, disable_logging, err_is_serialization_failure, max_retries,
    max_serialization_retries, rw_mode, wait_for_table,
};
use serde_json::json;
use uuid::Uuid;

/// Lower bound on the number of rows in the UUIDs table.
static ROW_COUNT_LOWER_BOUND: AtomicUsize = AtomicUsize::new(0);

fn table_name() -> String {
    std::env::var("UUIDS_TABLE_NAME").unwrap_or_else(|_| "uuids".to_string())
}

pub fn setup(pools: &DbPools) {
    if rw_mode() == RwMode::Read {
        let table_name = table_name();
        wait_for_table(pools, &table_name);
        return;
    }

    let start = std::time::Instant::now();
    let table_name = table_name();

    let mut attempt: u64 = 0u64;
    loop {
        attempt += 1;
        println!(
            "antigres [uuids] setup: attempt {attempt} (elapsed: {:?})",
            start.elapsed()
        );

        let mut client = match pools.get_connection() {
            Some(c) => c,
            None => {
                println!("antigres [uuids] setup: failed to get connection");
                continue;
            }
        };

        let mut tx = match client
            .build_transaction()
            .isolation_level(postgres::IsolationLevel::ReadCommitted)
            .start()
        {
            Ok(tx) => tx,
            Err(e) => {
                println!("antigres [uuids] setup: error starting txn: {e:?}");
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
            &[&table_name],
        ) {
            Ok(row) => row.get(0),
            Err(e) => {
                println!("antigres [uuids] setup: error checking table existence: {e:?}");
                false
            }
        };

        if table_exists {
            println!(
                "antigres [uuids] setup: table '{table_name}' already exists, done (elapsed: {:?})",
                start.elapsed()
            );
            return;
        }

        println!("antigres [uuids] setup: creating table '{table_name}'...");
        match tx.execute(
            &format!("CREATE TABLE {table_name} (value TEXT NOT NULL UNIQUE, created_at TIMESTAMPTZ NOT NULL DEFAULT NOW())"),
            &[],
        ) {
            Ok(_) => {
                println!("antigres [uuids] setup: table created");
            }
            Err(e) => {
                println!("antigres [uuids] setup: error creating table: {e:?}");
                continue;
            }
        }

        if let Ok(extra_sql) = std::env::var("UUIDS_EXTRA_SETUP_SQL") {
            println!("antigres [uuids] setup: running UUIDS_EXTRA_SETUP_SQL...");
            match tx.batch_execute(&extra_sql) {
                Ok(_) => {
                    println!("antigres [uuids] setup: extra SQL completed");
                }
                Err(e) => {
                    println!("antigres [uuids] setup: error running extra SQL: {e:?}");
                    continue;
                }
            }
        }

        println!("antigres [uuids] setup: committing...");
        match tx.commit() {
            Ok(_) => {
                println!(
                    "antigres [uuids] setup: committed successfully (elapsed: {:?})",
                    start.elapsed()
                );
                return;
            }
            Err(e) => {
                println!("antigres [uuids] setup: error committing: {e:?}");
                continue;
            }
        }
    }
}

pub fn parallel_action(pools: &DbPools, ctx: &Ctx) {
    if antithesis_sdk::random::get_random() % 250 < 5 {
        if let Some(mut client) = pools.get_connection() {
            disable_logging(&mut client);
        }
    }

    add_uuids(pools, ctx);
}

pub fn validate(pools: &DbPools) {
    let table_name = table_name();

    let mut client = match pools.get_connection() {
        Some(c) => c,
        None => {
            println!("antigres [uuids] validate: failed to get connection");
            return;
        }
    };

    let prev_lower_bound = ROW_COUNT_LOWER_BOUND.load(Ordering::SeqCst);

    let row_count: i64 = match client.query_one(&format!("SELECT COUNT(*) FROM {table_name}"), &[])
    {
        Ok(row) => row.get(0),
        Err(e) => {
            println!("antigres [uuids] validate: error counting rows: {e:?}");
            return;
        }
    };

    let row_count = row_count as usize;

    antithesis_sdk::assert_always!(
        prev_lower_bound <= row_count,
        "Antithesis Postgres workload: UUID table row count must never decrease.",
        &json!({
            "current_count": row_count,
            "previous_lower_bound": prev_lower_bound,
            "table": &table_name
        })
    );

    let mut current = prev_lower_bound;
    while current < row_count {
        match ROW_COUNT_LOWER_BOUND.compare_exchange(
            current,
            row_count,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => break,
            Err(actual) => current = actual,
        }
    }

    antithesis_sdk::assert_reachable!(
        "Antithesis Postgres workload (uuid_test): completed validate",
        &json!({
            "row_count": row_count,
            "lower_bound": ROW_COUNT_LOWER_BOUND.load(Ordering::SeqCst),
            "table": &table_name
        })
    );
}

fn add_uuids(pools: &DbPools, ctx: &Ctx) {
    let table_name = table_name();
    let count = (antithesis_sdk::random::get_random() % 10 + 1) as usize;

    for _ in 0..count {
        let uuid_str = Uuid::new_v4().to_string();
        let mut serialization_retries = 0;
        let mut retries = 0;

        loop {
            retries += 1;
            if retries > max_retries() {
                println!("add_uuids: max retries reached, skipping UUID");
                break;
            }

            let mut client = match pools.get_connection() {
                Some(c) => c,
                None => break,
            };

            let mut db_tx = match client
                .build_transaction()
                .isolation_level(postgres::IsolationLevel::ReadCommitted)
                .start()
            {
                Ok(tx) => tx,
                Err(e) => {
                    println!("Error starting txn in add_uuids: {e:?}");
                    continue;
                }
            };

            match db_tx.execute(
                &format!("INSERT INTO {table_name} (value) VALUES ($1)"),
                &[&uuid_str],
            ) {
                Ok(_) => {}
                Err(e) if err_is_serialization_failure(&e) => {
                    serialization_retries += 1;
                    if serialization_retries >= max_serialization_retries() {
                        println!("add_uuids: max serialization retries reached, skipping UUID");
                        break;
                    }
                    continue;
                }
                Err(e) => {
                    println!("Error inserting UUID {uuid_str}: {e:?}");
                    continue;
                }
            }

            match db_tx.commit() {
                Ok(_) => {
                    ctx.record_op("uuids", "add_uuid");
                    break;
                }
                Err(e) if err_is_serialization_failure(&e) => {
                    serialization_retries += 1;
                    if serialization_retries >= max_serialization_retries() {
                        println!("add_uuids: max serialization retries reached, skipping UUID");
                        break;
                    }
                    continue;
                }
                Err(e) => {
                    println!("Error committing add_uuids txn: {e:?}");
                    break;
                }
            }
        }
    }

    antithesis_sdk::assert_reachable!(
        "Antithesis Postgres workload (uuid_test): completed add UUID parallel driver",
        &serde_json::json!({"table": table_name})
    );
}
