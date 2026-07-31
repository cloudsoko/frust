---
tags: [frust, build-log, surrealdb, benchmark]
created: 2026-07-23
---

# Build Log: SurrealDB Week-1 Report Benchmark

**Goal:** settle [[SurrealDB#Risks & Reality Checks]] risk #1 (query-planner maturity on report-shaped workloads) with measurements, per the "benchmark week 1, not month 6" rule.
**Setup:** SurrealDB **v3.2.0** standalone binary, **file-backed** storage (honest production shape, not the memory engine). Dataset: 100 k sales invoices with ~500 k embedded lines, 2 k customers → 12 groups (record links), 5 k items. Artifacts: `D:\Dev\rust\frust-bench\` (100 MB — kept for upstream repro + future 1 M-row run). Note: v3.2 requires `OPTION IMPORT;` as an import file's first statement.

## Results — every Frappe report shape at interactive speed

| Report shape | Time |
|---|---|
| Sales register (date range + filter + sort + page), **unindexed scan** | ~40 ms |
| Monthly revenue, GROUP BY over 100 k | ~355 ms |
| Revenue by customer group (2-hop link traversal inside aggregation) | ~700 ms |
| Item-wise sales (flatten 500 k embedded lines, group, top 20) | ~900 ms |
| Point lookup — indexed / unindexed | 1–2 ms / 35 ms |
| Bulk import | 107 k records in 6.3 s |
| Index build (100 k rows, blocking) | 3.4 s |

**Both modality bets held:** embedded child tables aggregate fine ([[SurrealDB#3. Nested objects & arrays → child tables, rethought|§3]]); link traversal replaced JOINs without falling over ([[SurrealDB#2. Record links & graph edges → Link fields without JOINs|§2]]).

## Red flag — compound range index is a 30× trap

A plausible `(status, posting_date)` compound index made the register query **30× slower** (40 ms → 1.2 s). `EXPLAIN`: only the *lower* date bound is pushed into the index scan; the upper bound runs as a post-filter with a **document fetch per index entry** — ~13 k wasted fetches before the first hit. `WITH NOINDEX` restores 36 ms.

### Consequences (recorded in [[SurrealDB]] + [[SRS]])

1. Frust's query layer ([[SRS#1.2 Data Access & Query Engine|REQ-1.2.1]]) MUST expose **index hints** (`WITH NOINDEX` / `WITH INDEX`) from day one.
2. **Index policy is benchmark-driven, not schema-driven** — the ORM adapter's DocType→`DEFINE INDEX` pipeline treats range indexes as opt-in, never auto-derived.
3. ORM adapter uses `DEFINE INDEX … CONCURRENTLY` — 3.4 s of blocking build per 100 k rows is felt by a live tenant.

## Update 2026-07-24 — upstream issue filed

**[surrealdb/surrealdb#7432](https://github.com/surrealdb/surrealdb/issues/7432)** — self-contained pure-SurrealQL repro (no Node; paste into any root session), `EXPLAIN` plan, scaling table.

Extra datapoint from building the repro: **regression magnitude tracks document size** — 5× on slim docs → 30× on realistic invoices → **230× at 1 M rows** — because the broken plan fetches whole documents just to discard them. Reinforces consequence #2: range indexes stay opt-in.

Dialect notes (3.x): `type::thing` → `type::record`; `/sql` endpoint doesn't auto-create databases (import does); imports need `OPTION IMPORT;` first.

## Follow-ups

- [x] File the range-bound planner issue upstream → [#7432](https://github.com/surrealdb/surrealdb/issues/7432)
- [x] Back-fill [[ADR-002 SurrealDB Lock-In]] and [[ADR-003 Tenancy Model]]
- [ ] Re-run at 1 M rows before making GA-scale claims
- [ ] Materialized-aggregates strategy note (report shapes >500 ms at 100 k will not hold at 10 M)

## Related

[[Frust Hub]] · [[SurrealDB]] · [[SRS]] · [[2026-07-23 Topcoat prototype]]
