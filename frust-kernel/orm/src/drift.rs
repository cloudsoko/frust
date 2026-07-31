//! Live-schema drift reconciliation.
//!
//! Reads the live schema with `INFO FOR DB` (for the table definition) and
//! `INFO FOR TABLE` (fields + indexes), reconstructs a [`SchemaSnapshot`], and
//! compares **object presence** against the snapshot the engine last recorded.
//! This surfaces out-of-band DDL: objects added or removed outside the migrator.
//!
//! Scope: presence only (added/removed tables, fields, indexes). *Definitional*
//! drift — a field whose type was altered by hand — is intentionally not
//! flagged, because SurrealDB canonicalises the `DEFINE` echo (adds
//! `PERMISSIONS`, normalises whitespace/case), so a textual compare against the
//! engine's emitted form would be all false-positives. That needs snapshot
//! normalisation and is tracked separately.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::resource::{exec, Conn, EngineCtx, StorageLocation};
use crate::schema::SchemaSnapshot;
use crate::{load_history, ResourceMigrator};

/// Aggregate drift for one scope.
#[derive(Debug, Clone, Serialize, Default)]
pub struct DriftReport {
    pub scope: String,
    pub namespace: String,
    pub database: String,
    /// Resources whose live schema differs from the recorded snapshot.
    pub drifted: Vec<ResourceDrift>,
}

impl DriftReport {
    /// True when no resource has drifted.
    pub fn is_clean(&self) -> bool {
        self.drifted.is_empty()
    }
}

/// Per-resource structural drift between the live DB and its recorded snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct ResourceDrift {
    pub name: String,
    pub table: String,
    /// Object keys (`kind:name`) present live but absent from the snapshot —
    /// added out of band.
    pub unexpected: Vec<String>,
    /// Object keys present in the snapshot but missing live — never applied, or
    /// removed out of band.
    pub missing: Vec<String>,
}

impl ResourceMigrator {
    /// Detect structural drift in the platform scope.
    pub fn detect_drift_platform(&self, ctx: &EngineCtx) -> anyhow::Result<DriftReport> {
        let loc = ctx.tenancy.platform_scope();
        detect_drift_at(ctx, loc, "platform".to_string())
    }

    /// Detect structural drift in a tenant's scope.
    pub fn detect_drift_tenant(
        &self,
        ctx: &EngineCtx,
        tenant_id: &str,
    ) -> anyhow::Result<DriftReport> {
        let loc = ctx.tenancy.locate(tenant_id);
        detect_drift_at(ctx, loc, format!("tenant:{tenant_id}"))
    }
}

fn detect_drift_at(
    ctx: &EngineCtx,
    loc: StorageLocation,
    scope: String,
) -> anyhow::Result<DriftReport> {
    let conn_box = ctx.conns.acquire(&loc)?;
    let drifted = detect_scope_drift(conn_box.as_ref())?;
    Ok(DriftReport {
        scope,
        namespace: loc.namespace,
        database: loc.database,
        drifted,
    })
}

/// Core drift detection against an already-acquired connection. Read-only.
pub(crate) fn detect_scope_drift(db: &dyn Conn) -> anyhow::Result<Vec<ResourceDrift>> {
    let history = load_history(db)?;
    let mut out = Vec::new();

    for (name, (_version, snapshot)) in history {
        if snapshot.table.is_empty() {
            continue;
        }
        let table = snapshot.table.clone();
        let live = live_snapshot(db, &table)?;

        let stored: BTreeSet<&String> = snapshot.objects.keys().collect();
        let present: BTreeSet<&String> = live.objects.keys().collect();
        let unexpected: Vec<String> = present.difference(&stored).map(|k| (*k).clone()).collect();
        let missing: Vec<String> = stored.difference(&present).map(|k| (*k).clone()).collect();

        if !unexpected.is_empty() || !missing.is_empty() {
            out.push(ResourceDrift { name, table, unexpected, missing });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Reconstruct a [`SchemaSnapshot`] from the live DB via `INFO` statements.
fn live_snapshot(db: &dyn Conn, table: &str) -> anyhow::Result<SchemaSnapshot> {
    let mut sql = String::new();

    // Table definition: INFO FOR DB -> { tables: { <name>: "DEFINE TABLE ..." } }.
    let db_info: Option<Value> =
        Some(exec(db, "INFO FOR DB;").map_err(|e| anyhow::anyhow!("INFO FOR DB: {e}"))?);
    if let Some(def) = db_info
        .as_ref()
        .and_then(|v| v.get("tables"))
        .and_then(|t| t.get(table))
        .and_then(|d| d.as_str())
    {
        push_stmt(&mut sql, def);
    }

    // Fields + indexes: INFO FOR TABLE → { fields: {...}, indexes: {...}, ... }.
    let tbl_info: Option<Value> = Some(
        exec(db, &format!("INFO FOR TABLE {table};"))
            .map_err(|e| anyhow::anyhow!("INFO FOR TABLE {table}: {e}"))?,
    );
    if let Some(info) = tbl_info.as_ref() {
        for section in ["fields", "indexes", "events"] {
            if let Some(map) = info.get(section).and_then(|s| s.as_object()) {
                for def in map.values().filter_map(|d| d.as_str()) {
                    push_stmt(&mut sql, def);
                }
            }
        }
    }

    Ok(SchemaSnapshot::from_sql(table, &sql))
}

fn push_stmt(buf: &mut String, stmt: &str) {
    buf.push_str(stmt);
    buf.push_str(";\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{history_table_schema, schema, Direction, SchemaSnapshot};

    use crate::testkit::mem_db;
    use crate::resource::exec;

    const NOTE_SQL: &str = "\
DEFINE TABLE OVERWRITE note SCHEMAFULL;
DEFINE FIELD OVERWRITE title ON TABLE note TYPE string;
";

    /// Apply the recorded snapshot, then confirm a matching live schema is clean.
    #[test]
    fn matching_live_schema_has_no_drift() {
        let db = mem_db();
        exec(&db, &history_table_schema()).unwrap();

        let snap = SchemaSnapshot::from_sql("note", NOTE_SQL);
        let d = schema::diff(&SchemaSnapshot::default(), &snap);
        crate::apply_resource(&db, "crm__note", &snap, 1, &d.statements("note"), Direction::Up)
            .unwrap();

        let drift = detect_scope_drift(&db).unwrap();
        assert!(drift.is_empty(), "live schema matches snapshot: {drift:?}");
    }

    /// A field defined out of band shows up as `unexpected`.
    #[test]
    fn out_of_band_field_is_flagged_unexpected() {
        let db = mem_db();
        exec(&db, &history_table_schema()).unwrap();

        let snap = SchemaSnapshot::from_sql("note", NOTE_SQL);
        let d = schema::diff(&SchemaSnapshot::default(), &snap);
        crate::apply_resource(&db, "crm__note", &snap, 1, &d.statements("note"), Direction::Up)
            .unwrap();

        // Someone runs DDL directly, bypassing the migrator.
        exec(&db, "DEFINE FIELD OVERWRITE sneaky ON TABLE note TYPE string;").unwrap();

        let drift = detect_scope_drift(&db).unwrap();
        assert_eq!(drift.len(), 1);
        assert_eq!(drift[0].name, "crm__note");
        assert!(
            drift[0].unexpected.iter().any(|k| k == "field:sneaky"),
            "unexpected: {:?}",
            drift[0].unexpected
        );
        assert!(drift[0].missing.is_empty());
    }

    /// A field dropped out of band shows up as `missing`.
    #[test]
    fn out_of_band_drop_is_flagged_missing() {
        let db = mem_db();
        exec(&db, &history_table_schema()).unwrap();

        let snap = SchemaSnapshot::from_sql(
            "note",
            "DEFINE TABLE OVERWRITE note SCHEMAFULL;\nDEFINE FIELD OVERWRITE title ON TABLE note TYPE string;\nDEFINE FIELD OVERWRITE body ON TABLE note TYPE string;",
        );
        let d = schema::diff(&SchemaSnapshot::default(), &snap);
        crate::apply_resource(&db, "crm__note", &snap, 1, &d.statements("note"), Direction::Up)
            .unwrap();

        exec(&db, "REMOVE FIELD body ON TABLE note;").unwrap();

        let drift = detect_scope_drift(&db).unwrap();
        assert_eq!(drift.len(), 1);
        assert!(
            drift[0].missing.iter().any(|k| k == "field:body"),
            "missing: {:?}",
            drift[0].missing
        );
    }
}
