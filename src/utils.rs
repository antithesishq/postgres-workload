use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use antithesis_sdk::random::AntithesisRng;
use postgres::{Client, NoTls, error::SqlState};
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;
use rand::seq::SliceRandom;
use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RwMode {
    Read,
    Write,
}

pub fn rw_mode() -> RwMode {
    match std::env::var("RW_MODE") {
        Ok(val) => match val.to_uppercase().as_str() {
            "READ" => RwMode::Read,
            "WRITE" => RwMode::Write,
            other => {
                println!("antigres WARNING: unknown RW_MODE '{other}', defaulting to WRITE");
                RwMode::Write
            }
        },
        Err(_) => RwMode::Write,
    }
}

pub type PooledConnection = r2d2::PooledConnection<PostgresConnectionManager<NoTls>>;

#[derive(Clone)]
pub struct DbPools {
    pools: Vec<Pool<PostgresConnectionManager<NoTls>>>,
}

pub fn max_allowed_connections() -> u32 {
    let max_size: u32 = std::env::var("MAX_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    max_size
}

impl DbPools {
    pub fn new(connection_strings: &[String]) -> Self {
        let max_size = max_allowed_connections();

        let pools = connection_strings
            .iter()
            .map(|conn_str| {
                let mut config: postgres::Config = conn_str.parse().unwrap();
                config.tcp_user_timeout(Duration::from_millis(3000));
                config.connect_timeout(Duration::from_millis(3000));
                config.keepalives(true);
                config.keepalives_idle(Duration::from_secs(3));
                config.keepalives_interval(Duration::from_secs(3));
                config.keepalives_retries(3);
                let manager = PostgresConnectionManager::new(config, NoTls);
                Pool::builder()
                    .max_size(max_size)
                    .connection_timeout(Duration::from_secs(30))
                    .test_on_check_out(true)
                    .build(manager)
                    .expect("Failed to create connection pool")
            })
            .collect();

        DbPools { pools }
    }

    pub fn get_connection(&self) -> Option<PooledConnection> {
        for _ in 0..max_retries() {
            let pool = self
                .pools
                .choose(&mut AntithesisRng)
                .expect("pools must not be empty");
            match pool.get() {
                Ok(conn) => return Some(conn),
                Err(e) => {
                    println!("Error obtaining pooled connection: {e:?}");
                    std::thread::sleep(Duration::from_secs(3));
                }
            }
        }
        println!(
            "Failed to obtain connection after {} attempts",
            max_retries()
        );
        None
    }
}

pub fn err_is_serialization_failure(e: &postgres::Error) -> bool {
    e.code() == Some(&SqlState::T_R_SERIALIZATION_FAILURE)
        // TODO: how to handle deadlocks?
        || e.code() == Some(&SqlState::T_R_DEADLOCK_DETECTED)
}

pub fn max_serialization_retries() -> usize {
    std::env::var("MAX_SERIALIZATION_RETRIES")
        .map(|t| t.parse::<usize>().ok())
        .ok()
        .flatten()
        .unwrap_or(7)
}

/// Maximum number of non-serialization retries before giving up on an operation.
pub fn max_retries() -> usize {
    std::env::var("MAX_RETRIES")
        .map(|t| t.parse::<usize>().ok())
        .ok()
        .flatten()
        .unwrap_or(20)
}

#[derive(Clone)]
pub struct Ctx {
    op_tx: Sender<String>,
    pub egglog_tables: Option<std::sync::Arc<Vec<crate::egglog::schema_gen::TableMeta>>>,
    pub egglog_samples:
        Option<std::sync::Arc<HashMap<String, Vec<crate::egglog::schema_gen::SampleValue>>>>,
}

impl Ctx {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            let mut counters: HashMap<String, u64> = HashMap::new();
            for key in rx {
                let count = counters.entry(key.clone()).or_insert(0);
                *count += 1;
                let current = *count;
                if antithesis_sdk::random::get_random() % 100 == 0 {
                    *count = 0;
                    let details = json!({"type": format!("{key}"), "count": current});
                    println!("Operations since last message: {key} count={current}");
                    antithesis_sdk::lifecycle::send_event("logical_operations_completed", &details);
                }
            }
        });
        Ctx {
            op_tx: tx,
            egglog_tables: None,
            egglog_samples: None,
        }
    }

    pub fn record_op(&self, test_name: &str, op_name: &str) {
        let key = format!("{test_name}_{op_name}");
        let _ = self.op_tx.send(key);
    }
}

pub fn wait_for_table(pools: &DbPools, table_name: &str) {
    let mut attempts = 0u64;
    loop {
        if let Some(mut client) = pools.get_connection() {
            let exists: bool = match client.query_one(
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
                    println!("antigres [READ] error checking table '{table_name}': {e:?}");
                    false
                }
            };
            if exists {
                println!("antigres [READ] table '{table_name}' found");
                return;
            }
        }
        attempts += 1;
        if attempts % 10 == 1 {
            println!(
                "antigres [READ] waiting for table '{table_name}' to exist (attempt {attempts})..."
            );
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}

pub fn log_config(selected_tests: &[String], connection_strings: &[String]) {
    println!("antigres config: RW_MODE={:?}", rw_mode());
    println!("antigres config: SELECTED_TESTS={selected_tests:?}");
    println!(
        "antigres config: DATABASE_URL count={}",
        connection_strings.len()
    );
    println!(
        "antigres config: MAX_CONNECTIONS={}",
        max_allowed_connections()
    );
    println!("antigres config: MAX_RETRIES={}", max_retries());
    println!(
        "antigres config: MAX_SERIALIZATION_RETRIES={}",
        max_serialization_retries()
    );
    if selected_tests.iter().any(|t| t == "bank_test") {
        println!(
            "antigres config: BANK_TEST_TXN_LEVEL={:?}",
            std::env::var("BANK_TEST_TXN_LEVEL")
                .unwrap_or_else(|_| "SERIALIZABLE (default)".to_string())
        );
    }
}

pub fn disable_logging(client: &mut Client) {
    if rw_mode() == RwMode::Read {
        return;
    }

    if let Ok(t) = std::env::var("ANTIGRES_KEEP_LOGGING_ON")
        && (t == "1" || t.to_ascii_lowercase() == "true")
    {
        return;
    }

    for stmt in [
        "ALTER SYSTEM SET log_connections = off",
        "ALTER SYSTEM SET log_disconnections = off",
        "ALTER SYSTEM SET log_statement = 'none'",
        "ALTER SYSTEM SET log_duration = off",
        "ALTER SYSTEM SET log_min_messages = FATAL",
        "ALTER SYSTEM SET log_min_error_statement = PANIC",
    ] {
        match client.execute(stmt, &[]) {
            Ok(_) => {}
            Err(e) => {
                println!("Database error when trying to disable logging: {e:?}")
            }
        }
    }
    match client.execute("SELECT pg_reload_conf();", &[]) {
        Ok(_) => {}
        Err(e) => {
            println!("Database error when trying to disable logging: {e:?}")
        }
    }
}
