---
tags: [frust, build-log, tenancy, probe, milestone-4, adr-003]
created: 2026-07-28
status: PROBE CLEAN on isolation + per-tenant restore; ONE architectural finding for the build (hook-engine ownership) + two global counters to key per tenant
work-order: "[[WO-039 Multi-DB Tenancy Probe]]"
---

# Build Log — WO-039: Multi-DB Per-Tenant Tenancy (The Probe)

Milestone 4's opener, sequenced first because its bad outcome is an ADR-003
conversation. **It is not that outcome** — the model is clean where it matters,
and the build has one real architectural change, named below with runway.

## Criterion 0 — the evidence home (M4 prerequisite, closed)

`wf-proof/` → **`frust-e2e/`**: renamed (keeping the installed Playwright rather
than re-downloading), `proof.mjs` → `workflow.spec.mjs`, npm scripts for each
harness, and a **README** covering prerequisites, what each harness proves, the
perf-run hygiene rules, and — deliberately — *how to rebuild the
`naive-blocking-sse` control*, because a bench whose failure mode can't be
reproduced decays into a number nobody trusts. The fifth harness
(`tenancy-probe.mjs`) landed there, not in the old directory.

## THE HEADLINE — the finding that reframes the whole question

**A tenant already IS a database.** `Db::tenant()` returns `cfg.db`
(`db.rs:48`). Every log line, every metric label, every cache key that says
"tenant" has been naming a SurrealDB database all along. ADR-003's
database-per-tenant model is **already the data model**.

What is unbuilt is *routing*: one kernel process holds one `Broker` holding one
`Db`, so today multi-tenancy means **one process per tenant**. The build is not
"introduce per-tenant databases" — it is "let one process serve N of them."
That is a materially smaller and better-understood change than the WO assumed.

## Criterion 1.1 — isolation is DB-enforced, in the strongest form

Two tenants, separate databases, **separate signing keys**. Result:

| probe | result |
|---|---|
| A's token, A's database | reads A's data |
| **A's token, header naming B's database** | **returns A's own row — zero B rows** |
| forged token | **401** (tokens genuinely validated) |
| no auth | **403** |

The mechanism is stronger than "a permission clause refused it": the JWT carries
its own `ns`/`db` claims (`ns=frust, db=tenant_a`) and **SurrealDB binds the
session to those, ignoring a conflicting header**. A caller cannot *address*
another tenant's database at all. No kernel permission clause participates, so
**no kernel bug can widen this** — which is exactly the guarantee ADR-003 wanted.

### The near-miss worth recording

The probe's first run reported **"A's token CAN read B's database — 2 checks
failed"**, which would have been a cross-tenant data-leak finding. It was wrong.
I had asserted an *operation* — "non-200 or zero rows" — when the outcome I
needed was **whose data came back**. The call does return `200` with one row;
that row is A's own.

Diagnosis before reporting: a garbage token returned `401` and no-auth returned
`403`, proving tokens were validated, so the 200 could not mean "auth ignored" —
then reading the row's `owner_tenant` settled it. **A fabricated security finding
was two minutes of checking away from the record.** The assertion now reads
"zero rows belonging to B", which cannot pass for the wrong reason.

## Criterion 1.2 — per-tenant restore WORKS (the P-8.1 unlock)

The single thing single-DB tenancy could not do:

1. `surreal export` tenant A's database alone
2. **write to tenant B after the export** (so "untouched" has teeth)
3. `surreal import` A into a fresh `tenant_a_restored`

Result: **A restored intact; B untouched, including its post-export write.**
Restore-one is no longer restore-all. This is the P-8.1 ops-requirement,
answered — and it is the concrete payoff that justifies the build.

## Criterion 2 — what the full build costs

### FINDING — hook-engine ownership forces per-tenant wasmtime engines

`Broker::new(db, hooks: Box<dyn HookDispatch>)` takes **owned** hooks, so N
tenant brokers = N `WasmHooks` = **N wasmtime engines + N epoch-ticker threads**.
That is the mechanism that would multiply the 60 MB idle footprint (P-1.4,
killed) by tenant count.

**It is avoidable, and cheaply.** The script pool inside `WasmHooks` is *already*
keyed `(tenant, doctype)` (`hooks.rs:337`) — the hook layer was built
multi-tenant-ready. The blocker is only the ownership type. Changing
`Box<dyn HookDispatch>` → `Arc<dyn HookDispatch>` lets **one engine serve every
tenant**, and the per-tenant cost collapses to a `Db` (config + token cache) and
a `Broker` meta-cache — kilobytes, not tens of megabytes.

This is the WO's asymmetric-risk item, found early and **not** an ADR-003
amendment: it is a one-type change plus the routing work, identified before
anything was built around it.

### FINDING — two process-global counters need per-tenant keying

`SESSION_GEN` (`rest.rs:63`) and `META_GEN` (`broker.rs:192`) are process-global
`AtomicU64`s. With N tenants in one process, **one tenant's logout, revoke, or
metadata sync cold-starts every other tenant's caches.** Correctness is fine —
coarse invalidation over-invalidates, never under — but it is a cross-tenant
*performance* coupling, i.e. a noisy-neighbour vector on the very caches WO-026
built to reach 124 req/s. Key them per tenant in the build.

### Unchanged / no work needed

- **Permission compiler** — operates under the caller's own session, which is
  now pinned to the caller's database. No per-tenant context needed.
- **Meta boot (ADR-008)** — already per-database by construction; N tenants
  means running it per tenant database, not redesigning it.
- **Backup** — whole-instance = enumerate tenants (the WO-027 caveat, unchanged),
  now *with* a working per-tenant restore path.

## Verdict

**CLEAN → proceed to the full build.** No SurrealDB per-database wall was hit;
isolation is stronger than hoped; per-tenant restore works. The build (**WO-040**)
is scoped by this probe:

1. `Arc<dyn HookDispatch>` so one wasm engine serves all tenants *(do first — it
   is the memory finding)*
2. Request → tenant → `Db` routing (a tenant registry + per-tenant Broker map)
3. Per-tenant keying of `SESSION_GEN` / `META_GEN`
4. Per-tenant meta boot; whole-instance backup enumerating tenants
5. **Re-measure the idle footprint at N tenants** — P-1.4 is a killed verdict and
   this build is the thing most likely to undo it

## Files
`frust-e2e/` (rehomed harnesses + README) · `frust-e2e/tenancy-probe.mjs`

## Related
[[WO-039 Multi-DB Tenancy Probe]] · [[ADR-003 Tenancy Model]] · [[2026-07-27 WO-027 backup restore DR]] (P-8.1) · [[2026-07-26 WO-026 surrealdb write concurrency]] · [[v2.0 Deployability Gate]]
