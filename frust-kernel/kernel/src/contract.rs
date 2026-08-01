//! ADR-006 contract types: structured values, filters, and paths.
//! Never a raw query string, anywhere, including the edges.
//!
//! Evolution policy (ADR-006 edge 1): additive variants only. Do not remove
//! or repurpose a variant; removals require a contract major.

use serde::{Deserialize, Serialize};

/// The dynamic payload type (ADR-006 edge 3): typed envelope, dynamic fields.
/// `decimal` is first-class — money never crosses the boundary as a float.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Decimal as a validated numeric string; rendered with SurrealQL's
    /// `dec` suffix so the DB stores a true decimal.
    Decimal(String),
    Text(String),
    /// RFC3339, rendered as a `d'...'` literal after validation.
    Datetime(String),
    /// SurrealQL duration string, e.g. `5m30s`.
    Duration(String),
    /// Full record id `table:key`, rendered via `type::record(...)`.
    RecordId(String),
    List(Vec<Value>),
    Object(Vec<(String, Value)>),
}

/// One field path step (ADR-006 edge 2). Depth-capped at compile time by the
/// broker (default 3). Deep/recursive traversal is deliberately NOT here —
/// that's named queries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PathSegment {
    Field(String),
    LinkHop(String),
    Edge { direction: EdgeDirection, edge_type: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeDirection {
    Out,
    In,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CmpOp {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    Inside,
    Contains,
}

impl CmpOp {
    pub fn is_range(self) -> bool {
        matches!(self, CmpOp::Gt | CmpOp::Gte | CmpOp::Lt | CmpOp::Lte)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Filter {
    And(Vec<Filter>),
    Or(Vec<Filter>),
    Not(Box<Filter>),
    Cmp { path: Vec<PathSegment>, op: CmpOp, value: Value },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortDir {
    Asc,
    Desc,
}

/// Read options: pagination + ordering. No raw clauses.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReadOpts {
    pub order_by: Option<(Vec<PathSegment>, SortDir)>,
    pub limit: Option<u64>,
    pub start: Option<u64>,
}

/// db-aggregate (ADR-006 edge 5): closed metric set. No expressions, no
/// having, no nesting.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Metric {
    Count,
    Sum,
    Min,
    Max,
    Avg,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WriteOp {
    Create,
    Update,
}

/// Hook classes for the cycle trap (ADR-006 edge 6) — and, since WO-053,
/// REQ-2.2.1's lifecycle vocabulary itself.
///
/// ADR-006's cycle rule keys on `(record-id, hook-class)`, plural in the
/// ratified text, so the trap extends per class for free: a `before_insert`
/// that provokes a `validate` on the same record is not a cycle, and the same
/// class twice on the same record still is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookClass {
    /// Before a CREATE is written. May mutate.
    BeforeInsert,
    /// Before any write is written. May mutate. The one wired before WO-053.
    Validate,
    /// The docstatus 0→1 edge, BEFORE the write commits. May reject, not mutate.
    OnSubmit,
    /// The docstatus →2 edge. May reject, not mutate.
    OnCancel,
    OnWrite,
    /// Declared since WO-005 and never wired to a script or plugin. WO-007's
    /// scheduled work is real, but it runs KERNEL handlers (`Contrib`
    /// implementors driven by `RollupWorker`), not guest code — see the WO-053
    /// census. Kept because the cycle trap's key space is the honest place to
    /// record that the class exists.
    Scheduled,
}

impl HookClass {
    /// The wire name — what a manifest writes and an operator reads. Shared
    /// with the notification vocabulary (WO-043) on purpose: one set of event
    /// names across the kernel, so `on_submit` means the same thing to a mail
    /// rule and to a script.
    pub fn wire(self) -> &'static str {
        match self {
            HookClass::BeforeInsert => "before_insert",
            HookClass::Validate => "validate",
            HookClass::OnSubmit => "on_submit",
            HookClass::OnCancel => "on_cancel",
            HookClass::OnWrite => "on_write",
            HookClass::Scheduled => "scheduled",
        }
    }

    /// The classes a manifest may subscribe to today. **This list is the
    /// door** (WO-019's rule): a hook point that is not here is refused at
    /// install with a 400, never accepted and silently never fired.
    pub const SUBSCRIBABLE: [HookClass; 4] =
        [HookClass::BeforeInsert, HookClass::Validate, HookClass::OnSubmit, HookClass::OnCancel];

    pub fn from_wire(s: &str) -> Option<Self> {
        Self::SUBSCRIBABLE.into_iter().find(|c| c.wire() == s)
    }

    /// May a hook of this class change the document it is shown?
    ///
    /// Stated, not inherited (WO-053 criterion 3). The edge classes fire on a
    /// docstatus move, where ADR-009's lattice owns the value — they may
    /// refuse the transition, they may not rewrite it.
    pub fn may_mutate(self) -> bool {
        matches!(self, HookClass::BeforeInsert | HookClass::Validate)
    }
}

/// Typed contract errors — what a consumer (Desk, REST, plugin) sees.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum BrokerError {
    UnknownDoctype { name: String },
    FieldNotReadable { field: String },
    PathTooDeep { max: usize },
    InvalidValue { detail: String },
    HookCycle { record: String, hook: String },
    HookDepthExceeded { max: usize },
    HookRejected { stage: String, message: String },
    PermissionDenied { detail: String },
    /// WO-008: a record-user write whose identity stamp resolved NULL — the
    /// $auth sharp edge caught loud (E_IDENTITY_UNRESOLVED), never a silent
    /// NULL owner.
    IdentityUnresolved,
    /// WO-013 (P-8.2): the tenant's door budget is spent. Loud by design —
    /// a shaped tenant can see that it is being shaped, and the hint says
    /// when to come back. Never a silent slow.
    TenantThrottled { retry_after_ms: u64 },
    /// WO-018: the kernel refused a workflow transition BEFORE attempting any
    /// state or docstatus write (ADR-009 A2: workflow rules are kernel logic,
    /// evaluated before the transition). `code` is a stable
    /// `FRUST:E_WORKFLOW:*` machine code; the lattice EVENT is a separate,
    /// lower floor that still fires if this judgement is ever wrong.
    WorkflowDenied { code: String, detail: String },
    /// Optimistic-concurrency retry budget exhausted (E_WRITE_CONFLICT_EXHAUSTED).
    /// The raw DB wording never reaches callers (ADR-007 hygiene rule).
    WriteConflictExhausted { attempts: u32 },
    Db { detail: String },
}

impl std::fmt::Display for BrokerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for BrokerError {}
