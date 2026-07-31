---
tags: [frust, work-order, aggregates]
status: COMPLETED 2026-07-24 — all 5 criteria proven; ~275× monthly read (16–51 ms vs 7.7 s), counter costs ~0.4 ms/write, zero lost increments under contention (escalation clause: non-event); cancel-reversal via signed-contribution algebra; cursor+delta one-transaction; strict text-diff undo applier (fails loudly, never lies). Security finding → [[WO-008 Identity Hardening]]. → [[2026-07-24 WO-007 aggregates ladder implementation]]
created: 2026-07-24
---

# WO-007: Aggregates Ladder Implementation (ADR-010, Tiers 1–2)

> [!info] PM work order — the first post-kernel *feature* build. Results to `04 Build Log/`, live vault path verified first. The 1 M dataset at `D:\Dev\rust\frust-scale-data` is your before/after benchmark fixture.

## Scope

Implement [[ADR-010 Materialized Aggregates]] Tiers 1 and 2 in the kernel, proving each against the WO-006 numbers. Tier 0 (shape rules) is a Desk/report-layer concern — *document* the rule table for it here; implementation rides with Desk v1.

## Exit Criteria

1. **Tier-1 EVENT counter, generated from metadata:** a DocType metadata declaration (e.g. `aggregate: {kind: counter, key: month, metrics: [revenue, count]}`) compiles through the sync engine into a `DEFINE EVENT` counter passing ADR-009's two-clause test. Prove with **monthly revenue**: rollup docs exact after concurrent submits (no lost increments — the WO-004 EVENT machinery under burst), and the monthly report reads from 13 rollup docs in **< 100 ms** vs the measured 7.7 s live (a ~77× win, claimed only after measuring).
2. **Tier-1 second instance — AR outstanding:** the canonical delta counter (10 k keys), including the *cancel/amend* path: docstatus 2 reverses the delta. Exactness proven by full-scan reconciliation (`sum(rollups) == live aggregate`) after a mixed submit/cancel burst.
3. **Tier-2 worker rollup — revenue by customer group:** the module-5 worker consumes the changefeed with a versionstamp cursor, resolves the 2-hop key kernel-side, maintains 12 rollup docs. Prove: (a) reconciliation matches live query after a burst; (b) **lag is queryable** (cursor position exposed as data) and bounded under load; (c) worker restart mid-stream loses nothing (cursor replay — the ADR-009 story, now with rollup state at stake).
4. **Tier-2 second instance — item-wise from embedded lines:** line-level diffing (old vs new `lines` arrays from the feed's before/after), per-item rollups. This is the shape with **no live door** — its report exists only through this rollup; the acceptance is the report *existing* and reconciling.
5. **Rollups are DocTypes:** readable through the contract, permission-compiled, visible in the Desk list view like any record — no side-channel storage.

## Escalations

- Standard: silent misbehavior = instance #3 = stop, ADR-002 re-read.
- If EVENT-counter increments prove non-atomic under concurrent submits (lost updates), that's a *pillar behavior* finding — stop and report before designing around it; Tier 1's admission to ADR-010 depends on it.

**Related:** [[Frust Hub]] · [[ADR-010 Materialized Aggregates]] · [[ADR-009 Execution Model]] · [[2026-07-24 WO-006 1M-row scale proof]]
