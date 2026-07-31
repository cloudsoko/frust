//! WO-041 probe — **does the transport reuse its connection?**
//!
//! The kernel's `db.rs` calls `ureq::post(..)`, the bare free function. WO-040
//! Chunk B measured TIME_WAIT climbing at request rate against SurrealDB
//! (118 → 1252 → 4274 → 6150 across three benches), which points at a
//! connection per query — but "points at" is not "measured", and the fix
//! depends on WHY. So this is an A/B of the two client models, run before
//! anything in `db.rs` changes:
//!
//!   cargo run --release --example connprobe -- bare  200
//!   cargo run --release --example connprobe -- agent 200
//!
//! Count TIME_WAIT sockets to the endpoint before and after each. A model that
//! reuses its connection leaves a handful; one that does not leaves ~N.
//!
//! Prints the wall time too, so the throughput dimension WO-026 measured is
//! visible alongside the resource dimension it never measured.

use std::time::Instant;

const EP: &str = "http://127.0.0.1:8899/sql";

fn auth() -> String {
    use base64::Engine as _;
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("root:root")
    )
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).cloned().unwrap_or_else(|| "bare".into());
    let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(200);

    let t = Instant::now();
    let mut ok = 0usize;

    match mode.as_str() {
        // exactly what `db.rs` does today
        "bare" => {
            for _ in 0..n {
                let res = ureq::post(EP)
                    .header("Authorization", &auth())
                    .header("Accept", "application/json")
                    .header("surreal-ns", "frust")
                    .header("surreal-db", "skeleton")
                    .send("RETURN 1;");
                if let Ok(mut r) = res {
                    let _ = r.body_mut().read_to_string();
                    ok += 1;
                }
            }
        }
        // one persistent agent, reused for every request
        "agent" => {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .max_idle_connections_per_host(4)
                .build()
                .into();
            for _ in 0..n {
                let res = agent
                    .post(EP)
                    .header("Authorization", &auth())
                    .header("Accept", "application/json")
                    .header("surreal-ns", "frust")
                    .header("surreal-db", "skeleton")
                    .send("RETURN 1;");
                if let Ok(mut r) = res {
                    let _ = r.body_mut().read_to_string();
                    ok += 1;
                }
            }
        }
        other => {
            eprintln!("unknown mode {other:?} (bare | agent)");
            std::process::exit(2);
        }
    }

    let ms = t.elapsed().as_millis();
    println!(
        "mode={mode:<6} requests={n} ok={ok} wall={ms}ms rate={:.1} req/s",
        n as f64 / (ms as f64 / 1000.0)
    );
}
