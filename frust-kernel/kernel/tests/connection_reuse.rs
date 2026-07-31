//! **WO-041: the transport reuses its connection.**
//!
//! `db.rs` used `ureq::post(..)`, the bare free function, which opens a fresh
//! TCP connection per query and drops it. WO-026 measured that model for
//! *throughput* and exonerated it (`bare ≈ fresh ≈ pooled within 10%`) — still
//! true. What it never measured was **resource exhaustion**, and WO-040 Chunk
//! B's load leg found it: TIME_WAIT climbing at request rate until the
//! ephemeral-port range runs out.
//!
//! Two checks, because either alone is weak:
//!
//! 1. **Behavioural** — fire a burst of real queries and watch the socket
//!    count. This is the property; it would still pass if someone changed
//!    *how* reuse is achieved.
//! 2. **Structural** — no module may go back to the bare free functions. This
//!    catches the regression on the day it is written rather than on the day
//!    it exhausts a production box's ports.
//!
//! **The failure mode is demonstrated, not imagined.** `examples/connprobe.rs`
//! runs the same burst through the old model:
//!
//! ```text
//! cargo run --release --example connprobe -- bare  200   # ~200 TIME_WAIT
//! cargo run --release --example connprobe -- agent 200   # ~1  TIME_WAIT
//! ```
//!
//! Keep that runnable: a metric never seen to fail is not yet a metric.
//!
//! Requires the dev surreal on :8899. Windows-only socket counting (`netstat`).

use std::path::Path;

use frust_kernel::db::scoped_db;
use frust_kernel::tenancy::single_tenant;

const QUERIES: usize = 200;

/// TIME_WAIT sockets against the SurrealDB port, process-wide.
///
/// Process-wide rather than per-PID because `netstat -an` does not attribute
/// owners — which makes this a *noisy* instrument, so the threshold below is
/// set where noise cannot reach.
fn time_wait() -> Option<usize> {
    let out = std::process::Command::new("netstat").arg("-an").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    Some(
        text.lines()
            .filter(|l| l.contains("127.0.0.1:8899") && l.contains("TIME_WAIT"))
            .count(),
    )
}

#[test]
fn a_burst_of_queries_does_not_burn_a_socket_apiece() {
    let Some(before) = time_wait() else {
        eprintln!("netstat unavailable — skipping the behavioural half");
        return;
    };

    let target = single_tenant("wo041_reuse").expect("tenancy");
    frust_kernel::provision::provision(&target).expect("provision");
    let db = scoped_db(&target);

    for _ in 0..QUERIES {
        db.sql_root("RETURN 1;").expect("query");
    }

    // TIME_WAIT lingers for minutes, so anything this burst opened is still
    // counted — no settling window can hide it.
    let after = time_wait().expect("netstat");
    let delta = after.saturating_sub(before);

    // With reuse the burst leaves a handful; without it, one per query. The
    // threshold sits an order of magnitude from both, so ordinary background
    // churn cannot move the verdict either way.
    assert!(
        delta < QUERIES / 4,
        "{QUERIES} queries left {delta} new TIME_WAIT sockets (before={before} after={after}) — \
         the transport is opening a connection per query again. Reproduce the two models with \
         `cargo run --release --example connprobe -- bare|agent 200`."
    );
    println!("PASS: {QUERIES} queries left {delta} new TIME_WAIT sockets");
}

/// **The structural half.** `db.rs` is the only module that may speak HTTP to
/// SurrealDB, and it must do so through its shared `Agent`. The bare `ureq`
/// free functions each open and drop a connection, so a single reintroduced
/// call is a slow port leak that no unit test would notice.
#[test]
fn no_module_uses_the_bare_connection_per_call_client() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    visit(&src, &mut violations);
    assert!(
        violations.is_empty(),
        "bare ureq free functions open a fresh connection per call (WO-041) — go through the \
         shared agent instead:\n{}",
        violations.join("\n")
    );
}

fn visit(dir: &Path, violations: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("read src") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            visit(&path, violations);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let content = std::fs::read_to_string(&path).expect("read file");
        for (i, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue; // prose about the old model is not the old model
            }
            for needle in ["ureq::post(", "ureq::get(", "ureq::put(", "ureq::delete("] {
                if trimmed.contains(needle) {
                    violations.push(format!("{name}:{}: {trimmed}", i + 1));
                }
            }
        }
    }
}
