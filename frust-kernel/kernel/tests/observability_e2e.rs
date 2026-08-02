//! WO-010 exit criteria 1, 2, 3, 4 — EXECUTED.
//!
//! Criterion 1: one trace's spans across REST, broker, both hook runtimes,
//! the EVENT rejection path, and a follow-on job — reconstructed from the
//! structured log alone (the telemetry ring is the same lines stdout gets).
//! Criterion 2: /metrics answers in Prometheus text with the required series.
//! Criterion 3: a two-tenant burst produces separably-summable per-tenant
//! series. Criterion 4: machine codes appear as fields.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust + hook artifacts.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::*;
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::{access_ddl, identity_ddl};
use frust_kernel::rest::Rest;
use frust_kernel::sync::MetadataSync;
use frust_kernel::telemetry;
use frust_kernel::worker::{AuthorityResolver, Worker};

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

fn fresh(name: &str) -> (Db, ResolvedTenant) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    db.sql_root(&format!(
        "{}; {}; \
         CREATE app_user:ops SET name = 'ops', role = 'manager', pass = crypto::argon2::generate('pw-ops'); \
         DEFINE TABLE doctype SCHEMALESS PERMISSIONS NONE; \
         DEFINE TABLE job SCHEMALESS CHANGEFEED 1h PERMISSIONS NONE; \
         CREATE doctype CONTENT {{ name: 'odoc', submittable: true, \
           fields: [ {{ fieldname: 'title', fieldtype: 'Data' }}, {{ fieldname: 'total', fieldtype: 'Currency' }} ] }};",
        identity_ddl(),
        access_ddl(),
    ))
    .unwrap();
    MetadataSync { base: cfg.clone() }.sync(&db).expect("sync");
    (db, cfg)
}

fn ops() -> Caller {
    Caller { user: "ops".into(), pass: "pw-ops".into(), role: "manager".into() }
}

struct OpsResolver;
impl AuthorityResolver for OpsResolver {
    fn caller_for(&self, _requested_by: &str) -> Option<Caller> {
        Some(ops())
    }
}

/// Spans with this trace id, by event name, from the ring — "the log alone".
fn spans_of(trace: &str) -> Vec<serde_json::Value> {
    telemetry::recent().into_iter().filter(|l| l["trace"] == trace).collect()
}

fn events(lines: &[serde_json::Value]) -> Vec<String> {
    lines.iter().filter_map(|l| l["evt"].as_str().map(str::to_string)).collect()
}

/// Criterion 1 + 4: the whole story of one request under ONE trace id.
/// Runs at debug so successful `db_call` spans are in the reconstruction
/// (the Info default keeps the hot path cheap; failures always emit).
#[test]
fn one_trace_spans_the_whole_kernel() {
    std::env::set_var("FRUST_LOG", "debug");
    let (db, cfg) = fresh("obs_e2e");
    let broker = Arc::new(Broker::new(scoped_db(&cfg), Box::new(WasmHooks::load(artifacts()).unwrap())));

    // in-process REST on its own port
    let url = {
        let b = Arc::clone(&broker);
        let addr = "127.0.0.1:8796".to_string();
        let url = format!("http://{addr}");
        std::thread::spawn(move || {
            let _ = Rest::single(b, addr, None, None).serve(|| {});
        });
        for _ in 0..50 {
            if ureq::get(format!("{url}/health")).call().is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        url
    };

    let mut r = ureq::post(format!("{url}/login"))
        .send(serde_json::json!({ "user": "ops", "pass": "pw-ops" }).to_string())
        .unwrap();
    let token: serde_json::Value = r.body_mut().read_json().unwrap();
    let token = token["token"].as_str().unwrap().to_string();

    // ── the traced flow: ONE externally-supplied trace id ──
    let trace = "wo010proof0001";

    // 1) submit through REST: broker verb + both hook runtimes + db calls
    let mut r = ureq::post(format!("{url}/write/odoc"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("X-Trace-Id", trace)
        .send(
            serde_json::json!({ "doc": {
                "title": "traced", "status": "Draft",
                "total": { "kind": "float", "v": 12.0 }, "docstatus": { "kind": "int", "v": 1 }
            }})
            .to_string(),
        )
        .unwrap();
    let created: serde_json::Value = r.body_mut().read_json().unwrap();
    let id = created["created"]["id"].as_str().unwrap();
    let key = id.split_once(':').unwrap().1;

    // 2) violate the docstatus lattice (1 -> 0): the EVENT rejection path
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let r = agent
        .post(format!("{url}/write/odoc"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("X-Trace-Id", trace)
        .send(serde_json::json!({ "doc": { "docstatus": { "kind": "int", "v": 0 } }, "record": key }).to_string())
        .unwrap();
    assert_eq!(r.status().as_u16(), 500, "lattice rejection surfaced");

    // 3) follow-on job in the SAME trace: enqueue over REST, run via worker
    let mut r = ureq::post(format!("{url}/enqueue/create_doc"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("X-Trace-Id", trace)
        .send(
            serde_json::json!({ "payload": { "doctype": "odoc", "doc": { "title": "from job" } } }).to_string(),
        )
        .unwrap();
    let job: serde_json::Value = r.body_mut().read_json().unwrap();
    assert!(job["job"].as_str().is_some(), "enqueued: {job}");

    let worker = Worker::new(&db, &broker, "obs-worker");
    let claimed = worker.claim_next().unwrap().expect("job claimable");
    assert_eq!(claimed.trace.as_deref(), Some(trace), "job record carries the originating trace");
    worker.run(&claimed, &OpsResolver);

    // ── reconstruction from the log alone ──
    let lines = spans_of(trace);
    let evts = events(&lines);
    for required in ["rest_request", "broker_verb", "hook_dispatch", "db_call", "job_run"] {
        assert!(evts.iter().any(|e| e == required), "trace has a {required} span: {evts:?}");
    }
    // both hook runtimes, under the one trace
    for runtime in ["plugin", "script"] {
        assert!(
            lines.iter().any(|l| l["evt"] == "hook_dispatch" && l["runtime"] == runtime),
            "hook_dispatch span for {runtime}"
        );
    }
    // criterion 4: the lattice machine code is a FIELD, grep-able
    assert!(
        lines
            .iter()
            .any(|l| l["error_code"].as_str().is_some_and(|c| c.contains("FRUST:E_DOCSTATUS"))),
        "EVENT rejection carries its machine code: {lines:?}"
    );
    // the job ran to completion inside the same trace
    assert!(
        lines.iter().any(|l| l["evt"] == "job_run" && l["outcome"] == "done"),
        "job_run completed in-trace"
    );
    // every span carries the tenant label (criterion 3 substrate)
    assert!(lines.iter().all(|l| l["tenant"] == "obs_e2e"), "tenant on every span");
}

/// Criterion 2: /metrics is Prometheus text with the required series.
#[test]
fn metrics_endpoint_answers_without_shell() {
    let (db, cfg) = fresh("obs_metrics");
    // boot publishes meta-version info to the registry (fresh db: no ack needed)
    frust_kernel::boot::boot(
        &db,
        &frust_kernel::boot::BootOptions::default(),
        &MetadataSync { base: cfg.clone() },
    )
    .expect("boot");
    let broker = Arc::new(Broker::new(scoped_db(&cfg), Box::new(WasmHooks::load(artifacts()).unwrap())));
    // generate some signal
    let doc = vec![
        ("title".to_string(), Value::Text("m".into())),
        ("total".to_string(), Value::Float(1.0)),
    ];
    broker.db_write(&ops(), &HookChain::default(), WriteOp::Create, "odoc", None, &doc).unwrap();
    broker.enqueue(&ops(), "create_doc", &[("doctype".into(), Value::Text("odoc".into()))]).unwrap();

    let url = {
        let b = Arc::clone(&broker);
        let addr = "127.0.0.1:8795".to_string();
        let url = format!("http://{addr}");
        std::thread::spawn(move || {
            let _ = Rest::single(b, addr, None, None).serve(|| {});
        });
        for _ in 0..50 {
            if ureq::get(format!("{url}/health")).call().is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        url
    };
    let mut r = ureq::get(format!("{url}/metrics")).call().unwrap();
    assert_eq!(
        r.headers().get("Content-Type").unwrap().to_str().unwrap(),
        "text/plain; version=0.0.4"
    );
    let text = r.body_mut().read_to_string().unwrap();

    for series in [
        "# TYPE frust_verb_duration_ms histogram",
        "# TYPE frust_hook_duration_ms histogram",
        r#"verb="db_write""#,
        r#"runtime="plugin""#,
        r#"runtime="script""#,
        "frust_meta_version",
    ] {
        assert!(text.contains(series), "metrics text contains {series}\n---\n{text}");
    }
}

/// Criterion 3: two tenants, separably-summable series per tenant.
#[test]
fn two_tenant_burst_attributes_separably() {
    let (_db1, cfg1) = fresh("obs_ten_a");
    let (_db2, cfg2) = fresh("obs_ten_b");
    let hooks = || Box::new(WasmHooks::load(artifacts()).unwrap());
    let b1 = Broker::new(scoped_db(&cfg1), hooks());
    let b2 = Broker::new(scoped_db(&cfg2), hooks());

    let doc = vec![
        ("title".to_string(), Value::Text("t".into())),
        ("total".to_string(), Value::Float(2.0)),
    ];
    for _ in 0..6 {
        b1.db_write(&ops(), &HookChain::default(), WriteOp::Create, "odoc", None, &doc).unwrap();
    }
    for _ in 0..2 {
        b2.db_write(&ops(), &HookChain::default(), WriteOp::Create, "odoc", None, &doc).unwrap();
    }

    let text = telemetry::render_prometheus();
    let count_of = |tenant: &str| -> u64 {
        text.lines()
            .find(|l| {
                l.starts_with("frust_verb_duration_ms_count")
                    && l.contains(&format!(r#"tenant="{tenant}""#))
                    && l.contains(r#"verb="db_write""#)
            })
            .and_then(|l| l.rsplit(' ').next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    };
    assert_eq!(count_of("obs_ten_a"), 6, "tenant A's writes attributed to A\n{text}");
    assert_eq!(count_of("obs_ten_b"), 2, "tenant B's writes attributed to B");
    // hook time attributed per tenant too (fuel/time substrate for P-8.2)
    assert!(
        text.contains(r#"frust_hook_duration_ms_count{runtime="plugin",tenant="obs_ten_a"}"#),
        "hook timing carries tenant"
    );
}
