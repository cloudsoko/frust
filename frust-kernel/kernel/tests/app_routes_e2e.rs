//! WO-019 criterion 4: plugin routes served over REST.
//!
//! This runs against a REAL kernel over REAL HTTP with REAL bearer tokens.
//! The criterion-1 door probe proved `route == broker` when the host called
//! the broker directly, in-process. Bearer auth, session lookup, trace spans,
//! tenant attribution and the WO-013 throttle now sit between the two â€” each
//! one an opportunity for "the same rows for the same caller" to decay into
//! "nearly the same rows". So the equality is asserted **through the shipped
//! dispatch path**, not around it.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::*;
use frust_kernel::db::scoped_db;
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};
use frust_kernel::fairness::{self, Policy};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::meta_ddl;
use frust_kernel::rest::Rest;
use frust_kernel::sync::MetadataSync;

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

/// These tests share ONE kernel, ONE installed app, and the process-global
/// tenant policy â€” and two of them mutate that shared state deliberately
/// (throttle sets a budget; disable turns the app off). Run in parallel they
/// sabotage each other: a concurrent read gets the throttle's 429, or the
/// disable's "app is disabled".
///
/// Same mutex the permission proofs and perf gates use. Noting it because I
/// made this exact mistake in WO-013 with the fairness unit tests: shared
/// global state plus parallel tests is a collision every time, and passing
/// under `--test-threads=1` in isolation proves nothing about the suite.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static GUARD: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    GUARD
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

const DB: &str = "app_routes";
const ADDR: &str = "127.0.0.1:8794";

fn bundle() -> serde_json::Value {
    serde_json::json!({
        "manifest_version": 1,
        "name": "demo",
        "version": "1.0.0",
        "doctypes": [{
            "name": "purchase_order",
            "fields": [
                { "fieldname": "title", "fieldtype": "Data" },
                { "fieldname": "total", "fieldtype": "Currency" }
            ]
        }],
        "routes": [{ "path": "probe", "component": "plugin_demo.wasm" }],
        "components": ["plugin_demo.wasm"]
    })
}

fn post(url: &str, token: Option<&str>, body: serde_json::Value) -> (u16, serde_json::Value) {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let mut req = agent.post(format!("http://{ADDR}{url}"));
    if let Some(t) = token {
        req = req.header("Authorization", &format!("Bearer {t}"));
    }
    let mut r = req.send(body.to_string()).expect("http");
    let code = r.status().as_u16();
    (code, r.body_mut().read_json().unwrap_or(serde_json::json!({})))
}

/// One kernel for the whole file: starting a REST server per test would race
/// on the port, and these assertions are about one running system anyway.
fn kernel() -> &'static (Arc<Broker>, ResolvedTenant) {
    static K: std::sync::OnceLock<(Arc<Broker>, ResolvedTenant)> = std::sync::OnceLock::new();
    K.get_or_init(|| {
        let cfg = single_tenant(&DB.to_string()).expect("tenancy");
        let db = scoped_db(&cfg);
        db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {DB}; DEFINE DATABASE {DB};")).unwrap();
        db.sql_root(&meta_ddl()).unwrap();
        db.sql_root(
            "CREATE app_user:mgr SET name = 'mgr', role = 'manager', pass = crypto::argon2::generate('pw-mgr'); \
             CREATE app_user:clerk1 SET name = 'clerk1', role = 'clerk', pass = crypto::argon2::generate('pw-clerk1'); \
             CREATE app_user:clerk2 SET name = 'clerk2', role = 'clerk', pass = crypto::argon2::generate('pw-clerk2');",
        )
        .unwrap();

        let broker = Arc::new(Broker::new(
            scoped_db(&cfg),
            Box::new(WasmHooks::load(artifacts()).expect("hooks")),
        ));
        let rest = Rest::single(Arc::clone(&broker), ADDR.to_string(), Some(Arc::new(MetadataSync { base: cfg.clone() })), None);
        std::thread::spawn(move || {
            let _ = rest.serve(|| {});
        });
        // wait for the listener rather than sleeping a guess
        for _ in 0..100 {
            if std::net::TcpStream::connect(ADDR).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        // install the app that declares the route
        let (_, tok) = login("mgr", "pw-mgr");
        let (code, out) = post("/app/install", Some(&tok), bundle());
        assert_eq!(code, 200, "install: {out}");

        // one row per clerk, so a permission leak is visible as a count
        for (u, p, t) in [("clerk1", "pw-clerk1", "clerk1 order"), ("clerk2", "pw-clerk2", "clerk2 order")] {
            broker
                .db_write(
                    &Caller { user: u.into(), pass: p.into(), role: "clerk".into() },
                    &HookChain::default(),
                    WriteOp::Create,
                    "purchase_order",
                    None,
                    &[
                        ("title".to_string(), Value::Text(t.into())),
                        ("total".to_string(), Value::Decimal("100.00".into())),
                    ],
                )
                .expect("seed");
        }
        (broker, cfg)
    })
}

fn login(user: &str, pass: &str) -> (u16, String) {
    let (code, out) = post("/login", None, serde_json::json!({ "user": user, "pass": pass }));
    (code, out["token"].as_str().unwrap_or_default().to_string())
}

/// Bearer discipline: no token, no route.
#[test]
fn an_unauthenticated_route_call_is_refused() {
    let _serial = serial();
    kernel();
    let (code, out) = post("/app/demo/probe", None, serde_json::json!({}));
    println!("no bearer -> {code} {out}");
    assert_eq!(code, 401, "a plugin route is not a public endpoint: {out}");
}

/// THE EQUALITY, through the shipped path. Same caller, same question, asked
/// once over HTTP through the plugin route and once directly of the broker.
#[test]
fn route_read_equals_broker_read_through_the_full_stack() {
    let _serial = serial();
    let (broker, _) = kernel();
    let (_, tok) = login("clerk1", "pw-clerk1");

    let (code, out) = post("/app/demo/probe", Some(&tok), serde_json::json!({ "query": "probe=read" }));
    assert_eq!(code, 200, "route: {out}");
    let body = out["body"].as_str().unwrap_or_default().to_string();
    println!("route body: {body}");
    assert!(body.starts_with("ok:"), "route read succeeded: {body}");
    let via_route: Vec<serde_json::Value> =
        serde_json::from_str(body.trim_start_matches("ok:")).expect("rows");

    let via_broker = broker
        .db_read(
            &Caller { user: "clerk1".into(), pass: "pw-clerk1".into(), role: "clerk".into() },
            "purchase_order",
            None,
            &[],
            &ReadOpts::default(),
        )
        .expect("broker read");

    assert_eq!(
        via_route.len(),
        via_broker.len(),
        "route == broker for the same caller, ACROSS bearer auth + dispatch"
    );
    assert_eq!(via_route.len(), 1, "clerk1 sees its own row only: {via_route:?}");
    assert!(!body.contains("clerk2 order"), "ROW LEAK through the shipped route path: {body}");
}

/// The hostile probes again, but over HTTP this time â€” dispatch code between
/// the guest and the broker is exactly where a bypass would be introduced.
#[test]
fn the_hostile_probes_still_fail_over_http() {
    let _serial = serial();
    kernel();
    let (_, tok) = login("clerk1", "pw-clerk1");
    for probe in ["raw-doctype", "raw-filter", "root-leak", "connect", "fs"] {
        let (code, out) = post(
            "/app/demo/probe",
            Some(&tok),
            serde_json::json!({ "query": format!("probe={probe}") }),
        );
        let body = out["body"].as_str().unwrap_or_default();
        println!("{probe:<12} -> {body}");
        assert_eq!(code, 200, "{probe}");
        assert!(!body.starts_with("ESCAPED"), "{probe} ESCAPED over HTTP: {body}");
    }
}

/// **A plugin route is not a budget bypass.** WO-013's throttle fires through
/// `/app/{app}/{path}` exactly as it does at the broker door â€” and it is the
/// KERNEL's 429, not a status code the plugin chose.
#[test]
fn the_tenant_throttle_trips_through_a_plugin_route() {
    let _serial = serial();
    kernel();
    let (_, tok) = login("clerk1", "pw-clerk1");

    // a deliberately tiny budget for this tenant
    fairness::set_policy(
        DB,
        Policy { verbs_per_sec: 1.0, verb_burst: 1.0, ..Policy::default() },
    );

    let mut throttled = None;
    for i in 0..40 {
        let (code, out) = post("/app/demo/probe", Some(&tok), serde_json::json!({ "query": "probe=read" }));
        if code != 200 {
            println!("call {i} -> {code} {out}");
            throttled = Some((code, out));
            break;
        }
    }
    fairness::clear();

    let (code, out) = throttled.expect("a plugin route must be throttleable");
    assert_eq!(code, 429, "the kernel answers 429, not the plugin: {out}");
    assert!(
        out["error"].to_string().contains("TenantThrottled")
            || out["error"].to_string().contains("retry_after_ms"),
        "the refusal is WO-013's, with its retry hint: {out}"
    );
}

/// A disabled app stops serving its routes â€” the detach from criterion 3
/// reaches the route surface too, not just the scripts.
#[test]
fn a_disabled_app_stops_serving_routes() {
    let _serial = serial();
    kernel();
    let (_, mgr) = login("mgr", "pw-mgr");
    let (_, clerk) = login("clerk1", "pw-clerk1");

    let (code, _) = post("/app/demo/disable", Some(&mgr), serde_json::json!({}));
    assert_eq!(code, 200, "disable");

    // Observe first, RE-ENABLE, then assert. This test mutates state the whole
    // file shares, and an assertion between the disable and the enable leaves
    // the app off for every test that follows — which is exactly what happened:
    // one failed assertion here cascaded into four unrelated failures.
    let (code, out) = post("/app/demo/probe", Some(&clerk), serde_json::json!({ "query": "probe=read" }));
    println!("disabled -> {code} {out}");

    let (recode, _) = post("/app/demo/enable", Some(&mgr), serde_json::json!({}));
    let (serving, _) = post("/app/demo/probe", Some(&clerk), serde_json::json!({ "query": "probe=read" }));

    // PM ruling: a disabled app does not exist as a surface. 404, and
    // deliberately WITHOUT a "disabled" reason — that reason would confirm the
    // route is real-but-forbidden, which is the leak. The reason lives in the
    // log (`route_refused_app_disabled`), not in the response.
    assert_eq!(code, 404, "a disabled app must not serve: {out}");
    assert!(
        !out.to_string().contains("disabled"),
        "the response must NOT reveal that the app exists but is off: {out}"
    );
    assert_eq!(recode, 200, "enable");
    assert_eq!(serving, 200, "and serves again once enabled");
}

/// An unknown app or route is a clean refusal, not a 500.
#[test]
fn unknown_apps_and_routes_refuse_cleanly() {
    let _serial = serial();
    kernel();
    let (_, tok) = login("clerk1", "pw-clerk1");
    let (code, out) = post("/app/nosuchapp/probe", Some(&tok), serde_json::json!({}));
    assert_eq!(code, 404, "unknown app: {out}");
    let (code, out) = post("/app/demo/nosuchroute", Some(&tok), serde_json::json!({}));
    assert_eq!(code, 404, "unknown route: {out}");
}

