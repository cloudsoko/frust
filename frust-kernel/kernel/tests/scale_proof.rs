//! The 1 M-row scale proof, THROUGH the kernel (broker + permission
//! compiler + index policy + REST), under a record-user principal — not raw
//! /sql as root. Run explicitly, release mode, against the seeded scratch
//! instance:
//!
//!   cargo test --release --test scale_proof -- --ignored --nocapture
//!
//! Phase order matters: read shapes first, then the release-mode submit
//! floor on the clean table, then CHANGEFEED goes on (criterion 5) and the
//! submit floor repeats for the delta, then the index A/B (criterion 2) last
//! so no index pollutes the earlier phases.

use std::sync::Arc;
use std::time::Instant;

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::*;
use frust_kernel::db::{scoped_db, ConnConfig};
use frust_kernel::tenancy::{ResolvedTenant, Tenancy};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::rest::Rest;

const DATA_DIR: &str = "D:/Dev/rust/frust-scale-data";

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

fn cfg() -> ResolvedTenant {
    // the fixture instance: its own endpoint AND its own namespace, so
    // this goes through `from_config` rather than the environment default.
    Tenancy::from_config(
        ConnConfig { endpoint: "http://127.0.0.1:8898".into(), ..ConnConfig::default() },
        frust_kernel::tenancy::TenancyConfig {
            strategy: "database-per-tenant",
            namespace: Some("scale"),
            database: None,
            environment: None,
            tenants: &["erp"],
        },
    )
    .expect("tenancy")
    .sole()
    .expect("sole tenant")
}

fn store_bytes() -> u64 {
    fn walk(p: &std::path::Path, acc: &mut u64) {
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                let path = e.path();
                if path.is_dir() {
                    walk(&path, acc);
                } else if let Ok(m) = e.metadata() {
                    *acc += m.len();
                }
            }
        }
    }
    let mut n = 0;
    walk(std::path::Path::new(DATA_DIR), &mut n);
    n
}

/// Session token for the analyst (sessions replaced the header trio).
fn analyst_token(url: &str) -> String {
    static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TOKEN
        .get_or_init(|| {
            let mut r = ureq::post(format!("{url}/login"))
                .send(serde_json::json!({ "user": "analyst", "pass": "pw-analyst" }).to_string())
                .expect("login");
            let body: serde_json::Value = r.body_mut().read_json().expect("login json");
            body["token"].as_str().expect("token").to_string()
        })
        .clone()
}

/// POST to the REST surface as the analyst; returns (status, body, wall ms).
fn rest(url: &str, route: &str, body: serde_json::Value) -> (u16, serde_json::Value, u128) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(std::time::Duration::from_secs(600)))
        .build()
        .into();
    let token = analyst_token(url);
    let t = Instant::now();
    let mut r = agent
        .post(format!("{url}{route}"))
        .header("Authorization", &format!("Bearer {token}"))
        .send(body.to_string())
        .expect("rest transport");
    let ms = t.elapsed().as_millis();
    let code = r.status().as_u16();
    let parsed = r.body_mut().read_json().unwrap_or(serde_json::json!({}));
    (code, parsed, ms)
}

/// Run a shape 3x through REST; print week-1 style "cold  warm  warm".
fn shape(url: &str, name: &str, route: &str, body: serde_json::Value) -> u128 {
    let mut times = Vec::new();
    let mut rows = 0;
    for _ in 0..3 {
        let (code, out, ms) = rest(url, route, body.clone());
        assert_eq!(code, 200, "{name}: {out}");
        rows = out["rows"].as_array().map(|a| a.len()).unwrap_or(0);
        times.push(ms);
    }
    println!("{name:<44} {}ms  {}ms  {}ms   rows: {rows}", times[0], times[1], times[2]);
    times[2]
}

fn median(mut v: Vec<u128>) -> u128 {
    v.sort_unstable();
    v[v.len() / 2]
}

/// Warm submit medians through the broker (the REQ-6.1.1 path: hooks both
/// classes + authenticated write under the caller's session).
fn submit_median(b: &Broker, caller: &Caller, n: usize) -> u128 {
    let doc = vec![
        ("title".to_string(), Value::Text("scale probe".into())),
        ("status".to_string(), Value::Text("Draft".into())),
        ("total".to_string(), Value::Float(120.0)),
    ];
    for _ in 0..15 {
        b.db_write(caller, &HookChain::default(), WriteOp::Create, "sales_invoice", None, &doc).unwrap();
    }
    let mut samples = Vec::new();
    for _ in 0..n {
        let t = Instant::now();
        b.db_write(caller, &HookChain::default(), WriteOp::Create, "sales_invoice", None, &doc).unwrap();
        samples.push(t.elapsed().as_millis());
    }
    median(samples)
}

#[test]
#[ignore = "scale run: needs the seeded scratch instance on :8898"]
fn scale_proof_1m() {
    let admin = scoped_db(&cfg());
    let count = admin.sql_root("SELECT count() FROM sales_invoice GROUP ALL;").unwrap();
    let n = count.as_array().and_then(|a| a.first()).and_then(|r| r["count"].as_i64()).unwrap_or(0);
    println!("=== scale proof: {n} invoices, store {} MB, {} build ===",
        store_bytes() / 1_048_576,
        if cfg!(debug_assertions) { "DEBUG (invalid for criterion 3)" } else { "RELEASE" });

    let broker = Arc::new(Broker::new(scoped_db(&cfg()), Box::new(WasmHooks::load(artifacts()).unwrap())));
    let caller = Caller { user: "analyst".into(), pass: "pw-analyst".into(), role: "manager".into() };
    let addr = "127.0.0.1:8797";
    let url = format!("http://{addr}");
    {
        let b = Arc::clone(&broker);
        std::thread::spawn(move || {
            let _ = Rest::single(b, addr.into(), None, None).serve(|| {});
        });
    }
    for _ in 0..50 {
        if ureq::get(format!("{url}/health")).call().is_ok() { break; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // ── Criterion 1: the week-1 shapes, kernel path, record principal ──
    println!("\n-- shapes over REST (cold, warm, warm) --");
    shape(&url, "Q1 register (range+sort, broker NOINDEX)", "/read/sales_invoice", serde_json::json!({
        "filter": { "and": [
            { "path": "posting_date", "op": "gte", "value": { "kind": "datetime", "v": "2026-01-01T00:00:00Z" } },
            { "path": "posting_date", "op": "lt",  "value": { "kind": "datetime", "v": "2026-04-01T00:00:00Z" } },
            { "path": "status", "op": "eq", "value": "Paid" }
        ]},
        "fields": ["id", "posting_date", "customer", "total"],
        "order": { "path": "posting_date", "dir": "desc" },
        "limit": 50
    }));
    shape(&url, "Q2 monthly revenue (group by stored month)", "/aggregate/sales_invoice", serde_json::json!({
        "group_by": ["month"],
        "metrics": [ { "metric": "sum", "path": "total" }, { "metric": "count" } ]
    }));
    shape(&url, "Q3 revenue by customer group (2-hop)", "/aggregate/sales_invoice", serde_json::json!({
        "group_by": ["customer.grp.name"],
        "metrics": [ { "metric": "sum", "path": "total" } ]
    }));
    shape(&url, "Q5 outstanding by customer (filter+hop)", "/aggregate/sales_invoice", serde_json::json!({
        "filter": { "path": "status", "op": "ne", "value": "Paid" },
        "group_by": ["customer.name"],
        "metrics": [ { "metric": "sum", "path": "outstanding" } ]
    }));
    shape(&url, "Q6 one customer statement", "/read/sales_invoice", serde_json::json!({
        "filter": { "path": "customer", "op": "eq", "value": { "kind": "record", "v": "customer:1042" } },
        "fields": ["id", "posting_date", "total", "outstanding"],
        "order": { "path": "posting_date", "dir": "desc" }
    }));

    // Q4 (item-wise flatten) is NOT expressible through the closed contract —
    // that is a contract finding, not a harness gap. Engine-only number for the
    // decision table, root /sql, labeled as such:
    println!("\n-- Q4 flatten: contract-inexpressible; engine-only number --");
    for i in 0..3 {
        let t = Instant::now();
        let out = admin.sql_root(
            "SELECT item, math::sum(amount) AS amount FROM (SELECT VALUE lines FROM sales_invoice).flatten() GROUP BY item ORDER BY amount DESC LIMIT 20 TIMEOUT 590s;",
        );
        println!("Q4 item-wise flatten (ROOT, engine-only)   run{}: {}ms ok={}", i, t.elapsed().as_millis(), out.is_ok());
    }

    // ── Criterion 3: release-mode submit floor at 1 M ──
    println!("\n-- submit floor (broker path, warm median of 40) --");
    let pre_cf_bytes = store_bytes();
    let m_plain = submit_median(&broker, &caller, 40);
    println!("submit warm median, changefeed OFF: {m_plain} ms  (REQ-6.1.1 floor: 25 ms)");

    // ── Criterion 5: changefeed cost — latency delta + storage growth ──
    println!("\n-- changefeed on: latency delta + storage --");
    admin
        .sql_root(
            "DEFINE TABLE OVERWRITE sales_invoice SCHEMALESS \
             PERMISSIONS FOR select, create, update WHERE $auth.role = 'manager' \
             CHANGEFEED 3d;",
        )
        .unwrap();
    let m_cf = submit_median(&broker, &caller, 40);
    println!("submit warm median, changefeed ON:  {m_cf} ms  (delta {} ms)", m_cf as i128 - m_plain as i128);
    // 500 more writes to make storage growth measurable
    let g0 = store_bytes();
    let doc = vec![
        ("title".to_string(), Value::Text("cf growth".into())),
        ("status".to_string(), Value::Text("Draft".into())),
        ("total".to_string(), Value::Float(99.0)),
    ];
    for _ in 0..500 {
        broker.db_write(&caller, &HookChain::default(), WriteOp::Create, "sales_invoice", None, &doc).unwrap();
    }
    let g1 = store_bytes();
    println!("storage: pre-phase {} MB; +500 cf-writes: +{} KB ({} bytes/write incl. feed)",
        pre_cf_bytes / 1_048_576, (g1 - g0) / 1024, (g1 - g0) / 500);
    let t = Instant::now();
    let feed = admin.sql_root("SHOW CHANGES FOR TABLE sales_invoice SINCE 1 LIMIT 100;").unwrap();
    println!("SHOW CHANGES first-100 read: {} ms, {} entries", t.elapsed().as_millis(),
        feed.as_array().map(|a| a.len()).unwrap_or(0));

    // ── Criterion 2: the #7432 index posture at 1 M (engine A/B, root) ──
    println!("\n-- index A/B at scale (root /sql; broker stays NOINDEX) --");
    let t = Instant::now();
    admin.sql_root("DEFINE INDEX idx_si_status_date ON sales_invoice FIELDS status, posting_date;").unwrap();
    println!("compound index build (blocking): {} s", t.elapsed().as_secs());
    let q1 = "SELECT id, posting_date, customer, total FROM sales_invoice WHERE posting_date >= d'2026-01-01T00:00:00Z' AND posting_date < d'2026-04-01T00:00:00Z' AND status = 'Paid' ORDER BY posting_date DESC LIMIT 50 TIMEOUT 590s;";
    let q1_noindex = q1.replacen("FROM sales_invoice", "FROM sales_invoice WITH NOINDEX", 1);
    for i in 0..3 {
        let t = Instant::now();
        let r = admin.sql_root(q1);
        println!("Q1 WITH index    run{}: {} ms ok={}", i, t.elapsed().as_millis(), r.is_ok());
    }
    for i in 0..3 {
        let t = Instant::now();
        let r = admin.sql_root(&q1_noindex);
        println!("Q1 WITH NOINDEX  run{}: {} ms ok={}", i, t.elapsed().as_millis(), r.is_ok());
    }
    // kernel path unchanged by the index (its posture): re-run Q1 via REST
    shape(&url, "Q1 via kernel (posture check, index present)", "/read/sales_invoice", serde_json::json!({
        "filter": { "and": [
            { "path": "posting_date", "op": "gte", "value": { "kind": "datetime", "v": "2026-01-01T00:00:00Z" } },
            { "path": "posting_date", "op": "lt",  "value": { "kind": "datetime", "v": "2026-04-01T00:00:00Z" } },
            { "path": "status", "op": "eq", "value": "Paid" }
        ]},
        "fields": ["id", "posting_date", "customer", "total"],
        "order": { "path": "posting_date", "dir": "desc" },
        "limit": 50
    }));

    println!("\n=== scale proof complete ===");
}
