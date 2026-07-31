---
tags: [frust, adr, aggregates, reports]
status: accepted
decided: 2026-07-24
---

# ADR-010: Aggregates Strategy — Shape Rules First, Then Two Materialization Tiers

**Context:** [[2026-07-24 WO-006 1M-row scale proof]] criterion 4 — measured at 1 M through the kernel path, extrapolated to 10 M. Four of six canonical report shapes need materialization; one has *no live-query door at all* (contract-inexpressible flatten). Frappe's answer was "reports get slow" (P-1.x); this ADR is ours.

## Decision — a three-tier ladder, cheapest rung first

### Tier 0 — Query-shape rules (no materialization)
Before any rollup exists, the report layer applies shape rules:
- **Period bucketing:** stored `month`/period fields make range filters *equality* filters — equality indexes are immune to #7432 (the trap is range-only). The register at 10 M is solved by `month = 'YYYY-MM'`, not by infrastructure.
- **Entity-equality indexes** for scoped shapes (one-customer statement: 0.45 s live at 1 M; add the equality index at 10 M and stay live).
- Stored-period fields are part of the standard DocType shape (the seeder/meta convention from WO-006) — Frappe-realistic, costs one field.

### Tier 1 — `DEFINE EVENT` counters (transactional, exact)
**Admission = ADR-009's two-clause test + tiny key-space + record-local delta.** The counter increments in the write transaction: exact, never stale, survives kernel bugs.
- Monthly revenue: 13 rollup docs (`month → {revenue, count}`) — the purest fit.
- AR outstanding per customer: the canonical ERP counter (10 k keys, delta-per-write).
The rollup docs are DocTypes — queryable through the contract, permission-compiled like any record.

### Tier 2 — Worker-maintained rollups (eventually consistent, bounded lag)
**Admission = the delta needs what an EVENT can't see:** graph hops (revenue by customer group — the EVENT can't resolve a 2-hop key in-transaction) or embedded-line diffs (item-wise sales — the parent EVENT can't see line-level deltas cleanly). The module-5 worker consumes the changefeed (priced by criterion 5: **+2 ms, ~1 doc-copy per write** — cheap enough to be always-on) with a versionstamp cursor. Consequences stated honestly:
- **Eventually consistent** — lag is bounded by the worker loop and *queryable* (cursor position is data); dashboards show staleness rather than hiding it.
- Recovery is the ADR-009 story: cursor replay, rescan beyond retention.
- On-demand TTL cache is admitted only as interim (top-20 item-wise) while the rollup lands.

## The forcing findings
- **Q3 is super-linear (19× on 10× data)** — per-row 2-hop fetches degrade with table size. Tier 2 removes the hop from the hot path entirely; also an upstream watch-item.
- **Q4 flatten is contract-inexpressible** — its materialization verdict is structural, not performance-tuning. The contract stays closed (ADR-006); the report exists because the rollup does.
- The kernel's +20–100 % over root is the price of unbypassable row security and tracks scan count, not n — materialization shrinks scans, so it pays double.

## Rejected
- Widening the filter contract with date-truncation/flatten expressiveness — reopens ADR-006's closed-contract decision for a problem Tiers 0–2 solve.
- Materialize-everything — Tier 0 shapes stay live; rollups are admitted per-shape by the ladder, not by default.
- In-DB Tier-2 (EVENTs doing hops/line-diffs) — fails ADR-009's two-clause test; business logic in DB strings.

> [!success] Amendment RESOLVED (WO-016, 2026-07-26)
> Money is decimal end-to-end through both tiers. Probed first: v3.2.0 `decimal + decimal` in EVENT bodies **stays decimal** (float control proves the probe bites); `(NONE ?? 0) + 0.1dec` promotes toward decimal. **Root cause was wider than the escalation: `Currency` mapped to `TYPE float` in every DocType** — fixed at the mapping (`TYPE decimal`, `ASSERT >= 0dec`); the rollup was symptom, not disease. All reconciliations now **exact-equal, epsilons deleted** (an epsilon in a REQ-6.2.1 check hides the very defect the requirement forbids). Migration = recompute-from-source with a launder-proof test (legacy `0.30000000000000004` *replaced* by exact `0.30`, never converted). `decimal.rs` has no mul/div by design — rounding policy is REQ-6.2.2's decision, not a utility default. [[2026-07-26 WO-016 decimal rollup accumulation]]

> [!success] Tiers 1–2 implemented and proven (WO-007, 2026-07-24)
> Monthly: **16–51 ms vs 7.7 s live (~275×)**, counter +0.4 ms/write, backfill 4.3 s once, 1 M reconciliation exact, zero lost increments under 6-thread contention (3–5 conflict-retries absorbed by the module-2 transport contract). Cancel-reversal falls out of the signed-contribution algebra (contributes iff `docstatus = 1`) — not a special case. Tier-2: cursor + rollup delta commit in one transaction — restart-loses-nothing is structural. Rollups are write-closed DocTypes. [[2026-07-24 WO-007 aggregates ladder implementation]]

**Related:** [[Frust Hub]] · [[ADR-006 Plugin Capability Surface]] · [[ADR-009 Execution Model]] · [[2026-07-24 WO-006 1M-row scale proof]] · [[SurrealDB]]
