//! Additive BYO-frontend REST enrichments: metadata fidelity, ergonomic record
//! filters, exact money writes, and configured-store routing.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookDispatch};
use frust_kernel::contract::{BrokerError, Value};
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::meta::{access_ddl, identity_ddl, meta_ddl};
use frust_kernel::rest::Rest;
use frust_kernel::sync::MetadataSync;
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

fn manager() -> Caller {
    Caller {
        user: "manager".into(),
        pass: "pw-manager".into(),
        role: "manager".into(),
    }
}

fn setup(name: &str) -> (Rest, Db, ResolvedTenant) {
    let target = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&target);
    db.sql_root_ns(&format!(
        "REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};"
    ))
    .expect("fresh database");
    db.sql_root(&format!(
        "{}; {}; {}",
        meta_ddl(),
        identity_ddl(),
        access_ddl()
    ))
    .expect("meta schema");
    db.sql_root(
        "CREATE app_user:manager SET name = 'manager', role = 'manager', \
         pass = crypto::argon2::generate('pw-manager'); \
         CREATE doctype:invoice CONTENT { \
           name: 'invoice', label: 'Invoice', submittable: true, fields: [ \
             { fieldname: 'customer', fieldtype: 'Data', label: 'Bill To' }, \
             { fieldname: 'total', fieldtype: 'Currency', label: 'Grand Total' }, \
             { fieldname: 'workflow_state', fieldtype: 'Data', label: 'Approval State' } \
           ] \
         }; \
         CREATE workflow:invoice_approval CONTENT { \
           name: 'invoice_approval', doctype: 'invoice', \
           states: [ \
             { name: 'Draft', docstatus: 0 }, \
             { name: 'Approved', docstatus: 1 } \
           ], \
           transitions: [ \
             { from: 'Draft', to: 'Approved', role: 'manager', action: 'Approve' } \
           ], \
           state_rules: [] \
         };",
    )
    .expect("BYO metadata");
    MetadataSync {
        base: target.clone(),
    }
    .sync(&db)
    .expect("schema sync");

    let broker = Arc::new(Broker::new(scoped_db(&target), Box::new(PassHooks)));
    let rest = Rest::single(
        broker,
        "127.0.0.1:0".into(),
        Some(Arc::new(MetadataSync {
            base: target.clone(),
        })),
        None,
    );
    (rest, db, target)
}

fn call(rest: &Rest, path: &str, body: serde_json::Value) -> serde_json::Value {
    rest.route_for_test(path, &body, &manager())
        .unwrap_or_else(|error| {
            panic!("{path} failed: {error:?}");
        })
}

#[test]
fn meta_exposes_field_label_values_and_attached_workflow_content() {
    let (rest, _db, _target) = setup("wo071_meta");
    let response = call(&rest, "/meta/invoice", serde_json::json!({}));
    let doctype = &response["doctype"];

    let fields = doctype["fields"].as_array().expect("fields");
    let label = |fieldname: &str| {
        fields
            .iter()
            .find(|field| field["fieldname"] == fieldname)
            .and_then(|field| field["label"].as_str())
    };
    assert_eq!(label("customer"), Some("Bill To"));
    assert_eq!(label("total"), Some("Grand Total"));

    assert_eq!(
        doctype["workflow"]["name"],
        serde_json::json!("invoice_approval")
    );
    assert_eq!(
        doctype["workflow"]["states"],
        serde_json::json!([
            { "name": "Draft", "docstatus": 0 },
            { "name": "Approved", "docstatus": 1 }
        ])
    );
    assert_eq!(
        doctype["workflow"]["transitions"][0],
        serde_json::json!({
            "from": "Draft", "to": "Approved", "role": "manager", "action": "Approve"
        })
    );
}

#[test]
fn plain_string_id_filter_returns_the_same_row_as_typed_record_filter() {
    let (rest, _db, _target) = setup("wo071_id_filter");
    let created = call(
        &rest,
        "/write/invoice",
        serde_json::json!({ "doc": { "customer": "Filter Fidelity", "total": "7.25" } }),
    );
    let id = created["record"].as_str().expect("record id");
    // The bare key (no `doctype:` prefix) exercises the coercion that scopes an
    // unqualified id to the route's DocType; a fully-qualified string would skip
    // that branch entirely.
    let bare_key = id.split_once(':').map(|(_, key)| key).expect("bare key");

    let typed = call(
        &rest,
        "/read/invoice",
        serde_json::json!({
            "filter": { "path": "id", "op": "eq", "value": { "kind": "record", "v": id } }
        }),
    );
    let plain = call(
        &rest,
        "/read/invoice",
        serde_json::json!({
            "filter": { "path": "id", "op": "eq", "value": bare_key }
        }),
    );

    assert_eq!(typed["rows"].as_array().map(Vec::len), Some(1));
    assert_eq!(plain["rows"], typed["rows"]);
    assert_eq!(plain["rows"][0]["id"], serde_json::json!(id));
}

#[test]
fn bare_decimal_currency_write_persists_the_exact_string() {
    let (rest, db, _target) = setup("wo071_currency");
    let created = call(
        &rest,
        "/write/invoice",
        serde_json::json!({ "doc": { "customer": "Exact Money", "total": "1234.56789" } }),
    );
    let id = created["record"].as_str().expect("record id");
    assert_eq!(created["created"]["total"], serde_json::json!("1234.56789"));

    let stored = db
        .sql_root(&format!(
            "SELECT VALUE total FROM ONLY type::record('{id}');"
        ))
        .expect("stored decimal");
    assert_eq!(stored, serde_json::json!("1234.56789"));

    let invalid = rest.route_for_test(
        "/write/invoice",
        &serde_json::json!({ "doc": { "customer": "Bad Money", "total": "not-decimal" } }),
        &manager(),
    );
    assert!(matches!(invalid, Err(BrokerError::InvalidValue { .. })));

    // A fractional JSON NUMBER is a float, and money never coerces from a float:
    // the write is refused, never silently rounded into a Currency field.
    let fractional = rest.route_for_test(
        "/write/invoice",
        &serde_json::json!({ "doc": { "customer": "Float Money", "total": 1234.56789 } }),
        &manager(),
    );
    assert!(
        matches!(fractional, Err(BrokerError::InvalidValue { .. })),
        "fractional JSON number must be refused on a money field, got {fractional:?}"
    );
}

#[test]
fn configured_db_endpoint_is_used_for_live_queries() {
    let (_rest, db, target) = setup("wo071_endpoint");
    let configured = std::env::var("FRUST_DB_ENDPOINT").expect("test must pin FRUST_DB_ENDPOINT");

    assert_eq!(target.endpoint(), configured);
    assert_eq!(db.target().endpoint(), configured);
    assert_eq!(
        db.sql_root("RETURN 'configured-endpoint';").unwrap(),
        serde_json::json!("configured-endpoint")
    );
}
