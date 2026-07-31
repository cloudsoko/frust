---
tags: [frust, work-order, benchmark, scale]
status: COMPLETED 2026-07-24 — all 5 criteria measured; 25 ms floor holds at 1 M release-mode (CI gate tightened); #7432 alive at 1 M (16×, NOINDEX posture validated); aggregates table → [[ADR-010 Materialized Aggregates]]; changefeed priced (+2 ms, ~1 doc-copy). → [[2026-07-24 WO-006 1M-row scale proof]]
created: 2026-07-24
---

# WO-006: 1 M-Row Scale Proof (Through the Kernel, Not Beside It)

> [!info] PM work order — results to `04 Build Log/`, live vault path verified first.

## Why now

Two debts come due together: the week-1 benchmark's **1 M-row re-run** (blocking GA-scale claims since [[2026-07-23 SurrealDB week-1 benchmark]]) and the **materialized-aggregates strategy**, which needs 1 M-scale numbers to be written from evidence. The kernel's existence changes the terms: this run goes **through `frust serve`** — broker, permission compiler, index policy, REST — not raw `/sql`. We benchmark the product, not the database.

## Exit Criteria

1. **The week-1 shapes at 1 M invoices (~5 M embedded lines), kernel path, under a record-user principal** (not root): register query, monthly GROUP BY, 2-hop traversal aggregation, embedded-line flatten. Report vs the 100 k numbers; flag anything super-linear.
2. **The index-policy validation at scale:** confirm the broker's `NOINDEX` posture on range+sort shapes still wins at 1 M (the #7432 regression measured 230× there — if upstream fixed it since, that's a finding too; re-test with the fix's version *only* in a scratch env, we stay pinned).
3. **REQ-6.1.1 floors re-certified at 1 M in release mode** — including the ≤ 25 ms submit floor at true release-build speed (module 6's CI gate runs 60 ms debug-tolerant; this WO produces the release-mode number and, if it holds, tightens the CI gate config toward the floor).
4. **The aggregates decision table:** for each report shape — measured latency at 1 M, extrapolated 10 M, and a verdict: *live query acceptable / needs materialization*. Where materialization is indicated, characterize the candidate mechanism (`DEFINE EVENT` counter per ADR-009's two-clause test vs kernel-maintained rollup docs vs on-demand cache) with one paragraph each, evidence-linked. **This table is the input to the materialized-aggregates strategy note — I write it from your table.**
5. **Changefeed cost at scale, measured:** storage growth and submit-latency delta with `CHANGEFEED` on the 1 M table (the [[SurrealDB]] risk-list item that's never had a number).

## Escalations

- Silent-misbehavior instance #3 → stop, ADR-002 re-read fires.
- Disk (the dataset is ~GBs): if generation + indexes don't fit alongside the toolchains, say so before improvising — a trimmed 500 k run with documented scaling factors is a PM decision, not a builder workaround.

**Related:** [[Frust Hub]] · [[2026-07-23 SurrealDB week-1 benchmark]] · [[ADR-002 SurrealDB Lock-In]] · [[ADR-009 Execution Model]] · [[SRS]] (REQ-6.1)
