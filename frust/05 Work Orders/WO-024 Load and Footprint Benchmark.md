---
tags: [frust, work-order, benchmark, production, scale]
status: ACTIVE (2026-07-26)
created: 2026-07-26
---

# WO-024: Load & Footprint Benchmark (Close the Lone OPEN, Number the Bounded)

> [!info] PM work order — Milestone 3's opening move, and pure measurement, the project's home turf. Same discipline as the [[2026-07-23 SurrealDB week-1 benchmark]] that opened the whole build: **benchmark before claiming; the scorecard left two runtime rows without a number and this WO produces them.** Empirical-first — probe the real question before designing any fix.

## The two unmeasured claims (from the v1.0 scorecard)

- **P-1.4 (memory) — the lone OPEN.** `frust serve` + `surreal.exe` under realistic load: what does it cost? Never measured, so scored OPEN not KILLED. This WO gives it a number and moves the verdict.
- **P-1.1 (concurrency) — bounded by assumption.** The serve loop's real throughput under concurrent requests is unknown. tokio is async, but the scorecard suspected the single-threaded serve accept-loop; only a load test can confirm or refute it.

## Exit Criteria

1. **Concurrent-request throughput, measured:** drive `frust serve` at rising concurrency (1 / 10 / 50 / 200 / 500 concurrent clients) doing the real submit path (auth → hooks → write), report throughput (req/s) and latency distribution (p50/p95/p99) at each rung. Find the knee — the concurrency where latency leaves the 25 ms floor — and say whether the accept loop, the hook dispatch, or SurrealDB is the bottleneck (profile, don't guess). Substrate probe + dedicated scratch dir per the standing caveats.
2. **Memory footprint under load, measured:** RSS of `frust serve` and `surreal.exe` at idle, at the 1 M-row dataset resident, and under the criterion-1 concurrency peak. State the per-tenant and per-connection marginal cost if it's visible. This is P-1.4's number.
3. **The scorecard rows re-scored:** P-1.4 moves from OPEN to a verdict backed by the measured number; P-1.1's bound tightens to a real throughput figure or an honest "degrades at N concurrent, cause X." Update [[v1.0 Pain-Point Scorecard]]'s two rows (this WO is allowed to edit the scorecard — it's producing the evidence the scorecard demanded).
4. **The findings drive the next WO, not this one.** If concurrency has a real bottleneck, WO-026 fixes it — measured. This WO *finds and names*; it does not fix (a fix smuggled into a benchmark is how you stop trusting the benchmark).

## Boundaries

- No optimization in this WO. Measure, profile, name the bottleneck. The fix is its own order so the before/after is honest.
- If load-testing tooling isn't on the machine, a minimal concurrent driver (async Rust or a shell fan-out) is fine — note what was used; this is about the numbers, not the harness.
- The 1 M fixture at `D:\Dev\rust\frust-scale-data` is the resident-dataset condition (rebuild if the store was juggled).

## Escalations

Standard rules + full hygiene set. **If throughput is flat across concurrency rungs (the single-threaded-accept-loop suspicion confirmed), that's the headline finding — report it as the thing that most blocks "production," and it sequences the milestone.**

**Related:** [[Frust Hub]] · [[v1.0 Pain-Point Scorecard]] · [[Frappe Pain Points]] (P-1.1, P-1.4) · [[2026-07-23 SurrealDB week-1 benchmark]] · [[2026-07-24 WO-006 1M-row scale proof]]
