//! Reviewable on-disk `.surql` migration artifacts.
//!
//! Turns a [`MigrationReport`] — a dry-run plan or an applied run — into one
//! directly-runnable `.surql` file per resource under
//! `<out_dir>/<scope>/v####__<resource>.surql`. Useful for PR review and as an
//! audit trail; the engine never auto-applies these files.

use std::path::{Path, PathBuf};

use crate::{FieldChangeReport, FieldClass, MigrationReport};

const NONE: &[String] = &[];

/// Write reviewable `.surql` artifacts for every migration in `report` under
/// `out_dir/<scope>/`. Returns the written paths (ordered by version, then
/// resource name).
///
/// Works for both planned (dry-run) and applied reports — each migration
/// carries the statements it would run / did run. A report with neither writes
/// nothing and creates no directory.
pub fn write_artifacts(report: &MigrationReport, out_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    // (version, name, statements, destructive, classifications) per migration.
    let mut entries: Vec<(i64, &str, &[String], &[String], &FieldChangeReport)> = Vec::new();
    for p in &report.planned {
        entries.push((
            p.next_version,
            p.name.as_str(),
            &p.statements,
            &p.destructive,
            &p.classifications,
        ));
    }
    for a in &report.applied {
        entries.push((a.version, a.name.as_str(), &a.statements, NONE, &a.classifications));
    }
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));

    let scope_dir = out_dir.join(sanitize(&report.scope));
    std::fs::create_dir_all(&scope_dir)?;

    let mut written = Vec::with_capacity(entries.len());
    for (version, name, statements, destructive, classifications) in entries {
        let path = scope_dir.join(format!("v{version:04}__{}.surql", sanitize(name)));
        std::fs::write(
            &path,
            render(report, name, version, statements, destructive, classifications),
        )?;
        written.push(path);
    }
    Ok(written)
}

/// Render one artifact: a comment header + the statements wrapped in a
/// transaction, so the file is directly runnable via `surreal sql` / `import`.
fn render(
    report: &MigrationReport,
    name: &str,
    version: i64,
    statements: &[String],
    destructive: &[String],
    classifications: &FieldChangeReport,
) -> String {
    let mut s = String::new();
    s.push_str("-- framework-orm-adapter migration artifact (reviewable; not auto-applied)\n");
    s.push_str(&format!("-- scope:    {}\n", report.scope));
    s.push_str(&format!("-- ns/db:    {}/{}\n", report.namespace, report.database));
    s.push_str(&format!("-- resource: {name}\n"));
    s.push_str(&format!("-- version:  {version}\n"));
    if !destructive.is_empty() {
        s.push_str(&format!("-- WARNING destructive: {destructive:?}\n"));
    }
    // Phase 2a: advisory field-change classifications — **comment lines only**. The
    // transaction body below is built solely from the unchanged `statements`, so the
    // runnable DDL is identical whether or not classifications are present.
    if !classifications.is_empty() {
        s.push_str("-- classifications (advisory; not enforced):\n");
        for e in &classifications.entries {
            let transition = match (&e.from_type, &e.to_type) {
                (Some(f), Some(t)) => format!("{}: {f} -> {t}", e.field),
                (None, Some(t)) => format!("{} added: {t}", e.field),
                (Some(f), None) => format!("{} dropped (was {f})", e.field),
                (None, None) => e.field.clone(),
            };
            s.push_str(&format!("--   [{}] {transition} — {}\n", class_label(e.class), e.reason));
        }
        let c = &classifications.summary;
        s.push_str(&format!(
            "-- summary: {} safe, {} warn, {} needs-backfill, {} blocked\n",
            c.safe, c.warn, c.needs_backfill, c.blocked
        ));
    }
    s.push_str("\nBEGIN TRANSACTION;\n");
    for stmt in statements {
        s.push_str(stmt);
        s.push_str(";\n");
    }
    s.push_str("COMMIT TRANSACTION;\n");
    s
}

fn class_label(c: FieldClass) -> &'static str {
    match c {
        FieldClass::Safe => "SAFE",
        FieldClass::Warn => "WARN",
        FieldClass::NeedsBackfill => "NEEDS-BACKFILL",
        FieldClass::Blocked => "BLOCKED",
    }
}

/// Make a scope / resource name safe as a single path segment (`tenant:abc` →
/// `tenant_abc`).
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PlannedMigration;

    fn report_with_plan() -> MigrationReport {
        MigrationReport {
            scope: "tenant:acme".to_string(),
            namespace: "platform".to_string(),
            database: "tenant_acme".to_string(),
            planned: vec![PlannedMigration {
                name: "crm__note".to_string(),
                next_version: 2,
                statements: vec!["DEFINE FIELD pinned ON TABLE note TYPE bool".to_string()],
                destructive: vec![],
                warnings: vec![],
                classifications: FieldChangeReport::default(),
            }],
            ..Default::default()
        }
    }

    fn sample_classifications() -> FieldChangeReport {
        FieldChangeReport {
            entries: vec![
                crate::FieldChangeEntry {
                    field: "qty".into(),
                    change: crate::FieldChange::BecameRequired,
                    class: FieldClass::NeedsBackfill,
                    from_type: Some("option<int>".into()),
                    to_type: Some("int".into()),
                    reason: "optional → required".into(),
                },
                crate::FieldChangeEntry {
                    field: "body".into(),
                    change: crate::FieldChange::Dropped,
                    class: FieldClass::Blocked,
                    from_type: Some("string".into()),
                    to_type: None,
                    reason: "field dropped — data loss".into(),
                },
            ],
            summary: crate::ClassSummary { safe: 0, warn: 0, needs_backfill: 1, blocked: 1 },
        }
    }

    #[test]
    fn artifact_contains_classification_block() {
        let dir = std::env::temp_dir().join("fw_orm_artifacts_classify");
        let _ = std::fs::remove_dir_all(&dir);

        let mut report = report_with_plan();
        report.planned[0].classifications = sample_classifications();
        let paths = write_artifacts(&report, &dir).unwrap();
        let body = std::fs::read_to_string(&paths[0]).unwrap();

        assert!(body.contains("-- classifications (advisory; not enforced):"));
        assert!(body.contains("[NEEDS-BACKFILL] qty: option<int> -> int"));
        assert!(body.contains("[BLOCKED] body dropped (was string)"));
        assert!(body.contains("-- summary: 0 safe, 0 warn, 1 needs-backfill, 1 blocked"));
        // The DDL body is untouched by the advisory comments.
        assert!(body.contains("DEFINE FIELD pinned ON TABLE note TYPE bool;"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn artifact_body_is_unchanged_by_classifications() {
        // The BEGIN..COMMIT transaction body must be byte-identical whether or not
        // classifications are present — comments never touch the runnable DDL.
        let body_of = |r: &MigrationReport, tag: &str| -> String {
            let dir = std::env::temp_dir().join(format!("fw_orm_artifacts_eq_{tag}"));
            let _ = std::fs::remove_dir_all(&dir);
            let p = write_artifacts(r, &dir).unwrap();
            let s = std::fs::read_to_string(&p[0]).unwrap();
            std::fs::remove_dir_all(&dir).unwrap();
            s
        };
        let txn = |s: &str| -> String { s[s.find("BEGIN TRANSACTION;").unwrap()..].to_string() };

        let plain = report_with_plan();
        let mut annotated = report_with_plan();
        annotated.planned[0].classifications = sample_classifications();

        assert_eq!(
            txn(&body_of(&plain, "plain")),
            txn(&body_of(&annotated, "annotated")),
            "transaction body must not change when classifications are added"
        );
    }

    #[test]
    fn writes_one_runnable_file_per_planned_migration() {
        let dir = std::env::temp_dir().join("fw_orm_artifacts_basic");
        let _ = std::fs::remove_dir_all(&dir);

        let paths = write_artifacts(&report_with_plan(), &dir).unwrap();
        assert_eq!(paths.len(), 1);

        let body = std::fs::read_to_string(&paths[0]).unwrap();
        // Path is namespaced by sanitized scope + zero-padded version.
        assert!(paths[0].ends_with("tenant_acme/v0002__crm__note.surql"));
        // Header + transaction-wrapped statement.
        assert!(body.contains("-- resource: crm__note"));
        assert!(body.contains("BEGIN TRANSACTION;"));
        assert!(body.contains("DEFINE FIELD pinned ON TABLE note TYPE bool;"));
        assert!(body.contains("COMMIT TRANSACTION;"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn empty_report_writes_nothing() {
        let dir = std::env::temp_dir().join("fw_orm_artifacts_empty");
        let _ = std::fs::remove_dir_all(&dir);

        let paths = write_artifacts(&MigrationReport::default(), &dir).unwrap();
        assert!(paths.is_empty());
        assert!(!dir.exists(), "no directory created for an empty report");
    }

    #[test]
    fn destructive_plan_is_flagged_in_the_header() {
        let dir = std::env::temp_dir().join("fw_orm_artifacts_destructive");
        let _ = std::fs::remove_dir_all(&dir);

        let mut report = report_with_plan();
        report.planned[0].destructive = vec!["REMOVE FIELD body".to_string()];
        let paths = write_artifacts(&report, &dir).unwrap();
        let body = std::fs::read_to_string(&paths[0]).unwrap();
        assert!(body.contains("WARNING destructive"));
        assert!(body.contains("REMOVE FIELD body"));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
