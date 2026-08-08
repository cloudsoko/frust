//! Fixture lifecycle proof through a real `frust serve` process and REST.
//!
//! The process installs and reads the record, accepts a normal user write,
//! then refuses the app update from the already-running binary. No lifecycle
//! step recompiles or bypasses the HTTP/session path.

use std::process::{Child, Command, Stdio};

use frust_kernel::db::{Db, scoped_db};
use frust_kernel::meta::meta_ddl;
use frust_kernel::tenancy::single_tenant;

struct ServedKernel(Child);

impl Drop for ServedKernel {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn bundle(version: &str, label: &str) -> serde_json::Value {
    serde_json::json!({
        "manifest_version": 1,
        "name": "geo",
        "version": version,
        "doctypes": [{
            "name": "geo_country",
            "fields": [
                { "fieldname": "code", "fieldtype": "Data", "required": true },
                { "fieldname": "label", "fieldtype": "Data" }
            ]
        }],
        "fixtures": [{
            "doctype": "geo_country",
            "key": "kenya",
            "values": { "code": "KE", "label": label }
        }]
    })
}

fn post(
    base: &str,
    path: &str,
    token: Option<&str>,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let mut request = agent.post(format!("{base}{path}"));
    if let Some(token) = token {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let mut response = request.send(body.to_string()).expect("REST request");
    let status = response.status().as_u16();
    let json = response
        .body_mut()
        .read_json()
        .unwrap_or_else(|_| serde_json::json!({}));
    (status, json)
}

fn setup_database(name: &str) -> Db {
    let target = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&target);
    db.sql_root_ns(&format!(
        "REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};"
    ))
    .expect("fresh database");
    db.sql_root(&meta_ddl()).expect("meta schema");
    db.sql_root(
        "CREATE app_user:mgr SET name = 'mgr', role = 'manager', \
         pass = crypto::argon2::generate('pw-mgr');",
    )
    .expect("manager");
    db
}

#[test]
fn serve_installs_visible_fixtures_and_refuses_a_user_modified_update_over_rest() {
    let database = format!("fixture_serve_{}", std::process::id());
    let db = setup_database(&database);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let address = format!("127.0.0.1:{port}");
    let base = format!("http://{address}");
    let artifacts = concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts");

    let child = Command::new(env!("CARGO_BIN_EXE_frust"))
        .arg("serve")
        .env("FRUST_TENANT", &database)
        .env("FRUST_ADDR", &address)
        .env("FRUST_ARTIFACTS", artifacts)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start frust serve");
    let _kernel = ServedKernel(child);

    for _ in 0..200 {
        if ureq::get(format!("{base}/health")).call().is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        ureq::get(format!("{base}/health")).call().is_ok(),
        "serve did not become ready"
    );

    let (status, login) = post(
        &base,
        "/login",
        None,
        serde_json::json!({ "user": "mgr", "pass": "pw-mgr" }),
    );
    assert_eq!(status, 200, "login: {login}");
    let token = login["token"].as_str().expect("token").to_string();

    let (status, installed) = post(
        &base,
        "/app/install",
        Some(&token),
        bundle("1.0.0", "Kenya"),
    );
    assert_eq!(status, 200, "install: {installed}");
    let (status, read) = post(
        &base,
        "/read/geo_country",
        Some(&token),
        serde_json::json!({}),
    );
    assert_eq!(status, 200, "read: {read}");
    let row = read["rows"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["code"] == "KE"))
        .expect("fixture visible through REST");
    assert_eq!(row["label"], serde_json::json!("Kenya"));

    let (status, edited) = post(
        &base,
        "/write/geo_country",
        Some(&token),
        serde_json::json!({ "record": "kenya", "doc": { "label": "Local Name" } }),
    );
    assert_eq!(status, 200, "user edit: {edited}");
    let stored = db
        .sql_root("SELECT label FROM geo_country:kenya;")
        .expect("stored row");
    assert_eq!(
        stored.as_array().unwrap()[0]["label"],
        serde_json::json!("Local Name")
    );

    let (status, refused) = post(
        &base,
        "/app/update",
        Some(&token),
        bundle("1.1.0", "Republic of Kenya"),
    );
    assert_eq!(
        status, 409,
        "modified fixture update must conflict: {refused}"
    );
    assert_eq!(
        refused["error"]["kind"],
        serde_json::json!("fixture-refused")
    );
    assert_eq!(
        refused["error"]["doctype"],
        serde_json::json!("geo_country")
    );
    assert_eq!(refused["error"]["key"], serde_json::json!("kenya"));
    assert!(refused["error"]["apps"].to_string().contains("geo"));
    assert!(
        refused["error"]["detail"]
            .as_str()
            .unwrap_or("")
            .contains("acknowledge")
    );
    let after = db
        .sql_root("SELECT label FROM geo_country:kenya;")
        .expect("row after refusal");
    assert_eq!(
        after.as_array().unwrap()[0]["label"],
        serde_json::json!("Local Name")
    );
}
