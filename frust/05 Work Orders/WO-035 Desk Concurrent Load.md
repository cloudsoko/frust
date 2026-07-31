---
tags: [frust, work-order, desk, concurrency, measurement, v2.0-gate]
status: COMPLETE (2026-07-28) — A3 closed with a NUMBER. Peak ~135 req/s at 50 concurrent (knee), Desk-bound by the kernel's DB ceiling (124 req/s WO-026), NOT the blocking-ureq-in-async structure. **The refused arithmetic (640 req/s) was wrong 4.7× AND wrong-mechanism** — the vindication of measure-don't-infer. Contention: 48 SSE streams + page load coexist (124 req/s, 0 failures, SSE 68–87% delivery), neither starves (WO-032 async-sleep pays off). A3 → bounded-by-measurement in the gate. **NEW FINDING → WO-038:** Desk degrades by ERRORING (HTTP 500, ~45% at 200 / ~85% at 500), cumulative (fresh stack at 200 = 0 fail), realtime impaired >20s post-load — DB saturation propagating up; wants admission control (shed 429/503 not 500). Committed driver `wf-proof/desk-load.mjs`. → [[2026-07-28 WO-035 desk concurrent load]]
created: 2026-07-28
---

# WO-035: Desk Concurrent Load (Gate Assumption A3)

> [!info] PM work order — closes gate-blocker **A3** ([[v2.0 Deployability Gate]]). Sequenced FIRST of the three by asymmetric risk: its bad outcome is architectural (Desk-tier concurrency capped by blocking-`ureq`-in-async → the F1/rt-multi-thread finding materializing), where A1/A2 are more likely confirmations. **Measure, don't infer — the arithmetic (16 workers ÷ ~25 ms ≈ 640 req/s) is exactly what the gate refuses.**

## The assumption

WO-032 measured 160 SSE subscribers fine (the SSE drain is non-blocking async-sleep). But its ordinary-request latency loop was **sequential** (`await` in a `for`), so **Desk-tier concurrent page-request throughput has never been measured.** Each Desk page handler makes a *blocking* `ureq` call to the kernel inside an `async fn`, pinning a tokio worker for the round trip — so concurrency is structurally capped near core-count. Whether that cap is 640 req/s (fine) or something worse (page requests starving the SSE drains, or a lower real cap) is unknown.

## Exit Criteria

1. **Concurrent Desk page-request throughput, measured** the way WO-024 measured the kernel: rising concurrency (1 / 10 / 50 / 200 / 500 concurrent clients) hitting a real Desk page that proxies to the kernel, report req/s + p50/p95/p99 at each rung. State the knee and what bounds it (worker pool? kernel? the blocking-in-async pin?). Committed re-runnable driver (the WO-032 methodology ruling — a load harness that survives the session).
2. **The contention question:** page requests (pin a worker via blocking `ureq`) vs SSE drains (async-sleep + brief blocking drain) compete for the same 16-worker pool. Measure both *together* — N concurrent page requests WHILE M SSE subscribers stream — and show neither starves the other, or find the interaction. This is the interesting half; the single-axis throughput is the easy half.
3. **Verdict for A3:** a number bounds Desk concurrency, replacing the assumption. If the cap is comfortable (well above realistic Desk load), P-2.2/scorecard note it measured-fine. **If page requests cap at core-count AND that's too low, or the SSE/page contention starves one side, that's a finding** — the fix (`spawn_blocking` around kernel calls, or an async kernel client) is a *separate* WO; this one measures and names, doesn't fix (the WO-024 discipline: measure, don't fix in the same WO).
4. **Fresh store, quiet machine, dedicated scratch dir** — all three hygiene clauses; this is a perf measurement and the substrate must not confound it.

## Escalations

If the measurement shows a real Desk-tier concurrency ceiling that blocks "deployable," that's the F1 finding graduating from theoretical to load-bearing — report with the number, and the fix WO gets scoped against it (`spawn_blocking` is likely small; an async-client refactor of `frust-desk` is not — name which).

**Related:** [[Frust Hub]] · [[v2.0 Deployability Gate]] (A3) · [[2026-07-28 WO-032 sse retire polling]] (where A3 was found) · [[2026-07-26 WO-024 load and footprint benchmark]] (the kernel-tier precedent)
