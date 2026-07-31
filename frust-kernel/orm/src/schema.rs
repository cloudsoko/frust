//! Structured schema snapshots + diffing.
//!
//! A Resource's `surreal_schema()` is a block of `DEFINE TABLE / FIELD / INDEX`
//! statements (the derive emits them with `OVERWRITE`). This module turns that
//! block into a structured [`SchemaSnapshot`] — a set of named objects — so the
//! migration engine can compute a real **diff** against the last-applied
//! snapshot instead of blindly re-running the whole block or comparing an
//! opaque checksum.
//!
//! The diff is what makes migrations production-grade: a removed struct field
//! becomes an explicit `REMOVE FIELD` (no silent drift), and destructive
//! changes are classified so they can be gated behind operator approval.
//!
//! This module is intentionally DB-free and pure — it's fully unit-tested
//! without a SurrealDB instance.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The kinds of schema object the framework emits. `Event` joined in WO-005
/// module 3 (ADR-009: the docstatus lattice is the DB tier's one resident).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    Table,
    Field,
    Index,
    Event,
}

impl ObjectKind {
    /// Stable key prefix used in the snapshot map and the SurrealQL keyword.
    fn keyword(self) -> &'static str {
        match self {
            ObjectKind::Table => "table",
            ObjectKind::Field => "field",
            ObjectKind::Index => "index",
            ObjectKind::Event => "event",
        }
    }

    /// Apply order rank — tables before fields before indexes before events
    /// (a field may reference the table; an index references fields; an
    /// event's body references fields).
    fn rank(self) -> u8 {
        match self {
            ObjectKind::Table => 0,
            ObjectKind::Field => 1,
            ObjectKind::Index => 2,
            ObjectKind::Event => 3,
        }
    }
}

/// One DDL object: its kind, name, and the exact `DEFINE ... OVERWRITE`
/// statement (no trailing `;`) that the derive generated for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaObject {
    pub kind: ObjectKind,
    pub name: String,
    pub define_sql: String,
}

impl SchemaObject {
    fn key(&self) -> String {
        format!("{}:{}", self.kind.keyword(), self.name)
    }
}

/// Structured snapshot of a resource's desired schema. Stored in migration
/// history as JSON so the next run can diff against it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaSnapshot {
    pub table: String,
    /// `"field:title" -> SchemaObject`, ordered for stable serialization.
    pub objects: BTreeMap<String, SchemaObject>,
}

impl SchemaSnapshot {
    /// Parse a Resource's generated SurrealQL into a structured snapshot.
    /// Statements that aren't `DEFINE TABLE/FIELD/INDEX/EVENT` are ignored.
    pub fn from_sql(table: &str, sql: &str) -> Self {
        let mut objects = BTreeMap::new();
        for raw in split_statements(sql) {
            let stmt = raw.trim();
            if stmt.is_empty() {
                continue;
            }
            if let Some(obj) = parse_define(stmt) {
                objects.insert(obj.key(), obj);
            }
        }
        Self {
            table: table.to_string(),
            objects,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

/// Split a DDL block on statement-terminating semicolons only: semicolons
/// inside `{ … }` (EVENT bodies) or single-quoted strings do not terminate a
/// statement. A naive `split(';')` chops the docstatus-lattice EVENT apart.
fn split_statements(sql: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut in_str, mut start) = (0usize, false, 0usize);
    let bytes = sql.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_str => i += 1, // skip escaped char inside a string
            b'\'' => in_str = !in_str,
            b'{' if !in_str => depth += 1,
            b'}' if !in_str => depth = depth.saturating_sub(1),
            b';' if !in_str && depth == 0 => {
                out.push(&sql[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < sql.len() {
        out.push(&sql[start..]);
    }
    out
}

/// Parse a single `DEFINE TABLE|FIELD|INDEX|EVENT [OVERWRITE|IF NOT EXISTS] <name> …`
/// statement into a [`SchemaObject`]. Returns `None` for anything else.
fn parse_define(stmt: &str) -> Option<SchemaObject> {
    let mut toks = stmt.split_whitespace();
    if !toks.next()?.eq_ignore_ascii_case("DEFINE") {
        return None;
    }
    let kind = match toks.next()?.to_ascii_uppercase().as_str() {
        "TABLE" => ObjectKind::Table,
        "FIELD" => ObjectKind::Field,
        "INDEX" => ObjectKind::Index,
        "EVENT" => ObjectKind::Event,
        _ => return None,
    };
    // Skip an optional `OVERWRITE` or `IF NOT EXISTS` qualifier before the name.
    let mut name = toks.next()?;
    if name.eq_ignore_ascii_case("OVERWRITE") {
        name = toks.next()?;
    } else if name.eq_ignore_ascii_case("IF") {
        toks.next()?; // NOT
        toks.next()?; // EXISTS
        name = toks.next()?;
    }
    Some(SchemaObject {
        kind,
        name: name.to_string(),
        define_sql: stmt.to_string(),
    })
}

/// A schema object that exists in the old snapshot but not the new one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DroppedObject {
    pub kind: ObjectKind,
    pub name: String,
}

/// The computed difference between two snapshots.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SchemaDiff {
    /// True when there was no prior snapshot (first migration of this resource).
    pub new_table: bool,
    /// Objects present in `new` but absent in `old` (safe to add).
    pub creates: Vec<SchemaObject>,
    /// Objects whose `define_sql` changed (possible coercion — a warning).
    pub alters: Vec<SchemaObject>,
    /// Objects present in `old` but absent in `new`.
    pub drops: Vec<DroppedObject>,
}

impl SchemaDiff {
    pub fn is_empty(&self) -> bool {
        self.creates.is_empty() && self.alters.is_empty() && self.drops.is_empty()
    }

    /// **Blocking** destructive operations — dropped fields cause data loss and
    /// require explicit operator approval (`allow_destructive`).
    pub fn destructive(&self) -> Vec<String> {
        self.drops
            .iter()
            .filter(|d| d.kind == ObjectKind::Field)
            .map(|d| format!("REMOVE FIELD {}", d.name))
            .collect()
    }

    /// **Non-blocking** warnings — applied but worth surfacing: altered fields
    /// (a redefine may coerce data) and dropped indexes (rebuildable, but a
    /// query-planner change).
    pub fn warnings(&self) -> Vec<String> {
        let mut w: Vec<String> = self
            .alters
            .iter()
            .filter(|o| o.kind == ObjectKind::Field)
            .map(|o| format!("ALTER FIELD {}", o.name))
            .collect();
        w.extend(
            self.drops
                .iter()
                .filter(|d| d.kind == ObjectKind::Index)
                .map(|d| format!("REMOVE INDEX {}", d.name)),
        );
        // event changes are behavior changes, never silent: altered event
        // bodies and dropped events both surface as warnings
        w.extend(
            self.alters
                .iter()
                .filter(|o| o.kind == ObjectKind::Event)
                .map(|o| format!("ALTER EVENT {}", o.name)),
        );
        w.extend(
            self.drops
                .iter()
                .filter(|d| d.kind == ObjectKind::Event)
                .map(|d| format!("REMOVE EVENT {}", d.name)),
        );
        w
    }

    /// The ordered SurrealQL statements (no trailing `;`) that turn the old
    /// schema into the new one: creates + alters first (table → field → index),
    /// drops last.
    pub fn statements(&self, table: &str) -> Vec<String> {
        let mut applied: Vec<&SchemaObject> = self.creates.iter().chain(self.alters.iter()).collect();
        applied.sort_by_key(|o| o.kind.rank());

        let mut out: Vec<String> = applied.iter().map(|o| o.define_sql.clone()).collect();

        for d in &self.drops {
            match d.kind {
                ObjectKind::Field => {
                    out.push(format!("REMOVE FIELD IF EXISTS {} ON TABLE {table}", d.name))
                }
                ObjectKind::Index => {
                    out.push(format!("REMOVE INDEX IF EXISTS {} ON TABLE {table}", d.name))
                }
                ObjectKind::Event => {
                    out.push(format!("REMOVE EVENT IF EXISTS {} ON TABLE {table}", d.name))
                }
                // A registered Resource always provides its own table; we never
                // auto-drop tables (that's a resource deletion, handled out of band).
                ObjectKind::Table => {}
            }
        }
        out
    }
}

/// Compute the diff that migrates `old` → `new`.
pub fn diff(old: &SchemaSnapshot, new: &SchemaSnapshot) -> SchemaDiff {
    let mut d = SchemaDiff {
        new_table: old.objects.is_empty(),
        ..Default::default()
    };

    for (key, obj) in &new.objects {
        match old.objects.get(key) {
            None => d.creates.push(obj.clone()),
            Some(prev) if prev.define_sql != obj.define_sql => d.alters.push(obj.clone()),
            Some(_) => {} // unchanged
        }
    }
    for (key, obj) in &old.objects {
        if !new.objects.contains_key(key) {
            d.drops.push(DroppedObject {
                kind: obj.kind,
                name: obj.name.clone(),
            });
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE_V1: &str = "\
DEFINE TABLE OVERWRITE note SCHEMAFULL;
DEFINE FIELD OVERWRITE title ON TABLE note TYPE string;
DEFINE FIELD OVERWRITE body ON TABLE note TYPE string;
DEFINE INDEX OVERWRITE note_title_idx ON TABLE note FIELDS title;
";

    #[test]
    fn parses_all_objects() {
        let s = SchemaSnapshot::from_sql("note", NOTE_V1);
        assert_eq!(s.objects.len(), 4);
        assert!(s.objects.contains_key("table:note"));
        assert!(s.objects.contains_key("field:title"));
        assert!(s.objects.contains_key("field:body"));
        assert!(s.objects.contains_key("index:note_title_idx"));
    }

    #[test]
    fn first_migration_is_all_creates() {
        let new = SchemaSnapshot::from_sql("note", NOTE_V1);
        let d = diff(&SchemaSnapshot::default(), &new);
        assert!(d.new_table);
        assert_eq!(d.creates.len(), 4);
        assert!(d.alters.is_empty() && d.drops.is_empty());
        assert!(d.destructive().is_empty());
        // table statement must sort before field/index statements.
        let stmts = d.statements("note");
        assert!(stmts[0].contains("DEFINE TABLE"));
    }

    #[test]
    fn unchanged_schema_is_empty_diff() {
        let a = SchemaSnapshot::from_sql("note", NOTE_V1);
        let b = SchemaSnapshot::from_sql("note", NOTE_V1);
        assert!(diff(&a, &b).is_empty());
    }

    #[test]
    fn added_field_is_a_create() {
        let v2 = format!("{NOTE_V1}DEFINE FIELD OVERWRITE pinned ON TABLE note TYPE bool;\n");
        let d = diff(
            &SchemaSnapshot::from_sql("note", NOTE_V1),
            &SchemaSnapshot::from_sql("note", &v2),
        );
        assert_eq!(d.creates.len(), 1);
        assert_eq!(d.creates[0].name, "pinned");
        assert!(d.destructive().is_empty());
    }

    #[test]
    fn removed_field_is_destructive_drop() {
        // v2 drops `body`.
        let v2 = "\
DEFINE TABLE OVERWRITE note SCHEMAFULL;
DEFINE FIELD OVERWRITE title ON TABLE note TYPE string;
DEFINE INDEX OVERWRITE note_title_idx ON TABLE note FIELDS title;
";
        let d = diff(
            &SchemaSnapshot::from_sql("note", NOTE_V1),
            &SchemaSnapshot::from_sql("note", v2),
        );
        assert_eq!(d.drops.len(), 1);
        assert_eq!(d.drops[0].name, "body");
        assert_eq!(d.destructive(), vec!["REMOVE FIELD body"]);
        let stmts = d.statements("note");
        assert!(stmts.iter().any(|s| s == "REMOVE FIELD IF EXISTS body ON TABLE note"));
    }

    #[test]
    fn changed_field_type_is_an_alter_warning() {
        let v2 = NOTE_V1.replace("title ON TABLE note TYPE string", "title ON TABLE note TYPE int");
        let d = diff(
            &SchemaSnapshot::from_sql("note", NOTE_V1),
            &SchemaSnapshot::from_sql("note", &v2),
        );
        assert_eq!(d.alters.len(), 1);
        assert_eq!(d.alters[0].name, "title");
        assert!(d.destructive().is_empty(), "alter is a warning, not blocking");
        assert_eq!(d.warnings(), vec!["ALTER FIELD title"]);
    }

    // ---- WO-005 module 3: EVENT in the sync vocabulary (ADR-009) ----

    const NOTE_V1_EVENT: &str = "\
DEFINE TABLE OVERWRITE note SCHEMAFULL;
DEFINE FIELD OVERWRITE title ON TABLE note TYPE string;
DEFINE EVENT OVERWRITE docstatus_lattice ON TABLE note WHEN $event = 'UPDATE' THEN { IF $before.docstatus = 2 { THROW 'FRUST:E_DOCSTATUS:RESURRECTION'; }; };
";

    #[test]
    fn event_body_semicolons_do_not_split_the_statement() {
        // the naive split(';') bug: an EVENT body contains ';' inside braces
        let s = SchemaSnapshot::from_sql("note", NOTE_V1_EVENT);
        let ev = s.objects.get("event:docstatus_lattice").expect("event parses as ONE object");
        assert!(ev.define_sql.contains("THROW"), "full body retained");
        assert!(ev.define_sql.trim_end().ends_with('}'), "body intact to the closing brace: {}", ev.define_sql);
        // and no garbage fragments leaked into the snapshot
        assert_eq!(s.objects.len(), 3, "table + field + event only: {:?}", s.objects.keys());
    }

    #[test]
    fn event_parses_and_orders_last() {
        let s = SchemaSnapshot::from_sql("note", NOTE_V1_EVENT);
        assert!(s.objects.contains_key("event:docstatus_lattice"));
        let d = diff(&SchemaSnapshot::default(), &s);
        let stmts = d.statements("note");
        assert!(stmts.last().unwrap().contains("DEFINE EVENT"), "events apply after fields");
        assert!(d.destructive().is_empty() && d.warnings().is_empty());
    }

    #[test]
    fn event_body_change_is_a_warning_not_silent() {
        let v2 = NOTE_V1_EVENT.replace("RESURRECTION", "RESURRECTION_V2");
        let d = diff(
            &SchemaSnapshot::from_sql("note", NOTE_V1_EVENT),
            &SchemaSnapshot::from_sql("note", &v2),
        );
        assert_eq!(d.alters.len(), 1);
        assert!(d.warnings().contains(&"ALTER EVENT docstatus_lattice".to_string()));
        assert!(d.destructive().is_empty(), "event alter is gated by warning, not drop");
    }

    #[test]
    fn dropped_event_is_a_warning_and_emits_remove() {
        let v2 = "\
DEFINE TABLE OVERWRITE note SCHEMAFULL;
DEFINE FIELD OVERWRITE title ON TABLE note TYPE string;
";
        let d = diff(
            &SchemaSnapshot::from_sql("note", NOTE_V1_EVENT),
            &SchemaSnapshot::from_sql("note", v2),
        );
        assert!(d.warnings().contains(&"REMOVE EVENT docstatus_lattice".to_string()));
        assert!(d
            .statements("note")
            .iter()
            .any(|s| s == "REMOVE EVENT IF EXISTS docstatus_lattice ON TABLE note"));
        assert!(d.destructive().is_empty());
    }

    #[test]
    fn dropped_index_is_a_warning_not_destructive() {
        let v2 = "\
DEFINE TABLE OVERWRITE note SCHEMAFULL;
DEFINE FIELD OVERWRITE title ON TABLE note TYPE string;
DEFINE FIELD OVERWRITE body ON TABLE note TYPE string;
";
        let d = diff(
            &SchemaSnapshot::from_sql("note", NOTE_V1),
            &SchemaSnapshot::from_sql("note", v2),
        );
        assert!(d.destructive().is_empty());
        assert_eq!(d.warnings(), vec!["REMOVE INDEX note_title_idx"]);
    }

    // ---- Runtime Metadata v1, Stage 0 spike: FLEXIBLE custom-field bag ----
    // (claudedocs/design_runtime-metadata-v1.md — D1/Stage 0)

    // NOTE: SurrealDB 3.x requires `FLEXIBLE` *after* the TYPE clause —
    // `… FLEXIBLE TYPE option<object>` (the 1.x/2.x order) is a parse error.
    // Verified live in `lib.rs::flexible_custom_bag_applies_and_roundtrips`.
    const CONTACT_FLEX: &str = "\
DEFINE TABLE OVERWRITE contact SCHEMAFULL;
DEFINE FIELD OVERWRITE name ON TABLE contact TYPE string;
DEFINE FIELD OVERWRITE custom ON TABLE contact TYPE option<object> FLEXIBLE;
";

    #[test]
    fn flexible_bag_field_parses_with_define_sql_preserved() {
        let s = SchemaSnapshot::from_sql("contact", CONTACT_FLEX);
        assert_eq!(s.objects.len(), 3);
        let bag = s.objects.get("field:custom").expect("FLEXIBLE field must parse");
        assert_eq!(bag.kind, ObjectKind::Field);
        assert_eq!(bag.name, "custom");
        assert_eq!(
            bag.define_sql,
            "DEFINE FIELD OVERWRITE custom ON TABLE contact TYPE option<object> FLEXIBLE"
        );
    }

    #[test]
    fn flexible_bag_on_new_table_is_all_creates() {
        let new = SchemaSnapshot::from_sql("contact", CONTACT_FLEX);
        let d = diff(&SchemaSnapshot::default(), &new);
        assert!(d.new_table);
        assert_eq!(d.creates.len(), 3);
        assert!(d.alters.is_empty() && d.drops.is_empty());
        assert!(d.destructive().is_empty());
        assert!(d.warnings().is_empty());
    }

    #[test]
    fn flexible_bag_added_to_existing_table_is_a_plain_create() {
        let v1 = "\
DEFINE TABLE OVERWRITE contact SCHEMAFULL;
DEFINE FIELD OVERWRITE name ON TABLE contact TYPE string;
";
        let d = diff(
            &SchemaSnapshot::from_sql("contact", v1),
            &SchemaSnapshot::from_sql("contact", CONTACT_FLEX),
        );
        assert_eq!(d.creates.len(), 1);
        assert_eq!(d.creates[0].name, "custom");
        assert!(d.alters.is_empty() && d.drops.is_empty());
        assert!(d.destructive().is_empty());
        assert!(d.warnings().is_empty());
        // The emitted statement is the FLEXIBLE define, verbatim.
        assert_eq!(
            d.statements("contact"),
            vec!["DEFINE FIELD OVERWRITE custom ON TABLE contact TYPE option<object> FLEXIBLE".to_string()]
        );
    }
}
