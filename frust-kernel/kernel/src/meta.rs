//! The hardcoded minimal meta-schema (ADR-008): what the kernel needs to
//! exist before it can read anything else. For meta-tables the BINARY is
//! authoritative — sync is one-way up, and a DB that claims a newer meta
//! version than the binary refuses boot (fail-closed).
//!
//! Meta-DocTypes are closed to customization (ADR-008 A6): extensions
//! contribute their own records, never mutations of these definitions.

/// Bump on any change to `META_DDL`. DB > binary => refuse boot.
/// v2: identity is meta (WO-008) — `app_user` + the record access ship from
/// the binary.
/// v3: the app registry (WO-019) — `installed_app`, changefeed-audited.
/// v4: workflow definitions (WO-018) — `workflow`, metadata like DocTypes.
/// v5: email batteries (WO-043) — `notification` rules + the `_frust_mail`
/// outbox, and `email` on `app_user` (a `role:` recipient needs an address to
/// resolve to; SCHEMAFULL means the column must be declared, not assumed).
/// v6: the session table (WO-047) — `_frust_session` moves here from a lazy
/// `DEFINE TABLE IF NOT EXISTS` inside the login batch, which a whole-batch
/// conflict retry could turn into a duplicate session row.
/// v7: cross-app extension (WO-050) — `doctype.server_script` becomes a LIST of
/// `{app, script}` so more than one app can contribute a hook. See
/// `meta_data_migrations`.
pub const META_SCHEMA_VERSION: i64 = 8;

/// Version + application log records live here.
pub const META_TABLE: &str = "_frust_meta";
/// Boot advisory lock (holder-scoped release; stale takeover).
pub const BOOT_LOCK_TABLE: &str = "_frust_boot_lock";

/// WO-008 criterion 1: the identity posture, kernel-owned. Self-select ONLY
/// (`id = $auth.id` — the clause that makes `$auth.id`/`$auth.role` resolve
/// in DEFAULT/query contexts, v3.2.0 caveat); the hash is unreadable even to
/// its owner; record writes closed. Re-asserted on EVERY boot, not just meta
/// migrations — operator DDL drift on identity has no standing (ADR-008
/// binary-authoritative, extended). OVERWRITE preserves records (Finding A).
pub fn identity_ddl() -> String {
    [
        "DEFINE TABLE OVERWRITE app_user SCHEMAFULL \
         PERMISSIONS FOR select WHERE id = $auth.id FOR create, update, delete NONE",
        "DEFINE FIELD OVERWRITE name ON app_user TYPE string",
        "DEFINE FIELD OVERWRITE role ON app_user TYPE string",
        "DEFINE FIELD OVERWRITE pass ON app_user TYPE string PERMISSIONS NONE",
        "DEFINE FIELD OVERWRITE status ON app_user TYPE option<string>",
        // WO-043: `option<string>`, so every pre-v5 user keeps working. A
        // notification addressed to a role whose users have no email resolves
        // to ZERO recipients, which dead-letters loudly rather than sending
        // nothing quietly (`mail::Unroutable`).
        "DEFINE FIELD OVERWRITE email ON app_user TYPE option<string>",
    ]
    .join(";\n")
}

/// The record access, kernel-owned. IF NOT EXISTS (not OVERWRITE): an
/// OVERWRITE would mint a fresh JWT key and invalidate every live session on
/// each boot; absence is loud (signin fails), so repair-on-boot isn't needed.
pub fn access_ddl() -> String {
    "DEFINE ACCESS IF NOT EXISTS account ON DATABASE TYPE RECORD \
     SIGNIN (SELECT * FROM app_user WHERE name = $name AND crypto::argon2::compare(pass, $pass)) \
     DURATION FOR TOKEN 1h, FOR SESSION 12h"
        .to_string()
}

/// WO-027: the remediation an operator must run after restoring from a
/// `surreal export` dump, printed by the `FRUST:E_RESTORED_ACCESS_KEY` boot
/// refusal (ADR-013).
///
/// Lives here because DDL lives here — `keyguard.rs` detects the condition but
/// must not carry query text (the `surql_monopoly` invariant caught exactly
/// that and was right to). Fully static, with `<...>` placeholders the operator
/// fills: nothing dynamic enters this module.
///
/// **`OVERWRITE`, not `IF NOT EXISTS`** — the latter is what `access_ddl()`
/// above uses so ordinary boots never rotate the key and never sign everyone
/// out, and it is therefore a NO-OP against an already-defined placeholder
/// access. A remediation that used it would succeed and fix nothing.
pub fn restored_key_remediation_ddl() -> &'static str {
    "DEFINE ACCESS OVERWRITE <access-name> ON DATABASE TYPE RECORD \
     SIGNIN (SELECT * FROM app_user WHERE name = $name AND crypto::argon2::compare(pass, $pass)) \
     WITH JWT ALGORITHM HS512 KEY '<a fresh high-entropy secret>' \
     DURATION FOR TOKEN 1h, FOR SESSION 12h;"
}

/// WO-013: tenant quota policy is meta too — kernel-owned, binary-authoritative
/// (same discipline as identity). Absent rows mean unlimited, so a deployment
/// that never writes one keeps the pre-WO-013 posture exactly.
pub fn tenant_policy_ddl() -> String {
    let t = crate::fairness::POLICY_TABLE;
    [
        format!("DEFINE TABLE OVERWRITE {t} SCHEMALESS PERMISSIONS NONE"),
        format!("DEFINE FIELD OVERWRITE tenant ON {t} TYPE string"),
        format!("DEFINE FIELD OVERWRITE verbs_per_sec ON {t} TYPE option<float>"),
        format!("DEFINE FIELD OVERWRITE verb_burst ON {t} TYPE option<float>"),
        format!("DEFINE FIELD OVERWRITE jobs_per_round ON {t} TYPE option<int>"),
    ]
    .join(";\n")
}

/// The installed-app registry table.
pub const APP_TABLE: &str = "installed_app";

/// WO-019 criterion 3: the app registry, kernel-owned meta.
///
/// **CHANGEFEED is the audit trail.** Install, enable, disable and update are
/// all writes to this table, so "every action audited" costs nothing extra and
/// — more importantly — cannot be bypassed by whoever performs the action
/// (ADR-002 §7: changefeeds are unbypassable). An audit log the actor can skip
/// is the thing this project already refused once (P-5.4).
///
/// `manifest` holds the bundle verbatim. That is what makes disable reversible
/// without data loss: the script text a disable detaches is still on record, so
/// enable restores exactly what was there rather than a reconstruction.
///
/// PERMISSIONS NONE: the registry is meta, reached only through the manager
/// surface — record users never see it.
pub fn app_registry_ddl() -> String {
    [
        format!("DEFINE TABLE OVERWRITE {APP_TABLE} SCHEMALESS CHANGEFEED 30d PERMISSIONS NONE"),
        format!("DEFINE FIELD OVERWRITE name ON {APP_TABLE} TYPE string"),
        format!("DEFINE FIELD OVERWRITE version ON {APP_TABLE} TYPE string"),
        format!("DEFINE FIELD OVERWRITE enabled ON {APP_TABLE} TYPE bool"),
        format!("DEFINE FIELD OVERWRITE manifest ON {APP_TABLE} TYPE string"),
        format!("DEFINE FIELD OVERWRITE installed_at ON {APP_TABLE} TYPE datetime"),
        format!("DEFINE FIELD OVERWRITE updated_at ON {APP_TABLE} TYPE option<datetime>"),
    ]
    .join(";\n")
}

/// WO-018: workflow definitions are metadata, like DocTypes. SCHEMALESS
/// because a workflow is a shape the kernel validates on read, not a table the
/// DB polices — and PERMISSIONS NONE because it is read through the broker's
/// own door, never by a record user directly.
pub fn workflow_ddl() -> String {
    "DEFINE TABLE OVERWRITE workflow SCHEMALESS PERMISSIONS NONE".to_string()
}

/// WO-043: notification rules are metadata, exactly like workflows — same
/// SCHEMALESS + PERMISSIONS NONE posture, same reason (the kernel validates the
/// shape on the way in and reads it through its own door). This table is what
/// makes "email is metadata, not code" true: adding a notification is a record,
/// so it needs no recompile and no restart.
///
/// The outbox is separate and deliberately NOT the `job` table. Two reasons,
/// both structural: `ResidentWorker::tick` claims every queued `job` regardless
/// of kind and denies the ones it does not recognise, so a mail job would race
/// the resident worker and lose; and `Worker::requeue` retries a transient
/// failure forever, while criterion 4 requires a BOUNDED retry with a
/// dead-letter. Different claim owner, different retry algebra, different table.
///
/// CHANGEFEED on the outbox: a delivery record is an audit trail nobody can
/// skip (ADR-002 §7) — "did the customer get the invoice email" is exactly the
/// question an ERP gets asked six months later.
pub fn mail_ddl() -> String {
    let out = crate::mail::OUTBOX_TABLE;
    [
        format!("DEFINE TABLE OVERWRITE {} SCHEMALESS PERMISSIONS NONE", crate::mail::NOTIFICATION_TABLE),
        format!("DEFINE TABLE OVERWRITE {out} SCHEMALESS CHANGEFEED 30d PERMISSIONS NONE"),
        // The enqueue timestamp is the DB's job, not the caller's. `CREATE …
        // CONTENT {…} SET x = time::now()` is a PARSE ERROR on v3.2.0 (measured,
        // not assumed), and threading a clock through the enqueue would put a
        // "remember to stamp it" step on a path where forgetting means an outbox
        // that cannot be ordered — so the column defaults itself.
        format!("DEFINE FIELD OVERWRITE enqueued_at ON {out} TYPE datetime DEFAULT time::now()"),
    ]
    .join(";\n")
}

/// **Data migrations that travel with the meta version (WO-050).**
///
/// `meta_ddl` reshapes *schema*; this reshapes *records the kernel owns*. It
/// runs inside `apply_meta`'s transaction, so the version bump and the data it
/// implies commit together — a bumped version over unmigrated data would leave
/// list-expecting code reading a scalar, which is the worst of the two failure
/// directions.
///
/// **Every statement here must be idempotent**, because a re-run is always
/// possible (a crashed boot, a racing node losing the lock) and because
/// idempotence is what makes "apply again" a safe answer rather than a gamble.
///
/// v7 — `server_script` scalar → `[{app, script}]`. **Recompute, not launder**
/// (ADR-008): the owner's entry is *derived from the record itself* — the
/// existing script text paired with the doctype's own `app` — never invented.
/// A doctype with no `app` (kernel-created, not owned by a bundle) yields an
/// entry with no `app` key, which is exactly what "the owner" means there.
/// Guarded by `type::is_string`, so records already migrated are untouched.
pub fn meta_data_migrations() -> String {
    [
        // v7 (WO-050): the scalar script became a per-app list.
        "UPDATE doctype SET server_script = [{ app: app, script: server_script }]          WHERE type::is_string(server_script)",
        // v8 (WO-053): each entry names its hook class. Everything written
        // before this WO subscribed to the only class that existed, so the
        // backfill is `validate` — stated rather than defaulted at read time,
        // because a stored entry with no hook is indistinguishable from one
        // whose hook was lost.
        // The guard is INSIDE the expression, not in a WHERE clause.
        // v3.2.0 evaluates the SET expression for every row and only *applies*
        // it to the ones WHERE matches — so `WHERE type::is_array(...)` still
        // runs `.map()` against the doctypes whose `server_script` is NONE, and
        // "no such method found for the none type" fails the whole migration.
        // Caught by the app-upgrade test, not by my first probe: that probe's
        // population was all-arrays, so every row matched and the guard was
        // never asked to do anything.
        "UPDATE doctype SET server_script = IF type::is_array(server_script) {            server_script.map(|$s|              IF $s.hook = NONE { object::from_entries(object::entries($s).append(['hook', 'validate'])) }              ELSE { $s })          } ELSE { server_script }",
    ]
    .join("; ")
}

/// Idempotent meta DDL (OVERWRITE; WO-002 Finding A: re-defines preserve
/// changefeed history). PERMISSIONS NONE: record users never see meta.
pub fn meta_ddl() -> String {
    [
        app_registry_ddl(),
        workflow_ddl(),
        mail_ddl(),
        // kernel bookkeeping
        format!("DEFINE TABLE OVERWRITE {META_TABLE} SCHEMALESS PERMISSIONS NONE"),
        format!("DEFINE TABLE OVERWRITE {BOOT_LOCK_TABLE} SCHEMALESS PERMISSIONS NONE"),
        // the DocType metadata store — the self-hosting table
        "DEFINE TABLE OVERWRITE doctype SCHEMALESS PERMISSIONS NONE".to_string(),
        // the job queue (ADR-009): jobs are records; queue state is data
        "DEFINE TABLE OVERWRITE job SCHEMALESS CHANGEFEED 1d PERMISSIONS NONE".to_string(),
        // WO-047: the session store. It is ephemeral state rather than
        // schema — which is exactly why it used to be created lazily by the
        // login batch — but a `DEFINE` sharing a statement batch with a
        // `CREATE` is retried as a unit on conflict, and SurrealDB commits
        // unbraced statements separately, so a DDL race duplicated the row the
        // CREATE had already written. Defining it once at boot removes the DDL
        // from the auth path entirely. OVERWRITE preserves existing rows
        // (WO-002 Finding A), so live sessions survive the migration.
        format!(
            "DEFINE TABLE OVERWRITE {} SCHEMALESS PERMISSIONS NONE",
            crate::rest::SESSION_TABLE
        ),
        // identity is meta (WO-008)
        identity_ddl(),
        access_ddl(),
    ]
    .join(";\n")
}
