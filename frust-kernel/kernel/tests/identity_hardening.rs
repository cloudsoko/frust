//! WO-008: the $auth sharp edge, hardened. Criteria 1-3 executed:
//! kernel-owned identity posture with boot repair, typed refusal of
//! NULL-identity stamps, and null-safe permission clauses.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use frust_kernel::boot::{boot, BootOptions};
use frust_kernel::broker::{Broker, Caller, HookChain, HookDispatch};
use frust_kernel::contract::*;
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};
use frust_kernel::meta::{access_ddl, identity_ddl};
use frust_kernel::sync::MetadataSync;

struct PassHooks;
impl HookDispatch for PassHooks {
    fn validate(&self, doctype: &str, doc: &[(String, Value)]) -> Result<Vec<(String, Value)>, BrokerError> {
        Ok(doc.to_vec())
    }
}

fn fresh(name: &str) -> (Db, ResolvedTenant) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    db.sql_root(&format!(
        "{}; {}; \
         CREATE app_user:c1 SET name = 'c1', role = 'user', pass = crypto::argon2::generate('pw-c1'); \
         CREATE app_user:c2 SET name = 'c2', role = 'user', pass = crypto::argon2::generate('pw-c2'); \
         CREATE app_user:m1 SET name = 'm1', role = 'manager', pass = crypto::argon2::generate('pw-m1'); \
         DEFINE TABLE doctype SCHEMALESS PERMISSIONS NONE; \
         CREATE doctype CONTENT {{ name: 'idoc', fields: [ {{ fieldname: 'title', fieldtype: 'Data' }} ] }};",
        identity_ddl(),
        access_ddl(),
    ))
    .unwrap();
    MetadataSync { base: cfg.clone() }.sync(&db).expect("sync");
    (db, cfg)
}

fn caller(name: &str, role: &str) -> Caller {
    Caller { user: name.into(), pass: format!("pw-{name}"), role: role.into() }
}

/// Criteria 1 + 2, the exact drift scenario from the WO-007 finding:
/// healthy posture stamps owner; drifted posture (`PERMISSIONS NONE`) makes
/// the write FAIL TYPED instead of stamping NULL; boot repairs the drift.
#[test]
fn drift_scenario_fails_typed_and_boot_repairs() {
    let (db, cfg) = fresh("idh_drift");
    let b = Broker::new(scoped_db(&cfg), Box::new(PassHooks));
    let c1 = caller("c1", "user");
    let doc = vec![("title".to_string(), Value::Text("mine".into()))];

    // healthy: owner stamps, creator reads their own row back
    let out = b.db_write(&c1, &HookChain::default(), WriteOp::Create, "idoc", None, &doc).unwrap();
    assert_eq!(out["owner"].as_str(), Some("app_user:c1"), "owner stamped under kernel posture");

    // the drift: an operator flips app_user to PERMISSIONS NONE
    db.sql_root(
        "DEFINE TABLE OVERWRITE app_user SCHEMAFULL PERMISSIONS NONE; \
         DEFINE FIELD OVERWRITE name ON app_user TYPE string; \
         DEFINE FIELD OVERWRITE role ON app_user TYPE string; \
         DEFINE FIELD OVERWRITE pass ON app_user TYPE string; \
         DEFINE FIELD OVERWRITE status ON app_user TYPE option<string>;",
    )
    .unwrap();

    // the write REFUSES with the machine code â€” never a silent NULL owner
    let before = row_count(&db);
    let err = b.db_write(&c1, &HookChain::default(), WriteOp::Create, "idoc", None, &doc).unwrap_err();
    assert_eq!(err, BrokerError::IdentityUnresolved, "E_IDENTITY_UNRESOLVED, got {err:?}");
    assert_eq!(row_count(&db), before, "refused write stored nothing");

    // boot repairs the posture (every boot, not just migrations) â€” drift has
    // no standing
    boot(&db, &BootOptions::default(), &MetadataSync { base: cfg.clone() }).expect("boot repairs");
    let out = b.db_write(&c1, &HookChain::default(), WriteOp::Create, "idoc", None, &doc).unwrap();
    assert_eq!(out["owner"].as_str(), Some("app_user:c1"), "repaired posture stamps again");
}

fn row_count(db: &Db) -> i64 {
    db.sql_root("SELECT count() FROM idoc GROUP ALL;")
        .unwrap()
        .as_array()
        .and_then(|a| a.first())
        .and_then(|r| r["count"].as_i64())
        .unwrap_or(0)
}

/// Criterion 3: NONE = NONE can never grant. A NULL-owner row (root-written,
/// the legitimate system-write case) is invisible to record principals; each
/// user still sees exactly their own rows; the manager path is the ROLE
/// clause, not the NULL hole.
#[test]
fn null_owner_rows_invisible_to_record_principals() {
    let (db, cfg) = fresh("idh_null");
    let b = Broker::new(scoped_db(&cfg), Box::new(PassHooks));

    // root/system write: owner legitimately NONE
    db.sql_root("CREATE idoc:sys SET title = 'system row';").unwrap();
    // two users write their own
    let doc = |t: &str| vec![("title".to_string(), Value::Text(t.into()))];
    b.db_write(&caller("c1", "user"), &HookChain::default(), WriteOp::Create, "idoc", None, &doc("c1 doc")).unwrap();
    b.db_write(&caller("c2", "user"), &HookChain::default(), WriteOp::Create, "idoc", None, &doc("c2 doc")).unwrap();

    let read = |c: &Caller| b.db_read(c, "idoc", None, &[], &ReadOpts::default()).unwrap();
    let c1_rows = read(&caller("c1", "user"));
    let c2_rows = read(&caller("c2", "user"));
    // before WO-008 the null-owner row was visible to BOTH via NONE = NONE
    assert_eq!(c1_rows.len(), 1, "c1 sees exactly their own row: {c1_rows:?}");
    assert_eq!(c1_rows[0]["owner"].as_str(), Some("app_user:c1"));
    assert_eq!(c2_rows.len(), 1, "c2 sees exactly their own row");
    // the manager sees all three â€” via the role clause, which is the design
    assert_eq!(read(&caller("m1", "manager")).len(), 3, "manager path unchanged");
}

