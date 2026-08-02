//! WO-005 exit criterion 1: THE SENTENCE, through the two-process kernel.
//! "Create a DocType at runtime, submit a document through both hook classes,
//! show the audit trail — no restarts, composition deleted."
//!
//! This exercises exactly what the Desk-as-client does: everything over the
//! REST surface + kernel APIs, no direct hook-runner process, no sliver.
//! `frust` + surreal.exe — two processes.

use std::sync::Arc;

use frust_kernel::boot::{boot, BootOptions};
use frust_kernel::broker::Broker;
use frust_kernel::db::scoped_db;
use frust_kernel::tenancy::single_tenant;
use frust_kernel::hooks::WasmHooks;
use frust_kernel::rest::Rest;
use frust_kernel::sync::MetadataSync;

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

fn rest_url(broker: Arc<Broker>, port: u16) -> String {
    let addr = format!("127.0.0.1:{port}");
    let url = format!("http://{addr}");
    std::thread::spawn(move || {
        let _ = Rest::single(broker, addr, None, None).serve(|| {});
    });
    for _ in 0..50 {
        if ureq::get(format!("{url}/health")).call().is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    url
}

#[test]
fn the_sentence_through_two_processes() {
    // fresh database, kernel boots it (meta schema + queue) — no manual setup
    let cfg = single_tenant(&"accept_e2e".to_string()).expect("tenancy");
    let admin = scoped_db(&cfg);
    admin.sql_root_ns("REMOVE DATABASE IF EXISTS accept_e2e; DEFINE DATABASE accept_e2e;").unwrap();
    // record auth for the acting user
    admin
        .sql_root(
            "DEFINE TABLE app_user SCHEMAFULL PERMISSIONS NONE; \
             DEFINE FIELD name ON app_user TYPE string; \
             DEFINE FIELD role ON app_user TYPE string; \
             DEFINE FIELD pass ON app_user TYPE string; \
             DEFINE ACCESS account ON DATABASE TYPE RECORD \
               SIGNIN (SELECT * FROM app_user WHERE name = $name AND crypto::argon2::compare(pass, $pass)) \
               DURATION FOR TOKEN 1h, FOR SESSION 12h; \
             CREATE app_user:clerk SET name = 'clerk', role = 'manager', pass = crypto::argon2::generate('pw-clerk');",
        )
        .unwrap();

    // kernel boot: meta schema applied, sync runs (no user doctypes yet)
    let report = boot(&admin, &BootOptions::default(), &MetadataSync { base: cfg.clone() }).unwrap();
    assert!(report.applied_meta);

    // ── CREATE A DOCTYPE AT RUNTIME (metadata written; kernel syncs schema) ──
    let invoice_meta = serde_json::json!({
        "name": "sinv",
        "submittable": true,
        "fields": [
            { "fieldname": "title", "fieldtype": "Data", "required": true },
            { "fieldname": "total", "fieldtype": "Currency" }
        ]
    });
    admin.sql_root(&format!("CREATE doctype CONTENT {invoice_meta};")).unwrap();
    // sync the new doctype through the ported engine (what `frust` does on run)
    let synced = MetadataSync { base: cfg.clone() }.sync(&admin).unwrap().applied;
    assert_eq!(synced, 1, "the runtime DocType synced to live schema");

    // ── SUBMIT THROUGH BOTH HOOK CLASSES, over REST (Desk-as-client) ──
    let broker = Arc::new(Broker::new(scoped_db(&cfg), Box::new(WasmHooks::load(artifacts()).unwrap())));
    let url = rest_url(Arc::clone(&broker), 8794);

    // agent that returns non-2xx responses instead of erroring, so error
    // bodies are readable
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    // WO-009: session login replaces the header trio
    let mut lr = agent
        .post(format!("{url}/login"))
        .send(serde_json::json!({ "user": "clerk", "pass": "pw-clerk" }).to_string())
        .unwrap();
    let login_body: serde_json::Value = lr.body_mut().read_json().unwrap();
    let token = login_body["token"].as_str().expect("session token").to_string();

    let write = |doc: serde_json::Value| -> (u16, serde_json::Value) {
        let mut r = agent
            .post(format!("{url}/write/sinv"))
            .header("Authorization", &format!("Bearer {token}"))
            .send(serde_json::json!({ "doc": doc }).to_string())
            .unwrap();
        (r.status().as_u16(), r.body_mut().read_json().unwrap_or(serde_json::json!({})))
    };

    // a 20k draft: plugin flags it -> Needs Approval (both hook runtimes fire
    // in-process; no :8787)
    let (code, body) = write(serde_json::json!({
        "title": "Big one", "status": "Draft",
        "total": { "kind": "float", "v": 20000.0 }
    }));
    assert_eq!(code, 200, "submit failed: {body}");
    let created = &body["created"];
    assert_eq!(created["status"].as_str(), Some("Needs Approval"), "hooks fired via REST: {body}");
    let id = created["id"].as_str().unwrap().to_string();

    // reject path travels too: negative total -> 422 from the plugin
    let (rej_code, _) = write(serde_json::json!({ "title": "bad", "total": { "kind": "float", "v": -1.0 } }));
    assert_eq!(rej_code, 422, "hook rejection surfaces as 422");

    // ── SHOW THE AUDIT TRAIL (changefeed, unbypassable) ──
    let feed = admin
        .sql_root("SHOW CHANGES FOR TABLE sinv SINCE 1 LIMIT 1000;")
        .unwrap();
    let entries = feed.as_array().cloned().unwrap_or_default();
    let touches_our_doc = entries.iter().any(|e| e.to_string().contains(&id));
    assert!(touches_our_doc, "the submitted document appears in the audit trail");

    // ── NO RESTARTS: the same running processes did all of the above ──
    // (the DocType didn't exist when the DB was created; the schema, the
    //  submit, and the audit all happened without bouncing anything.)
}
