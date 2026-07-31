---
tags: [frust, build-log, benchmark, scale, surrealdb, work-order]
created: 2026-07-24
work-order: "[[WO-006 One-Million-Row Scale Proof]]"
---

# Build Log — WO-006: 1 M-Row Scale Proof (Through the Kernel)

**Dataset:** 1,000,000 sales invoices / **5,000,046** embedded lines / 10 k customers / 12 groups / 5 k items — week-1 generator, same PRNG seed, plus a stored `month` field. Store: **779 MB** (surrealkv, v3.2.0 pinned). Scratch instance, isolated from the dev DB.
**Path under test:** `Rest` + `Broker` + record-user session (`analyst`, role `manager`, row permission `WHERE $auth.role = 'manager'` evaluated by the DB) — the product's door, not root `/sql`. Harness: `kernel/tests/scale_proof.rs` (release mode, `--ignored`).

## The five criteria

| # | Criterion | Evidence | Result |
|---|---|---|---|
| 1 | Week-1 shapes at 1 M, kernel path, record principal | `scale_proof_1m` shape table below | ✅ measured; Q3 flagged super-linear; Q4 contract-inexpressible (a finding, see below) |
| 2 | Index policy at 1 M (#7432) | A/B + `EXPLAIN` below | ✅ compound index **~16× slower** (7.42 s vs 0.46 s warm) — the trap is alive in pinned v3.2.0; broker's NOINDEX posture wins |
| 3 | REQ-6.1.1 floors, release mode, 1 M | `scale_proof_1m`: **24 ms** warm median vs 25 ms floor | ✅ holds → CI gate tightened: `perf_gates` now gates release at **25 ms** (debug stays 60), re-run green at 23 ms |
| 4 | Aggregates decision table | table below | ✅ delivered — input to the strategy note |
| 5 | Changefeed cost, measured | latency A/B + isolated bytes/write below | ✅ **+2 ms** submit delta (24→26 ms); **329 → 558 bytes/write** (feed ≈ one doc-copy per write); `SHOW CHANGES` first-100: 26 ms |

## Criterion 1 — the shapes (cold/warm/warm, wall ms)

| Shape | 100 k engine (week-1) | 1 M engine (root) | engine factor | 1 M **kernel** (REST, record user) | kernel Δ |
|---|---|---|---|---|---|
| Q1 register (range+sort, NOINDEX) | 40 ms | 466 ms | 11.6× | **794/692/621 ms** | +33 % |
| Q2 monthly revenue (13 groups) | 355 ms | ~4.0 s | ~11× | **8.4/8.2/7.7 s** | ~2× |
| Q3 revenue by customer group (2-hop) | 700 ms | ~13.4 s | **19× — SUPER-LINEAR** | **19.6/16.9/15.1 s** | +20 % |
| Q4 item-wise (flatten 5 M lines) | 900 ms | 9.3–9.5 s | 10.4× (linear) | *contract-inexpressible* | — |
| Q5 outstanding by customer (filter+hop) | (not in week-1 log) | 7.2–7.9 s | — | **6.9/6.3/6.5 s** | ≈ parity (noise) |
| Q6 one-customer statement (116 rows) | 35 ms | ~515 ms | ~15× | **464/434/470 ms** | ≈ parity |

**Readings.** Engine scaling is ~11× on 10× data across scan shapes — mildly super-linear (versioned-store overhead), acceptable. **Q3's 19× is genuinely super-linear** (per-row 2-hop record fetches degrade with table size) — flagged. The kernel's overhead over root is the cost of *unbypassable row security* (the permission clause evaluates per scanned row) plus REST/serde: +20–100 % depending on how much of the table the shape scans. Not super-linear in n — it tracks the engine's scan count.

**Two contract findings (by design, now with numbers):**
- **Q2** required a stored `month` field — the closed contract has no date truncation (named-query territory per ADR-006). The stored-period field is the Frappe-realistic shape anyway; seeder now writes it.
- **Q4** (subquery + `.flatten()`) cannot be expressed through the contract at all. That is the decision table's strongest "needs materialization" verdict — the product literally has no live-query door for this report.

## Criterion 2 — the #7432 trap at 1 M, this dataset

- Compound `(status, posting_date)` index, built `CONCURRENTLY`: **~41 s** at 1 M rows.
- Q1 **indexed: 8.56/7.46/7.45 s** vs **NOINDEX: 1.10/0.49/0.47 s** → **~16× slower with the index** (30× at 100 k; 230× on the slim-doc issue repro — magnitude varies with doc shape, the trap's direction never does).
- `EXPLAIN`: identical pathology to week-1 — `IndexScan` pushes only the lower date bound; the upper bound is a post-filter with a document fetch per index entry.
- Kernel posture check: the broker renders `WITH NOINDEX` on every range+sort read (module-1 test + static code path) — kernel Q1 is unchanged by the index's existence. Posture **validated at 1 M; we stay pinned.**
- **New finding — blocking index build is disk-fatal at scale:** the non-concurrent `DEFINE INDEX` at 1 M ballooned the WAL by **>6 GB** transient and died on disk exhaustion (os error 112). `CONCURRENTLY` built the same index in 41 s with ~1.8 GB transient. Week-1's consequence #3 (ORM adapter uses CONCURRENTLY) upgrades from "politeness to live tenants" to **the only viable build path at scale**. Loud failure, classified: silent-misbehavior counter stays at 2.

## Criterion 3 — release floors at 1 M

Submit (hooks both classes + authenticated write, warm median of 40, against the 1 M table): **24 ms, changefeed off; 26 ms, changefeed on.** REQ-6.1.1's ≤ 25 ms floor **holds at 1 M in release**. `perf_gates::gate_submit_latency` now gates release builds at the floor itself (25 ms) and debug at 60 ms; re-run green at 23 ms. Hook-chain median stays < 1 ms.

## Criterion 4 — the aggregates decision table

Extrapolations use each shape's measured 100 k→1 M factor applied once more (labeled estimates, not measurements). Latencies are kernel-path.

| Report shape | 1 M measured | 10 M extrapolated | Verdict | Candidate mechanism |
|---|---|---|---|---|
| Register (paged, range+sort) | 0.62 s | ~6–7 s | **live acceptable at 1 M, degraded at 10 M** | Stored-period bucketing: filter `month = 'YYYY-MM'` turns the range into an *equality* — equality indexes are safe from #7432 (point lookups indexed fine at week-1). No materialization; a query-shape rule the Desk applies. |
| Monthly revenue | 7.7 s | ~80 s | **needs materialization** | `DEFINE EVENT` counter per ADR-009's two-clause test: 13 rollup docs (`month → {revenue, count}`), incremented in the write transaction. Smallest key-space, purest fit for the EVENT mechanism. |
| Revenue by customer group | 15.1 s | ~250 s+ (super-linear) | **needs materialization** | Kernel-maintained rollup docs (12 keys), updated by the worker off the changefeed (criterion 5 shows the feed is cheap). EVENT can't cheaply resolve the 2-hop group key in-transaction; the worker can. |
| Item-wise sales | contract-inexpressible; engine 9.4 s | ~95 s | **needs materialization** (no live door exists) | Worker-maintained per-item rollups from the changefeed, diffing embedded `lines` — EVENT on the parent can't see line-level deltas cleanly. On-demand cache (TTL) acceptable interim for top-20. |
| Outstanding by customer (AR) | 6.5 s | ~65 s | **needs materialization** | Classic AR balance: `DEFINE EVENT` counter on `outstanding` delta per customer (10 k keys) — two-clause test applies; this is the canonical ERP counter. |
| One-customer statement | 0.45 s | ~4.5 s | **live acceptable** with equality index | `customer` equality index (safe from #7432). At 10 M, add the index; no materialization. |

## Criterion 5 — changefeed, priced at last

- Submit latency: **24 ms → 26 ms** (A/B, same phase, warm medians of 40). ~8 % on the floor shape.
- Storage, isolated matched-path A/B (500 writes each): **329 B/write off → 558 B/write on** — the feed appends ≈ one copy of the changed doc per write. At 1 M invoice-sized docs: on the order of ~1 GB of feed per full-table rewrite cycle, pruned by the `CHANGEFEED 3d` window.
- `SHOW CHANGES ... SINCE <versionstamp> LIMIT 100`: 26 ms against the 1 M table. The audit-trail read stays interactive.
- The [[SurrealDB]] risk-list item now has its number: **the unbypassable audit costs +2 ms and ~1 doc-copy per write.** Cheap enough to stay always-on.

## Disk clause (executed, no trim needed)

D: was at 0.7 GB at WO start. Freed 8.2 GB (topcoat + kernel debug targets). 100 k probe measured 80 MB/100 k → 1 M projected <1 GB — full run authorized by the numbers, no PM trim decision required. One casualty en route: the blocking index build (finding above) exhausted C: transiently; store relocated to `D:\Dev\rust\frust-scale-data` (779 MB, kept for repro alongside `frust-bench/seed-scale.mjs`; scratch instance stopped — restart is one command). The cargo registry on C: was NOT touched.

## Related
[[WO-006 One-Million-Row Scale Proof]] · [[2026-07-23 SurrealDB week-1 benchmark]] · [[2026-07-24 Module 6 + WO-005 close — frust serve]] · [[ADR-002 SurrealDB Lock-In]] · [[ADR-009 Execution Model]] · [[SRS]] (REQ-6.1)
