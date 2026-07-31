//! Structured field snapshots + field-level change classification (migration-gap
//! Phase 1).
//!
//! The existing [`crate::schema`] diff is **text-level** — an `alter` just means a
//! field's `define_sql` string changed, so it can't tell a safe widening from an
//! unsafe narrowing. This module parses a `DEFINE FIELD … OVERWRITE …` statement into
//! a structured [`FieldSnapshot`] (type / nullability / readonly / value / assert) and
//! classifies a field-level change with real severity.
//!
//! **This module is pure and DB-free.** It runs *parallel* to the existing diff
//! engine. As of Phase 2a it is consumed by [`crate::classify`] to produce
//! **observable, non-authoritative** reports — it does not drive apply/refuse
//! decisions, change the ledger, or change the CLI.
//!
//! Conservative by default: an unrecognized / flexible (`any`) field type classifies
//! any change as **blocked / manual**, never silently safe. `OVERWRITE` is treated as
//! syntax, not a safety guarantee.

use serde::Serialize;

/// A normalized SurrealDB field type, parsed from a `TYPE …` clause. Mirrors the
/// strings `framework-derive`'s `surreal_type` emits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurrealType {
    String,
    Int,
    Float,
    Bool,
    Datetime,
    Duration,
    Decimal,
    Bytes,
    Uuid,
    Object,
    /// `any` / flexible / no `TYPE` clause — treated conservatively (opaque).
    Any,
    /// `record` or `record<target>`.
    Record(String),
    /// `array<inner>`.
    Array(Box<SurrealType>),
    /// `geometry<point>` etc.
    Geometry(String),
    /// Anything we didn't recognize — treated conservatively (opaque).
    Other(String),
}

impl SurrealType {
    /// Whether this type is opaque to the classifier (`Any` / `Other`) — any change
    /// touching it is classified conservatively (blocked).
    fn is_opaque(&self) -> bool {
        matches!(self, SurrealType::Any | SurrealType::Other(_))
    }

    /// Canonical SurrealQL spelling of this (inner) type — the inverse of
    /// `parse_inner`. Used for human-readable change descriptions; does not include
    /// any `option<…>` wrapper (nullability lives on [`FieldSnapshot::optional`]).
    pub fn render(&self) -> String {
        match self {
            SurrealType::String => "string".into(),
            SurrealType::Int => "int".into(),
            SurrealType::Float => "float".into(),
            SurrealType::Bool => "bool".into(),
            SurrealType::Datetime => "datetime".into(),
            SurrealType::Duration => "duration".into(),
            SurrealType::Decimal => "decimal".into(),
            SurrealType::Bytes => "bytes".into(),
            SurrealType::Uuid => "uuid".into(),
            SurrealType::Object => "object".into(),
            SurrealType::Any => "any".into(),
            SurrealType::Record(t) if t.is_empty() => "record".into(),
            SurrealType::Record(t) => format!("record<{t}>"),
            SurrealType::Array(inner) => format!("array<{}>", inner.render()),
            SurrealType::Geometry(g) => format!("geometry<{g}>"),
            SurrealType::Other(s) => s.clone(),
        }
    }

    /// Parse an inner type token (already stripped of any `option<…>` wrapper).
    fn parse_inner(s: &str) -> SurrealType {
        let s = s.trim();
        match s {
            "string" => SurrealType::String,
            "int" => SurrealType::Int,
            "float" => SurrealType::Float,
            "bool" => SurrealType::Bool,
            "datetime" => SurrealType::Datetime,
            "duration" => SurrealType::Duration,
            "decimal" => SurrealType::Decimal,
            "bytes" => SurrealType::Bytes,
            "uuid" => SurrealType::Uuid,
            "object" => SurrealType::Object,
            "any" => SurrealType::Any,
            "record" => SurrealType::Record(String::new()),
            "array" => SurrealType::Array(Box::new(SurrealType::Any)),
            _ => {
                if let Some(t) = s.strip_prefix("record<").and_then(|x| x.strip_suffix('>')) {
                    SurrealType::Record(t.to_string())
                } else if let Some(inner) = s.strip_prefix("array<").and_then(|x| x.strip_suffix('>')) {
                    SurrealType::Array(Box::new(SurrealType::parse_inner(inner)))
                } else if let Some(g) = s.strip_prefix("geometry<").and_then(|x| x.strip_suffix('>')) {
                    SurrealType::Geometry(g.to_string())
                } else {
                    SurrealType::Other(s.to_string())
                }
            }
        }
    }
}

/// Structured view of one field, parsed from its `DEFINE FIELD … OVERWRITE …`
/// statement. Built on read from the existing snapshot's `define_sql`; nothing is
/// stored in a new format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSnapshot {
    pub name: String,
    pub table: String,
    pub ty: SurrealType,
    /// `TYPE option<…>` (nullable). `false` = required.
    pub optional: bool,
    /// A `READONLY` clause is present.
    pub readonly: bool,
    /// A `VALUE …` or `DEFAULT …` clause is present (the field is computed/defaulted).
    pub has_value_or_default: bool,
    /// An `ASSERT …` clause is present.
    pub has_assert: bool,
}

impl FieldSnapshot {
    /// Parse a single `DEFINE FIELD … ON TABLE … [TYPE …] [VALUE/DEFAULT …]
    /// [READONLY] [ASSERT …]` statement. Returns `None` for any statement that isn't
    /// a `DEFINE FIELD` (e.g. `DEFINE TABLE` / `DEFINE INDEX`), so callers can skip
    /// them. A `DEFINE FIELD` with no/unrecognized `TYPE` still parses, with
    /// `ty = Any/Other` (classified conservatively).
    pub fn parse(define_sql: &str) -> Option<FieldSnapshot> {
        let stmt = define_sql.trim().trim_end_matches(';').trim();
        let toks: Vec<&str> = stmt.split_whitespace().collect();
        let mut i = 0;

        if !toks.get(i)?.eq_ignore_ascii_case("DEFINE") {
            return None;
        }
        i += 1;
        if !toks.get(i)?.eq_ignore_ascii_case("FIELD") {
            return None;
        }
        i += 1;
        // Optional `OVERWRITE` / `IF NOT EXISTS` qualifier before the name.
        if toks.get(i)?.eq_ignore_ascii_case("OVERWRITE") {
            i += 1;
        } else if toks.get(i)?.eq_ignore_ascii_case("IF") {
            i += 3; // IF NOT EXISTS
        }
        let name = toks.get(i)?.to_string();
        i += 1;
        if !toks.get(i)?.eq_ignore_ascii_case("ON") {
            return None;
        }
        i += 1;
        if !toks.get(i)?.eq_ignore_ascii_case("TABLE") {
            return None;
        }
        i += 1;
        let table = toks.get(i)?.to_string();
        i += 1;

        // Scan the remainder for TYPE / READONLY / VALUE|DEFAULT / ASSERT.
        // The TYPE clause may span multiple tokens (`option<string | int>`), so
        // collect everything up to the next clause keyword instead of taking a
        // single token — truncating a union to `option<string` would misparse.
        let mut type_parts: Vec<&str> = Vec::new();
        let mut readonly = false;
        let mut has_value_or_default = false;
        let mut has_assert = false;
        let mut j = i;
        while j < toks.len() {
            let t = toks[j];
            if t.eq_ignore_ascii_case("TYPE") {
                j += 1;
                while j < toks.len() && !is_clause_keyword(toks[j]) {
                    type_parts.push(toks[j]);
                    j += 1;
                }
                continue;
            }
            if t.eq_ignore_ascii_case("READONLY") {
                readonly = true;
            } else if t.eq_ignore_ascii_case("VALUE") || t.eq_ignore_ascii_case("DEFAULT") {
                has_value_or_default = true;
            } else if t.eq_ignore_ascii_case("ASSERT") {
                has_assert = true;
            }
            j += 1;
        }

        let type_str = type_parts.join(" ");
        let (optional, ty) = if type_str.is_empty() {
            // No TYPE clause → flexible/any (opaque → conservative).
            (true, SurrealType::Any)
        } else {
            let lower = type_str.to_ascii_lowercase();
            if lower.starts_with("option<") && type_str.ends_with('>') {
                // strip `option<` (7 ASCII bytes) and the trailing `>`
                let inner = &type_str[7..type_str.len() - 1];
                (true, SurrealType::parse_inner(inner))
            } else {
                (false, SurrealType::parse_inner(&type_str))
            }
        };

        Some(FieldSnapshot {
            name,
            table,
            ty,
            optional,
            readonly,
            has_value_or_default,
            has_assert,
        })
    }
}

/// Tokens that terminate a `TYPE …` clause inside a `DEFINE FIELD` statement.
fn is_clause_keyword(t: &str) -> bool {
    [
        "VALUE", "DEFAULT", "READONLY", "ASSERT", "PERMISSIONS", "COMMENT", "FLEXIBLE",
        "REFERENCE",
    ]
    .iter()
    .any(|k| t.eq_ignore_ascii_case(k))
}

/// Severity of a field-level change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldClass {
    /// Auto-applyable without risk.
    Safe,
    /// Applied, but worth surfacing.
    Warn,
    /// Cannot apply safely without a data backfill (existing rows would violate it).
    NeedsBackfill,
    /// Refused without explicit operator approval (data loss / lossy coercion / opaque).
    Blocked,
}

/// What kind of change a field underwent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldChange {
    NoOp,
    Added,
    Dropped,
    /// Type widened to a strictly larger domain (e.g. `int → float`).
    TypeWidened,
    /// Type narrowed or changed incompatibly (possible data loss / coercion).
    TypeNarrowed,
    /// `required → option<…>` (widening nullability).
    BecameOptional,
    /// `option<…> → required` (narrowing nullability — existing NULLs need backfill).
    BecameRequired,
    /// Only `readonly` / `value` / `assert` changed; type + nullability unchanged.
    AttributesChanged,
    /// An opaque/unparsed type is involved — can't reason about it.
    Conservative,
}

/// A classified field change: what happened + how severe + why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldChangeClassification {
    pub change: FieldChange,
    pub class: FieldClass,
    pub reason: String,
}

impl FieldChangeClassification {
    fn new(change: FieldChange, class: FieldClass, reason: &str) -> Self {
        Self { change, class, reason: reason.to_string() }
    }
}

/// Whether `old → new` is a known-safe type widening. Conservative: only an explicit
/// allow-list widens; everything else is treated as narrowing/incompatible.
fn is_widening(old: &SurrealType, new: &SurrealType) -> bool {
    use SurrealType::*;
    matches!(
        (old, new),
        (Int, Float) | (Int, Decimal) | (Float, Decimal)
    )
}

/// Classify a field-level change between two structured snapshots. Pure — the Phase-2
/// consumer will call this over the snapshot diff; here it stands alone with tests.
pub fn classify_field_change(
    old: Option<&FieldSnapshot>,
    new: Option<&FieldSnapshot>,
) -> FieldChangeClassification {
    match (old, new) {
        (None, None) => FieldChangeClassification::new(FieldChange::NoOp, FieldClass::Safe, "no field"),

        // Added: safe if nullable or defaulted; otherwise existing rows have no value.
        (None, Some(n)) => {
            if n.optional || n.has_value_or_default {
                FieldChangeClassification::new(FieldChange::Added, FieldClass::Safe, "added optional/defaulted field")
            } else {
                FieldChangeClassification::new(
                    FieldChange::Added,
                    FieldClass::NeedsBackfill,
                    "added required field without default — existing rows need a value",
                )
            }
        }

        // Dropped: data loss — always blocked (rename intent is a later phase).
        (Some(_), None) => FieldChangeClassification::new(
            FieldChange::Dropped,
            FieldClass::Blocked,
            "field dropped — data loss",
        ),

        (Some(o), Some(n)) => classify_modify(o, n),
    }
}

fn attrs_changed(o: &FieldSnapshot, n: &FieldSnapshot) -> bool {
    o.readonly != n.readonly || o.has_value_or_default != n.has_value_or_default || o.has_assert != n.has_assert
}

fn classify_modify(o: &FieldSnapshot, n: &FieldSnapshot) -> FieldChangeClassification {
    // Opaque types: only an *identical* opaque field is a no-op; any difference is
    // conservatively blocked.
    if o.ty.is_opaque() || n.ty.is_opaque() {
        if o == n {
            return FieldChangeClassification::new(FieldChange::NoOp, FieldClass::Safe, "no meaningful change");
        }
        return FieldChangeClassification::new(
            FieldChange::Conservative,
            FieldClass::Blocked,
            "unrecognized / flexible field type — manual review",
        );
    }

    let null_narrows = o.optional && !n.optional; // option<T> → T
    let null_widens = !o.optional && n.optional; // T → option<T>

    if o.ty == n.ty {
        if null_narrows {
            FieldChangeClassification::new(
                FieldChange::BecameRequired,
                FieldClass::NeedsBackfill,
                "optional → required — existing NULLs need a backfill",
            )
        } else if null_widens {
            FieldChangeClassification::new(FieldChange::BecameOptional, FieldClass::Safe, "required → optional (widening)")
        } else if attrs_changed(o, n) {
            FieldChangeClassification::new(
                FieldChange::AttributesChanged,
                FieldClass::Warn,
                "field attributes changed (assert/value/readonly)",
            )
        } else {
            FieldChangeClassification::new(FieldChange::NoOp, FieldClass::Safe, "no meaningful change")
        }
    } else if is_widening(&o.ty, &n.ty) {
        // Type widened; but a simultaneous nullability narrowing still needs a backfill.
        if null_narrows {
            FieldChangeClassification::new(
                FieldChange::BecameRequired,
                FieldClass::NeedsBackfill,
                "type widened but optional → required — backfill needed",
            )
        } else {
            FieldChangeClassification::new(FieldChange::TypeWidened, FieldClass::Safe, "type widened")
        }
    } else {
        FieldChangeClassification::new(
            FieldChange::TypeNarrowed,
            FieldClass::Blocked,
            "type narrowed / incompatible — possible data loss or coercion",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `DEFINE FIELD` statement like the derive emits, then parse it.
    fn field(name: &str, ty_clause: &str, extra: &str) -> FieldSnapshot {
        let sql = format!("DEFINE FIELD OVERWRITE {name} ON TABLE thing TYPE {ty_clause} {extra}");
        FieldSnapshot::parse(&sql).expect("parses as a field")
    }

    // ---- parser ----

    #[test]
    fn parses_current_derive_define_field_shapes() {
        let s = field("title", "string", "");
        assert_eq!(s.name, "title");
        assert_eq!(s.table, "thing");
        assert_eq!(s.ty, SurrealType::String);
        assert!(!s.optional);

        let o = field("note", "option<string>", "");
        assert_eq!(o.ty, SurrealType::String);
        assert!(o.optional);

        let f = field("total", "float", "");
        assert_eq!(f.ty, SurrealType::Float);

        // audit field: TYPE datetime VALUE time::now() READONLY
        let a = field("created_at", "datetime", "VALUE time::now() READONLY");
        assert_eq!(a.ty, SurrealType::Datetime);
        assert!(a.readonly);
        assert!(a.has_value_or_default);
        assert!(!a.has_assert);

        // required-string ASSERT shape
        let asserted = field("name", "string", "ASSERT string::len($value) > 0");
        assert!(asserted.has_assert);
        assert!(!asserted.optional);

        // the `$value != NONE` assert the derive emits for required strings
        let asserted2 = field("name2", "string", "ASSERT $value != NONE");
        assert!(asserted2.has_assert);
    }

    #[test]
    fn union_type_with_spaces_parses_whole_not_truncated() {
        // Multi-token TYPE clause: must capture the full union, not `option<string`.
        let s = field("status", "option<string | int>", "");
        assert!(s.optional, "option<…> wrapper must be recognized around a union");
        assert_eq!(s.ty, SurrealType::Other("string | int".into()));

        // Parsing is stable: identical definitions compare equal → NoOp, not Blocked.
        let again = field("status", "option<string | int>", "");
        let c = classify_field_change(Some(&s), Some(&again));
        assert_eq!(c.change, FieldChange::NoOp);
        assert_eq!(c.class, FieldClass::Safe);

        // A *change* involving a union stays conservatively Blocked (opaque type).
        let n = field("status", "string", "");
        let c = classify_field_change(Some(&s), Some(&n));
        assert_eq!(c.change, FieldChange::Conservative);
        assert_eq!(c.class, FieldClass::Blocked);
    }

    #[test]
    fn union_type_followed_by_clauses_still_sets_flags() {
        // The multi-token TYPE scan must stop at clause keywords, not swallow them.
        let s = field("x", "string | int", "ASSERT $value != NONE");
        assert_eq!(s.ty, SurrealType::Other("string | int".into()));
        assert!(!s.optional);
        assert!(s.has_assert);
        assert!(!s.readonly && !s.has_value_or_default);

        let v = field("y", "int | float", "VALUE 0 READONLY");
        assert_eq!(v.ty, SurrealType::Other("int | float".into()));
        assert!(v.has_value_or_default);
        assert!(v.readonly);
    }

    #[test]
    fn non_field_statements_are_skipped() {
        assert!(FieldSnapshot::parse("DEFINE TABLE OVERWRITE thing SCHEMAFULL").is_none());
        assert!(FieldSnapshot::parse("DEFINE INDEX OVERWRITE thing_title_idx ON TABLE thing FIELDS title").is_none());
        assert!(FieldSnapshot::parse("SELECT * FROM thing").is_none());
    }

    // ---- classification (required cases) ----

    #[test]
    fn add_optional_field_is_safe() {
        let n = field("note", "option<string>", "");
        let c = classify_field_change(None, Some(&n));
        assert_eq!(c.change, FieldChange::Added);
        assert_eq!(c.class, FieldClass::Safe, "{}", c.reason);
    }

    #[test]
    fn drop_field_is_blocked() {
        let o = field("note", "string", "");
        let c = classify_field_change(Some(&o), None);
        assert_eq!(c.change, FieldChange::Dropped);
        assert_eq!(c.class, FieldClass::Blocked, "{}", c.reason);
    }

    #[test]
    fn string_to_int_is_blocked_narrowing() {
        let o = field("x", "string", "");
        let n = field("x", "int", "");
        let c = classify_field_change(Some(&o), Some(&n));
        assert_eq!(c.change, FieldChange::TypeNarrowed);
        assert_eq!(c.class, FieldClass::Blocked, "{}", c.reason);
    }

    #[test]
    fn optional_string_to_string_is_not_safe() {
        let o = field("x", "option<string>", "");
        let n = field("x", "string", "");
        let c = classify_field_change(Some(&o), Some(&n));
        assert_eq!(c.change, FieldChange::BecameRequired);
        assert_ne!(c.class, FieldClass::Safe, "must not be safe: {}", c.reason);
        assert_eq!(c.class, FieldClass::NeedsBackfill);
    }

    #[test]
    fn int_to_optional_int_is_not_blocked() {
        let o = field("x", "int", "");
        let n = field("x", "option<int>", "");
        let c = classify_field_change(Some(&o), Some(&n));
        assert_eq!(c.change, FieldChange::BecameOptional);
        assert_ne!(c.class, FieldClass::Blocked, "must not be blocked: {}", c.reason);
        assert_eq!(c.class, FieldClass::Safe);
    }

    #[test]
    fn identical_field_is_noop() {
        let o = field("x", "string", "");
        let n = field("x", "string", "");
        let c = classify_field_change(Some(&o), Some(&n));
        assert_eq!(c.change, FieldChange::NoOp);
        assert_eq!(c.class, FieldClass::Safe);
    }

    // ---- a few extras that lock in the conservative posture ----

    #[test]
    fn known_numeric_widening_is_safe() {
        let o = field("x", "int", "");
        let n = field("x", "float", "");
        let c = classify_field_change(Some(&o), Some(&n));
        assert_eq!(c.change, FieldChange::TypeWidened);
        assert_eq!(c.class, FieldClass::Safe);
    }

    #[test]
    fn float_to_int_is_blocked() {
        let o = field("x", "float", "");
        let n = field("x", "int", "");
        let c = classify_field_change(Some(&o), Some(&n));
        assert_eq!(c.class, FieldClass::Blocked, "{}", c.reason);
    }

    #[test]
    fn added_required_field_needs_backfill() {
        let n = field("x", "string", ""); // required, no default
        let c = classify_field_change(None, Some(&n));
        assert_eq!(c.class, FieldClass::NeedsBackfill, "{}", c.reason);
    }

    #[test]
    fn opaque_type_change_is_conservatively_blocked() {
        // a field with no TYPE clause parses as `any` (opaque)
        let o = FieldSnapshot::parse("DEFINE FIELD OVERWRITE x ON TABLE thing").unwrap();
        assert_eq!(o.ty, SurrealType::Any);
        let n = field("x", "string", "");
        let c = classify_field_change(Some(&o), Some(&n));
        assert_eq!(c.change, FieldChange::Conservative);
        assert_eq!(c.class, FieldClass::Blocked, "{}", c.reason);
    }

    #[test]
    fn attribute_only_change_warns() {
        let o = field("x", "string", "");
        let n = field("x", "string", "ASSERT string::len($value) > 0");
        let c = classify_field_change(Some(&o), Some(&n));
        assert_eq!(c.change, FieldChange::AttributesChanged);
        assert_eq!(c.class, FieldClass::Warn);
    }

    // ---- Runtime Metadata v1, Stage 0 spike: FLEXIBLE custom-field bag ----
    // (claudedocs/design_runtime-metadata-v1.md — D1/Stage 0)

    // NOTE: SurrealDB 3.x requires `FLEXIBLE` *after* the TYPE clause (verified
    // live in `lib.rs::flexible_custom_bag_applies_and_roundtrips`).
    const FLEX_BAG: &str = "DEFINE FIELD OVERWRITE custom ON TABLE contact TYPE option<object> FLEXIBLE";

    #[test]
    fn flexible_bag_parses_as_optional_object() {
        let s = FieldSnapshot::parse(FLEX_BAG).expect("FLEXIBLE clause must not break the parser");
        assert_eq!(s.name, "custom");
        assert_eq!(s.table, "contact");
        assert_eq!(s.ty, SurrealType::Object, "object, not opaque — FLEXIBLE must not degrade the type");
        assert!(s.optional);
        // FLEXIBLE must not be misread as an attribute clause.
        assert!(!s.readonly && !s.has_value_or_default && !s.has_assert);
    }

    #[test]
    fn adding_flexible_bag_is_safe() {
        let n = FieldSnapshot::parse(FLEX_BAG).unwrap();
        let c = classify_field_change(None, Some(&n));
        assert_eq!(c.change, FieldChange::Added);
        assert_eq!(c.class, FieldClass::Safe, "{}", c.reason);
    }

    #[test]
    fn identical_flexible_bag_is_noop() {
        let o = FieldSnapshot::parse(FLEX_BAG).unwrap();
        let n = FieldSnapshot::parse(FLEX_BAG).unwrap();
        let c = classify_field_change(Some(&o), Some(&n));
        assert_eq!(c.change, FieldChange::NoOp);
        assert_eq!(c.class, FieldClass::Safe);
    }
}
