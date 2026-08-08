//! Server-script provenance through the resident multi-tenant HTTP service.
//!
//! Both tenants are booted by the shipped binary, share its one Wasm engine,
//! and write concurrently through REST. Each tenant's script stamps a distinct
//! marker, so the assertion identifies which script ran rather than merely
//! observing that some script ran.
//!
//! Requires SurrealDB on 127.0.0.1:8899 with root/root credentials and the
//! compiled hook artifacts.

use std::process::{Child, Command, Stdio};

use frust_kernel::db::{scoped_db, Db};
use frust_kernel::meta::meta_ddl;
use frust_kernel::sync::MetadataSync;
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};

const TENANT_A: &str = "script_provenance_a";
const TENANT_B: &str = "script_provenance_b";
const DOCTYPE: &str = "tenant_script_note";
const WRITES_PER_TENANT: usize = 4;

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

fn seed(tenant: &str, marker: &str) -> (ResolvedTenant, Db) {
    let target = single_tenant(tenant).expect("resolve tenant");
    let db = scoped_db(&target);
    db.sql_root_ns(&format!(
        "REMOVE DATABASE IF EXISTS {tenant}; DEFINE DATABASE {tenant};"
    ))
    .expect("reset tenant database");
    db.sql_root(&meta_ddl()).expect("install kernel metadata");
    db.sql_root(
        "CREATE app_user:writer SET name = 'writer', role = 'manager', \
         pass = crypto::argon2::generate('pw-writer');",
    )
    .expect("create REST user");

    let definition = serde_json::json!({
        "name": DOCTYPE,
        "label": "Tenant script note",
        "app": "tenant_proof",
        "fields": [
            { "fieldname": "title", "fieldtype": "Data", "required": true },
            { "fieldname": "script_marker", "fieldtype": "Data", "required": false }
        ],
        "server_script": [{
            "app": "tenant_proof",
            "hook": "validate",
            "script": format!("doc.script_marker = '{marker}';")
        }]
    });
    db.sql_root(&format!("CREATE doctype CONTENT {definition};"))
        .expect("create scripted doctype");
    MetadataSync {
        base: target.clone(),
    }
    .sync(&db)
    .expect("sync scripted table");
    (target, db)
}

struct Resident(Child);

impl Drop for Resident {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into()
}

fn start_resident(addr: &str) -> Resident {
    let child = Command::new(env!("CARGO_BIN_EXE_frust"))
        .arg("serve")
        .env("FRUST_TENANCY", "database-per-tenant")
        .env("FRUST_TENANTS", format!("{TENANT_A},{TENANT_B}"))
        .env("FRUST_ADDR", addr)
        .env("FRUST_ARTIFACTS", artifacts())
        .env_remove("FRUST_DATABASE")
        .env_remove("FRUST_ENVIRONMENT")
        .env_remove("FRUST_TENANT")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start resident kernel");
    Resident(child)
}

fn wait_until_ready(resident: &mut Resident, url: &str) {
    for _ in 0..120 {
        if let Some(status) = resident.0.try_wait().expect("inspect resident") {
            panic!("resident kernel exited during boot: {status}");
        }
        if agent()
            .get(format!("{url}/health"))
            .call()
            .is_ok_and(|response| response.status().as_u16() == 200)
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("resident kernel did not become ready at {url}");
}

fn login(url: &str, tenant: &str) -> String {
    let mut response = agent()
        .post(format!("{url}/login"))
        .send(
            serde_json::json!({
                "tenant": tenant,
                "user": "writer",
                "pass": "pw-writer"
            })
            .to_string(),
        )
        .expect("login response");
    assert_eq!(response.status().as_u16(), 200, "login failed for {tenant}");
    let body: serde_json::Value = response.body_mut().read_json().expect("login JSON");
    body["token"].as_str().expect("login token").to_string()
}

fn write(url: &str, token: &str, title: &str) -> serde_json::Value {
    let mut response = agent()
        .post(format!("{url}/write/{DOCTYPE}"))
        .header("Authorization", &format!("Bearer {token}"))
        .send(serde_json::json!({ "doc": { "title": title } }).to_string())
        .expect("write response");
    let status = response.status().as_u16();
    let body: serde_json::Value = response.body_mut().read_json().expect("write JSON");
    assert_eq!(status, 200, "write {title} failed: {body}");
    body["created"].clone()
}

fn assert_stored_provenance(db: &Db, tenant: &str, marker: &str) {
    let rows = db
        .sql_root(&format!(
            "SELECT title, script_marker FROM {DOCTYPE} ORDER BY title;"
        ))
        .expect("read stored documents");
    let rows = rows.as_array().expect("stored row array");
    assert_eq!(
        rows.len(),
        WRITES_PER_TENANT,
        "{tenant}: missing stored writes"
    );
    for row in rows {
        assert!(
            row["title"]
                .as_str()
                .is_some_and(|title| title.starts_with(tenant)),
            "{tenant}: found a document from another tenant: {row}"
        );
        assert_eq!(
            row["script_marker"].as_str(),
            Some(marker),
            "{tenant}: stored document proves the wrong tenant's script ran: {row}"
        );
    }
}

#[test]
fn concurrent_rest_writes_run_each_requests_own_tenant_script() {
    let (_target_a, db_a) = seed(TENANT_A, "marker-from-a");
    let (_target_b, db_b) = seed(TENANT_B, "marker-from-b");

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve test port");
    let addr = listener.local_addr().expect("test address").to_string();
    drop(listener);
    let url = format!("http://{addr}");
    let mut resident = start_resident(&addr);
    wait_until_ready(&mut resident, &url);

    let token_a = login(&url, TENANT_A);
    let token_b = login(&url, TENANT_B);
    let barrier = std::sync::Barrier::new(2);

    std::thread::scope(|scope| {
        for (tenant, marker, token) in [
            (TENANT_A, "marker-from-a", token_a),
            (TENANT_B, "marker-from-b", token_b),
        ] {
            let barrier = &barrier;
            let url = &url;
            scope.spawn(move || {
                barrier.wait();
                for i in 0..WRITES_PER_TENANT {
                    let title = format!("{tenant}-{i}");
                    let created = write(url, &token, &title);
                    assert_eq!(
                        created["script_marker"].as_str(),
                        Some(marker),
                        "{tenant}: REST response proves the wrong tenant's script ran: {created}"
                    );
                }
            });
        }
    });

    assert_stored_provenance(&db_a, TENANT_A, "marker-from-a");
    assert_stored_provenance(&db_b, TENANT_B, "marker-from-b");
}
