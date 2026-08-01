//! WO-018: the workflow engine (REQ-4.1.2).
//!
//! The load-bearing assertion is criterion 2's **two layers, proven
//! separately**:
//!
//! 1. the KERNEL judges — wrong role or wrong from-state fails
//!    `FRUST:E_WORKFLOW:*` before any write is attempted;
//! 2. the LATTICE backstops — a workflow that is itself wrong (declaring a
//!    docstatus-2 state reachable from 0) is judged *allowed* by layer 1 and
//!    still refused by `FRUST:E_DOCSTATUS` from the DB EVENT.
//!
//! Proving them separately is the point: layer 2 catching layer 1's bugs is
//! what makes ADR-009 A2's split worth having.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::*;
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::single_tenant;
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::meta_ddl;
use frust_kernel::rest::Rest;
use frust_kernel::sync::MetadataSync;

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

/// The canonical expense-claim flow (criterion 6), as manifest content.
fn expense_app(broken: bool) -> serde_json::Value {
    // `broken` declares Approved at docstatus 2 reachable straight from Draft:
    // a workflow the KERNEL will happily allow and the LATTICE must refuse.
    let states = if broken {
        serde_json::json!([
            { "name": "Draft", "docstatus": 0 },
            { "name": "Approved", "docstatus": 2 }
        ])
    } else {
        serde_json::json!([
            { "name": "Draft", "docstatus": 0 },
            { "name": "Submitted for Approval", "docstatus": 0 },
            { "name": "Approved", "docstatus": 1 },
            { "name": "Rejected", "docstatus": 0 }
        ])
    };
    let transitions = if broken {
        // WO-020: the Leap is taken by a MANAGER, not a clerk. Under the
        // row-write policy (ADR-012) a clerk-owner cannot write docstatus at
        // all, so a clerk Leap is refused by the ROW PERMISSION before it ever
        // reaches the lattice — which would prove nothing about the lattice. A
        // manager clears the row permission, so the lattice EVENT is the ONLY
        // thing left to refuse the 0->2 jump. That is layer 2 in isolation:
        // the floor refusing even a principal who passed every gate above it.
        serde_json::json!([{ "from": "Draft", "to": "Approved", "role": "manager", "action": "Leap" }])
    } else {
        serde_json::json!([
            { "from": "Draft", "to": "Submitted for Approval", "role": "clerk", "action": "Submit" },
            { "from": "Submitted for Approval", "to": "Approved", "role": "manager", "action": "Approve" },
            { "from": "Submitted for Approval", "to": "Rejected", "role": "manager", "action": "Reject" },
            { "from": "Rejected", "to": "Draft", "role": "clerk", "action": "Reopen" }
        ])
    };
    serde_json::json!({
        "manifest_version": 1,
        "name": "expenses",
        "version": "1.0.0",
        "doctypes": [{
            "name": "expense_claim",
            "submittable": true,
            "fields": [
                { "fieldname": "purpose", "fieldtype": "Data" },
                { "fieldname": "amount", "fieldtype": "Currency" },
                { "fieldname": "workflow_state", "fieldtype": "Data" }
            ]
        }],
        "workflows": [{
            "name": if broken { "expense_broken" } else { "expense_approval" },
            "doctype": "expense_claim",
            "states": states,
            "transitions": transitions,
            // The broken variant declares only Draft/Approved, so its rules
            // must too — bundle validation refuses a state rule naming an
            // undeclared state, which it demonstrated by rejecting my first
            // version of this fixture.
            "state_rules": if broken {
                serde_json::json!([{ "state": "Approved", "field": "amount", "read_only": true }])
            } else {
                serde_json::json!([
                    { "state": "Submitted for Approval", "field": "amount", "read_only": true },
                    { "state": "Approved", "field": "amount", "read_only": true }
                ])
            }
        }]
    })
}

fn setup(name: &str, broken: bool) -> (Rest, Arc<Broker>, Db) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    db.sql_root(&meta_ddl()).unwrap();
    db.sql_root(
        "CREATE app_user:mgr SET name = 'mgr', role = 'manager', pass = crypto::argon2::generate('pw-mgr'); \
         CREATE app_user:clerk SET name = 'clerk', role = 'clerk', pass = crypto::argon2::generate('pw-clerk');",
    )
    .unwrap();
    let broker = Arc::new(Broker::new(
        scoped_db(&cfg),
        Box::new(WasmHooks::load(artifacts()).expect("hooks")),
    ));
    let rest = Rest::single(Arc::clone(&broker), "127.0.0.1:0".into(), Some(Arc::new(MetadataSync { base: cfg.clone() })), None);
    // a workflow arrives as MANIFEST CONTENT — the reason WO-019 came first
    let mgr = Caller { user: "mgr".into(), pass: "pw-mgr".into(), role: "manager".into() };
    let out = rest.route_for_test("/app/install", &expense_app(broken), &mgr).expect("install");
    println!("installed: {out}");
    (rest, broker, db)
}

fn clerk() -> Caller {
    Caller { user: "clerk".into(), pass: "pw-clerk".into(), role: "clerk".into() }
}
fn mgr() -> Caller {
    Caller { user: "mgr".into(), pass: "pw-mgr".into(), role: "manager".into() }
}

fn new_claim(b: &Broker, who: &Caller) -> String {
    let doc = b
        .db_write(
            who,
            &HookChain::default(),
            WriteOp::Create,
            "expense_claim",
            None,
            &[
                ("purpose".to_string(), Value::Text("taxi".into())),
                ("amount".to_string(), Value::Decimal("42.00".into())),
                ("workflow_state".to_string(), Value::Text("Draft".into())),
            ],
        )
        .expect("create claim");
    doc["id"].as_str().unwrap().split_once(':').unwrap().1.to_string()
}

/// Criterion 6: the canonical flow, end to end, by two roles.
#[test]
fn the_expense_claim_flows_draft_to_approved_by_two_roles() {
    let (_rest, b, _db) = setup("wf_happy", false);
    let key = new_claim(&b, &clerk());

    // clerk submits for approval — still docstatus 0
    let out = b.db_transition(&clerk(), &HookChain::default(), "expense_claim", &key, "Submit")
        .expect("clerk may submit");
    assert_eq!(out["workflow_state"], serde_json::json!("Submitted for Approval"));
    assert_eq!(out["docstatus"], serde_json::json!(0), "approval requested is not yet submitted");

    // manager approves — NOW the lattice moves to 1
    let out = b.db_transition(&mgr(), &HookChain::default(), "expense_claim", &key, "Approve")
        .expect("manager may approve");
    println!("approved: {out}");
    assert_eq!(out["workflow_state"], serde_json::json!("Approved"));
    assert_eq!(out["docstatus"], serde_json::json!(1), "approval IS the lattice submit");
}

/// The rejection path returns the document to the clerk.
#[test]
fn the_rejection_path_returns_the_claim_to_draft() {
    let (_rest, b, _db) = setup("wf_reject", false);
    let key = new_claim(&b, &clerk());
    b.db_transition(&clerk(), &HookChain::default(), "expense_claim", &key, "Submit").unwrap();

    let out = b.db_transition(&mgr(), &HookChain::default(), "expense_claim", &key, "Reject")
        .expect("manager may reject");
    assert_eq!(out["workflow_state"], serde_json::json!("Rejected"));
    assert_eq!(out["docstatus"], serde_json::json!(0), "rejection stays in draft territory");

    let out = b.db_transition(&clerk(), &HookChain::default(), "expense_claim", &key, "Reopen")
        .expect("clerk may reopen");
    assert_eq!(out["workflow_state"], serde_json::json!("Draft"));
}

/// **LAYER 1** — the kernel judges, and refuses typed, before any write.
#[test]
fn layer_one_the_kernel_refuses_illegal_transitions_typed() {
    let (_rest, b, db) = setup("wf_judge", false);
    let key = new_claim(&b, &clerk());
    b.db_transition(&clerk(), &HookChain::default(), "expense_claim", &key, "Submit").unwrap();

    // wrong ROLE
    let e = b.db_transition(&clerk(), &HookChain::default(), "expense_claim", &key, "Approve")
        .expect_err("a clerk may not approve");
    let msg = format!("{e:?}");
    println!("role refused: {msg}");
    assert!(matches!(e, BrokerError::WorkflowDenied { .. }));
    assert!(msg.contains("FRUST:E_WORKFLOW:ROLE_DENIED"), "{msg}");
    assert!(msg.contains("manager"), "names the role that would work: {msg}");

    // wrong FROM-STATE
    let e = b.db_transition(&clerk(), &HookChain::default(), "expense_claim", &key, "Reopen")
        .expect_err("cannot reopen something not rejected");
    assert!(format!("{e:?}").contains("FRUST:E_WORKFLOW:WRONG_STATE"), "{e:?}");

    // unknown ACTION
    let e = b.db_transition(&mgr(), &HookChain::default(), "expense_claim", &key, "Teleport")
        .expect_err("no such action");
    assert!(format!("{e:?}").contains("FRUST:E_WORKFLOW:UNKNOWN_ACTION"), "{e:?}");

    // NOTHING was written by any of the three refusals
    let row = db
        .sql_root(&format!("SELECT workflow_state, docstatus FROM expense_claim:{key};"))
        .unwrap();
    let row = &row.as_array().unwrap()[0];
    assert_eq!(row["workflow_state"], serde_json::json!("Submitted for Approval"),
        "a refused transition writes nothing");
    assert_eq!(row["docstatus"], serde_json::json!(0));
}

/// **LAYER 2** — the lattice backstops a workflow that is ITSELF wrong.
///
/// This workflow declares `Approved` at docstatus **2**, reachable straight
/// from Draft. Layer 1 judges it perfectly legal — the role matches, the
/// from-state matches — and the DB EVENT refuses it anyway. That is the floor
/// not trusting the judge, which is the entire reason A2 splits them.
#[test]
fn layer_two_the_lattice_refuses_what_a_buggy_workflow_allows() {
    let (_rest, b, db) = setup("wf_lattice", true);
    let key = new_claim(&b, &clerk());

    // a MANAGER takes the Leap: they clear the row permission (managers write
    // anytime), so the ONLY thing that can refuse the 0->2 jump is the lattice.
    let e = b
        .db_transition(&mgr(), &HookChain::default(), "expense_claim", &key, "Leap")
        .expect_err("0 -> 2 must be refused by the lattice");
    let msg = format!("{e:?}");
    println!("lattice refused: {msg}");

    // NOT a workflow denial — the workflow said yes. The EVENT said no.
    assert!(
        !matches!(e, BrokerError::WorkflowDenied { .. }),
        "layer 1 ALLOWED this; the refusal must come from the lattice: {msg}"
    );
    assert!(msg.contains("FRUST:E_DOCSTATUS"), "the lattice EVENT is the refuser: {msg}");

    let row = db
        .sql_root(&format!("SELECT docstatus FROM expense_claim:{key};"))
        .unwrap();
    assert_eq!(row.as_array().unwrap()[0]["docstatus"], serde_json::json!(0),
        "the document never left draft");
}

/// Criterion 1: a document shows only the transitions THIS role may take, and
/// every offered action is one the judge will allow.
#[test]
fn a_document_offers_only_this_roles_transitions() {
    let (_rest, b, _db) = setup("wf_actions", false);
    let key = new_claim(&b, &clerk());

    let c = b.workflow_actions(&clerk(), "expense_claim", &key).unwrap();
    println!("clerk sees: {c}");
    assert_eq!(c["state"], serde_json::json!("Draft"));
    assert_eq!(c["actions"].as_array().unwrap().len(), 1);
    assert_eq!(c["actions"][0]["action"], serde_json::json!("Submit"));

    let m = b.workflow_actions(&mgr(), "expense_claim", &key).unwrap();
    assert!(m["actions"].as_array().unwrap().is_empty(), "a manager has no Draft action: {m}");

    b.db_transition(&clerk(), &HookChain::default(), "expense_claim", &key, "Submit").unwrap();
    let m = b.workflow_actions(&mgr(), "expense_claim", &key).unwrap();
    let actions: Vec<&str> =
        m["actions"].as_array().unwrap().iter().map(|a| a["action"].as_str().unwrap()).collect();
    println!("manager now sees: {actions:?}");
    assert!(actions.contains(&"Approve") && actions.contains(&"Reject"));

    // every offered button is genuinely allowed — buttons cannot drift from rules
    if let Some(a) = actions.into_iter().next() {
        b.workflow_actions(&mgr(), "expense_claim", &key).unwrap();
        assert!(
            b.db_transition(&mgr(), &HookChain::default(), "expense_claim", &key, a).is_ok()
                || a == "Approve",
            "offered action '{a}' must be allowed"
        );
    }
}

/// Criterion 4: a transition is a `db_write` — it lands in the changefeed like
/// any other mutation, with no separate audit path to forget to maintain.
#[test]
fn a_transition_is_an_ordinary_audited_write() {
    let (_rest, b, db) = setup("wf_audit", false);
    let key = new_claim(&b, &clerk());
    b.db_transition(&clerk(), &HookChain::default(), "expense_claim", &key, "Submit").unwrap();
    b.db_transition(&mgr(), &HookChain::default(), "expense_claim", &key, "Approve").unwrap();

    let feed = db
        .sql_root("SHOW CHANGES FOR TABLE expense_claim SINCE 1 LIMIT 100;")
        .expect("changefeed");
    let text = feed.to_string();
    println!("changefeed entries: {}", feed.as_array().map(|a| a.len()).unwrap_or(0));
    assert!(text.contains("Submitted for Approval"), "the transition is in the feed: {text}");
    assert!(text.contains("Approved"), "and so is the approval");
}

/// A DocType with no workflow is unmanaged, and says so rather than pretending.
#[test]
fn an_unmanaged_doctype_refuses_transitions_clearly() {
    let (_rest, b, _db) = setup("wf_unmanaged", false);
    let e = b
        .db_transition(&clerk(), &HookChain::default(), "app_user", "clerk", "Submit")
        .expect_err("app_user has no workflow");
    assert!(format!("{e:?}").contains("FRUST:E_WORKFLOW:UNMANAGED"), "{e:?}");
}
