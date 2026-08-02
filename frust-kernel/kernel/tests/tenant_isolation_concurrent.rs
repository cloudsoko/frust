//! **WO-040 Chunk A, ADR-003 invariant 2: no shared-context mutation.**
//!
//! Two tenants, one process, **one shared wasmtime engine**, concurrent
//! requests — and the assertion is *whose* row came back, never that a call
//! succeeded. That rule is the WO-039 near-miss made permanent: the first
//! tenancy probe reported a cross-tenant leak that did not exist because it
//! asserted an operation ("non-200 or zero rows") instead of the outcome it
//! needed. A routing bug that serves tenant B's data to tenant A is the one
//! defect this work order cannot ship, and a status code cannot see it.
//!
//! What is genuinely on trial: `Broker::with_shared_hooks` means both tenants
//! run their validation through the SAME `Arc<WasmHooks>` — the one piece of
//! per-request machinery that is now shared between them. If a target could
//! leak through that shared context, this is where it would show.
//!
//!   cargo test --test tenant_isolation_concurrent
//!
//! Requires the dev surreal on :8899.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookChain, HookDispatch};
use frust_kernel::contract::{ReadOpts, Value, WriteOp};
use frust_kernel::db::{scoped_db, ConnConfig};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::{access_ddl, identity_ddl};
use frust_kernel::rest::Rest;
use frust_kernel::router::{TenantContext, TenantRouter};
use frust_kernel::sync::MetadataSync;
use frust_kernel::tenancy::{single_tenant, ResolvedTenant, Tenancy, TenancyConfig};

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

const THREADS_PER_TENANT: usize = 4;
const ROUNDS: usize = 6;

/// A tenant with its own database, its own users, and **data that names its
/// owner**. Every title carries the tenant id, which is what makes provenance
/// checkable rather than inferable.
fn seed(tenant: &str) -> ResolvedTenant {
    let target = single_tenant(tenant).expect("tenancy");
    let db = scoped_db(&target);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {tenant}; DEFINE DATABASE {tenant};"))
        .expect("provision database");
    db.sql_root(&format!(
        "{}; {}; DEFINE TABLE doctype SCHEMALESS PERMISSIONS NONE; \
         CREATE app_user:worker SET name = 'worker', role = 'manager', pass = crypto::argon2::generate('pw'); \
         CREATE doctype CONTENT {{ name: 'purchase_order', label: 'purchase order', fields: [ \
            {{ fieldname: 'title', fieldtype: 'Data', required: true }}, \
            {{ fieldname: 'total', fieldtype: 'Currency', required: true }} ] }};",
        identity_ddl(),
        access_ddl(),
    ))
    .expect("seed metadata");
    MetadataSync { base: target.clone() }.sync(&db).expect("sync purchase_order");
    db.sql_root(&format!(
        "CREATE purchase_order SET title = '{tenant}-seed', owner = app_user:worker, status = 'Draft', total = 1.00dec;"
    ))
    .expect("seed row");
    target
}

fn caller() -> Caller {
    Caller { user: "worker".into(), pass: "pw".into(), role: "manager".into() }
}

fn doc(title: &str) -> Vec<(String, Value)> {
    vec![
        ("title".to_string(), Value::Text(title.to_string())),
        ("total".to_string(), Value::Float(10.0)),
    ]
}

fn titles(rows: &[serde_json::Value]) -> Vec<String> {
    rows.iter()
        .map(|r| r.get("title").and_then(|t| t.as_str()).unwrap_or("<missing>").to_string())
        .collect()
}

#[test]
fn two_tenants_under_concurrency_only_ever_see_their_own_rows() {
    let a = seed("wo040_iso_a");
    let b = seed("wo040_iso_b");

    // ONE engine for both tenants — the WO-040 criterion-2 change, and the
    // only shared per-request machinery in the process.
    let hooks: Arc<dyn HookDispatch> =
        Arc::from(Box::new(WasmHooks::load(artifacts()).expect("hooks")) as Box<dyn HookDispatch>);
    let brokers: Vec<(String, Arc<Broker>)> = vec![
        ("wo040_iso_a".to_string(), Arc::new(Broker::with_shared_hooks(scoped_db(&a), Arc::clone(&hooks)))),
        ("wo040_iso_b".to_string(), Arc::new(Broker::with_shared_hooks(scoped_db(&b), Arc::clone(&hooks)))),
    ];

    let foreign: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed: Arc<std::sync::Mutex<Vec<(String, usize)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));

    std::thread::scope(|scope| {
        for (tenant, broker) in &brokers {
            for t in 0..THREADS_PER_TENANT {
                let (tenant, broker) = (tenant.clone(), Arc::clone(broker));
                let (foreign, observed) = (Arc::clone(&foreign), Arc::clone(&observed));
                scope.spawn(move || {
                    for i in 0..ROUNDS {
                        let title = format!("{tenant}-t{t}-r{i}");
                        broker
                            .db_write(
                                &caller(),
                                &HookChain::default(),
                                WriteOp::Create,
                                "purchase_order",
                                None,
                                &doc(&title),
                            )
                            .expect("write");

                        // THE ASSERTION THAT MATTERS: whose rows are these?
                        let rows = broker
                            .db_read(&caller(), "purchase_order", None, &[], &ReadOpts::default())
                            .expect("read");
                        let seen = titles(&rows);
                        for title in &seen {
                            if !title.starts_with(&tenant) {
                                foreign.lock().unwrap().push(format!(
                                    "{tenant} read a row belonging to someone else: {title}"
                                ));
                            }
                        }
                        observed.lock().unwrap().push((tenant.clone(), seen.len()));
                    }
                });
            }
        }
    });

    let foreign = foreign.lock().unwrap();
    let observed = observed.lock().unwrap();

    // The check must be able to fail: a run that read nothing would satisfy
    // "no foreign rows" for the wrong reason. Both tenants must be seen
    // returning their own data, and every read must have found the writes.
    for tenant in ["wo040_iso_a", "wo040_iso_b"] {
        let reads: Vec<usize> =
            observed.iter().filter(|(t, _)| t == tenant).map(|(_, n)| *n).collect();
        assert_eq!(reads.len(), THREADS_PER_TENANT * ROUNDS, "{tenant}: missing reads");
        assert!(reads.iter().all(|n| *n > 0), "{tenant}: a read returned no rows at all");
        assert_eq!(
            *reads.iter().max().unwrap(),
            1 + THREADS_PER_TENANT * ROUNDS,
            "{tenant}: final read should see the seed row plus every write of ITS OWN tenant"
        );
    }

    assert!(foreign.is_empty(), "CROSS-TENANT LEAK:\n{}", foreign.join("\n"));
    println!(
        "PASS: {} reads across 2 tenants on one shared engine, 0 foreign rows",
        observed.len()
    );
}

// ── WO-040 Chunk B2: the same proof, through the SHIPPED REQUEST PATH ──────
//
// Everything above builds its brokers in-process and calls them directly. This
// is the version that counts: **one `Rest`, one port, one process**, two
// tenants, and the only thing telling a request which database to land in is
// the tenant prefix on its own bearer token.

const RT_A: &str = "wo040c_rt_a";
const RT_B: &str = "wo040c_rt_b";

fn agent() -> ureq::Agent {
    // keep-alive on purpose: without it a few hundred requests churn Windows
    // ephemeral ports and we would measure the client (WO-041)
    ureq::Agent::config_builder().http_status_as_error(false).build().into()
}

fn login_as(url: &str, tenant: &str) -> String {
    let mut r = agent()
        .post(format!("{url}/login"))
        .send(
            serde_json::json!({ "user": "worker", "pass": "pw", "tenant": tenant }).to_string(),
        )
        .expect("login");
    let body: serde_json::Value = r.body_mut().read_json().expect("login json");
    assert_eq!(
        body["tenant"].as_str(),
        Some(tenant),
        "login answered for a different tenant than it was asked for: {body}"
    );
    body["token"].as_str().expect("token").to_string()
}

fn http_write(url: &str, token: &str, title: &str) -> u16 {
    agent()
        .post(format!("{url}/write/purchase_order"))
        .header("Authorization", &format!("Bearer {token}"))
        .send(
            serde_json::json!({ "doc": { "title": title, "total": 10.0 } }).to_string(),
        )
        .expect("write")
        .status()
        .as_u16()
}

fn http_read(url: &str, token: &str) -> (u16, Vec<String>) {
    let mut r = agent()
        .post(format!("{url}/read/purchase_order"))
        .header("Authorization", &format!("Bearer {token}"))
        .send("{}")
        .expect("read");
    let code = r.status().as_u16();
    let body: serde_json::Value = r.body_mut().read_json().unwrap_or(serde_json::json!({}));
    let mut titles: Vec<String> = body["rows"]
        .as_array()
        .map(|rows| {
            rows.iter().map(|x| x["title"].as_str().unwrap_or("<missing>").to_string()).collect()
        })
        .unwrap_or_default();
    titles.sort();
    (code, titles)
}

/// **THE EXIT PROOF: one process, N tenants, isolation held.**
#[test]
fn one_process_routes_two_tenants_and_no_request_ever_crosses() {
    let tenancy = Tenancy::from_config(
        ConnConfig::default(),
        TenancyConfig {
            strategy: "database-per-tenant",
            namespace: Some("frust"),
            database: None,
            environment: None,
            tenants: &[RT_A, RT_B],
        },
    )
    .expect("tenancy");

    for t in [RT_A, RT_B] {
        seed(t);
    }

    // ONE engine and ONE router for both tenants — the shipped shape.
    let hooks: Arc<dyn HookDispatch> =
        Arc::from(Box::new(WasmHooks::load(artifacts()).expect("hooks")) as Box<dyn HookDispatch>);
    let router = TenantRouter::build(&tenancy, |target| {
        Ok(TenantContext {
            broker: Arc::new(Broker::with_shared_hooks(scoped_db(target), Arc::clone(&hooks))),
            sync: None,
        })
    })
    .expect("router");
    assert_eq!(router.len(), 2, "the router must carry both tenants");

    let addr = "127.0.0.1:8814";
    let url = format!("http://{addr}");
    std::thread::spawn(move || {
        let _ = Rest { router: Arc::new(router), addr: addr.into(), realtime: None }.serve(|| {});
    });
    for _ in 0..50 {
        if ureq::get(format!("{url}/health")).call().is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // SAME URL, SAME PROCESS — only the token differs
    let tok_a = login_as(&url, RT_A);
    let tok_b = login_as(&url, RT_B);
    assert!(tok_a.starts_with(&format!("{RT_A}.")), "token does not carry its tenant: {tok_a}");
    assert!(tok_b.starts_with(&format!("{RT_B}.")));

    let foreign: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed: Arc<std::sync::Mutex<Vec<(String, usize)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));

    std::thread::scope(|scope| {
        for (tenant, token) in [(RT_A, &tok_a), (RT_B, &tok_b)] {
            for t in 0..THREADS_PER_TENANT {
                let (url, token) = (url.clone(), token.clone());
                let (foreign, observed) = (Arc::clone(&foreign), Arc::clone(&observed));
                scope.spawn(move || {
                    for i in 0..ROUNDS {
                        let title = format!("{tenant}-t{t}-r{i}");
                        assert_eq!(http_write(&url, &token, &title), 200, "write {title}");

                        let (code, seen) = http_read(&url, &token);
                        assert_eq!(code, 200, "read for {tenant}");
                        for title in &seen {
                            if !title.starts_with(tenant) {
                                foreign.lock().unwrap().push(format!(
                                    "{tenant} was served a row belonging to someone else: {title}"
                                ));
                            }
                        }
                        observed.lock().unwrap().push((tenant.to_string(), seen.len()));
                    }
                });
            }
        }
    });

    let foreign = foreign.lock().unwrap();
    let observed = observed.lock().unwrap();

    // non-vacuous: the run happened, every read found rows, and the biggest
    // read saw the seed row plus exactly this tenant's own writes
    for tenant in [RT_A, RT_B] {
        let reads: Vec<usize> =
            observed.iter().filter(|(t, _)| t == tenant).map(|(_, n)| *n).collect();
        assert_eq!(reads.len(), THREADS_PER_TENANT * ROUNDS, "{tenant}: missing reads");
        assert!(reads.iter().all(|n| *n > 0), "{tenant}: a read returned no rows");
        assert_eq!(
            *reads.iter().max().unwrap(),
            1 + THREADS_PER_TENANT * ROUNDS,
            "{tenant}: final read should see its seed row plus its OWN writes and nothing else"
        );
    }
    assert!(foreign.is_empty(), "CROSS-TENANT LEAK OVER HTTP:\n{}", foreign.join("\n"));

    // ── spoofing the prefix fails closed ──
    //
    // A's secret, presented under B's tenant. The lookup happens IN B's
    // database, where that secret does not exist — so it is refused, and no
    // cross-tenant read is possible even in principle. There is no shared
    // session table for it to be found in.
    let a_secret = tok_a.split_once('.').unwrap().1;
    let forged = format!("{RT_B}.{a_secret}");
    let (code, rows) = http_read(&url, &forged);
    assert_eq!(code, 401, "a forged tenant prefix was accepted, returning {rows:?}");
    assert!(rows.is_empty(), "a refused request still returned rows: {rows:?}");

    // an unregistered tenant prefix is refused the same way — resolution is
    // not a tenant oracle
    let (code, _) = http_read(&url, &format!("wo040c_not_a_tenant.{a_secret}"));
    assert_eq!(code, 401);

    println!(
        "PASS: {} HTTP reads across 2 tenants through ONE process, 0 foreign rows, spoof refused",
        observed.len()
    );
}

/// **The control.** The provenance check above only means something if the
/// query shape it uses *can* find another tenant's rows when pointed at them.
/// Without this, "A never saw B's data" is indistinguishable from "the read
/// never worked" — the WO-039 failure mode in its other direction.
#[test]
fn the_provenance_check_can_see_a_foreign_row_when_one_is_really_there() {
    let a = seed("wo040_ctl_a");
    let b = seed("wo040_ctl_b");

    // plant a row TITLED as A's, physically inside B's database
    scoped_db(&b)
        .sql_root(
            "CREATE purchase_order SET title = 'wo040_ctl_a-planted', owner = app_user:worker, \
             status = 'Draft', total = 1.00dec;",
        )
        .expect("plant");

    let hooks: Arc<dyn HookDispatch> =
        Arc::from(Box::new(WasmHooks::load(artifacts()).expect("hooks")) as Box<dyn HookDispatch>);
    let read = |target: &ResolvedTenant| {
        let broker = Broker::with_shared_hooks(scoped_db(target), Arc::clone(&hooks));
        titles(&broker.db_read(&caller(), "purchase_order", None, &[], &ReadOpts::default()).expect("read"))
    };

    let from_b = read(&b);
    assert!(
        from_b.iter().any(|t| t.starts_with("wo040_ctl_a")),
        "the control failed to find the planted row in B: {from_b:?} — the provenance \
         assertion in this file would then be passing for the wrong reason"
    );

    // ...and A's own database still contains nothing of the sort
    let from_a = read(&a);
    assert!(
        from_a.iter().all(|t| t.starts_with("wo040_ctl_a")),
        "A saw a foreign row: {from_a:?}"
    );
    assert!(!from_a.iter().any(|t| t.ends_with("-planted")), "A saw B's planted row: {from_a:?}");
}
