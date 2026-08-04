//! The workflow engine (REQ-4.1.2).
//!
//! Multi-step, role-gated approval flows as **runtime metadata**. This module
//! is the *judge* half of the two-layer approval design:
//!
//! > EVENTs enforce the docstatus lattice; workflow transition rules are
//! > kernel logic evaluated **before** the kernel attempts the transition.
//!
//! Two layers, deliberately separate and separately provable:
//!
//! 1. **The kernel judges.** A wrong role, an unknown action, or an action
//!    taken from the wrong state fails typed (`FRUST:E_WORKFLOW:*`) here,
//!    before any write is attempted.
//! 2. **The lattice backstops.** If a workflow is *itself* wrong — declaring a
//!    state at docstatus 2 reachable from docstatus 0 — the DB EVENT throws
//!    `FRUST:E_DOCSTATUS:*` anyway. The floor does not trust the judge.
//!
//! That the second layer catches the first layer's bugs is the whole point of
//! the split, and why a buggy workflow is a rejected write rather than a
//! corrupted document.
//!
//! **Workflow rules never enter EVENT bodies.** The rule against
//! "Server Scripts with extra steps" — stands guard: the EVENT knows only the
//! docstatus lattice, which is fixed and kernel-owned. Everything role-shaped
//! or state-shaped is evaluated here, in Rust, where it can be read.
//!
//! Like `app.rs`, this module holds **no query text**; loading is `sync.rs`'s
//! job (`surql_monopoly` covers both).

use serde::{Deserialize, Serialize};

use crate::contract::BrokerError;

/// The field a workflow keeps its current state in.
pub const STATE_FIELD: &str = "workflow_state";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkflowDef {
    pub name: String,
    /// The DocType this workflow governs.
    pub doctype: String,
    pub states: Vec<StateDef>,
    pub transitions: Vec<TransitionDef>,
    /// State-scoped field behaviour. Compiled into the Desk's
    /// declarative rule shape — a workflow state imposing
    /// read-only is Tier-1 dynamics, not a new mechanism.
    #[serde(default)]
    pub state_rules: Vec<StateRule>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StateDef {
    pub name: String,
    /// The lattice position this state corresponds to (0 draft, 1 submitted,
    /// 2 cancelled). The workflow *proposes*; the EVENT disposes.
    #[serde(default)]
    pub docstatus: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TransitionDef {
    pub from: String,
    pub to: String,
    /// The single role permitted to take this transition. One role per
    /// transition keeps the judgement readable; a state needing two roles
    /// declares two transitions.
    pub role: String,
    /// The button label — what the user is actually doing.
    pub action: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StateRule {
    pub state: String,
    pub field: String,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub required: bool,
}

/// A judged transition: what the kernel will write if it proceeds.
#[derive(Debug, Clone)]
pub struct Judged {
    pub from: String,
    pub to: String,
    pub action: String,
    pub docstatus: i64,
}

impl WorkflowDef {
    pub fn initial_state(&self) -> &str {
        self.states.first().map(|s| s.name.as_str()).unwrap_or("Draft")
    }

    fn state(&self, name: &str) -> Option<&StateDef> {
        self.states.iter().find(|s| s.name == name)
    }

    /// The state to judge from, given a document's stored `workflow_state`.
    ///
    /// An absent **or empty** value means the document has not yet entered the
    /// workflow, so it is judged from the initial state. Every doc created
    /// through the Desk carries an empty `workflow_state`, so without this a
    /// fresh document showed no transition buttons at all.
    pub fn state_or_initial<'a>(&'a self, stored: Option<&'a str>) -> &'a str {
        match stored {
            Some(s) if !s.is_empty() => s,
            _ => self.initial_state(),
        }
    }

    /// The transitions THIS role may take from THIS state.
    ///
    /// Used for the Desk's buttons, and deliberately the same data the judge
    /// uses — a button that renders is a transition that will be allowed, and
    /// a transition that is allowed renders a button. One source, so the UI
    /// cannot drift from the rule.
    pub fn available(&self, state: &str, role: &str) -> Vec<&TransitionDef> {
        self.transitions
            .iter()
            .filter(|t| t.from == state && t.role == role)
            .collect()
    }

    /// **The judgement.** Called before any write is attempted.
    ///
    /// Order matters for the message quality: an unknown action is a different
    /// mistake from a wrong state, which is a different mistake from a wrong
    /// role, and an operator deserves to be told which.
    pub fn judge(&self, current: &str, action: &str, role: &str) -> Result<Judged, BrokerError> {
        if self.state(current).is_none() {
            return Err(BrokerError::WorkflowDenied {
                code: "FRUST:E_WORKFLOW:UNKNOWN_STATE".into(),
                detail: format!("'{current}' is not a state of workflow '{}'", self.name),
            });
        }

        let by_action: Vec<&TransitionDef> =
            self.transitions.iter().filter(|t| t.action == action).collect();
        if by_action.is_empty() {
            return Err(BrokerError::WorkflowDenied {
                code: "FRUST:E_WORKFLOW:UNKNOWN_ACTION".into(),
                detail: format!("workflow '{}' has no action '{action}'", self.name),
            });
        }

        let from_here: Vec<&&TransitionDef> =
            by_action.iter().filter(|t| t.from == current).collect();
        if from_here.is_empty() {
            return Err(BrokerError::WorkflowDenied {
                code: "FRUST:E_WORKFLOW:WRONG_STATE".into(),
                detail: format!("'{action}' is not available from '{current}'"),
            });
        }

        let Some(t) = from_here.iter().find(|t| t.role == role) else {
            // Name the role that WOULD work: the user cannot fix a refusal
            // they cannot understand, and this leaks nothing they could not
            // read off the workflow metadata anyway.
            let needs: Vec<&str> = from_here.iter().map(|t| t.role.as_str()).collect();
            return Err(BrokerError::WorkflowDenied {
                code: "FRUST:E_WORKFLOW:ROLE_DENIED".into(),
                detail: format!(
                    "'{action}' from '{current}' requires role {}; you are '{role}'",
                    needs.join(" or ")
                ),
            });
        };

        let Some(to) = self.state(&t.to) else {
            return Err(BrokerError::WorkflowDenied {
                code: "FRUST:E_WORKFLOW:UNKNOWN_STATE".into(),
                detail: format!("transition '{action}' targets undeclared state '{}'", t.to),
            });
        };

        Ok(Judged {
            from: current.to_string(),
            to: t.to.clone(),
            action: action.to_string(),
            docstatus: to.docstatus,
        })
    }

    /// Compiles `state_rules` into the Desk's declarative rule shape, keyed by
    /// fieldname.
    ///
    /// The source field is always `workflow_state`, so the Desk renders these
    /// with the machinery it already has: per-field signals, render-time
    /// operator match, **zero round-trips**. A workflow state imposing
    /// read-only is Tier-1 dynamics wearing a workflow's name — not a second
    /// rendering path that would then need its own bugs fixed.
    pub fn field_rules(&self, state_of_interest: Option<&str>) -> Vec<(String, serde_json::Value)> {
        let mut out = Vec::new();
        for r in &self.state_rules {
            if let Some(s) = state_of_interest {
                if r.state != s {
                    continue;
                }
            }
            let rule = serde_json::json!({
                "field": STATE_FIELD,
                "op": "eq",
                "value": r.state,
                "message": format!("Not editable while {}", r.state),
            });
            if r.read_only {
                out.push((r.field.clone(), serde_json::json!({ "read_only_when": rule })));
            }
            if r.required {
                out.push((r.field.clone(), serde_json::json!({ "required_when": rule })));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wf() -> WorkflowDef {
        serde_json::from_value(serde_json::json!({
            "name": "expense_approval",
            "doctype": "expense_claim",
            "states": [
                { "name": "Draft", "docstatus": 0 },
                { "name": "Submitted for Approval", "docstatus": 0 },
                { "name": "Approved", "docstatus": 1 },
                { "name": "Rejected", "docstatus": 0 }
            ],
            "transitions": [
                { "from": "Draft", "to": "Submitted for Approval", "role": "clerk", "action": "Submit" },
                { "from": "Submitted for Approval", "to": "Approved", "role": "manager", "action": "Approve" },
                { "from": "Submitted for Approval", "to": "Rejected", "role": "manager", "action": "Reject" },
                { "from": "Rejected", "to": "Draft", "role": "clerk", "action": "Reopen" }
            ],
            "state_rules": [
                { "state": "Submitted for Approval", "field": "amount", "read_only": true }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn the_happy_path_is_judged_allowed() {
        let j = wf().judge("Draft", "Submit", "clerk").expect("clerk may submit");
        assert_eq!(j.to, "Submitted for Approval");
        assert_eq!(j.docstatus, 0, "submitting for approval is not yet docstatus 1");

        let j = wf().judge("Submitted for Approval", "Approve", "manager").expect("manager approves");
        assert_eq!(j.docstatus, 1, "approval is the lattice submit");
    }

    /// Each refusal names ITS OWN mistake — the operator is told which of the
    /// three things was wrong, not merely that something was.
    #[test]
    fn each_refusal_is_typed_and_specific() {
        let e = wf().judge("Draft", "Approve", "manager").unwrap_err();
        assert!(format!("{e:?}").contains("WRONG_STATE"), "{e:?}");

        let e = wf().judge("Submitted for Approval", "Approve", "clerk").unwrap_err();
        let msg = format!("{e:?}");
        assert!(msg.contains("ROLE_DENIED"), "{msg}");
        assert!(msg.contains("manager"), "it names the role that would work: {msg}");

        let e = wf().judge("Draft", "Teleport", "manager").unwrap_err();
        assert!(format!("{e:?}").contains("UNKNOWN_ACTION"), "{e:?}");

        let e = wf().judge("Nowhere", "Submit", "clerk").unwrap_err();
        assert!(format!("{e:?}").contains("UNKNOWN_STATE"), "{e:?}");
    }

    /// The buttons a user sees are computed from the same data the judge uses.
    #[test]
    fn available_transitions_match_what_the_judge_would_allow() {
        let w = wf();
        let clerk = w.available("Draft", "clerk");
        assert_eq!(clerk.len(), 1);
        assert_eq!(clerk[0].action, "Submit");
        assert!(w.available("Draft", "manager").is_empty(), "a manager has no Draft action");

        let mgr = w.available("Submitted for Approval", "manager");
        assert_eq!(mgr.len(), 2, "Approve and Reject");
        // and every offered button is genuinely allowed
        for t in mgr {
            assert!(w.judge("Submitted for Approval", &t.action, "manager").is_ok());
        }
    }

    /// A document with an absent OR empty `workflow_state` is judged
    /// from the initial state — a fresh Desk-created doc enters the workflow
    /// and shows its initial transitions, rather than resolving to an empty
    /// state that matches no transition.
    #[test]
    fn empty_or_missing_state_resolves_to_initial() {
        let w = wf();
        assert_eq!(w.state_or_initial(None), "Draft", "missing → initial");
        assert_eq!(w.state_or_initial(Some("")), "Draft", "empty → initial (the Desk-created case)");
        assert_eq!(w.state_or_initial(Some("Approved")), "Approved", "a real state is kept");
        // and the resolved initial state genuinely offers the clerk's first move
        assert_eq!(w.available(w.state_or_initial(Some("")), "clerk")[0].action, "Submit");
    }

    /// State rules compile to the Desk's rule shape — no new rendering mechanism.
    #[test]
    fn state_rules_compile_into_wo014_rule_shape() {
        let rules = wf().field_rules(None);
        assert_eq!(rules.len(), 1);
        let (field, rule) = &rules[0];
        assert_eq!(field, "amount");
        assert_eq!(rule["read_only_when"]["field"], serde_json::json!(STATE_FIELD));
        assert_eq!(rule["read_only_when"]["op"], serde_json::json!("eq"));
        assert_eq!(rule["read_only_when"]["value"], serde_json::json!("Submitted for Approval"));
    }
}
