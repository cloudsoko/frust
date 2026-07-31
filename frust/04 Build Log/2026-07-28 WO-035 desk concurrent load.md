---
tags: [frust, build-log, desk, concurrency, measurement, v2.0-gate, work-order]
created: 2026-07-28
status: complete — A3 closed with a number; Desk is NOT the bottleneck; one new finding (degrades by erroring, not queueing)
work-order: "[[WO-035 Desk Concurrent Load]]"
---

# Build Log — WO-035: Desk Concurrent Load (Gate Assumption A3)

The gate refused the arithmetic. Here is the measurement.

## The headline

**Peak ~135 req/s at 50 concurrent clients** (three independent runs: 133.3,
147.3, 136.9 — call it 135 ± 7). Measured, closed-loop, against a real Desk page
that proxies to the kernel, on a dedicated fresh store.

| concurrent | req/s | p50 | p95 | p99 | failures |
|---:|---:|---:|---:|---:|---|
| 1 | 39.5 | 25 ms | 31 | 42 | — |
| 10 | 122–138 | ~75 ms | ~110 | ~140 | — |
| **50** | **133–147** ← knee | ~355 ms | ~530 | ~620 | — |
| 200 | 78–107 | ~900 ms | ~2100 | ~2900 | **596–2001 × HTTP 500** |
| 500 | 43–50 | ~1400 ms | ~3200 | ~4000 | **2213–2858 × HTTP 500** |

## A3's verdict: the Desk is not the bottleneck

The assumption was that blocking-`ureq`-inside-`async fn` would cap the Desk near
core-count. **It does not bind at this scale.** The measured ceiling (~135 req/s)
sits essentially on top of the kernel's own DB-bound ceiling
(**124 req/s**, [[2026-07-26 WO-026 surrealdb write concurrency]]) — the Desk
adds no ceiling of its own. The failures under load are **`E_DB`** in the kernel
log (3468 of them in one run), i.e. SurrealDB saturating, which is the same
ceiling every tier already reported.

Note the arithmetic the gate refused predicted **640 req/s** (16 workers ÷ 25 ms).
The real number is **~4.7× lower**, and it is lower for a completely different
reason than the arithmetic modelled. The inference would have been wrong in both
magnitude and mechanism while sounding perfectly reasonable — which is the case
for refusing it.

## The contention question (the interesting half) — answered

48 SSE streams live across 4 tables **while** page load runs on the same
16-worker pool:

| | req/s | p50 | failures | SSE events during |
|---|---:|---:|---|---|
| 50 concurrent + 48 SSE | 124.4 | 393 ms | **0** | 336 (~87% of expected) |
| 200 concurrent + 48 SSE | 124.8 | 1476 ms | **0** | 263 (~68% of expected) |

**Neither side starves the other.** Page throughput holds ~91% of its
no-SSE baseline, and the SSE streams keep ticking throughout. The WO-032 design
(async sleep between non-blocking drains) is what makes this true: subscribers
cost no pinned worker, so they cannot crowd out page requests.

## FINDING — the Desk degrades by ERRORING, not by queueing

Past the ~50-concurrent knee the stack does not merely slow down: it returns
**HTTP 500** for a large fraction of requests (≈45% at 200 concurrent, ≈85% at
500). A load spike therefore *fails* users' requests rather than making them
wait. For a UI tier that is the wrong failure mode — a slow page is a bad
experience, a 500 is a broken one.

Two related observations, both measured:

1. **The errors are cumulative, not purely load-level.** The escalating sweep
   (1→10→50→200→500, 500 ms apart) produced 500s at 200; a *fresh* stack driven
   straight at 200 concurrent produced **zero**. The system does not fully
   recover between rungs.
2. **Realtime subscribe stays impaired for >20 s after heavy load.** Opening 48
   SSE streams 20 s after the sweep: 47 refused. The same 48 opens on an
   unloaded stack: **48/48, zero refused**. It does recover — repeated probes
   returned 200 consistently ~60 s later.

This is **not** the blocking-`ureq` structure and **not** a Desk worker cap; it
is saturation propagating from the DB up. The fix is backpressure/admission
control (shed or queue past the knee, return 429/503 with intent rather than 500
by accident) — **a separate WO**, per the WO-024 measure-don't-fix discipline.

## Method notes (what could have made this wrong)

- **Keep-alive by construction.** 500 concurrent clients without connection
  reuse would exhaust Windows ephemeral ports and measure the load generator —
  the WO-032 near-miss. The driver uses a keep-alive agent, and counts client
  connect errors separately from Desk responses.
- **Status codes recorded, not just "bad".** The first run reported failures
  without saying *which*. "Something failed" is not a measurement; the driver now
  buckets by code, which is how the 500s were attributed to `E_DB` rather than
  guessed at.
- **The driver aborts rather than reporting a void verdict.** Two runs died at
  phase 2 with only 1–2 of 48 streams live; instead of printing a contention
  ratio computed from nothing, it exits non-zero saying so. That abort is what
  surfaced the post-load impairment finding.
- **Hygiene:** dedicated fresh scratch store (`wo035-load`), dev data untouched.
  The machine carried ~20 idle MCP/editor node processes that belong to the
  user's environment and were **not** killed — stated rather than pretended away.

## Verdict for the gate

**A3 is closed.** The assumption "Desk-tier concurrent throughput is unmeasured"
is replaced by: **~135 req/s at 50 concurrent, bounded by the DB not the Desk,
with SSE and page traffic co-existing without starvation.** That is a
`bounded-by-measurement`. The new degradation finding becomes its own row/WO —
it is a real limit, honestly named, and it does not re-open A3.

## Files
`wf-proof/desk-load.mjs` — the committed, re-runnable driver (`all` | `sweep` |
`contention` modes).

## Related
[[v2.0 Deployability Gate]] (A3) · [[2026-07-28 WO-032 sse retire polling]] · [[2026-07-26 WO-024 load and footprint benchmark]] · [[2026-07-26 WO-026 surrealdb write concurrency]]
