//! The document delete door, proven through HTTP and the broker's caller-session
//! statement. Authorization provenance stays in compiled table permissions;
//! lifecycle provenance stays in database events.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookDispatch};
use frust_kernel::contract::{BrokerError, Value};
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::meta::meta_ddl;
use frust_kernel::realtime::Realtime;
use frust_kernel::rest::Rest;
use frust_kernel::sync::{MetadataSync, DOCSTATUS_DELETE_REFUSAL, SINGLE_DELETE_REFUSAL};
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};

struct PassHooks;

impl HookDispatch for PassHooks {
    fn validate(
        &self,
        _doctype: &str,
        doc: &[(String, Value)],
    ) -> Result<Vec<(String, Value)>, BrokerError> {
        Ok(doc.to_vec())
    }
}

fn fresh() -> (Db, ResolvedTenant) {
    let name = format!("delete_door_{}", std::process::id());
    let target = single_tenant(&name).expect("tenancy");
    let db = scoped_db(&target);
    db.sql_root_ns(&format!(
        "REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};"
    ))
    .expect("fresh database");
    db.sql_root(&meta_ddl()).expect("meta schema");
    db.sql_root(
        "CREATE app_user:manager SET name = 'manager', role = 'manager', pass = crypto::argon2::generate('pw-manager'); \
         CREATE app_user:clerk SET name = 'clerk', role = 'clerk', pass = crypto::argon2::generate('pw-clerk'); \
         CREATE doctype:claim CONTENT { name: 'claim', submittable: true, fields: [ \
           { fieldname: 'title', fieldtype: 'Data', required: true } ] }; \
         CREATE doctype:settings CONTENT { name: 'settings', issingle: true, fields: [ \
           { fieldname: 'title', fieldtype: 'Data' } ] };",
    )
    .expect("users and doctypes");
    MetadataSync {
        base: target.clone(),
    }
    .sync(&db)
    .expect("sync delete fixtures");
    (db, target)
}

fn start_rest(target: &ResolvedTenant) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let address = format!("127.0.0.1:{port}");
    let base = format!("http://{address}");
    let broker = Arc::new(Broker::new(scoped_db(target), Box::new(PassHooks)));
    let realtime = Arc::new(Realtime::new(target.endpoint()));
    std::thread::spawn(move || {
        let _ = Rest::single(broker, address, None, Some(realtime)).serve(|| {});
    });
    for _ in 0..100 {
        if ureq::get(format!("{base}/health")).call().is_ok() {
            return base;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("REST did not become ready");
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into()
}

fn post(
    base: &str,
    path: &str,
    token: Option<&str>,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let mut request = agent().post(format!("{base}{path}"));
    if let Some(token) = token {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let mut response = request.send(body.to_string()).expect("POST");
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .read_json()
        .unwrap_or_else(|_| serde_json::json!({}));
    (status, body)
}

fn delete(base: &str, path: &str, token: &str) -> (u16, serde_json::Value) {
    let mut response = agent()
        .delete(format!("{base}{path}"))
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .expect("DELETE");
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .read_json()
        .unwrap_or_else(|_| serde_json::json!({}));
    (status, body)
}

fn get(base: &str, path: &str, token: &str) -> (u16, serde_json::Value) {
    let mut response = agent()
        .get(format!("{base}{path}"))
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .expect("GET");
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .read_json()
        .unwrap_or_else(|_| serde_json::json!({}));
    (status, body)
}

fn login(base: &str, user: &str) -> String {
    let (status, body) = post(
        base,
        "/login",
        None,
        serde_json::json!({ "user": user, "pass": format!("pw-{user}") }),
    );
    assert_eq!(status, 200, "login {user}: {body}");
    body["token"].as_str().expect("token").to_string()
}

fn create_claim(base: &str, token: &str, title: &str) -> (String, String) {
    let (status, body) = post(
        base,
        "/write/claim",
        Some(token),
        serde_json::json!({ "doc": { "title": title } }),
    );
    assert_eq!(status, 200, "create claim: {body}");
    let id = body["record"].as_str().expect("created record").to_string();
    let key = id.split_once(':').expect("record id").1.to_string();
    (id, key)
}

fn events(base: &str, token: &str, sub: &str) -> Vec<serde_json::Value> {
    let (status, body) = get(base, &format!("/events/{sub}"), token);
    assert_eq!(status, 200, "events: {body}");
    body["events"].as_array().cloned().unwrap_or_default()
}

#[test]
fn delete_door_preserves_authorization_lifecycle_and_realtime_provenance() {
    let (db, target) = fresh();
    let base = start_rest(&target);
    let manager = login(&base, "manager");
    let clerk = login(&base, "clerk");

    let (status, subscription) = post(
        &base,
        "/subscribe/claim",
        Some(&manager),
        serde_json::json!({}),
    );
    assert_eq!(status, 200, "subscribe: {subscription}");
    let sub = subscription["sub"].as_str().unwrap();

    let (manager_id, manager_key) = create_claim(&base, &manager, "manager draft");
    std::thread::sleep(std::time::Duration::from_millis(300));
    let _ = events(&base, &manager, sub);
    let (status, deleted) = delete(&base, &format!("/doc/claim/{manager_key}"), &manager);
    assert_eq!(status, 200, "manager deletes own draft: {deleted}");
    assert_eq!(deleted["action"], serde_json::json!("deleted"));
    assert_eq!(deleted["id"], serde_json::json!(manager_id));

    let (status, read_after) = post(
        &base,
        "/read/claim",
        Some(&manager),
        serde_json::json!({
            "filter": { "path": "id", "op": "eq", "value": manager_id }
        }),
    );
    assert_eq!(status, 200, "read after delete: {read_after}");
    assert_eq!(read_after["rows"], serde_json::json!([]), "row is gone");

    let mut delete_ticks = Vec::new();
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        delete_ticks.extend(events(&base, &manager, sub));
        if delete_ticks.iter().any(|tick| tick["id"] == deleted["id"]) {
            break;
        }
    }
    let tick = delete_ticks
        .iter()
        .find(|tick| tick["id"] == deleted["id"])
        .expect("delete tick observed");
    assert_eq!(
        tick.as_object().unwrap().len(),
        2,
        "tick is action plus id only"
    );
    assert_eq!(tick["action"], serde_json::json!("DELETE"));

    let (clerk_id, clerk_key) = create_claim(&base, &clerk, "clerk draft");
    let (status, refused) = delete(&base, &format!("/doc/claim/{clerk_key}"), &clerk);
    assert_eq!(
        status, 403,
        "compiled delete permission refuses clerk: {refused}"
    );
    assert_eq!(
        refused["error"]["kind"],
        serde_json::json!("permission-denied")
    );
    assert!(
        refused["error"]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("E_DELETE_NO_ROWS"),
        "typed no-row refusal: {refused}"
    );
    let survived = db
        .sql_root(&format!("SELECT owner FROM {clerk_id};"))
        .expect("clerk row survives");
    assert_eq!(survived.as_array().unwrap().len(), 1);
    assert_eq!(
        survived.as_array().unwrap()[0]["owner"],
        serde_json::json!("app_user:clerk")
    );

    let (submitted_id, submitted_key) = create_claim(&base, &manager, "submitted");
    db.sql_root(&format!("UPDATE {submitted_id} SET docstatus = 1;"))
        .expect("submit fixture");
    let (status, submitted_refusal) =
        delete(&base, &format!("/doc/claim/{submitted_key}"), &manager);
    assert_eq!(status, 422, "submitted delete refused: {submitted_refusal}");
    assert_eq!(
        submitted_refusal["error"]["kind"],
        serde_json::json!("delete-refused")
    );
    assert_eq!(
        submitted_refusal["error"]["code"],
        serde_json::json!(DOCSTATUS_DELETE_REFUSAL)
    );
    db.sql_root(&format!("UPDATE {submitted_id} SET docstatus = 2;"))
        .expect("cancel fixture");
    let (status, cancelled_refusal) =
        delete(&base, &format!("/doc/claim/{submitted_key}"), &manager);
    assert_eq!(status, 422, "cancelled delete refused: {cancelled_refusal}");
    assert_eq!(
        cancelled_refusal["error"]["code"],
        serde_json::json!(DOCSTATUS_DELETE_REFUSAL)
    );
    assert_eq!(
        db.sql_root(&format!("SELECT docstatus FROM {submitted_id};"))
            .unwrap()
            .as_array()
            .unwrap()[0]["docstatus"],
        serde_json::json!(2),
        "lattice refusal leaves the row intact"
    );

    let (status, single_refusal) = delete(&base, "/doc/settings/settings", &manager);
    assert_eq!(status, 422, "Single delete refused: {single_refusal}");
    assert_eq!(
        single_refusal["error"]["kind"],
        serde_json::json!("delete-refused")
    );
    assert_eq!(
        single_refusal["error"]["code"],
        serde_json::json!(SINGLE_DELETE_REFUSAL)
    );
    assert_eq!(
        db.sql_root("SELECT id FROM settings:settings;")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1,
        "Single survives"
    );

    let (status, missing) = delete(&base, "/doc/claim/not_here", &manager);
    assert_eq!(status, 403, "missing delete is not success: {missing}");
    assert!(missing["error"]["detail"]
        .as_str()
        .unwrap_or_default()
        .contains("E_DELETE_NO_ROWS"));

    let broker = Broker::new(scoped_db(&target), Box::new(PassHooks));
    let direct = broker.db_delete(
        &Caller {
            user: "manager".into(),
            pass: "pw-manager".into(),
            role: "manager".into(),
        },
        "claim",
        "not_here_either",
    );
    assert!(
        matches!(direct, Err(BrokerError::PermissionDenied { ref detail }) if detail.contains("E_DELETE_NO_ROWS")),
        "broker verb preserves typed no-row refusal: {direct:?}"
    );
}
