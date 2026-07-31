//! WO-026 criterion 1 — the connection-model probe.
//!
//! Drives writes STRAIGHT AT SurrealDB, bypassing the kernel, so the DB's own
//! write ceiling can be compared against the through-kernel number (WO-025:
//! 48 req/s, `db_write` 222 ms avg). Three modes, because "the DB is slow" and
//! "we talk to the DB badly" look identical from the kernel:
//!
//!   pooled  — one shared `ureq::Agent`, connections reused. SurrealDB's true
//!             concurrent-write ceiling.
//!   fresh   — a NEW Agent per request, mimicking a client that does not pool.
//!             The delta against `pooled` IS the connection-setup cost.
//!   bare    — `ureq::post(...)`, the exact call shape `db.rs` uses today.
//!             Tells us which of the two above the kernel actually behaves like.
//!
//!   cargo run --release --example dbbench -- <mode> <concurrency> <secs>

use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

const ENDPOINT: &str = "http://127.0.0.1:8899";
const AUTH: &str = "Basic cm9vdDpyb290"; // root:root

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).cloned().unwrap_or_else(|| "pooled".into());
    let concurrency: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);
    let secs: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(8);

    let shared: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_idle_connections_per_host(concurrency + 8)
        .build()
        .into();

    // one table, schemaless, no events/permissions — the raw engine write cost
    // `w`  — bare schemaless: the engine's raw write cost.
    // `ws` — KERNEL-SHAPED: SCHEMAFULL + changefeed + the identity guard and
    //        docstatus-lattice EVENTs the compiler emits for a submittable
    //        DocType. Writing here isolates "what our table shape costs" from
    //        "what the kernel's per-request work costs" — without it, a 3× gap
    //        between raw and through-kernel is unattributable.
    let _ = shared
        .post(format!("{ENDPOINT}/sql"))
        .header("Authorization", AUTH)
        .header("Accept", "application/json")
        .send(
            "USE NS bench DB bench; DEFINE DATABASE IF NOT EXISTS bench; \
             DEFINE TABLE IF NOT EXISTS w SCHEMALESS; \
             DEFINE TABLE IF NOT EXISTS ws SCHEMAFULL CHANGEFEED 7d; \
             DEFINE FIELD IF NOT EXISTS title ON ws TYPE string; \
             DEFINE FIELD IF NOT EXISTS amount ON ws TYPE decimal ASSERT $value >= 0dec; \
             DEFINE FIELD IF NOT EXISTS status ON ws TYPE string DEFAULT 'Draft'; \
             DEFINE FIELD IF NOT EXISTS docstatus ON ws TYPE int DEFAULT 0; \
             DEFINE EVENT IF NOT EXISTS identity_guard ON TABLE ws WHEN $event = 'CREATE' THEN { \
               IF $auth != NONE AND $after.owner = NONE { THROW 'FRUST:E_IDENTITY_UNRESOLVED'; }; }; \
             DEFINE EVENT IF NOT EXISTS docstatus_lattice ON TABLE ws WHEN $event = 'UPDATE' THEN { \
               IF $before.docstatus = 2 { THROW 'FRUST:E_DOCSTATUS:RESURRECTION'; }; };",
        );

    let deadline = Instant::now() + Duration::from_secs(secs);
    let barrier = Arc::new(Barrier::new(concurrency + 1));
    let mut handles = Vec::new();

    for _ in 0..concurrency {
        let shared = shared.clone();
        let mode = mode.clone();
        let barrier = Arc::clone(&barrier);
        let h = std::thread::Builder::new()
            .stack_size(512 * 1024)
            .spawn(move || {
                let mut lat = Vec::with_capacity(4096);
                let (mut ok, mut err) = (0u64, 0u64);
                let body = if mode == "schema" {
                    "USE NS bench DB bench; CREATE ws SET title = 'load', amount = 1.00dec;"
                } else {
                    "USE NS bench DB bench; CREATE w SET title = 'load', amount = 1.00dec;"
                };
                barrier.wait();
                while Instant::now() < deadline {
                    let t = Instant::now();
                    let res = match mode.as_str() {
                        // a fresh Agent per request: no connection reuse
                        "fresh" => {
                            let a: ureq::Agent = ureq::Agent::config_builder()
                                .http_status_as_error(false)
                                .build()
                                .into();
                            a.post(format!("{ENDPOINT}/sql"))
                                .header("Authorization", AUTH)
                                .header("Accept", "application/json")
                                .send(body)
                        }
                        // exactly what db.rs does today
                        "bare" => ureq::post(format!("{ENDPOINT}/sql"))
                            .header("Authorization", AUTH)
                            .header("Accept", "application/json")
                            .send(body),
                        // shared pooled agent
                        _ => shared
                            .post(format!("{ENDPOINT}/sql"))
                            .header("Authorization", AUTH)
                            .header("Accept", "application/json")
                            .send(body),
                    };
                    lat.push(t.elapsed().as_micros() as u64);
                    match res {
                        Ok(mut r) => {
                            let _ = r.body_mut().read_to_string();
                            if r.status().as_u16() == 200 { ok += 1 } else { err += 1 }
                        }
                        Err(_) => err += 1,
                    }
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
        let i = ((all.len() as f64 * p).ceil() as usize).saturating_sub(1).min(all.len() - 1);
        all[i] as f64 / 1000.0
    };
    let total = ok + err;
    println!(
        "mode={mode:<7} concurrency={concurrency:<4} reqs={total:<7} ok={ok:<7} err={err:<5} \
         thru={:8.1} w/s  p50={:7.1}ms p95={:7.1}ms p99={:7.1}ms",
        total as f64 / wall,
        pct(0.50),
        pct(0.95),
        pct(0.99),
    );
}
