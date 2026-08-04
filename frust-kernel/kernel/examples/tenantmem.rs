//! Tenancy footprint guard.
//!
//! The idle-memory floor (~60 MB idle) is the verdict this tenancy build is
//! most likely to undo, which is why footprint was probed first. This holds N
//! tenant brokers in ONE process and reports resident memory, so the claim
//! "per-tenant cost is kilobytes, not a wasm engine" is measured rather than
//! argued.
//!
//! The whole point of `Broker::with_shared_hooks` is on trial here: every tenant
//! shares ONE `Arc<WasmHooks>` — one wasmtime engine, one epoch ticker, one
//! component load. If footprint grows linearly with tenant count, findings 1–3
//! did not cover it and there is more per-tenant state to share.
//!
//!   cargo run --release --example tenantmem -- <N>
//!
//! Prints one line of JSON so a runner can sweep N and compare.

use std::sync::Arc;

use frust_kernel::broker::Broker;
use frust_kernel::db::scoped_db;
use frust_kernel::tenancy::single_tenant;
use frust_kernel::hooks::WasmHooks;

/// Resident set size, in bytes. **Windows/PowerShell only.** It shells out to
/// PowerShell for `WorkingSet64`; on a system without PowerShell (Linux, macOS,
/// or a stripped Windows) the command fails and this returns `0`, meaning
/// "unavailable" rather than "zero memory". The JSON output carries an
/// `rss_available` flag so a consumer is not misled by the zero. A
/// cross-platform reading (e.g. `/proc/self/statm` on Linux) is not implemented.
fn rss_bytes() -> u64 {
    // Windows: ask the OS for this process's working set via wmic-free PS.
    let pid = std::process::id();
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!("(Get-Process -Id {pid}).WorkingSet64"),
        ])
        .output();
    out.ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(1);
    let artifacts = std::env::var("FRUST_ARTIFACTS")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts").to_string());

    let baseline = rss_bytes();

    // ONE engine for the whole process — the shared-hooks change under test.
    let hooks: Arc<dyn frust_kernel::broker::HookDispatch> =
        Arc::from(Box::new(WasmHooks::load(&artifacts).expect("load hooks")) as Box<dyn frust_kernel::broker::HookDispatch>);
    let after_engine = rss_bytes();

    // N tenants = N (Db, Broker) pairs, all sharing that engine. `Db::new` is
    // lazy (no connection until a query), so this measures the in-process
    // per-tenant cost — which is exactly what "idle footprint at N tenants" means.
    let mut tenants: Vec<Broker> = Vec::with_capacity(n);
    for i in 0..n {
        let cfg = single_tenant(&format!("tenant_{i}")).expect("tenancy");
        tenants.push(Broker::with_shared_hooks(scoped_db(&cfg), Arc::clone(&hooks)));
    }
    let loaded = rss_bytes();

    // keep them alive across the measurement
    let alive = tenants.len();
    let per_tenant = if alive > 0 { (loaded.saturating_sub(after_engine)) as f64 / alive as f64 } else { 0.0 };

    // If every reading is 0 the RSS probe is unavailable on this platform (see
    // `rss_bytes`); flag it so the memory figures are not read as real zeros.
    let rss_available = baseline > 0 || after_engine > 0 || loaded > 0;

    println!(
        "{{\"tenants\":{alive},\"rss_available\":{rss_available},\"baseline_mb\":{:.1},\"after_engine_mb\":{:.1},\"loaded_mb\":{:.1},\"per_tenant_kb\":{:.1}}}",
        baseline as f64 / 1e6,
        after_engine as f64 / 1e6,
        loaded as f64 / 1e6,
        per_tenant / 1e3,
    );
    std::hint::black_box(&tenants);
}
