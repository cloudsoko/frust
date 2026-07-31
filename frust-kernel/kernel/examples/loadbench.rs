//! WO-024 load driver — pure measurement, no optimization.
//!
//! Drives `frust serve`'s real submit path (login once, then N concurrent
//! clients POSTing /write) and reports throughput + p50/p95/p99. Blocking
//! `ureq` across N OS threads: each thread is one concurrent client, which is
//! the honest model for measuring latency under contention (an async client
//! would hide queueing behind its own scheduler).
//!
//!   cargo run --release --bin loadbench -- <concurrency> <seconds> [addr]

use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let concurrency: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);
    let addr = args.get(3).cloned().unwrap_or_else(|| "http://127.0.0.1:8790".into());

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_idle_connections_per_host(concurrency + 8)
        .build()
        .into();

    // login once — the token is read-only, shared across clients
    let token: String = {
        let mut r = agent
            .post(format!("{addr}/login"))
            .send(r#"{"user":"u","pass":"pw-u"}"#)
            .expect("login");
        let v: serde_json::Value = r.body_mut().read_json().expect("login json");
        v["token"].as_str().expect("token").to_string()
    };

    let deadline = Instant::now() + Duration::from_secs(secs);
    let barrier = Arc::new(Barrier::new(concurrency + 1));
    let mut handles = Vec::new();

    for _ in 0..concurrency {
        let agent = agent.clone();
        let addr = addr.clone();
        let token = token.clone();
        let barrier = Arc::clone(&barrier);
        let h = std::thread::Builder::new()
            .stack_size(512 * 1024)
            .spawn(move || {
                let mut lat = Vec::with_capacity(4096);
                let mut ok = 0u64;
                let mut err = 0u64;
                let body = r#"{"doc":{"title":"load","amount":{"kind":"decimal","v":"1.00"}}}"#;
                barrier.wait();
                while Instant::now() < deadline {
                    let t = Instant::now();
                    let res = agent
                        .post(format!("{addr}/write/thing"))
                        .header("Authorization", &format!("Bearer {token}"))
                        .send(body);
                    let us = t.elapsed().as_micros() as u64;
                    match res {
                        Ok(mut r) => {
                            let _ = r.body_mut().read_to_string();
                            if r.status().as_u16() == 200 { ok += 1 } else { err += 1 }
                        }
                        Err(_) => err += 1,
                    }
                    lat.push(us);
                }
                (lat, ok, err)
            })
            .unwrap();
        handles.push(h);
    }

    barrier.wait();
    let wall = Instant::now();
    let mut all = Vec::new();
    let (mut ok, mut err) = (0u64, 0u64);
    for h in handles {
        let (lat, o, e) = h.join().unwrap();
        all.extend(lat);
        ok += o;
        err += e;
    }
    let wall = wall.elapsed().as_secs_f64();

    all.sort_unstable();
    let pct = |p: f64| -> f64 {
        if all.is_empty() {
            return 0.0;
        }
        let idx = ((all.len() as f64 * p).ceil() as usize).saturating_sub(1).min(all.len() - 1);
        all[idx] as f64 / 1000.0 // ms
    };
    let total = ok + err;
    let thru = total as f64 / wall;

    println!(
        "concurrency={concurrency:<4} reqs={total:<7} ok={ok:<7} err={err:<5} \
         thru={thru:8.1} req/s  p50={:6.1}ms p95={:6.1}ms p99={:6.1}ms",
        pct(0.50),
        pct(0.95),
        pct(0.99),
    );
}
