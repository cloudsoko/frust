//! Diff-level field-change classification (migration-gap Phase 2a).
//!
//! Turns two [`SchemaSnapshot`]s into a [`FieldChangeReport`] — the **observable**
//! surface of the Phase-1 field classifier ([`crate::field`]). It is deliberately
//! *non-authoritative*: nothing here feeds `is_ok()`, the apply-refusal gate, or
//! `SchemaDiff::{destructive,warnings,statements}`. It only *describes*.
//!
//! ## Why snapshots, not `SchemaDiff`
//!
//! [`crate::schema::SchemaDiff`] is lossy for alters — it keeps only the *new*
//! `define_sql` and discards the old, and a dropped object carries just `{kind,
//! name}`. Classifying a transition (e.g. `string → int`) needs *both* sides, so the
//! classifier consumes the two snapshots directly. A welcome consequence: this module
//! never touches `schema.rs`, so the four frozen methods are untouched by
//! construction.

use serde::Serialize;

use crate::field::{
    classify_field_change, FieldChange, FieldChangeClassification, FieldClass, FieldSnapshot,
};
use crate::schema::{ObjectKind, SchemaObject, SchemaSnapshot};

/// One classified field-level change inside a resource's migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldChangeEntry {
    pub field: String,
    pub change: FieldChange,
    pub class: FieldClass,
    /// Rendered old type incl. nullability (`option<string>`); `None` for an add.
    pub from_type: Option<String>,
    /// Rendered new type incl. nullability; `None` for a drop.
    pub to_type: Option<String>,
    pub reason: String,
}

/// Per-resource tally of field-change severities.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ClassSummary {
    pub safe: usize,
    pub warn: usize,
    pub needs_backfill: usize,
    pub blocked: usize,
}

impl ClassSummary {
    pub fn total(&self) -> usize {
        self.safe + self.warn + self.needs_backfill + self.blocked
    }

    /// Highest severity present. **This is the Phase-3 read-point** — the single value
    /// a future enforcement gate would consult. Nothing calls it in Phase 2a.
    pub fn worst(&self) -> Option<FieldClass> {
        if self.blocked > 0 {
            Some(FieldClass::Blocked)
        } else if self.needs_backfill > 0 {
            Some(FieldClass::NeedsBackfill)
        } else if self.warn > 0 {
            Some(FieldClass::Warn)
        } else if self.safe > 0 {
            Some(FieldClass::Safe)
        } else {
            None
        }
    }

    fn tally(&mut self, class: FieldClass) {
        match class {
            FieldClass::Safe => self.safe += 1,
            FieldClass::Warn => self.warn += 1,
            FieldClass::NeedsBackfill => self.needs_backfill += 1,
            FieldClass::Blocked => self.blocked += 1,
        }
    }
}

/// All field-level classifications for one resource's diff, plus a summary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FieldChangeReport {
    pub entries: Vec<FieldChangeEntry>,
    pub summary: ClassSummary,
}

impl FieldChangeReport {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// A severity tally that **excludes dropped fields**. Drops remain governed solely
    /// by the destructive / `allow_destructive` gate; the Phase-3 policy gate operates
    /// on this non-drop summary so it never double-gates a drop that
    /// `allow_destructive` already permits.
    pub fn non_drop_summary(&self) -> ClassSummary {
        let mut s = ClassSummary::default();
        for e in &self.entries {
            if e.change == FieldChange::Dropped {
                continue;
            }
            s.tally(e.class);
        }
        s
    }

    fn push(
        &mut self,
        field: &str,
        c: FieldChangeClassification,
        from: Option<&FieldSnapshot>,
        to: Option<&FieldSnapshot>,
    ) {
        self.summary.tally(c.class);
        self.entries.push(FieldChangeEntry {
            field: field.to_string(),
            change: c.change,
            class: c.class,
            from_type: from.map(render_field_type),
            to_type: to.map(render_field_type),
            reason: c.reason,
        });
    }

    /// A field object that exists but won't parse — conservatively Blocked.
    fn push_conservative(&mut self, field: &str, had_from: bool, had_to: bool) {
        self.summary.tally(FieldClass::Blocked);
        self.entries.push(FieldChangeEntry {
            field: field.to_string(),
            change: FieldChange::Conservative,
            class: FieldClass::Blocked,
            from_type: had_from.then(|| "?".to_string()),
            to_type: had_to.then(|| "?".to_string()),
            reason: "unparseable field definition — manual review".to_string(),
        });
    }
}

/// Render a field's type with its nullability wrapper (`option<string>` / `string`).
fn render_field_type(fs: &FieldSnapshot) -> String {
    if fs.optional {
        format!("option<{}>", fs.ty.render())
    } else {
        fs.ty.render()
    }
}

fn parse_field(obj: &SchemaObject) -> Option<FieldSnapshot> {
    FieldSnapshot::parse(&obj.define_sql)
}

/// Field-level classification of `old → new`. Infallible (never panics). Considers
/// **FIELD** objects only — tables and indexes are not field changes and remain the
/// concern of the existing creates / `warnings()`. Unchanged fields produce no entry.
pub fn classify_diff(old: &SchemaSnapshot, new: &SchemaSnapshot) -> FieldChangeReport {
    let mut report = FieldChangeReport::default();

    // The table is being *created* in this migration if the prior snapshot has no
    // table object. Field additions to a brand-new table have no existing rows to
    // violate, so even a required field is Safe (no backfill needed).
    let table_is_new = !old.objects.values().any(|o| o.kind == ObjectKind::Table);

    // Added + altered: walk the new fields.
    for (key, obj) in &new.objects {
        if obj.kind != ObjectKind::Field {
            continue;
        }
        match old.objects.get(key) {
            None => match parse_field(obj) {
                Some(n) => {
                    let mut c = classify_field_change(None, Some(&n));
                    // New table → no existing rows → a required add needs no backfill.
                    // (Optional adds are already Safe; existing-table adds are untouched.)
                    if table_is_new && c.class == FieldClass::NeedsBackfill {
                        c.class = FieldClass::Safe;
                        c.reason = "added to a newly-created table (no existing rows)".to_string();
                    }
                    report.push(&obj.name, c, None, Some(&n));
                }
                None => report.push_conservative(&obj.name, false, true),
            },
            Some(prev) if prev.define_sql != obj.define_sql => {
                match (parse_field(prev), parse_field(obj)) {
                    (Some(o), Some(n)) => {
                        report.push(&obj.name, classify_field_change(Some(&o), Some(&n)), Some(&o), Some(&n))
                    }
                    _ => report.push_conservative(&obj.name, true, true),
                }
            }
            Some(_) => {} // unchanged — no entry
        }
    }

    // Dropped: old fields absent in new.
    for (key, obj) in &old.objects {
        if obj.kind != ObjectKind::Field {
            continue;
        }
        if !new.objects.contains_key(key) {
            match parse_field(obj) {
                Some(o) => report.push(&obj.name, classify_field_change(Some(&o), None), Some(&o), None),
                None => report.push_conservative(&obj.name, true, false),
            }
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{self, SchemaSnapshot};

    /// Build a snapshot from a table + `(field, type-clause)` pairs (+ optional extra
    /// DDL like an index), mirroring what the derive emits.
    fn snap(table: &str, fields: &[(&str, &str)], extra: &[&str]) -> SchemaSnapshot {
        let mut sql = format!("DEFINE TABLE OVERWRITE {table} SCHEMAFULL");
        for (name, ty) in fields {
            sql.push_str(&format!(
                "; DEFINE FIELD OVERWRITE {name} ON TABLE {table} TYPE {ty}"
            ));
        }
        for e in extra {
            sql.push_str("; ");
            sql.push_str(e);
        }
        SchemaSnapshot::from_sql(table, &sql)
    }

    // ---- classify_diff: data + summary ----

    #[test]
    fn classify_diff_reports_add_alter_drop() {
        let old = snap("note", &[("title", "string"), ("body", "string"), ("count", "int")], &[]);
        // body dropped; count widened int->float; note added optional.
        let new = snap("note", &[("title", "string"), ("count", "float"), ("note", "option<string>")], &[]);

        let r = classify_diff(&old, &new);

        let by = |f: &str| r.entries.iter().find(|e| e.field == f).cloned();
        assert_eq!(by("note").unwrap().class, FieldClass::Safe); // added optional
        assert_eq!(by("count").unwrap().class, FieldClass::Safe); // int -> float widen
        assert_eq!(by("count").unwrap().from_type.as_deref(), Some("int"));
        assert_eq!(by("count").unwrap().to_type.as_deref(), Some("float"));
        assert_eq!(by("body").unwrap().class, FieldClass::Blocked); // dropped
        assert_eq!(by("body").unwrap().to_type, None);
        assert!(by("title").is_none(), "unchanged field must not produce an entry");

        assert_eq!(r.summary, ClassSummary { safe: 2, warn: 0, needs_backfill: 0, blocked: 1 });
        assert_eq!(r.summary.worst(), Some(FieldClass::Blocked));
    }

    #[test]
    fn classify_diff_optional_to_required_needs_backfill() {
        let old = snap("t", &[("x", "option<string>")], &[]);
        let new = snap("t", &[("x", "string")], &[]);
        let r = classify_diff(&old, &new);
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].class, FieldClass::NeedsBackfill);
        assert_eq!(r.entries[0].from_type.as_deref(), Some("option<string>"));
        assert_eq!(r.entries[0].to_type.as_deref(), Some("string"));
    }

    #[test]
    fn classify_diff_required_add_needs_backfill() {
        let old = snap("t", &[], &[]); // table EXISTS (DEFINE TABLE present), no fields yet
        let new = snap("t", &[("x", "string")], &[]); // required, no default
        let r = classify_diff(&old, &new);
        assert_eq!(r.summary, ClassSummary { safe: 0, warn: 0, needs_backfill: 1, blocked: 0 });
    }

    #[test]
    fn new_table_required_fields_are_safe() {
        // Empty prior snapshot → the table is created in this migration → no existing
        // rows → even required fields are Safe (no backfill).
        let old = SchemaSnapshot::default();
        let new = snap("note", &[("title", "string"), ("body", "option<string>"), ("created_at", "datetime")], &[]);
        let r = classify_diff(&old, &new);
        for f in ["title", "body", "created_at"] {
            let e = r.entries.iter().find(|e| e.field == f).unwrap();
            assert_eq!(e.class, FieldClass::Safe, "`{f}` on a newly-created table must be Safe");
        }
        assert_eq!(r.summary, ClassSummary { safe: 3, warn: 0, needs_backfill: 0, blocked: 0 });
    }

    #[test]
    fn existing_table_required_add_still_needs_backfill() {
        // The table already exists (in `old`) → a required add can violate existing
        // rows → NeedsBackfill (the new-table rule must NOT weaken this).
        let old = snap("note", &[("title", "string")], &[]);
        let new = snap("note", &[("title", "string"), ("body", "string")], &[]);
        let r = classify_diff(&old, &new);
        let body = r.entries.iter().find(|e| e.field == "body").unwrap();
        assert_eq!(body.class, FieldClass::NeedsBackfill);
    }

    #[test]
    fn classify_diff_skips_tables_and_indexes() {
        // Only an index is dropped + the table stays; no FIELD changes.
        let old = snap("t", &[("a", "string")], &["DEFINE INDEX OVERWRITE t_a_idx ON TABLE t FIELDS a"]);
        let new = snap("t", &[("a", "string")], &[]);
        let r = classify_diff(&old, &new);
        assert!(r.is_empty(), "table/index changes must not appear in field classifications");
    }

    // ---- non_drop_summary (the Phase-3b drop-exclusion seam) ----

    #[test]
    fn non_drop_summary_excludes_drops() {
        // `a` narrows (string→int = Blocked, non-drop); `b` is dropped (Blocked, drop).
        let old = snap("t", &[("a", "string"), ("b", "string")], &[]);
        let new = snap("t", &[("a", "int")], &[]);
        let r = classify_diff(&old, &new);

        assert_eq!(r.summary.blocked, 2, "full summary counts both blocked changes");
        let nd = r.non_drop_summary();
        assert_eq!(nd.blocked, 1, "non-drop summary drops the dropped field");
        assert_eq!(nd.worst(), Some(FieldClass::Blocked));
    }

    #[test]
    fn non_drop_summary_of_only_drops_is_empty() {
        let old = snap("t", &[("a", "string")], &[]);
        let new = snap("t", &[], &[]); // `a` dropped
        let r = classify_diff(&old, &new);

        assert_eq!(r.summary.blocked, 1);
        assert_eq!(r.non_drop_summary().worst(), None, "a drop-only diff has no non-drop severity");
    }

    // ---- CHARACTERIZATION: freeze SchemaDiff::{statements,destructive,warnings} ----
    // These live here (not in schema.rs) so schema.rs stays byte-for-byte untouched.
    // If any of the three frozen outputs change, these fail.

    fn frozen_fixture() -> schema::SchemaDiff {
        let old = snap(
            "note",
            &[("title", "string"), ("body", "string"), ("count", "int")],
            &["DEFINE INDEX OVERWRITE note_title_idx ON TABLE note FIELDS title"],
        );
        // title unchanged; count int->float (alter); pinned added; body + index dropped.
        let new = snap("note", &[("title", "string"), ("count", "float"), ("pinned", "bool")], &[]);
        schema::diff(&old, &new)
    }

    #[test]
    fn statements_are_frozen() {
        let d = frozen_fixture();
        assert_eq!(
            d.statements("note"),
            vec![
                "DEFINE FIELD OVERWRITE pinned ON TABLE note TYPE bool".to_string(),
                "DEFINE FIELD OVERWRITE count ON TABLE note TYPE float".to_string(),
                "REMOVE FIELD IF EXISTS body ON TABLE note".to_string(),
                "REMOVE INDEX IF EXISTS note_title_idx ON TABLE note".to_string(),
            ]
        );
    }

    #[test]
    fn destructive_is_frozen() {
        assert_eq!(frozen_fixture().destructive(), vec!["REMOVE FIELD body".to_string()]);
    }

    #[test]
    fn warnings_are_frozen() {
        assert_eq!(
            frozen_fixture().warnings(),
            vec!["ALTER FIELD count".to_string(), "REMOVE INDEX note_title_idx".to_string()]
        );
    }

    // ---- Runtime Metadata v1, Stage 0 spike: FLEXIBLE custom-field bag ----
    // (claudedocs/design_runtime-metadata-v1.md — D1/Stage 0)

    // NOTE: SurrealDB 3.x requires `FLEXIBLE` *after* the TYPE clause (verified
    // live in `lib.rs::flexible_custom_bag_applies_and_roundtrips`).
    const FLEX_BAG: &str = "DEFINE FIELD OVERWRITE custom ON TABLE contact TYPE option<object> FLEXIBLE";

    #[test]
    fn flexible_bag_add_on_existing_table_classifies_safe() {
        let old = snap("contact", &[("name", "string")], &[]);
        let new = snap("contact", &[("name", "string")], &[FLEX_BAG]);
        let r = classify_diff(&old, &new);
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].field, "custom");
        assert_eq!(r.entries[0].class, FieldClass::Safe, "{}", r.entries[0].reason);
        assert_eq!(r.entries[0].to_type.as_deref(), Some("option<object>"));
        assert_eq!(r.summary.worst(), Some(FieldClass::Safe));
    }

    #[test]
    fn flexible_bag_on_new_table_classifies_safe() {
        let new = snap("contact", &[("name", "string")], &[FLEX_BAG]);
        let r = classify_diff(&SchemaSnapshot::default(), &new);
        assert!(!r.entries.is_empty());
        assert!(r.entries.iter().all(|e| e.class == FieldClass::Safe));
        assert_eq!(r.summary.worst(), Some(FieldClass::Safe));
    }
}
