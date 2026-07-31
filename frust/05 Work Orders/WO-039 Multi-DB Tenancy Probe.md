---
tags: [frust, work-order, tenancy, tenant, probe, milestone-4]
status: COMPLETE - PROBE CLEAN (2026-07-28). Headline: a tenant ALREADY IS a database (Db::tenant() = cfg.db) - ADR-003's model is the data model; only ROUTING is unbuilt (one process = one Db today). Isolation is DB-enforced in the strongest form (JWT ns/db claims pin the session; a caller cannot address another tenant's DB - forged token 401, no-auth 403). Per-tenant restore WORKS (export A, restore to fresh, B untouched incl. post-export write) = the P-8.1 unlock. Two findings for the build: (1) Broker takes Box<dyn HookDispatch> so N tenants = N wasmtime engines -> Arc<> fixes it, script pool is already (tenant,doctype)-keyed; (2) SESSION_GEN/META_GEN are process-global -> cross-tenant cache cold-start. NO ADR-003 amendment needed. Full build = WO-040. See [[2026-07-28 WO-039 multi-db tenancy probe]].
created: 2026-07-28
---

# WO-039: Multi-DB Per-Tenant Tenancy — The Probe

> [!info] PM work order — Milestone 4's opener, sequenced FIRST by asymmetric risk: its bad outcome is an ADR-003 amendment (SurrealDB per-DB limits, or a connection-model explosion that undoes the 60 MB memory win), where batteries/re-skin have flat cost-of-delay. **This is a PROBE, not the full tenancy build** — empirical-first, the WO-026/WO-027 discipline: answer whether per-tenant databases are clean *before* committing to the build. Governing: [[ADR-003 Tenancy Model]] (platform namespace → database-per-tenant was always the model; v0 shipped single-DB), [[2026-07-27 WO-027 backup restore DR]] (P-8.1: per-DB export → no per-tenant restore — the finding this unlocks), [[2026-07-26 WO-026 surrealdb write concurrency]] (the connection/cache model per-tenant would extend), [[SurrealDB]] (namespace→database hierarchy).

## The M4 anchor

v0 put all tenants in one SurrealDB database — single-DB tenancy. ADR-003 always envisioned **database-per-tenant**; it just wasn't built, and WO-027 measured the cost: no per-tenant restore, restore-one = restore-all. M4's scalability story turns on whether the kernel can cleanly run per-tenant databases. This probe answers that with a running two-tenant kernel, or surfaces the wall as an ADR-003 conversation — in daylight.

## Criterion 0 — establish the evidence-harness home first (closes the M4 prerequisite)

This WO adds a multi-tenant harness. Before it does: give the committed test artifacts a real home + a documented runner (the 4 in `wf-proof/` — workflow spec, SSE spec, SSE bench, Desk load driver — live outside any repo on an ad-hoc pnpm install). Move them into a proper location with a README/run script; this WO's tenancy harness lands there, not in `wf-proof/`. Small, but do it before adding the fifth.

## Criterion 1 — THE PROBE

Stand up **two tenants in separate SurrealDB databases** (the ADR-003 namespace→database model) under one kernel, and prove:

1. **Isolation is DB-enforced, not app-layer:** tenant A's principal cannot read tenant B's data — refused by the database (separate DB), not by a permission clause the kernel could bug. The strongest form of the tenancy guarantee.
2. **Per-tenant restore works — the P-8.1 unlock:** export tenant A's database, restore it into a fresh database, and tenant B is **untouched**. This is the single thing single-DB tenancy could not do (WO-027); proving it is the probe's headline.
3. **Request→tenant→DB routing surfaced:** show how a request resolves to the right tenant database — what the session/connection/permission-compiler model needs. WO-026's session + doctype caches are currently global; per-tenant they'd key on tenant. Characterize the change, don't necessarily build it all.

## Criterion 2 — characterize the full build (the probe's real deliverable)

State, with evidence from the probe, what the full tenancy build requires and what it costs:
- **Connection model:** N tenants = N databases. Does the kernel need N connection pools / N cache sets? **Measure the resource cost** — if per-tenant state explodes the 60 MB idle footprint (P-1.4, killed) linearly with tenant count, that's a finding, not a footnote.
- **Permission compiler & meta:** does the binary-authoritative meta boot (ADR-008) run per-tenant-DB? Does the permission compiler need per-tenant context?
- **Backup:** whole-instance backup = enumerate tenants (the WO-027 caveat), now with a per-tenant restore path.
- The verdict: *clean → full build is WO-040, scoped by this; wall → ADR-003 amendment conversation, named.*

## Escalations

**If the probe finds per-tenant DBs blow up the connection model or hit a SurrealDB per-database limit, STOP and report** — that's the ADR-003 amendment, the asymmetric-risk outcome this WO was sequenced first to surface early. Do not build around it; a tenancy model that trades away the 60 MB memory win or the permission-compiler simplicity is a daylight conversation, not an implementation detail.

**Related:** [[Frust Hub]] · [[ADR-003 Tenancy Model]] · [[2026-07-27 WO-027 backup restore DR]] (P-8.1) · [[2026-07-26 WO-026 surrealdb write concurrency]] · [[SurrealDB]] · [[v2.0 Deployability Gate]]
