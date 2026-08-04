//! REQ-6.1.2: performance floors as CI GATES.
//! These are failing tests, not log numbers â€” a regression past the budget
//! turns the build red. Warm-path medians on the reference machine; generous
//! enough to absorb machine variance, tight enough to catch a real
//! regression (a synchronous hook stall, an accidental N+1, a lost index).

use std::sync::Arc;
use std::time::Instant;

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::*;
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};
use frust_kernel::hooks::WasmHooks;

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

/// REQ-6.1.1 warm submit budget. 25 ms is the spec floor and the RELEASE
/// gate — a measured 24 ms warm median in release against a 1 M-row
/// table, so release builds are held to the floor itself. Debug builds gate
/// at 60 ms (JIT + unoptimized wasmtime inflate the same path). Adjust ONLY
/// with a recorded ruling â€” this number is a contract (REQ-6.1.2).
const SUBMIT_GATE_MS: u128 = if cfg!(debug_assertions) { 60 } else { 25 };
const HOOK_GATE_MS: u128 = 30;

fn setup(name: &str) -> (Db, ResolvedTenant) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    db.sql_root(
        "DEFINE TABLE app_user SCHEMAFULL PERMISSIONS NONE; \
         DEFINE FIELD name ON app_user TYPE string; \
         DEFINE FIELD role ON app_user TYPE string; \
         DEFINE FIELD pass ON app_user TYPE string; \
         DEFINE ACCESS account ON DATABASE TYPE RECORD \
           SIGNIN (SELECT * FROM app_user WHERE name = $name AND crypto::argon2::compare(pass, $pass)) \
           DURATION FOR TOKEN 1h, FOR SESSION 12h; \
         CREATE app_user:u SET name = 'u', role = 'manager', pass = crypto::argon2::generate('pw-u');",
    )
    .unwrap();
    db.sql_root("DEFINE TABLE thing SCHEMALESS PERMISSIONS FULL; DEFINE FIELD owner ON thing TYPE option<record<app_user>> DEFAULT $auth.id;").unwrap();
    db.sql_root("DEFINE TABLE doctype SCHEMALESS; CREATE doctype CONTENT { name: 'thing', label: 'Thing', fields: [] };").unwrap();
    (db, cfg)
}

fn broker(cfg: &ResolvedTenant) -> Arc<Broker> {
    Arc::new(Broker::new(scoped_db(&cfg), Box::new(WasmHooks::load(artifacts()).unwrap())))
}

/// The submit gates measure LATENCY: they must not run concurrently with
/// each other (or they measure their own contention). Same serialization
/// pattern the permission proofs use.
fn gate_lock() -> std::sync::MutexGuard<'static, ()> {
    static GUARD: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    GUARD.get_or_init(|| std::sync::Mutex::new(())).lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn median(mut v: Vec<u128>) -> u128 {
    v.sort_unstable();
    v[v.len() / 2]
}

/// One batch of warm submit samples, in MICROSECONDS, over `n` parked live
/// subscriptions on the gated table.
///
/// Microseconds, not milliseconds: `as_millis()` truncates every sample to a
/// whole millisecond, and the realtime gate below adjudicates a 2 ms allowance
/// on a ~27 ms base. A Â±1 ms-quantized instrument cannot resolve that, and it
/// was reading noise as tax (an earlier instrument read five runs at
/// 3/3/0/2/6 ms, one of them measuring the loaded case FASTER than its own
/// baseline). Ruling: fix the instrument, move neither published number.
///
/// The broker is passed IN, never built here: `broker()` calls
/// `WasmHooks::load`, which compiles the 4 MB engine. Building one per batch
/// turned an interleaved run into twelve wasm compiles and made every batch a
/// cold start â€” noise the instrument itself manufactured.
fn submit_batch_us(
    b: &Arc<Broker>,
    cfg: &ResolvedTenant,
    db: &Db,
    n: usize,
    warm: usize,
    samples: usize,
) -> Vec<u128> {
    use frust_kernel::realtime::Realtime;

    let caller = Caller { user: "u".into(), pass: "pw-u".into(), role: "manager".into() };
    // Every batch starts from an EMPTY table. Otherwise the table grows
    // monotonically across a multi-round run and each batch writes into a
    // bigger one than the last â€” a cost that counterbalancing can only average
    // out, never remove, and that swamps the ~1 ms signal being measured.
    db.sql_root("DELETE thing;").expect("reset the gated table");
    let realtime = Realtime::new(cfg.endpoint());
    let mut subs = Vec::new();
    if n > 0 {
        let jwt = db.signin("u", "pw-u").expect("jwt for subscriptions");
        for _ in 0..n {
            subs.push(realtime.subscribe("u", &jwt, &cfg, "thing").expect("parked subscription"));
        }
    }
    let doc = vec![
        ("title".to_string(), Value::Text("perf".into())),
        ("status".to_string(), Value::Text("Draft".into())),
        ("total".to_string(), Value::Float(100.0)),
    ];
    // warm the token cache + hook instances
    for _ in 0..warm {
        b.db_write(&caller, &HookChain::default(), WriteOp::Create, "thing", None, &doc).unwrap();
    }
    let mut out = Vec::with_capacity(samples);
    for _ in 0..samples {
        let t = Instant::now();
        b.db_write(&caller, &HookChain::default(), WriteOp::Create, "thing", None, &doc).unwrap();
        out.push(t.elapsed().as_micros());
    }
    for s in &subs {
        let _ = realtime.unsubscribe(s, "u");
    }
    out
}

/// Warm submit median in whole milliseconds â€” the shape the absolute floor
/// gates are written against (REQ-6.1.1).
fn submit_median_with_subs(cfg: &ResolvedTenant, db: &Db, n: usize) -> u128 {
    median(submit_batch_us(&broker(cfg), cfg, db, n, 20, 40)) / 1000
}

/// GATE: warm submit (form -> hooks -> authenticated write) median under
/// budget. REQ-6.1.1's contract, measured exactly as specified â€” the write
/// path alone, no optional features loaded into it.
#[test]
fn gate_submit_latency() {
    let _g = gate_lock();
    let (db, cfg) = setup("perf_gate");
    let m = submit_median_with_subs(&cfg, &db, 0);
    println!("submit warm median: {m} ms (gate {SUBMIT_GATE_MS} ms)");
    assert!(m <= SUBMIT_GATE_MS, "REQ-6.1.2 REGRESSION: submit median {m} ms > gate {SUBMIT_GATE_MS} ms");
}

/// GATE: the realtime writer tax, priced into CI rather than around it.
/// Parks the FULL per-table subscription budget on the written
/// table and asserts the DELTA over the same run's unsubscribed baseline
/// stays inside `LIVE_TAX_BUDGET_MS`.
///
/// Measuring the delta in-run (not against a remembered number) is what
/// makes this honest: machine drift moves both halves together, so the gate
/// judges realtime's cost and nothing else. If it breaches, the fix is a
/// smaller `LIVE_SUB_BUDGET_PER_TABLE` â€” never a bigger allowance.
#[test]
fn gate_submit_with_live_subscriptions() {
    use frust_kernel::realtime::{LIVE_SUB_BUDGET_PER_TABLE, LIVE_TAX_BUDGET_MS};

    // A/B/A/B, not AAAA/BBBB. The original measured the whole baseline, then
    // the whole loaded run, on the premise that "machine drift moves both
    // halves together". That premise only holds while drift is slow relative
    // to the measurement â€” and it is not: one observed run drifted 13 ms
    // BETWEEN the halves and reported the loaded case as faster than its own
    // baseline. Alternating short batches puts any drift into both conditions
    // in equal measure, which is what makes the difference attributable to
    // subscriptions rather than to the machine.
    const ROUNDS: usize = 12;
    const PER_BATCH: usize = 12;

    let _g = gate_lock();
    let (db, cfg) = setup("perf_gate_rt");

    // PAIRED: the tax is the median of per-round DIFFERENCES, not the
    // difference of pooled medians. Interleaving only buys drift cancellation
    // if the two conditions are compared to their own neighbour in time;
    // pooling first throws that away and leaves the estimator as exposed to
    // slow drift as the original was.
    let bk = broker(&cfg); // ONE engine compile for the whole gate
    let mut base_us = Vec::new();
    let mut loaded_us = Vec::new();
    let mut diffs: Vec<i128> = Vec::with_capacity(ROUNDS);
    for round in 0..ROUNDS {
        // only the first batch of each condition pays the warm-up
        let warm = if round == 0 { 20 } else { 2 };
        // COUNTERBALANCED: A/B on even rounds, B/A on odd. Every batch writes
        // rows into `thing`, so the table grows monotonically through the run
        // and a later batch is inherently slower than an earlier one. With a
        // fixed A-then-B order that growth lands entirely on B and is
        // indistinguishable from tax â€” which is exactly what it was being
        // read as: lowering the subscription budget made the "tax" go UP.
        // Alternating the order puts the growth penalty on both conditions
        // equally, so it cancels in the paired difference.
        let (b, l) = if round % 2 == 0 {
            let b = submit_batch_us(&bk, &cfg, &db, 0, warm, PER_BATCH);
            let l = submit_batch_us(&bk, &cfg, &db, LIVE_SUB_BUDGET_PER_TABLE, warm, PER_BATCH);
            (b, l)
        } else {
            let l = submit_batch_us(&bk, &cfg, &db, LIVE_SUB_BUDGET_PER_TABLE, warm, PER_BATCH);
            let b = submit_batch_us(&bk, &cfg, &db, 0, warm, PER_BATCH);
            (b, l)
        };
        diffs.push(median(l.clone()) as i128 - median(b.clone()) as i128);
        base_us.extend(b);
        loaded_us.extend(l);
    }
    diffs.sort_unstable();
    let paired = diffs[diffs.len() / 2];
    println!("per-round tax (Âµs): {diffs:?}");

    let base = median(base_us);
    let loaded = median(loaded_us);
    let tax_us = paired.max(0) as u128;
    let allowance_us = LIVE_TAX_BUDGET_MS * 1000;
    println!(
        "submit warm median: {:.2} ms baseline -> {:.2} ms with {LIVE_SUB_BUDGET_PER_TABLE} parked subs \
         (realtime tax {:.2} ms, allowance {LIVE_TAX_BUDGET_MS} ms; {ROUNDS}x{PER_BATCH} interleaved, Âµs-sampled)",
        base as f64 / 1000.0,
        loaded as f64 / 1000.0,
        tax_us as f64 / 1000.0
    );
    assert!(
        tax_us <= allowance_us,
        "REALTIME TAX REGRESSION: {:.2} ms over baseline at budget {LIVE_SUB_BUDGET_PER_TABLE} \
         (allowance {LIVE_TAX_BUDGET_MS} ms) â€” lower the budget, do not widen the allowance",
        tax_us as f64 / 1000.0
    );
}

/// GATE: hook overhead (both classes, chained) median under budget.
#[test]
fn gate_hook_overhead() {
    use frust_kernel::broker::HookDispatch;
    let hooks = WasmHooks::load(artifacts()).unwrap();
    let doc = vec![
        ("id".to_string(), Value::Text("thing:1".into())),
        ("status".to_string(), Value::Text("Draft".into())),
        ("total".to_string(), Value::Float(100.0)),
    ];
    for _ in 0..50 {
        hooks.validate("thing", &doc).unwrap();
    }
    let mut samples = Vec::new();
    for _ in 0..100 {
        let t = Instant::now();
        hooks.validate("thing", &doc).unwrap();
        samples.push(t.elapsed().as_millis());
    }
    let m = median(samples);
    println!("hook chain warm median: {m} ms (gate {HOOK_GATE_MS} ms)");
    assert!(m <= HOOK_GATE_MS, "hook overhead REGRESSION: {m} ms > gate {HOOK_GATE_MS} ms");
}

