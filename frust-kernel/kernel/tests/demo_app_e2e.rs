//! WO-019 criterion 7: **the demo app, end to end.**
//!
//! One small app, installed from a bundle file through the kernel's manager
//! surface, then exercised the way a real one would be: a DocType that holds
//! data, a server script that demonstrably runs, a client script carried as
//! metadata, a route that serves under bearer auth â€” installed, used,
//! disabled, re-enabled, updated to v2 with a field addition, and used again.
//!
//! **No restarts anywhere.** That is the criterion: one kernel process from
//! the first install to the last assertion. If any step needed a restart, the
//! zero-compilation-extension claim (REQ-2.1.1) would be false in the only
//! way that matters.
//!
//! This is where every piece built across the WO runs as one app: the manifest
//! (2), the install gate (3), the route door (1, 4), uninstall's honesty (5),
//! and server-script delivery (6).
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::*;
use frust_kernel::db::scoped_db;
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::{meta_ddl, APP_TABLE};
use frust_kernel::rest::Rest;
use frust_kernel::sync::MetadataSync;

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

const DB: &str = "demo_app";
const ADDR: &str = "127.0.0.1:8795";

/// The bundle, as a file would carry it. v2 adds a field and revises the
/// server script â€” the two changes an app update actually makes.
fn ledger_app(version: &str) -> serde_json::Value {
    let mut fields = vec![
        serde_json::json!({ "fieldname": "memo", "fieldtype": "Data", "required": true }),
        serde_json::json!({ "fieldname": "amount", "fieldtype": "Currency" }),
        serde_json::json!({ "fieldname": "band", "fieldtype": "Data" }),
    ];
    let script = if version == "1.0.0" {
        // money stays a STRING across the boundary (REQ-6.2.1): the script
        // classifies, it does not compute
        r#"doc.band = Number(doc.amount) >= 1000 ? "large" : "small";"#
    } else {
        fields.push(serde_json::json!({ "fieldname": "reviewer", "fieldtype": "Data" }));
        r#"doc.band = Number(doc.amount) >= 1000 ? "large" : "small";
           doc.reviewer = doc.band === "large" ? "finance" : "auto";"#
    };
    serde_json::json!({
        "manifest_version": 1,
        "name": "ledger",
        "version": version,
        "label": "Ledger",
        "doctypes": [{ "name": "ledger_entry", "app": "ledger", "fields": fields }],
        "server_scripts": [{ "doctype": "ledger_entry", "hook": "validate", "script": script }],
        "client_scripts": [{ "doctype": "ledger_entry", "script": "doc.memo = doc.memo;" }],
        "routes": [{ "path": "entries", "component": "plugin_demo.wasm" }],
        "components": ["plugin_demo.wasm"],
        // WO-018 typed this slot. A minimal-but-valid workflow, so the demo
        // app also exercises workflow-as-manifest-content end to end.
        "workflows": [{
            "name": "ledger_approval",
            "doctype": "ledger_entry",
            "states": [
                { "name": "Draft", "docstatus": 0 },
                { "name": "Approved", "docstatus": 1 }
            ],
            "transitions": [
                { "from": "Draft", "to": "Approved", "role": "manager", "action": "Approve" }
            ]
        }]
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

fn get(url: &str, token: Option<&str>) -> (u16, serde_json::Value) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.get(format!("http://{ADDR}{url}"));
    if let Some(t) = token {
        req = req.header("Authorization", &format!("Bearer {t}"));
    }
    let mut r = req.call().expect("http");
    let code = r.status().as_u16();
    (
        code,
        r.body_mut().read_json().unwrap_or(serde_json::json!({})),
    )
}

fn login(user: &str, pass: &str) -> String {
    let (_, out) = post("/login", None, serde_json::json!({ "user": user, "pass": pass }));
    out["token"].as_str().unwrap_or_default().to_string()
}

/// ONE kernel, started once, never restarted.
fn boot() -> &'static (Arc<Broker>, ResolvedTenant) {
    static K: std::sync::OnceLock<(Arc<Broker>, ResolvedTenant)> = std::sync::OnceLock::new();
    K.get_or_init(|| {
        let cfg = single_tenant(&DB.to_string()).expect("tenancy");
        let db = scoped_db(&cfg);
        db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {DB}; DEFINE DATABASE {DB};")).unwrap();
        db.sql_root(&meta_ddl()).unwrap();
        db.sql_root(
            "CREATE app_user:mgr SET name = 'mgr', role = 'manager', pass = crypto::argon2::generate('pw-mgr'); \
             CREATE app_user:clerk SET name = 'clerk', role = 'clerk', pass = crypto::argon2::generate('pw-clerk');",
        )
        .unwrap();

        let hooks = WasmHooks::load(artifacts())
            .expect("hooks")
            .with_script_source(scoped_db(&cfg));
        let broker = Arc::new(Broker::new(scoped_db(&cfg), Box::new(hooks)));
        let rest = Rest::single(Arc::clone(&broker), ADDR.to_string(), Some(Arc::new(MetadataSync { base: cfg.clone() })), None);
        std::thread::spawn(move || {
            let _ = rest.serve(|| {});
        });
        for _ in 0..100 {
            if std::net::TcpStream::connect(ADDR).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        (broker, cfg)
    })
}

/// The whole lifecycle as ONE test, because the criterion is the *sequence*:
/// each step depends on the last, and splitting it into six tests would prove
/// six things about six kernels instead of one thing about one app.
#[test]
fn the_demo_app_lives_its_whole_life_without_a_restart() {
    let (broker, _cfg) = boot();
    let mgr = login("mgr", "pw-mgr");
    let clerk = login("clerk", "pw-clerk");
    let write = |caller: &Caller, memo: &str, amount: &str| {
        broker.db_write(
            caller,
            &HookChain::default(),
            WriteOp::Create,
            "ledger_entry",
            None,
            &[
                ("memo".to_string(), Value::Text(memo.into())),
                ("amount".to_string(), Value::Decimal(amount.into())),
            ],
        )
    };
    let clerk_caller = Caller { user: "clerk".into(), pass: "pw-clerk".into(), role: "clerk".into() };

    // â”€â”€ 1. PREVIEW, then install â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
    let (code, plan) = post("/app/plan", Some(&mgr), ledger_app("1.0.0"));
    assert_eq!(code, 200, "plan: {plan}");
    assert_eq!(plan["needs_acknowledgement"], serde_json::json!(false));
    assert_eq!(plan["routes"], serde_json::json!(["/app/ledger/entries"]));
    assert_eq!(plan["workflows"], serde_json::json!(1), "the reserved slot survives the preview");
    println!("STEP 1 plan ok: {} planned", plan["planned"].as_array().unwrap().len());

    let (code, out) = post("/app/install", Some(&mgr), ledger_app("1.0.0"));
    assert_eq!(code, 200, "install: {out}");
    println!("STEP 2 installed v{}", out["version"]);

    // â”€â”€ 2. EXERCISE: the server script runs on a real write â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
    let small = write(&clerk_caller, "coffee", "12.50").expect("write small");
    let large = write(&clerk_caller, "server rack", "4200.00").expect("write large");
    println!("STEP 3 small={} large={}", small["band"], large["band"]);
    assert_eq!(small["band"], serde_json::json!("small"), "the app's own script ran");
    assert_eq!(large["band"], serde_json::json!("large"));
    // Money crossed as decimal and came back exact (REQ-6.2.1). Assert the
    // VALUE, not its representation: SurrealDB normalises trailing zeros, so
    // `4200.00` comes back as `"4200"` â€” string-comparing here is the very
    // mistake WO-016 recorded, made again.
    let amount = large["amount"].as_str().unwrap_or_else(|| {
        panic!("money must cross as a decimal STRING, never a float: {}", large["amount"])
    });
    assert_eq!(
        amount.parse::<f64>().ok(),
        Some(4200.0),
        "and it must be the exact value we wrote (shown as {amount})"
    );

    // â”€â”€ 3. EXERCISE: the route serves under the clerk's own session â”€â”€â”€â”€â”€â”€â”€
    let (code, r) = post("/app/ledger/entries", Some(&clerk), serde_json::json!({ "query": "probe=read&doctype=ledger_entry" }));
    assert_eq!(code, 200, "route: {r}");
    let rbody = r["body"].as_str().unwrap_or("").to_string();
    println!("STEP 4 route served: {}", rbody.chars().take(90).collect::<String>());
    assert!(rbody.starts_with("ok:"), "the route reads the APP'S OWN doctype: {rbody}");
    assert!(rbody.contains("coffee"), "and returns the app's own rows: {rbody}");

    // â”€â”€ 4. DISABLE: routes stop, DATA STAYS â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
    let (code, _) = post("/app/ledger/disable", Some(&mgr), serde_json::json!({}));
    assert_eq!(code, 200, "disable");
    let (code, _) = post("/app/ledger/entries", Some(&clerk), serde_json::json!({ "query": "probe=read&doctype=ledger_entry" }));
    assert_eq!(code, 404, "a disabled app does not exist as a surface");
    let rows = broker.db.sql_root("SELECT count() AS n FROM ledger_entry GROUP ALL;").unwrap();
    let n = rows.as_array().and_then(|a| a.first()).and_then(|r| r["n"].as_u64()).unwrap_or(0);
    assert_eq!(n, 2, "disable touched no data");
    println!("STEP 5 disabled: route 404, {n} rows intact");

    // â”€â”€ 5. RE-ENABLE: restoration, not reconstruction â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
    let (code, _) = post("/app/ledger/enable", Some(&mgr), serde_json::json!({}));
    assert_eq!(code, 200, "enable");
    let (code, _) = post("/app/ledger/entries", Some(&clerk), serde_json::json!({ "query": "probe=read&doctype=ledger_entry" }));
    assert_eq!(code, 200, "serving again");
    let dt = broker
        .db
        .sql_root("SELECT client_script FROM doctype WHERE name = 'ledger_entry';")
        .unwrap();
    assert!(
        dt.as_array().unwrap()[0]["client_script"].as_str().unwrap_or("").contains("doc.memo"),
        "the exact client script came back from the stored manifest"
    );
    println!("STEP 6 re-enabled: route serves, client script restored");

    // â”€â”€ 6. UPDATE to v2: new field + revised script, live â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
    let (code, out) = post("/app/update", Some(&mgr), ledger_app("2.0.0"));
    assert_eq!(code, 200, "update: {out}");
    assert_eq!(out["action"], serde_json::json!("updated"));
    assert!(
        out["destructive"].as_array().map(|a| a.is_empty()).unwrap_or(false),
        "adding a field is not destructive: {out}"
    );
    println!("STEP 7 updated to v{}", out["version"]);

    // the NEW field exists and the REVISED script runs â€” no restart
    let after = write(&clerk_caller, "office chairs", "2500.00").expect("write after update");
    println!("STEP 8 post-update: band={} reviewer={}", after["band"], after["reviewer"]);
    assert_eq!(after["band"], serde_json::json!("large"));
    assert_eq!(
        after["reviewer"], serde_json::json!("finance"),
        "v2's revised server script took effect with NO RESTART"
    );

    // rows written under v1 are untouched by the update
    let rows = broker.db.sql_root("SELECT count() AS n FROM ledger_entry GROUP ALL;").unwrap();
    let n = rows.as_array().and_then(|a| a.first()).and_then(|r| r["n"].as_u64()).unwrap_or(0);
    assert_eq!(n, 3, "v1 data survived the update");

    // â”€â”€ 7. The registry tells the whole story â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
    let (code, apps) = get("/app", Some(&mgr));
    assert_eq!(code, 200, "list apps: {apps}");
    assert_eq!(apps["apps"][0]["version"], serde_json::json!("2.0.0"));
    assert_eq!(apps["apps"][0]["enabled"], serde_json::json!(true));

    // every lifecycle action is in the changefeed, unbypassably
    let feed = broker
        .db
        .sql_root(&format!("SHOW CHANGES FOR TABLE {APP_TABLE} SINCE 1 LIMIT 100;"))
        .expect("changefeed");
    let entries = feed.as_array().map(|a| a.len()).unwrap_or(0);
    println!("STEP 9 registry at v2, {entries} audited lifecycle actions");
    assert!(entries >= 4, "install + disable + enable + update all audited: {entries}");
}
