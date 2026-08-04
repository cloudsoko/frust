//! The six-import interface. What `framework-core` used to
//! provide is defined here and fed by the kernel from RUNTIME DocType
//! metadata. Everything below this interface — diff, classify, gate, apply,
//! revert, drift, fleet — is source-agnostic and unchanged from the reviewed
//! adapter.
//!
//! Old import -> replacement:
//! - `registered_resources()` / `ResourceRegistration` -> `ResourceSpec`
//!   slices passed as arguments (runtime metadata, not compile-time registry)
//! - `FieldKind::Record` dep scan -> `ResourceSpec::deps`, precomputed by the
//!   caller from DocType link fields
//! - `AppContext` -> [`EngineCtx`] (connection factory + tenancy + bootstrap)
//! - `StorageLocation` -> defined here
//! - `ANALYZER_SQL` -> `EngineCtx::bootstrap_sql`

/// A resource's desired state, built at runtime from DocType metadata.
#[derive(Debug, Clone)]
pub struct ResourceSpec {
    /// Owning app; `"platform"` scopes to the platform namespace.
    pub app: String,
    /// Table name.
    pub name: String,
    /// The desired schema: a block of `DEFINE` statements.
    pub schema: String,
    /// Record-link targets for dependency ordering (toposort).
    pub deps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageLocation {
    pub namespace: String,
    pub database: String,
}

/// One statement result off the wire.
#[derive(Debug, Clone)]
pub struct StmtResult {
    pub ok: bool,
    pub result: serde_json::Value,
}

/// The transport seam: whatever the kernel speaks (HTTP `/sql` today).
/// Statement errors come back as data — the engine decides what is fatal.
pub trait Conn {
    fn query(&self, sql: &str) -> anyhow::Result<Vec<StmtResult>>;
}

/// All-or-error execution: any failed statement is an error; returns the
/// last statement's result.
pub fn exec(conn: &dyn Conn, sql: &str) -> anyhow::Result<serde_json::Value> {
    let stmts = conn.query(sql)?;
    for s in &stmts {
        if !s.ok {
            anyhow::bail!("statement failed: {}", s.result);
        }
    }
    Ok(stmts.last().map(|s| s.result.clone()).unwrap_or(serde_json::Value::Null))
}

pub trait ConnFactory: Sync {
    fn acquire(&self, loc: &StorageLocation) -> anyhow::Result<Box<dyn Conn>>;
}

/// Tenancy strategy: pluggable; the kernel's v0 is single-database.
pub trait Tenancy: Sync {
    fn platform_scope(&self) -> StorageLocation;
    fn locate(&self, tenant_id: &str) -> StorageLocation;
    fn requires_per_tenant_schema_deploy(&self) -> bool;
    fn strategy_name(&self) -> &'static str;
}

/// Replaces `AppContext` for the engine's purposes.
pub struct EngineCtx<'a> {
    pub conns: &'a dyn ConnFactory,
    pub tenancy: &'a dyn Tenancy,
    /// Framework bootstrap DDL applied before migration (was `ANALYZER_SQL`).
    pub bootstrap_sql: Option<String>,
}

/// Escape a string for a single-quoted SurrealQL literal (the engine embeds
/// its own bookkeeping values; user schema text passes through verbatim).
pub fn escape_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Parse a SurrealQL duration string (e.g. `1m30s`) into seconds.
pub fn duration_secs(s: &str) -> f64 {
    let mut total = 0.0;
    let mut num = String::new();
    let mut unit = String::new();
    let flush = |num: &mut String, unit: &mut String, total: &mut f64| {
        if let Ok(v) = num.parse::<f64>() {
            *total += v
                * match unit.as_str() {
                    "ns" => 1e-9,
                    "us" | "µs" => 1e-6,
                    "ms" => 1e-3,
                    "s" | "" => 1.0,
                    "m" => 60.0,
                    "h" => 3600.0,
                    "d" => 86400.0,
                    "w" => 604800.0,
                    _ => 0.0,
                };
        }
        num.clear();
        unit.clear();
    };
    for c in s.chars() {
        if c.is_ascii_digit() || c == '.' {
            if !unit.is_empty() {
                flush(&mut num, &mut unit, &mut total);
            }
            num.push(c);
        } else {
            unit.push(c);
        }
    }
    flush(&mut num, &mut unit, &mut total);
    total
}
