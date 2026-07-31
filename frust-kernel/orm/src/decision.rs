//! Migration policy engine (migration-gap Phase 3a).
//!
//! Maps `(environment, classification summary, acknowledgments)` → a [`BootDecision`]
//! (`Allow` / `Warn` / `Refuse`). This is the point where field classifications would
//! become **authoritative**.
//!
//! **Phase 3a is pure and inert.** Nothing calls [`decide`] yet — the apply-time gate
//! (3b) and the opt-in `boot_check` (3c) will. It changes no existing behavior:
//! not `migrate_*`, not apply/revert, not `MigrationReport::is_ok()`, not the
//! `SchemaDiff` methods, not the CLI.
//!
//! Scope of `decide`: the **severity × environment** matrix over a [`ClassSummary`].
//! The separate "prod refuses *any pending* migration" rule and the per-drop
//! `allow_destructive` gate are boot/apply concerns (3b) layered on top — not here.

use serde::Serialize;

use crate::classify::ClassSummary;
use crate::field::FieldClass;

/// Deployment environment governing policy strictness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Environment {
    Dev,
    Test,
    Prod,
}

impl Environment {
    /// Strict environments refuse risky changes; permissive ones apply them with a
    /// warning. Only `Prod` is strict.
    pub fn is_strict(self) -> bool {
        matches!(self, Environment::Prod)
    }

    /// Lowercase label for CLI/log output.
    pub fn as_str(self) -> &'static str {
        match self {
            Environment::Dev => "dev",
            Environment::Test => "test",
            Environment::Prod => "prod",
        }
    }
}

impl Default for Environment {
    /// Fail-closed: an unspecified environment is treated as production.
    fn default() -> Self {
        Environment::Prod
    }
}

/// Operator acknowledgments that can downgrade a refusal. Drops specifically remain
/// governed by the existing `MigrationOptions::allow_destructive` (consumed by the
/// apply gate in 3b); this broad `acknowledge` covers the NeedsBackfill / Blocked tier
/// at policy granularity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecisionOptions {
    /// Operator accepts the strict tier (NeedsBackfill / Blocked changes).
    pub acknowledge: bool,
}

/// One reason contributing to a `Warn` / `Refuse` verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DecisionNote {
    pub severity: FieldClass,
    pub message: String,
}

/// The policy verdict for a planned/applied change set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BootDecision {
    /// Proceed; nothing worth surfacing.
    Allow,
    /// Proceed, but surface these notes.
    Warn(Vec<DecisionNote>),
    /// Do not proceed.
    Refuse(Vec<DecisionNote>),
}

impl BootDecision {
    pub fn is_allow(&self) -> bool {
        matches!(self, BootDecision::Allow)
    }

    pub fn is_refuse(&self) -> bool {
        matches!(self, BootDecision::Refuse(_))
    }

    /// The notes attached to a `Warn` / `Refuse` (empty for `Allow`).
    pub fn notes(&self) -> &[DecisionNote] {
        match self {
            BootDecision::Allow => &[],
            BootDecision::Warn(n) | BootDecision::Refuse(n) => n,
        }
    }
}

/// Build the notes for the strict / warn tiers present in a summary, worst-first.
fn notes(summary: &ClassSummary) -> Vec<DecisionNote> {
    let mut v = Vec::new();
    if summary.blocked > 0 {
        v.push(DecisionNote {
            severity: FieldClass::Blocked,
            message: format!("{} blocked change(s) — data loss / lossy coercion", summary.blocked),
        });
    }
    if summary.needs_backfill > 0 {
        v.push(DecisionNote {
            severity: FieldClass::NeedsBackfill,
            message: format!("{} change(s) need a data backfill", summary.needs_backfill),
        });
    }
    if summary.warn > 0 {
        v.push(DecisionNote {
            severity: FieldClass::Warn,
            message: format!("{} change(s) worth review", summary.warn),
        });
    }
    v
}

/// Pure policy: `(environment, classification summary, acknowledgments)` → decision.
///
/// **Inert in Phase 3a — nothing calls this yet.**
///
/// - `Safe` / nothing classified → `Allow`.
/// - `Warn` worst → `Warn` (proceed + surface), in every environment.
/// - `NeedsBackfill` / `Blocked` worst → `Refuse` in a strict (prod) environment
///   unless `acknowledge` is set; `Warn` (permissive apply) otherwise.
pub fn decide(env: Environment, summary: &ClassSummary, opts: &DecisionOptions) -> BootDecision {
    match summary.worst() {
        None | Some(FieldClass::Safe) => BootDecision::Allow,
        Some(FieldClass::Warn) => BootDecision::Warn(notes(summary)),
        Some(FieldClass::NeedsBackfill) | Some(FieldClass::Blocked) => {
            if env.is_strict() && !opts.acknowledge {
                BootDecision::Refuse(notes(summary))
            } else {
                BootDecision::Warn(notes(summary))
            }
        }
    }
}

/// What a startup gate may do automatically. **Boot's analog of [`decide`], but
/// stricter and severity-based** — boot is automatic, so it auto-applies only the
/// Safe/Warn tier and only in a permissive environment, and *never* drops /
/// NeedsBackfill / Blocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootAction {
    /// Nothing pending — boot as-is.
    Proceed,
    /// Permissive env + only Safe/Warn + no drops — auto-apply, then boot.
    Converge,
    /// Do not auto-apply, and do not proceed (the caller aborts boot).
    Refuse,
}

/// Boot policy over the **non-drop** worst severity + a drops flag. Distinct from
/// [`decide`] on purpose: a Dev `Blocked` change is `decide → Warn` (proceed) but
/// `boot_action → Refuse` — boot must not silently perform what only an explicit
/// `apply` should.
pub fn boot_action(env: Environment, non_drop: &ClassSummary, has_drops: bool) -> BootAction {
    // Nothing pending at all.
    if non_drop.worst().is_none() && !has_drops {
        return BootAction::Proceed;
    }
    // Prod never auto-applies: any pending change refuses boot.
    if env.is_strict() {
        return BootAction::Refuse;
    }
    // Dev / Test: never auto-apply destructive drops.
    if has_drops {
        return BootAction::Refuse;
    }
    match non_drop.worst() {
        None => BootAction::Proceed,
        Some(FieldClass::Safe) | Some(FieldClass::Warn) => BootAction::Converge,
        Some(FieldClass::NeedsBackfill) | Some(FieldClass::Blocked) => BootAction::Refuse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(safe: usize, warn: usize, needs_backfill: usize, blocked: usize) -> ClassSummary {
        ClassSummary { safe, warn, needs_backfill, blocked }
    }

    fn ack() -> DecisionOptions {
        DecisionOptions { acknowledge: true }
    }
    fn no_ack() -> DecisionOptions {
        DecisionOptions { acknowledge: false }
    }

    const ENVS: [Environment; 3] = [Environment::Dev, Environment::Test, Environment::Prod];

    #[test]
    fn empty_summary_allows_everywhere() {
        for env in ENVS {
            assert_eq!(decide(env, &summary(0, 0, 0, 0), &no_ack()), BootDecision::Allow);
        }
    }

    #[test]
    fn all_safe_allows_everywhere() {
        for env in ENVS {
            assert_eq!(decide(env, &summary(5, 0, 0, 0), &no_ack()), BootDecision::Allow);
        }
    }

    #[test]
    fn warn_is_warn_never_refuse() {
        for env in ENVS {
            let d = decide(env, &summary(1, 2, 0, 0), &no_ack());
            assert!(!d.is_refuse(), "warn must never refuse in {env:?}");
            assert!(matches!(d, BootDecision::Warn(_)));
        }
    }

    #[test]
    fn prod_refuses_blocked_without_ack() {
        let d = decide(Environment::Prod, &summary(1, 0, 0, 2), &no_ack());
        assert!(d.is_refuse());
        assert!(d.notes().iter().any(|n| n.severity == FieldClass::Blocked));
    }

    #[test]
    fn prod_refuses_needs_backfill_without_ack() {
        assert!(decide(Environment::Prod, &summary(0, 0, 1, 0), &no_ack()).is_refuse());
    }

    #[test]
    fn prod_blocked_with_ack_warns_not_refuses() {
        let d = decide(Environment::Prod, &summary(0, 0, 0, 3), &ack());
        assert!(!d.is_refuse());
        assert!(matches!(d, BootDecision::Warn(_)));
    }

    #[test]
    fn dev_and_test_warn_blocked_never_refuse() {
        for env in [Environment::Dev, Environment::Test] {
            let d = decide(env, &summary(0, 0, 1, 2), &no_ack());
            assert!(!d.is_refuse(), "permissive env {env:?} must not refuse");
            assert!(matches!(d, BootDecision::Warn(_)));
        }
    }

    /// The full severity × environment matrix (no acknowledgment).
    #[test]
    fn severity_environment_matrix() {
        // (worst-severity summary, env) -> is_refuse
        let cases: &[(ClassSummary, Environment, bool)] = &[
            (summary(0, 0, 0, 0), Environment::Prod, false), // empty
            (summary(3, 0, 0, 0), Environment::Prod, false), // safe
            (summary(0, 1, 0, 0), Environment::Prod, false), // warn
            (summary(0, 0, 1, 0), Environment::Prod, true),  // needs-backfill
            (summary(0, 0, 0, 1), Environment::Prod, true),  // blocked
            (summary(0, 0, 1, 0), Environment::Dev, false),
            (summary(0, 0, 0, 1), Environment::Dev, false),
            (summary(0, 0, 1, 0), Environment::Test, false),
            (summary(0, 0, 0, 1), Environment::Test, false),
        ];
        for (s, env, expect_refuse) in cases {
            assert_eq!(
                decide(*env, s, &no_ack()).is_refuse(),
                *expect_refuse,
                "summary={s:?} env={env:?}"
            );
        }
    }

    #[test]
    fn worst_severity_drives_the_decision() {
        // A summary mixing safe + blocked refuses in prod on the blocked tier.
        let d = decide(Environment::Prod, &summary(10, 1, 0, 1), &no_ack());
        assert!(d.is_refuse());
    }

    #[test]
    fn environment_defaults_to_prod_fail_closed() {
        assert_eq!(Environment::default(), Environment::Prod);
        assert!(Environment::Prod.is_strict());
        assert!(!Environment::Dev.is_strict());
        assert!(!Environment::Test.is_strict());
    }

    // ---- boot_action: the (stricter, severity-based) startup gate ----

    #[test]
    fn boot_action_nothing_pending_proceeds_everywhere() {
        for env in ENVS {
            assert_eq!(boot_action(env, &summary(0, 0, 0, 0), false), BootAction::Proceed);
        }
    }

    #[test]
    fn boot_action_prod_refuses_any_pending() {
        // Even all-Safe pending refuses in prod (prod never auto-applies).
        assert_eq!(boot_action(Environment::Prod, &summary(3, 0, 0, 0), false), BootAction::Refuse);
        assert_eq!(boot_action(Environment::Prod, &summary(0, 1, 0, 0), false), BootAction::Refuse);
        assert_eq!(boot_action(Environment::Prod, &summary(0, 0, 0, 0), true), BootAction::Refuse);
    }

    #[test]
    fn boot_action_dev_test_converge_only_safe_warn_no_drops() {
        for env in [Environment::Dev, Environment::Test] {
            assert_eq!(boot_action(env, &summary(2, 0, 0, 0), false), BootAction::Converge);
            assert_eq!(boot_action(env, &summary(0, 1, 0, 0), false), BootAction::Converge);
        }
    }

    #[test]
    fn boot_action_refuses_risky_tiers_in_all_envs() {
        for env in ENVS {
            // NeedsBackfill, Blocked, and drops are never auto-applied at boot.
            assert_eq!(boot_action(env, &summary(0, 0, 1, 0), false), BootAction::Refuse, "needs-backfill {env:?}");
            assert_eq!(boot_action(env, &summary(0, 0, 0, 1), false), BootAction::Refuse, "blocked {env:?}");
            assert_eq!(boot_action(env, &summary(5, 0, 0, 0), true), BootAction::Refuse, "drops {env:?}");
        }
    }
}
