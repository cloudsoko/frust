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

/// Hook classes for the cycle trap (ADR-006 edge 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookClass {
    Validate,
    OnWrite,
    Scheduled,
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
