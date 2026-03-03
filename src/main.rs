mod bank_test;
mod cksum;
mod egglog;
mod utils;
mod uuids;

use std::env;
use std::thread;
use std::time::Duration;

use antithesis_sdk::random::AntithesisRng;
use rand_distr::{Distribution, Normal};
use serde_json::json;
use threadpool::ThreadPool;

use utils::{Ctx, DbPools, RwMode, disable_logging, log_config, rw_mode};

use crate::utils::max_allowed_connections;

fn parse_selected_tests() -> Vec<String> {
    let raw = env::var("SELECTED_TESTS").unwrap_or_else(|_| "bank_test,uuids,cksum".to_string());
    let tests: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if tests.is_empty() {
        vec![
            "bank_test".to_string(),
            "uuids".to_string(),
            "cksum".to_string(),
            "egglog".to_string(),
        ]
    } else {
        tests
    }
}

fn parse_connection_strings() -> Vec<String> {
    let raw = env::var("DATABASE_URL").unwrap_or_else(|_| {
        eprintln!("DATABASE_URL environment variable must be set");
        std::process::exit(1);
    });
    let strings: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if strings.is_empty() {
        eprintln!("DATABASE_URL must contain at least one connection string");
        std::process::exit(1);
    }
    strings
}

fn stop_faults(duration_secs: u32) {
    if let Ok(cmd) = env::var("ANTITHESIS_STOP_FAULTS") {
        println!("antigres: stopping faults for {duration_secs}s via {cmd}");
        match std::process::Command::new(&cmd)
            .arg(duration_secs.to_string())
            .status()
        {
            Ok(status) => {
                println!("antigres: stop_faults exited with {status}");
            }
            Err(e) => {
                println!("antigres: failed to run ANTITHESIS_STOP_FAULTS ({cmd}): {e}");
            }
        }
    }
}

fn setup(test: String, pools: &DbPools, ctx: &mut Ctx) {
    // note: it is important that a percentage of rollouts are generated from
    // here for the current setup of the egglog test.
    antithesis_sdk::assert_reachable!(
        "Antithesis Postgres workload: starting setup for selected workload.",
        &json!({"test": test})
    );

    stop_faults(20);

    match test.as_str() {
        "bank_test" => {
            bank_test::setup(pools);
        }
        "uuids" => uuids::setup(pools),
        "cksum" => {
            cksum::setup(pools);
        }
        "egglog" => {
            egglog::workload::setup(pools, ctx);
        }
        other => {
            eprintln!("Unknown test: {other}");
        }
    };

    println!("antigres [{test}]: setup function returned");
    antithesis_sdk::assert_reachable!(
        "Antithesis Postgres workload: completed setup for workload.",
        &json!({"test": test})
    );
}

fn supervisor_loop(test: String, n_enabled: usize, pools: &DbPools, ctx: &Ctx) {
    assert!(n_enabled > 0);
    let max_threads = max_allowed_connections() as usize;
    assert!(max_threads > 0);
    let mean = (max_threads as f64) * 2.0 / 3.0;
    let std_dev = (mean - 1.0) / 2.0; // 1 thread is 2 stddev from the mean
    let dist = Normal::new(mean, std_dev.max(0.1)).unwrap();

    let initial_n = (dist.sample(&mut AntithesisRng) as usize).clamp(1, max_threads);
    let mut pool = ThreadPool::new(initial_n);
    let mut n_since_last_new = 0;
    loop {
        // Some bugs only appear with specific numbers of threads so we make
        // sure to adjust the maximum number of running threads here.
        if antithesis_sdk::random::get_random() % 100 == 0 {
            let n = (dist.sample(&mut AntithesisRng) as usize).clamp(1, max_threads);
            println!("Adjusting threadpool size to {n}");
            pool.set_num_threads(n);
        }

        // Wait for pool capacity before submitting new work
        while pool.max_count() <= pool.active_count() + pool.queued_count() {
            thread::sleep(Duration::from_secs(3));
            n_since_last_new += 1;
            if n_since_last_new > 100 {
                println!(
                    "Warning: waited {}s since last job was enqueued (test: {test}).",
                    n_since_last_new * 3
                );
            }
        }

        n_since_last_new = 0;

        let is_read_mode = rw_mode() == RwMode::Read;
        let do_validate = if is_read_mode {
            // always valiudate in read mode
            true
        } else {
            antithesis_sdk::random::get_random() % 250 < 10
        };
        let pools = pools.clone();
        let ctx = ctx.clone();

        let test = test.clone();
        pool.execute(move || {
            if do_validate {
                match test.as_str() {
                    "bank_test" => bank_test::validate(&pools),
                    "uuids" => uuids::validate(&pools),
                    "cksum" => cksum::validate(&pools),
                    "egglog" => egglog::workload::parallel_action(&pools, &ctx),
                    other => eprintln!("Unknown test in validate: {other}"),
                }
            } else {
                match test.as_str() {
                    "bank_test" => bank_test::parallel_action(&pools, &ctx),
                    "uuids" => uuids::parallel_action(&pools, &ctx),
                    "cksum" => cksum::parallel_action(&pools, &ctx),
                    "egglog" => egglog::workload::parallel_action(&pools, &ctx),
                    other => eprintln!("Unknown test in parallel action: {other}"),
                }
            }
        });
    }
}

fn main() {
    println!("antigres: starting up");
    let selected_tests: Vec<String> = parse_selected_tests();

    let connection_strings = parse_connection_strings();
    log_config(&selected_tests, &connection_strings);
    let pools = DbPools::new(&connection_strings);
    let ctx = Ctx::new();

    println!("antigres: verifying postgres connectivity...");
    {
        let mut conn = pools
            .get_connection()
            .expect("Failed to get initial connection");
        conn.query_one("SELECT 1;", &[])
            .expect("Failed initial SELECT 1");
        println!("antigres: postgres is reachable");
        disable_logging(&mut conn);
        println!("antigres: disabled verbose postgres logging");
    }

    let pre_setup_tests: Vec<String> = selected_tests
        .iter()
        .filter(|t| t.as_str() != "egglog")
        .cloned()
        .collect();
    let post_setup_tests: Vec<String> = selected_tests
        .iter()
        .filter(|t| t.as_str() == "egglog")
        .cloned()
        .collect();

    println!("antigres: running pre-setup_complete setups for: {pre_setup_tests:?}");
    let mut pre_results = Vec::new();
    {
        let mut handles = Vec::new();
        for test in &pre_setup_tests {
            let pools = pools.clone();
            let test = test.clone();
            let mut test_ctx = ctx.clone();
            handles.push(thread::spawn(move || {
                setup(test.clone(), &pools, &mut test_ctx);
                (test, test_ctx)
            }));
        }
        for h in handles {
            pre_results.push(h.join().expect("setup thread panicked"));
        }
    }

    println!("antigres: deterministic setups done, signaling setup_complete");
    antithesis_sdk::lifecycle::setup_complete(&().into());

    let mut post_results = Vec::new();
    if !post_setup_tests.is_empty() {
        println!("antigres: running post-setup_complete setups for: {post_setup_tests:?}");
        let mut handles = Vec::new();
        for test in &post_setup_tests {
            let pools = pools.clone();
            let test = test.clone();
            let mut test_ctx = ctx.clone();
            handles.push(thread::spawn(move || {
                setup(test.clone(), &pools, &mut test_ctx);
                (test, test_ctx)
            }));
        }
        for h in handles {
            post_results.push(h.join().expect("setup thread panicked"));
        }
    }

    let n_tests = selected_tests.len();
    let all_results = pre_results.into_iter().chain(post_results);
    for (test, test_ctx) in all_results {
        let pools = pools.clone();
        println!("antigres [{test}]: starting supervisor loop");
        thread::spawn(move || {
            supervisor_loop(test, n_tests, &pools, &test_ctx);
        });
    }

    loop {
        thread::sleep(Duration::from_secs(120));
    }
}
