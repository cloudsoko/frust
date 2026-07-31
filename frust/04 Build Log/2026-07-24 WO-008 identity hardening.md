---
tags: [frust, build-log, security, identity, work-order]
created: 2026-07-24
work-order: "[[WO-008 Identity Hardening]]"
---

# Build Log — WO-008: Identity Hardening (the `$auth` Sharp Edge)

The family — identity resolution failing quiet — treated as a family: the posture that makes `$auth` resolve is now binary-authoritative, the one place NULL could still slip in throws a machine code, and the clauses that compared NULLs can no longer grant.

## Exit criteria

| # | Criterion | Evidence | Result |
|---|---|---|---|
| 1 | `app_user` DDL kernel-owned | `meta.rs::identity_ddl()` — **meta v2**; boot **re-asserts it every boot**, not just on migration | ✅ drift has no standing: repaired at next boot; posture ships from the binary |
| 2 | NULL-identity fails typed | `identity_hardening::drift_scenario_fails_typed_and_boot_repairs` | ✅ exact drift scenario: `PERMISSIONS NONE` forced → write refuses with `E_IDENTITY_UNRESOLVED` (typed `BrokerError::IdentityUnresolved`), zero rows stored → boot repairs → stamping resumes |
| 3 | `NONE = NONE` can never grant | `identity_hardening::null_owner_rows_invisible_to_record_principals` | ✅ compiled clause is `(owner != NONE AND owner = $auth.id) OR $auth.role = 'manager'`; a root-seeded NULL-owner row is invisible to every record principal; managers see it via the ROLE clause, which is the design |
| 4 | EVENT-bypass canary pinned | `event_bypass_canary.rs` | ✅ both directions asserted (EVENT writes bypass; direct writes filtered), failure messages name the ADR-010 dependency and the re-ruling obligation |
| 5 | Family sweep | table below | ✅ every `$auth` touchpoint proven loud or fixed |

## What was built

- **`meta.rs` — identity is meta (v2).** `identity_ddl()`: `app_user` SCHEMAFULL, `FOR select WHERE id = $auth.id` (self-select — the clause that makes `$auth.id`/`$auth.role` resolve in DEFAULT/query contexts), record writes closed, and the `pass` field `PERMISSIONS NONE` — the hash is unreadable even to its owner. `access_ddl()` is `IF NOT EXISTS`, deliberately not OVERWRITE: re-defining the access mints a fresh JWT key and would invalidate every live session per boot; absence is loud (signin fails), so repair isn't needed there.
- **`boot.rs` — repair every boot.** The identity DDL re-asserts on the no-op path too, not just migrations. Records survive (Finding A). `acceptance_e2e` now demonstrates this live: its hand-rolled drifted posture gets silently repaired by its own boot call.
- **`sync.rs` — the guard and the clause.** Every synced table gets `identity_guard`: a CREATE under a record session (`$auth != NONE`) whose `owner` stamped NONE throws `FRUST:E_IDENTITY_UNRESOLVED` — surfaced as a typed contract variant (additive, per ADR-006 evolution policy). The select clause is null-safe. Root/system writes remain legitimately NULL-owner ($auth absent, guard skips) — and criterion 3 makes those rows *manager-only* instead of *visible-to-all*, which closes the module-3 root-`$auth` caveat's consequence.

## The named number-check (25 ms floor)

- `identity_guard` write cost, A/B at the DB: **3.6 ms with vs 3.8 ms without — pure noise.** The null-safe clause is read-side only (one extra NONE comparison per scanned row).
- Release gate after the change: **22/25/24 ms across three runs — passing at the floor.** No security/latency trade to report.
- **Incident during verification, resolved:** the gate first read 31→43 ms, degrading run-over-run. Cause was *instance-session degradation*, not WO-008 — the dev surrealkv process had absorbed hundreds of `REMOVE/DEFINE DATABASE` cycles today; a restart restored 22-25 ms immediately. The gated table carries none of the new DDL, and the guard A/B confirms the write path is untouched. Recorded for [[SurrealDB]] caveats: **long-lived dev instances degrade write latency under database-churn; restart restores.** (Production doesn't churn databases; the test harness does.)

## Family sweep — every `$auth` touchpoint

| Touchpoint | Context | NULL behavior | Verdict |
|---|---|---|---|
| `owner DEFAULT $auth.id` (sync.rs, every synced table) | field DEFAULT — needs app_user self-select to resolve | was: silent NULL stamp | **FIXED** — guard throws `E_IDENTITY_UNRESOLVED`; posture kernel-owned + boot-repaired |
| `owner = $auth.id` select clause (sync.rs) | permission | was: `NONE = NONE` grants to all | **FIXED** — null-safe `(owner != NONE AND …)` |
| `$auth.role = 'manager'` (sync.rs select/update/delete, rollup targets) | permission | `NONE = 'manager'` is false | proven safe — NULL denies |
| `$auth.id != NONE` create clause (sync.rs) | permission | NULL denies (anon can't create) | proven safe — deny-direction |
| `agg_cursor` select `$auth.id != NONE` (aggregates.rs) | permission | NULL denies | proven safe |
| `id = $auth.id` app_user self-select (meta.rs) | permission | stored `id` is never NONE → NULL denies | proven safe; this clause is also the resolver for row 1 |
| `identity_guard` / lattice `$auth != NONE` (sync.rs) | EVENT body | root/system sessions skip the guard by design | intentional — system rows are NULL-owner AND manager-only (row 2's fix pairs with this) |
| `AppUserResolver` (worker.rs) | root read, not `$auth` | missing/disabled principal → `None` → typed non-retryable `Denied` | proven loud (module 5) |
| *Adjacent, out of scope, flagged:* REST `X-Frust-Role` header | feeds the FIELD-envelope half of the permission compiler | row half is DB-enforced regardless; a spoofed header widens field visibility on rows the DB already grants | **Desk v1 WO input**: replace header trust with session-token-derived role |

## Operator note

Existing databases (skeleton included) sit at meta v1; the next boot refuses with `E_META_MIGRATION_PENDING` until run with `--accept-meta-migrations` — the two-step ack working as designed. The v1→v2 apply OVERWRITEs the access definition once (fresh JWT key → live sessions invalidated at migration; a one-time, migration-scoped event, not a per-boot one).

## Housekeeping
- Full suite green: 19 test binaries, including the two new files and both perf gates (debug 60 ms / release 25 ms).
- v3.2.0 permission-context asymmetry (permission clauses resolve `$auth` privileged; DEFAULT/query contexts require self-select) confirmed load-bearing and now *designed for* rather than stumbled over.

## Related
[[WO-008 Identity Hardening]] · [[ADR-008 Data Shape]] · [[ADR-010 Materialized Aggregates]] · [[2026-07-24 WO-007 aggregates ladder implementation]] · [[SurrealDB]]
