use crate::utils::{
    Ctx, DbPools, RwMode, disable_logging, err_is_serialization_failure, max_retries,
    max_serialization_retries, rw_mode, wait_for_table,
};
use serde_json::json;

const STARTING_BALANCE: u64 = 10_000;
const STARTING_ACCOUNTS: u64 = 100;

fn table_name() -> String {
    std::env::var("BANK_TEST_TABLE_NAME").unwrap_or_else(|_| "accounts".to_string())
}

fn txn_level() -> postgres::IsolationLevel {
    match std::env::var("BANK_TEST_TXN_LEVEL") {
        Ok(val) => match val.to_uppercase().as_str() {
            "READ_COMMITTED" => postgres::IsolationLevel::ReadCommitted,
            "SERIALIZABLE" => postgres::IsolationLevel::Serializable,
            other => {
                println!(
                    "antigres [bank_test] WARNING: unknown BANK_TEST_TXN_LEVEL '{other}', defaulting to Serializable"
                );
                postgres::IsolationLevel::Serializable
            }
        },
        Err(_) => postgres::IsolationLevel::Serializable,
    }
}

pub fn setup(pools: &DbPools) {
    if rw_mode() == RwMode::Read {
        wait_for_table(pools, &table_name());
        return;
    }

    let start = std::time::Instant::now();
    let table_name = table_name();

    for attempt in 1..=max_retries() {
        println!(
            "antigres [bank_test] setup: attempt {attempt}/{} (elapsed: {:?})",
            max_retries(),
            start.elapsed()
        );

        let mut client = match pools.get_connection() {
            Some(c) => c,
            None => {
                println!("antigres [bank_test] setup: failed to get connection, giving up");
                return;
            }
        };

        let mut tx = match client
            .build_transaction()
            .isolation_level(txn_level())
            .start()
        {
            Ok(tx) => tx,
            Err(e) => {
                println!("antigres [bank_test] setup: error starting txn: {e:?}");
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
                println!("antigres [bank_test] setup: error checking table existence: {e:?}");
                false
            }
        };

        if table_exists {
            println!(
                "antigres [bank_test] setup: table '{table_name}' already exists, done (elapsed: {:?})",
                start.elapsed()
            );
            return;
        }

        println!("antigres [bank_test] setup: creating table '{table_name}'...");
        match tx.execute(
            &format!("CREATE TABLE {table_name} (id SERIAL8 PRIMARY KEY, balance INT8 NOT NULL)"),
            &[],
        ) {
            Ok(_) => {}
            Err(e) => {
                println!("antigres [bank_test] setup: error creating table: {e:?}");
                continue;
            }
        }

        println!("antigres [bank_test] setup: inserting {STARTING_ACCOUNTS} initial accounts...");
        match tx.execute(
            &format!("INSERT INTO {table_name} SELECT g, $2::INT8 FROM generate_series(1::INT8, $1::INT8) g;"),
            &[&(STARTING_ACCOUNTS as i64), &(STARTING_BALANCE as i64)],
        ) {
            Ok(n) => {
                assert_eq!(n, STARTING_ACCOUNTS);
                println!("antigres [bank_test] setup: inserted {n} accounts");
            }
            Err(e) => {
                println!("antigres [bank_test] setup: error inserting accounts: {e:?}");
                continue;
            }
        }

        if let Ok(extra_sql) = std::env::var("BANK_TEST_EXTRA_SETUP_SQL") {
            println!("antigres [bank_test] setup: running BANK_TEST_EXTRA_SETUP_SQL...");
            match tx.batch_execute(&extra_sql) {
                Ok(_) => {
                    println!("antigres [bank_test] setup: extra SQL completed");
                }
                Err(e) => {
                    println!("antigres [bank_test] setup: error running extra SQL: {e:?}");
                    continue;
                }
            }
        }

        println!("antigres [bank_test] setup: committing...");
        match tx.commit() {
            Ok(_) => {
                println!(
                    "antigres [bank_test] setup: committed successfully (elapsed: {:?})",
                    start.elapsed()
                );
                antithesis_sdk::assert_reachable!(
                    "Antithesis Postgres workload (bank_test): setup completed successfully",
                    &json!({"table": table_name})
                );
                return;
            }
            Err(e) => {
                println!("antigres [bank_test] setup: error committing: {e:?}");
                continue;
            }
        }
    }
    println!(
        "antigres [bank_test] setup: FAILED after {} retries (elapsed: {:?})",
        max_retries(),
        start.elapsed()
    );
}

pub fn parallel_action(pools: &DbPools, ctx: &Ctx) {
    if antithesis_sdk::random::get_random() % 250 < 5 {
        if let Some(mut client) = pools.get_connection() {
            disable_logging(&mut client);
        }
    }

    let random = antithesis_sdk::random::get_random() % 20 + 1;
    if random <= 15 {
        do_account_transfers(pools, ctx);
    } else if antithesis_sdk::random::get_random() % 3 + 1 <= 2 {
        add_accounts(pools, ctx);
    } else {
        delete_accounts(pools, ctx);
    }
}

pub fn validate(pools: &DbPools) {
    let table_name = table_name();

    for _ in 0..max_retries() {
        let mut client = match pools.get_connection() {
            Some(c) => c,
            None => return,
        };

        let row = match client.query_one(
            &format!("SELECT COALESCE(SUM(balance), 0)::INT8 FROM {table_name}"),
            &[],
        ) {
            Ok(t) => t,
            _ => continue,
        };
        let net: i64 = row.get(0);

        let expected_balance = (STARTING_ACCOUNTS as i64) * (STARTING_BALANCE as i64);
        antithesis_sdk::assert_always!(
            net == expected_balance,
            "Antithesis Postgres workload (bank_test): Net balance is always constant",
            &json!({"net": net, "expected": expected_balance, "table": table_name})
        );

        let negative_accounts = match client.query(
            &format!("SELECT * FROM {table_name} WHERE balance < 0"),
            &[],
        ) {
            Ok(t) => t,
            _ => continue,
        };

        let failing_accounts = negative_accounts
            .iter()
            .map(|row| {
                let id: i64 = row.get(0);
                let balance: i64 = row.get(1);
                json!({
                    "id": id,
                    "balance": balance
                })
            })
            .collect::<Vec<_>>();

        antithesis_sdk::assert_always!(
            negative_accounts.is_empty(),
            "Antithesis Postgres workload (bank_test): Accounts are always non-negative",
            &json!({
                "failing_accounts": failing_accounts,
                "table": table_name
            })
        );

        return;
    }
}

fn do_account_transfers(pools: &DbPools, ctx: &Ctx) {
    let table_name = table_name();

    antithesis_sdk::assert_reachable!(
        "Antithesis Postgres workload (bank_test): started do_account_transfers parallel driver",
        &json!({"table": table_name})
    );
    let n_txns = antithesis_sdk::random::get_random() % 6 + 5;

    for _ in 0..n_txns {
        let mut serialization_retries = 0;
        let mut retries = 0;

        loop {
            retries += 1;
            if retries > max_retries() {
                println!("do_account_transfers: max retries reached, skipping txn");
                break;
            }

            let mut client = match pools.get_connection() {
                Some(c) => c,
                None => break,
            };

            let mut tx = match client
                .build_transaction()
                .isolation_level(txn_level())
                .start()
            {
                Ok(tx) => tx,
                Err(e) => {
                    println!("do_txns, cannot start transaction: {e:?}");
                    continue;
                }
            };

            let count: i64 = match tx
                .query_one(&format!("SELECT COUNT(*)::INT8 FROM {table_name}"), &[])
            {
                Ok(row) => row.get(0),
                Err(e) if err_is_serialization_failure(&e) => {
                    serialization_retries += 1;
                    if serialization_retries >= max_serialization_retries() {
                        println!(
                            "do_account_transfers: max serialization retries reached, skipping txn"
                        );
                        break;
                    }
                    continue;
                }
                Err(_) => continue,
            };

            if count < 2 {
                break;
            }
            let count = count as u64;

            let sender_offset = (antithesis_sdk::random::get_random() % count) as i64;
            let sender: i64 = match tx.query_one(
                &format!("SELECT id::INT8 FROM {table_name} ORDER BY id OFFSET $1 LIMIT 1"),
                &[&sender_offset],
            ) {
                Ok(row) => row.get(0),
                Err(e) if err_is_serialization_failure(&e) => {
                    serialization_retries += 1;
                    if serialization_retries >= max_serialization_retries() {
                        println!(
                            "do_account_transfers: max serialization retries reached, skipping txn"
                        );
                        break;
                    }
                    continue;
                }
                Err(_) => continue,
            };

            let recipient_offset = loop {
                let ret = (antithesis_sdk::random::get_random() % count) as i64;
                if ret != sender_offset {
                    break ret;
                }
            };
            let recipient: i64 = match tx.query_one(
                &format!("SELECT id::INT8 FROM {table_name} ORDER BY id OFFSET $1 LIMIT 1"),
                &[&recipient_offset],
            ) {
                Ok(row) => row.get(0),
                Err(e) if err_is_serialization_failure(&e) => {
                    serialization_retries += 1;
                    if serialization_retries >= max_serialization_retries() {
                        println!(
                            "do_account_transfers: max serialization retries reached, skipping txn"
                        );
                        break;
                    }
                    continue;
                }
                Err(_) => continue,
            };

            let transfer_amt = antithesis_sdk::random::get_random() % (STARTING_BALANCE / 2) + 1;

            let sender_balance: i64 = match tx.query_one(
                &format!("SELECT balance FROM {table_name} WHERE id = $1::INT8"),
                &[&sender],
            ) {
                Ok(row) => row.get(0),
                Err(e) if err_is_serialization_failure(&e) => {
                    serialization_retries += 1;
                    if serialization_retries >= max_serialization_retries() {
                        println!(
                            "do_account_transfers: max serialization retries reached, skipping txn"
                        );
                        break;
                    }
                    continue;
                }
                Err(_) => continue,
            };

            if (sender_balance as u64) < transfer_amt {
                break;
            }

            let r = tx.execute(
                &format!(
                    "UPDATE {table_name} SET balance = balance - $1::INT8 WHERE id = $2::INT8"
                ),
                &[&(transfer_amt as i64), &sender],
            );
            match r {
                Ok(n) => {
                    assert_eq!(n, 1);
                }
                Err(e) if err_is_serialization_failure(&e) => {
                    serialization_retries += 1;
                    if serialization_retries >= max_serialization_retries() {
                        println!(
                            "do_account_transfers: max serialization retries reached, skipping txn"
                        );
                        break;
                    }
                    continue;
                }
                Err(_) => continue,
            }

            let r = tx.execute(
                &format!(
                    "UPDATE {table_name} SET balance = balance + $1::INT8 WHERE id = $2::INT8"
                ),
                &[&(transfer_amt as i64), &recipient],
            );
            match r {
                Ok(n) => {
                    assert_eq!(n, 1);
                }
                Err(e) if err_is_serialization_failure(&e) => {
                    serialization_retries += 1;
                    if serialization_retries >= max_serialization_retries() {
                        println!(
                            "do_account_transfers: max serialization retries reached, skipping txn"
                        );
                        break;
                    }
                    continue;
                }
                Err(_) => continue,
            }

            match tx.commit() {
                Ok(_) => {
                    ctx.record_op("bank_test", "account_transfer");
                    antithesis_sdk::assert_reachable!(
                        "Antithesis Postgres workload (bank_test): transfer transaction committed",
                        &json!({
                            "sender": sender,
                            "recipient": recipient,
                            "amount": transfer_amt,
                            "table": table_name
                        })
                    );
                    break;
                }
                Err(e) if err_is_serialization_failure(&e) => {
                    serialization_retries += 1;
                    if serialization_retries >= max_serialization_retries() {
                        println!(
                            "do_account_transfers: max serialization retries reached, skipping txn"
                        );
                        return;
                    }
                    continue;
                }
                Err(_) => continue,
            }
        }
    }
}

fn add_accounts(pools: &DbPools, ctx: &Ctx) {
    let table_name = table_name();

    antithesis_sdk::assert_reachable!(
        "Antithesis Postgres workload (bank_test): started add_accounts operation",
        &json!({"table": table_name})
    );

    let mut serialization_retries = 0;
    let mut retries = 0;

    loop {
        retries += 1;
        if retries > max_retries() {
            println!("add_accounts: max retries reached, skipping");
            return;
        }

        let mut client = match pools.get_connection() {
            Some(c) => c,
            None => return,
        };

        let mut tx = match client
            .build_transaction()
            .isolation_level(txn_level())
            .start()
        {
            Ok(tx) => tx,
            _ => continue,
        };

        match tx.execute(
            &format!(
                "INSERT INTO {table_name} (id, balance) \
                 SELECT MIN(t.id), 0::INT8 \
                 FROM (SELECT 1::INT8 AS id UNION ALL SELECT id + 1 FROM {table_name}) t \
                 WHERE NOT EXISTS (SELECT 1 FROM {table_name} a WHERE a.id = t.id)"
            ),
            &[],
        ) {
            Ok(n) => {
                assert_eq!(n, 1);
            }
            Err(e) if err_is_serialization_failure(&e) => {
                serialization_retries += 1;
                if serialization_retries >= max_serialization_retries() {
                    println!("add_accounts: max serialization retries reached, skipping");
                    return;
                }
                continue;
            }
            Err(e) => {
                println!("Unexpected error: {e:?}");
                continue;
            }
        }

        match tx.commit() {
            Ok(_) => {
                ctx.record_op("bank_test", "add_account");
                antithesis_sdk::assert_reachable!(
                    "Antithesis Postgres workload (bank_test): successfully added account",
                    &json!({
                        "table": table_name
                    })
                );
                return;
            }
            Err(e) if err_is_serialization_failure(&e) => {
                serialization_retries += 1;
                if serialization_retries >= max_serialization_retries() {
                    println!("add_accounts: max serialization retries reached, skipping");
                    return;
                }
                continue;
            }
            Err(_) => continue,
        }
    }
}

fn delete_accounts(pools: &DbPools, ctx: &Ctx) {
    let table_name = table_name();
    let mut serialization_retries = 0;
    let mut retries = 0;

    loop {
        retries += 1;
        if retries > max_retries() {
            println!("delete_accounts: max retries reached, skipping");
            return;
        }

        let mut client = match pools.get_connection() {
            Some(c) => c,
            None => return,
        };

        let mut tx = match client
            .build_transaction()
            .isolation_level(txn_level())
            .start()
        {
            Ok(tx) => tx,
            _ => continue,
        };

        let count: i64 =
            match tx.query_one(&format!("SELECT COUNT(*)::INT8 FROM {table_name}"), &[]) {
                Ok(row) => row.get(0),
                Err(e) if err_is_serialization_failure(&e) => {
                    serialization_retries += 1;
                    if serialization_retries >= max_serialization_retries() {
                        println!("delete_accounts: max serialization retries reached, skipping");
                        return;
                    }
                    continue;
                }
                Err(e) => {
                    println!("Unexpected error counting accounts: {e:?}");
                    continue;
                }
            };

        if count < 3 {
            return;
        }

        // Pick a random account to delete
        let offset = (antithesis_sdk::random::get_random() % count as u64) as i64;
        let row = match tx.query_one(
            &format!(
                "SELECT id::INT8, balance::INT8 FROM {table_name} ORDER BY id OFFSET $1 LIMIT 1"
            ),
            &[&offset],
        ) {
            Ok(row) => row,
            Err(e) if err_is_serialization_failure(&e) => {
                serialization_retries += 1;
                if serialization_retries >= max_serialization_retries() {
                    println!("delete_accounts: max serialization retries reached, skipping");
                    return;
                }
                continue;
            }
            Err(e) => {
                println!("Unexpected error picking account to delete: {e:?}");
                continue;
            }
        };
        let delete_id: i64 = row.get(0);
        let delete_balance: i64 = row.get(1);

        // Transfer balance to a random other account
        let recipient_offset = (antithesis_sdk::random::get_random() % (count - 1) as u64) as i64;
        let recipient_id: i64 = match tx.query_one(
            &format!(
                "SELECT id::INT8 FROM {table_name} WHERE id != $1 ORDER BY id OFFSET $2 LIMIT 1"
            ),
            &[&delete_id, &recipient_offset],
        ) {
            Ok(row) => row.get(0),
            Err(e) if err_is_serialization_failure(&e) => {
                serialization_retries += 1;
                if serialization_retries >= max_serialization_retries() {
                    println!("delete_accounts: max serialization retries reached, skipping");
                    return;
                }
                continue;
            }
            Err(e) => {
                println!("Unexpected error finding recipient: {e:?}");
                continue;
            }
        };

        match tx.execute(
            &format!("UPDATE {table_name} SET balance = balance + $1::INT8 WHERE id = $2::INT8"),
            &[&delete_balance, &recipient_id],
        ) {
            Ok(_) => {}
            Err(e) if err_is_serialization_failure(&e) => {
                serialization_retries += 1;
                if serialization_retries >= max_serialization_retries() {
                    println!("delete_accounts: max serialization retries reached, skipping");
                    return;
                }
                continue;
            }
            Err(e) => {
                println!("Unexpected error transferring balance: {e:?}");
                continue;
            }
        }

        match tx.execute(
            &format!("DELETE FROM {table_name} WHERE id = $1::INT8"),
            &[&delete_id],
        ) {
            Ok(_) => {}
            Err(e) if err_is_serialization_failure(&e) => {
                serialization_retries += 1;
                if serialization_retries >= max_serialization_retries() {
                    println!("delete_accounts: max serialization retries reached, skipping");
                    return;
                }
                continue;
            }
            Err(e) => {
                println!("Unexpected error deleting account: {e:?}");
                continue;
            }
        }

        match tx.commit() {
            Ok(_) => {
                ctx.record_op("bank_test", "delete_account");
                antithesis_sdk::assert_reachable!(
                    "Antithesis Postgres workload (bank_test): successfully deleted account",
                    &json!({
                        "deleted_id": delete_id,
                        "table": table_name
                    })
                );
                return;
            }
            Err(e) if err_is_serialization_failure(&e) => {
                serialization_retries += 1;
                if serialization_retries >= max_serialization_retries() {
                    println!("delete_accounts: max serialization retries reached, skipping");
                    return;
                }
                continue;
            }
            Err(_) => continue,
        }
    }
}
