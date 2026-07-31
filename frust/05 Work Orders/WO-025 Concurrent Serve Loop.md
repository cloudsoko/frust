---
tags: [frust, work-order, concurrency, production, performance]
status: COMPLETED 2026-07-26 — all 5 criteria. Throughput rises with cores: 15→48 req/s at 500 clients (3.2×), p50 halved; P-1.1 bounded→KILLED (scorecard 21·13·0). Bottleneck moved to SurrealDB write concurrency (db_write 222 ms vs hook 0.087 ms — HookInstance mutex EXONERATED; DB 10:1 CPU, 16 kernel workers idle). Audit found 1 hazard (concurrent rollup drains → dedicated ticker thread); all else already race-safe; `Contrib: Send` not `Sync` (honest bound). Free win: old idle-only tick meant background jobs never ran under sustained load — dedicated ticker fixes it. RSS held 84 MB (P-1.4 safe). `loadbench` moved out of src/ (not guard widened). → [[2026-07-26 WO-025 concurrent serve loop]]
created: 2026-07-26
---

# WO-025: Concurrent Serve Loop (the #1 Production Blocker, Fixed and Measured)

> [!info] PM work order. This is the FIX WO — [[2026-07-24 WO-024 load and footprint benchmark]] found the bottleneck; this closes it, measured against WO-024's exact before-numbers so the before/after is honest. `loadbench` (committed `src/bin/`) is the instrument; do not modify it, or the comparison stops meaning anything.

## The before-numbers (WO-024, must be beaten)

| concurrency | throughput | p50 |
|---|---|---|
| 1 | 16.1 req/s | 61 ms |
| 10 | 15.0 | 653 ms |
| 50 | 13.5 | 3.7 s |
| 500 | 15.0 | 21.9 s |

Flat throughput, linear latency, kernel pegged at **0.85 of one core** — single-threaded accept loop serializing every request. Memory baseline to protect: **60 MB idle / 76 MB under load** (P-1.4 killed — a fix that balloons this reopens it).

## Exit Criteria

1. **Throughput scales with cores, proven on the same rungs:** re-run `loadbench` at 1/10/50/200/500 on the same 1 M store, release build, and show throughput *rising* with concurrency until it saturates available cores (not flat). State the new knee and what now bounds it (cores? SurrealDB? the `HookInstance` mutex WO-024 named as the secondary suspect?).
2. **The floor is preserved under concurrency:** the 25 ms single-request submit floor still holds at low concurrency — parallelism must not tax the uncontended path. Both perf gates green on a fresh store.
3. **Correctness is untouched by concurrency:** the full suite stays green — permission enforcement, the hook cycle-trap, the atomic job claim, the docstatus lattice all still hold when requests run in parallel. Concurrency must not open a race in any of them. Pay special attention to shared state: the `HookInstance` pool (WO-024's secondary bottleneck — does making it concurrent introduce a data race?), the session cache, the tenant-fairness token buckets (WO-013).
4. **Memory stays bounded:** RSS under the new concurrent peak stays in the tens-of-MB class — a thread-per-request model that spawns unboundedly, or a pool that leaks, reopens P-1.4. State the model (worker pool sized to cores is the obvious lazy-correct choice; justify whatever you pick).
5. **Re-score P-1.1 with the new number:** update [[v1.0 Pain-Point Scorecard]] — P-1.1 moves from "bounded, ceiling ~15 req/s" to its real scaled figure. This is the row that carries the milestone.

## Boundaries

- **Simplest thing that scales:** a bounded worker pool (tokio tasks or OS threads sized to cores) draining the same request channel is almost certainly the answer — the accept loop already receives on a channel, so fan-out is a pool of consumers, not a rewrite. Do NOT reach for a full async-runtime overhaul of the whole kernel if a worker pool clears the bottleneck; measure the lazy fix first.
- Whatever shared state the pool exposes to races is the real work — the concurrency is easy, the correctness under it is the WO. Name every piece of shared mutable state the requests now touch in parallel and state how each is made safe.

## Escalations

Standard rules + full hygiene set (substrate probe, dedicated scratch dir, fresh store for gates, drop scratch DBs at close). **If making the loop concurrent surfaces a data race in permission enforcement or the job claim, STOP — a concurrency win that weakens an isolation guarantee is not a win, and it's an ADR-level conversation, not a fix.**

**Related:** [[Frust Hub]] · [[2026-07-24 WO-024 load and footprint benchmark]] · [[v1.0 Pain-Point Scorecard]] · [[Frappe Pain Points]] (P-1.1) · [[ADR-006 Plugin Capability Surface]] (hook cycle-trap under concurrency) · [[ADR-009 Execution Model]] (atomic claim under concurrency)
