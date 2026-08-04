//! The App manifest.
//!
//! An App is a **versioned bundle**: DocTypes, client scripts, server scripts,
//! route declarations, `.wasm` components, aggregate declarations (which ride
//! inside their DocTypes), and a reserved slot for workflows.
//!
//! Two properties this module exists to hold:
//!
//! 1. **Validated before anything applies.** Validation collects *every*
//!    problem rather than stopping at the first, exactly as
//!    `MigrationReport::errors` does — an operator fixing a bundle should see
//!    the whole list, not peel it one error per attempt.
//!
//! 2. **The gate discipline is reused literally, not analogously.** A bundle's
//!    DocTypes become `ResourceSpec`s and go through the *same*
//!    `ResourceMigrator` with the *same* `MigrationOptions` that REQ-6.6
//!    already governs. Dry-run-before-install is not a new feature; it is
//!    REQ-6.6.1 machinery surfaced as UX. There is no second migration path,
//!    for the same reason there is no second permission compiler.
//!
//! Like `routes.rs`, this module is **absent from `surql_monopoly`'s
//! allowlist**: a manifest is data, and turning it into schema is the sync
//! engine's job, not this module's.

use serde::{Deserialize, Serialize};

use crate::contract::BrokerError;
use crate::db::Db;
use crate::sync::{doctype_spec_in, load_doctypes, DocTypeDef, KernelConns, StrategyTenancy};
use crate::tenancy::ResolvedTenant;

use frust_orm::{EngineCtx, MigrationOptions, MigrationReport, ResourceMigrator};

/// Bump only for a breaking manifest change; unknown versions are refused at
/// validation, never half-applied.
pub const MANIFEST_VERSION: i64 = 1;

/// Route names the lifecycle surface already owns under `/app/{app}/…`.
pub const RESERVED_ROUTE_PATHS: [&str; 2] = ["enable", "disable"];

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    pub manifest_version: i64,
    /// App id: an identifier, because it namespaces everything it owns.
    pub name: String,
    /// `MAJOR.MINOR.PATCH`.
    pub version: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub doctypes: Vec<DocTypeDef>,
    #[serde(default)]
    pub client_scripts: Vec<ScriptDecl>,
    #[serde(default)]
    pub server_scripts: Vec<ScriptDecl>,
    #[serde(default)]
    pub routes: Vec<RouteDecl>,
    /// `.wasm` filenames the bundle ships, relative to its artifacts dir.
    #[serde(default)]
    pub components: Vec<String>,
    /// The workflows this app installs.
    #[serde(default)]
    pub workflows: Vec<crate::workflow::WorkflowDef>,
    /// **What this app adds to DocTypes it does NOT own.**
    ///
    /// Kept separate from `doctypes` on purpose: `doctypes` means "I ship and
    /// own this", `extends` means "I add to someone else's". Conflating them is
    /// how an extension quietly becomes an owner.
    #[serde(default)]
    pub extends: Vec<ExtensionDecl>,
}

/// One app's extension of a DocType another app owns.
///
/// Additive by construction: fields and a hook, nothing that can remove or
/// replace what the owner declared. The owner's hook always runs first and
/// cannot be displaced — that is enforced in the dispatcher, not here.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExtensionDecl {
    /// The DocType being extended (owned by another app, or by the kernel).
    pub doctype: String,
    /// Namespaced fields this extension adds. **Namespacing is enforced**, not
    /// advised: a collision between two apps' fields is refused at install
    /// naming both, and a prefix is what makes that refusal rare enough to be
    /// an error rather than a fact of life.
    #[serde(default)]
    pub fields: Vec<crate::sync::FieldDef>,
    /// The lifecycle point. `validate` only, as for owned scripts — the wider
    /// hook vocabulary is deferred so composition lands on one hook first.
    #[serde(default = "default_hook")]
    pub hook: String,
    /// The script text. Optional: an extension may add fields only.
    #[serde(default)]
    pub script: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScriptDecl {
    pub doctype: String,
    /// The lifecycle point. Only `validate` exists today (the engine exports
    /// no other), so anything else is refused rather than silently ignored.
    #[serde(default = "default_hook")]
    pub hook: String,
    pub script: String,
}

/// The subscribable vocabulary, rendered for a refusal message. Sourced from
/// `HookClass::SUBSCRIBABLE` so the door and the dispatcher can never disagree
/// about what exists.
fn known_hooks() -> String {
    crate::contract::HookClass::SUBSCRIBABLE
        .iter()
        .map(|c| format!("'{}'", c.wire()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn default_hook() -> String {
    "validate".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RouteDecl {
    /// Served under `/app/{app}/{path}` — the app name namespaces it, so two
    /// apps cannot collide however carelessly they are written.
    pub path: String,
    /// Which shipped component handles it.
    pub component: String,
}

/// What a bundle would do, before it does anything.
///
/// `schema` is the sync engine's own dry-run report — the same structure a
/// plain metadata sync produces, carrying planned DDL and destructive flags.
#[derive(Debug)]
pub struct InstallPlan {
    pub app: String,
    pub version: String,
    pub schema: MigrationReport,
    /// Non-schema attachments, listed so the preview is the whole truth and
    /// not just its DDL half.
    pub client_scripts: Vec<String>,
    pub server_scripts: Vec<String>,
    pub routes: Vec<String>,
    /// Reserved-slot passthrough count.
    pub workflows: usize,
}

impl InstallPlan {
    /// Destructive statements the schema half would perform, flattened for
    /// display. Non-empty means the install needs an explicit acknowledgment
    /// under prod strictness (REQ-6.6.2).
    pub fn destructive(&self) -> Vec<String> {
        self.schema.planned.iter().flat_map(|p| p.destructive.clone()).collect()
    }
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_semver(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Ordering for update detection (criterion 3). Returns `None` if either side
/// is not a well-formed version.
pub fn version_gt(a: &str, b: &str) -> Option<bool> {
    let nums = |s: &str| -> Option<Vec<u64>> {
        if !is_semver(s) {
            return None;
        }
        s.split('.').map(|p| p.parse().ok()).collect()
    };
    Some(nums(a)? > nums(b)?)
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, BrokerError> {
        serde_json::from_str(text)
            .map_err(|e| BrokerError::InvalidValue { detail: format!("bad manifest: {e}") })
    }

    /// Every problem at once. An empty list means the bundle is structurally
    /// installable; it does NOT mean the schema change is safe — that is the
    /// migration gate's job, and it runs next.
    pub fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();
        if self.manifest_version != MANIFEST_VERSION {
            errs.push(format!(
                "manifest_version {} is not supported (this kernel speaks {MANIFEST_VERSION})",
                self.manifest_version
            ));
        }
        if !is_ident(&self.name) {
            errs.push(format!("app name '{}' is not an identifier", self.name));
        }
        if !is_semver(&self.version) {
            errs.push(format!("version '{}' is not MAJOR.MINOR.PATCH", self.version));
        }

        let mut seen = std::collections::BTreeSet::new();
        for dt in &self.doctypes {
            if !is_ident(&dt.name) {
                errs.push(format!("doctype '{}' is not an identifier", dt.name));
            }
            if !seen.insert(dt.name.clone()) {
                errs.push(format!("doctype '{}' is declared twice", dt.name));
            }
            for f in &dt.fields {
                if !is_ident(&f.fieldname) {
                    errs.push(format!("{}.{} is not an identifier", dt.name, f.fieldname));
                }
            }
        }

        // A script or route pointing at a doctype the bundle does not ship is
        // the classic half-installed app; caught here, not at first use.
        let owns = |d: &str| self.doctypes.iter().any(|dt| dt.name == d);
        for (kind, list) in [("client_script", &self.client_scripts), ("server_script", &self.server_scripts)] {
            for s in list.iter() {
                if !owns(&s.doctype) {
                    errs.push(format!("{kind} targets '{}', which this bundle does not declare", s.doctype));
                }
                // The vocabulary is the door. A hook point that does not
                // exist is refused HERE, at install, with the list of the ones
                // that do — never accepted and then silently never fired, which
                // is the failure a typo would otherwise buy you.
                if crate::contract::HookClass::from_wire(&s.hook).is_none() {
                    errs.push(format!(
                        "{kind} on '{}' uses hook '{}'; known hooks are {}",
                        s.doctype,
                        s.hook,
                        known_hooks()
                    ));
                }
                if s.script.trim().is_empty() {
                    errs.push(format!("{kind} on '{}' is empty", s.doctype));
                }
            }
        }

        // ── extension declarations ──
        //
        // Validated with the same discipline as everything else: a
        // bundle that cannot be installed says so completely, in one pass, at
        // the door. The `owns` predicate above deliberately does NOT apply to
        // these — extending someone else's DocType is the whole point — so
        // these checks are what stands in its place.
        let owns_ext = |d: &str| self.doctypes.iter().any(|dt| dt.name == d);
        for e in &self.extends {
            if !is_ident(&e.doctype) {
                errs.push(format!("extends targets '{}', which is not an identifier", e.doctype));
            }
            if owns_ext(&e.doctype) {
                errs.push(format!(
                    "extends targets '{}', which this bundle ALSO declares — own it or extend                      it, not both",
                    e.doctype
                ));
            }
            if crate::contract::HookClass::from_wire(&e.hook).is_none() {
                errs.push(format!(
                    "extension on '{}' uses hook '{}'; known hooks are {}",
                    e.doctype,
                    e.hook,
                    known_hooks()
                ));
            }
            if e.script.trim().is_empty() && e.fields.is_empty() {
                errs.push(format!(
                    "extension on '{}' adds nothing — no fields and no script",
                    e.doctype
                ));
            }
            // **Namespacing is enforced, not advised.** Two apps adding `note`
            // to the same DocType is a collision that has to be refused; two
            // apps adding `crm_note` and `bill_note` never collide at all. The
            // prefix is what turns a routine conflict into a real error.
            let prefix = format!("{}_", self.name);
            for f in &e.fields {
                if !is_ident(&f.fieldname) {
                    errs.push(format!("extension field '{}' is not an identifier", f.fieldname));
                } else if !f.fieldname.starts_with(&prefix) {
                    errs.push(format!(
                        "extension field '{}' on '{}' must be namespaced '{}…' — an app may                          only add fields under its own name",
                        f.fieldname, e.doctype, prefix
                    ));
                }
            }
        }

        for r in &self.routes {
            if !is_ident(&r.path) {
                errs.push(format!("route path '{}' is not an identifier", r.path));
            }
            // `/app/{app}/{path}` shares its shape with the lifecycle verbs.
            // Refuse the collision at validation rather than let a route be
            // silently shadowed by `disable` for the rest of its life.
            if RESERVED_ROUTE_PATHS.contains(&r.path.as_str()) {
                errs.push(format!(
                    "route path '{}' is reserved by the app lifecycle surface",
                    r.path
                ));
            }
            if !self.components.contains(&r.component) {
                errs.push(format!(
                    "route '{}' names component '{}', which the bundle does not ship",
                    r.path, r.component
                ));
            }
        }
        for c in &self.components {
            if !c.ends_with(".wasm") || c.contains('/') || c.contains('\\') || c.contains("..") {
                errs.push(format!("component '{c}' must be a bare .wasm filename"));
            }
        }

        // A workflow whose transitions name states it never declared
        // is the half-installed app again, one layer up. Caught here, not on
        // the first click.
        for w in &self.workflows {
            if !owns(&w.doctype) {
                errs.push(format!(
                    "workflow '{}' governs '{}', which this bundle does not declare",
                    w.name, w.doctype
                ));
            }
            if w.states.is_empty() {
                errs.push(format!("workflow '{}' declares no states", w.name));
            }
            let known: Vec<&str> = w.states.iter().map(|s| s.name.as_str()).collect();
            for t in &w.transitions {
                for (which, s) in [("from", &t.from), ("to", &t.to)] {
                    if !known.contains(&s.as_str()) {
                        errs.push(format!(
                            "workflow '{}': transition '{}' names an undeclared {which}-state '{s}'",
                            w.name, t.action
                        ));
                    }
                }
                if t.role.trim().is_empty() {
                    errs.push(format!(
                        "workflow '{}': transition '{}' has no role — an ungated transition is not a workflow",
                        w.name, t.action
                    ));
                }
            }
            for r in &w.state_rules {
                if !known.contains(&r.state.as_str()) {
                    errs.push(format!(
                        "workflow '{}': state rule targets undeclared state '{}'",
                        w.name, r.state
                    ));
                }
            }
        }
        errs
    }

    /// REQ-6.6.1 as UX: what installing this bundle *would* do, computed
    /// without taking a lock or touching schema.
    ///
    /// This is the sync engine's own dry-run, not a reimplementation — the
    /// bundle's DocTypes become the same `ResourceSpec`s a plain sync builds,
    /// and the same migrator classifies them.
    pub fn plan(&self, base: &ResolvedTenant, db: &Db) -> Result<InstallPlan, BrokerError> {
        let errs = self.validate();
        if !errs.is_empty() {
            return Err(BrokerError::InvalidValue {
                detail: format!("bundle is not installable:\n- {}", errs.join("\n- ")),
            });
        }
        self.plan_unchecked(base, db, MigrationOptions::dry_run_for(Default::default()))
    }

    /// The plan/apply core. `opts` decides which it is: the *same* call shape
    /// for preview and install is what keeps "what you were shown" and "what
    /// ran" from drifting apart.
    pub fn plan_unchecked(
        &self,
        base: &ResolvedTenant,
        db: &Db,
        opts: MigrationOptions,
    ) -> Result<InstallPlan, BrokerError> {
        // Rollup targets come from the whole world, not just this bundle: an
        // app may declare an aggregate onto a rollup that already exists.
        let existing = load_doctypes(db).unwrap_or_default();
        let rollup_targets: Vec<String> = existing
            .iter()
            .chain(self.doctypes.iter())
            .flat_map(|dt| dt.aggregates.iter().map(|a| a.rollup.clone()))
            .collect();

        let mut specs = Vec::new();
        for dt in &self.doctypes {
            specs.push(doctype_spec_in(dt, &rollup_targets)?);
        }

        let conns = KernelConns { base: base.clone() };
        let tenancy = StrategyTenancy { target: base.clone() };
        let ctx = EngineCtx { conns: &conns, tenancy: &tenancy, bootstrap_sql: None };
        let migrator = ResourceMigrator::with_holder(format!("app-{}", self.name));
        let schema = migrator
            .migrate_tenant_with(&ctx, &specs, "default", opts)
            .map_err(|e| BrokerError::Db { detail: format!("bundle migration: {e}") })?;

        Ok(InstallPlan {
            app: self.name.clone(),
            version: self.version.clone(),
            schema,
            client_scripts: self.client_scripts.iter().map(|s| s.doctype.clone()).collect(),
            server_scripts: self.server_scripts.iter().map(|s| s.doctype.clone()).collect(),
            routes: self.routes.iter().map(|r| format!("/app/{}/{}", self.name, r.path)).collect(),
            workflows: self.workflows.len(),
        })
    }
}
