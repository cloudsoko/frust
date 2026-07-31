---
tags: [frust, work-order, concurrency, surrealdb, performance]
status: COMPLETED 2026-07-26 — all 5 criteria. Probe found a THIRD outcome (not the surrealkv wall, not the connection model, not schema shape — 3 DB round-trips/request); SurrealDB bet NOT at its limit, fix kernel-side bounded. Generation-invalidated caches (session + doctype) collapse 3→1 round-trips: **48→124 req/s (milestone arc 15→48→124, 8.2×)**, warm submit 32→5 ms (5× under floor), RSS 75.4 MB (a memory *dividend* — faster completion = fewer in-flight). Correctness gated FIRST: 9 invalidation tests green before any throughput claim (hit≡miss catches JWT-reseed; uninstall-refuses-from-cache catches the dangerous direction). **P-1.2 re-earned KILLED with evidence** (killed→bounded→killed same day). New bound named: ~80% of raw SurrealDB ~150 w/s — beating it is an architecture conversation (batching/sharding/ADR-003 per-tenant process), not kernel tuning. Rulings: 5 s OOB-revocation TTL ratified (logout instant), coarse invalidation ratified (asymmetric failure modes favor coarse), v1.1 revoke-endpoint queued. → [[2026-07-26 WO-026 surrealdb write concurrency]]
created: 2026-07-26
---

# WO-026: SurrealDB Write Concurrency (the New Ceiling, Measured)

> [!info] PM work order — the handoff [[2026-07-26 WO-025 concurrent serve loop]] made explicit. WO-025 moved the bottleneck from the kernel to the DB *with numbers*; this WO attacks the DB write path — **empirical-first, and the connection model is the load-bearing unknown.** `loadbench` (unchanged) is the instrument; WO-025's after-numbers (48 req/s / 500 clients, db_write 222 ms avg) are the before.

## The measured target (WO-025)

`db_write` averages **222 ms/call** under load while hook dispatch is 0.087 ms; SurrealDB burns ~10× the kernel's CPU (371.8 s vs 34.3 s) with 16 kernel workers idle behind it. The ceiling is the DB write path. The first question is *why* 222 ms — and whether it's SurrealDB's actual write cost or something in how the kernel talks to it.

## Handoff (from WO-025 mechanics — do this before probing)

- **Rebuild or snapshot the `frust-scale-data` fixture first.** WO-024+WO-025 left ~2,700 rows in a `thing` table plus WAL growth. This vault's own finding is that a churned store degrades writes ~3× — which matters *more* for a DB-ceiling investigation than it did for either prior run. The raw-surreal microbench and the through-kernel number must be measured against the *same, known* substrate, or the comparison that decides the whole WO is confounded. Fresh store, quiet machine, substrate probe — all three clauses, and here they're load-bearing not ceremonial.
- **`loadbench` moved to `kernel/examples/`** (WO-025 kept the no-bare-prints gate honest rather than widening it): invoke `cargo run --release --example loadbench -- <concurrency> <secs>`. Measurement logic byte-identical → WO-024 before / WO-025 after numbers remain directly comparable to whatever this WO produces.

## Criterion 1 — THE CONNECTION-MODEL PROBE, before any fix

How does the kernel currently reach SurrealDB — one shared HTTP connection, a new connection per request, a pool? The 222 ms may be write cost, connection setup, head-of-line blocking on a shared connection, or lock contention inside surrealkv. **Probe it: measure a raw concurrent-write microbenchmark straight at surreal (bypassing the kernel) at the same concurrency, and compare.** If raw surreal also caps ~48 writes/s, the ceiling is the engine/storage (surrealkv single-writer?) and the fix is architectural (batching, or the ADR-003 per-tenant-process question). If raw surreal scales but the kernel doesn't, the ceiling is the kernel's connection model and the fix is a connection pool. **The probe decides the whole WO — do not design the fix until it answers.**

> [!success] Criterion 1 ANSWERED 2026-07-26 — a third outcome the fork didn't predict. **Not** the surrealkv wall (raw scales 40→144→152 w/s), **not** the connection model (`bare`≈`fresh`≈`pooled` within 10% — `ureq::post` free-function is fine), **not** schema shape (kernel-shaped table 147.6 vs bare 143.7 w/s — events/changefeed/permissions near-free). **It's kernel per-request round-trips:** every authenticated write does **3 sequential DB round-trips** — `session_caller` SELECT, `load_doctype` SELECT, the write. Raw ceiling ~150/s ÷ 3 ≈ 50 req/s = measured 45-48. The SurrealDB bet is NOT at its limit; the fix is kernel-side and bounded. **Scorecard consequence ruled: P-1.2 killed→bounded** — `load_doctype`-per-write IS "metadata loaded per request," killed on latency but mechanism reproduced; this WO's cache re-kills it with evidence.

## Criterion 2 — the fix (dual mandate: throughput AND re-kill P-1.2)

Cache the two read-mostly per-request lookups — session-caller and doctype-metadata — behind the existing `Mutex` pattern, collapsing 3 round-trips toward 1. **Invalidation is the real work** (a cache that serves stale doctype metadata after a sync, or a stale session after logout/revocation, reintroduces the WO-020 Finding-A class of silent-wrong): session cache must honor logout + the ADR-012 revocation path; doctype cache must invalidate on sync/install/update (the WO-019 lifecycle mutates it). Re-measure on the same rungs; re-score P-1.2 → KILLED only if the cache demonstrably eliminates the per-request reads *and* invalidation is proven correct.

> [!note] PM rulings on criterion 2's session cache (2026-07-26)
> - **5 s TTL backstop for out-of-band revocation: ratified.** Logout (the path users + Desk actually take) is instant via generation-bump; only a *direct DB-row deletion that bypasses the kernel entirely* waits ≤5 s. For ERP, a forced-revoke taking effect within 5 s is fine, and it's a trivially tunable constant. Correctness-dominant: the instant path is the common path.
> - **Coarse invalidation (logout drops every entry): ratified** — "correctness beats hit-rate" is the right call for a permission-path cache; obviously-correct coarse beats subtly-wrong fine. Named ceiling with upgrade path: if a high-logout-rate workload ever shows cache thrash (every logout cold-starts all sessions → re-read burst), per-token invalidation is the upgrade. Not needed at measured scale.
> - **v1.1 backlog (the cleaner shape, not a WO-026 gate):** a first-class kernel *revoke-session* endpoint that bumps the generation would make even admin-forced revocation instant, leaving the TTL as pure safety-net for the truly-out-of-band case. Frappe has admin session revocation; Frust should, as a battery. Deferred, noted.

## Remaining Criteria (shape depends on the probe)

2. **The fix, matched to the probe's verdict** — connection pool if kernel-side; write batching / async-commit exploration if engine-side; escalate to an ADR-003 amendment if it's the shared-process wall (per-tenant surreal processes is the DB-compute-isolation trade WO-013 already priced the *need* for).
3. **Throughput rises again, measured on the same rungs** — state the new figure and the new bound (there is always a next bound; name it).
4. **Every guarantee intact** — the full suite green; write concurrency must not weaken the atomic job claim, the permission-refused-write typing (ADR-012 Finding A), or the changefeed's completeness. surrealkv WAL behavior under concurrent writes gets an empirical first-exercise (it's a write-path change on the engine whose risk list this vault maintains).
5. **Floor + footprint hold** — 25 ms single-request floor, tens-of-MB RSS class; fresh store, dedicated scratch dir, quiet machine (all three clauses).

## Escalations

Standard rules + full hygiene set. **If the probe says the ceiling is surrealkv's storage engine (single-writer), that is a pillar-level finding** — it's the SurrealDB adoption bet meeting its scale limit, and the response is an ADR (batching, sharding, or the per-tenant-process trade), not a workaround. Report before building.

**Related:** [[Frust Hub]] · [[2026-07-26 WO-025 concurrent serve loop]] · [[SurrealDB]] (write-concurrency ceiling caveat; risk list) · [[ADR-003 Tenancy Model]] (the per-tenant-process trade, priced but unbuilt) · [[v1.0 Pain-Point Scorecard]] (P-1.1)
