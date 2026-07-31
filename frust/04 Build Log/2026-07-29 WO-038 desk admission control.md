---
tags: [frust, build-log, desk, production, hardening, milestone-4, topcoat]
created: 2026-07-29
status: COMPLETE — the Desk sheds with 503 + Retry-After past its bound; `shed` moved 0 → 23 655 and matches the 503 count exactly; p99 under overload 6.2 s → 0.96 s; realtime recovery window 20-60 s → **0 s**. A3 re-recorded 135 → 146 req/s.
work-order: "[[WO-038 Desk Admission Control]]"
---

# Build Log — WO-038: Desk Admission Control

Two attempts. **The first one was built, measured inert, and reported rather
than shipped** — and the reason it was inert is the finding that re-ruled the
work order.

## Attempt 1 — measured inert, and why that mattered

The obvious build: count in-flight kernel calls, refuse past a ceiling, return
503. It compiled, it looked right, and at 500 concurrent clients across three
sweeps **`shed` stayed 0**. Not once.

The cause was structural, not a mistuned threshold. Each Desk handler made a
**blocking `ureq` call inside an `async fn`**, pinning one of ~16 tokio workers
for the whole round trip — so no more than ~16 handlers could be executing at
any instant and in-flight kernel calls were *already bounded by the worker
count*. The ceiling of 24 was unreachable by construction.

| observation | value |
|---|---|
| requests inside the Desk (Little's law, 157.8 req/s × 2.476 s p50) | **~391** |
| in-flight kernel calls the counter could ever see | **≤ 24** |
| kernel round-trip latency the Desk observed | **29 ms** |
| Desk page p50 at that moment | **2 376 ms** |
| kernel errors across all sweeps | **0** |

~2 350 ms of every slow request was queueing **in the tokio scheduler**,
upstream of any code this crate runs. A second attempt at *latency*-based
shedding was inert for the mirror-image reason: kernel latency stayed at 29 ms
**because the kernel was never the bottleneck**.

> **The standing note this earned:** an `async fn` making a blocking call is a
> hidden concurrency cap **and** blinds any admission gate written inside it,
> because the queue forms in the scheduler, not in your counter.

**The improved numbers from that run were not reported as a result.** Failures
fell 563 → 91 between the control and the shipped build while `shed` was 0 in
both — so the gate cannot have caused it. Claiming otherwise would have been
the exact "decorative metric" failure this project keeps catching.

## Attempt 2 — the fix and the shed are the same act

Per the re-ruling: `tokio::sync::Semaphore::try_acquire` → `spawn_blocking`,
reusing the WO-041 shared `Agent`. No reqwest migration; the default
512-thread blocking pool sits comfortably above the permit count.

Unpinning the worker is what makes the bound *visible*: concurrency moves from
**16 invisible workers** to **N explicit permits**, and the queue moves out of
the scheduler and into a counter an operator can read. `try_acquire`, never
`acquire` — waiting for a permit would rebuild the queue somewhere else.

**N = 64**, sized from the measured plateau (throughput flattens by ~50
concurrent), not guessed. At N = 24 the first run shed ~88 % at 50 concurrent —
correct mechanism, wrong bound, and the measurement said so.

### A framework gap, fixed in the vendored trunk

The first working run sheds *fired* and still answered **HTTP 500**:
`shed: 32464` with `codes {"500": ...}`. Topcoat maps errors to statuses by
downcasting against a **closed list** of its own error types and falls back to
500 for anything else — a bespoke `Busy` with its own `IntoResponse` never got
a look in. The framework had **no 429/503 constructor at all**.

So `ServiceUnavailableError` + `service_unavailable(retry_after_secs)` was
added to the vendored Topcoat trunk — a carried patch per the fork's practice,
not a workaround in the Desk. It carries `Retry-After` as the point of the
type: a bare 503 tells a caller to go away without saying when to come back, so
every well-behaved client invents its own backoff and they all synchronise.
Topcoat's own suite stays green (288 + 67 passed).

## The measured before/after

Three sweeps, fresh scratch store, WO-035 driver, same rungs.

**Failure mode — the criterion:**

| rung | before (control, shedding off) | **after** |
|---|---|---|
| 1 / 10 / 50 | 0 bad | **0 bad** |
| 200 | 22 dropped connections (`code 0`) | **9 938 · 10 874 · 12 145 × HTTP 503** |
| 500 | 563 dropped connections (`code 0`) | **13 717 · 13 872 · 11 582 × HTTP 503** |

Zero 500s, zero timeouts, zero dropped connections in every post-fix sample.

**A note on the "before".** WO-035 recorded ~45 % / ~85 % HTTP 500s. On today's
stack the control failed as **dropped connections** instead — arguably worse,
since the client gets nothing at all. The 500s appear to have come from the
Desk's own transport: it was building a **fresh `ureq::Agent` per kernel call**,
the identical defect WO-041 had just closed in the kernel. That is fixed here
too (one shared agent), and it would have contaminated this very measurement.

**Attributable, not inferred:** `shed` moved **0 → 23 655**, and 9 938 + 13 717
= 23 655 — the counter matches the 503 count exactly. `GET /admission` reports
`inflight / max_inflight / served / shed / kernel_latency_ms`, unauthenticated
and dependency-free so it answers while the Desk is too busy to serve a page.

**p99 under overload is bounded** — the other half of criterion 3:

| rung | before | after |
|---|---|---|
| 200 | 3 091 ms | **613 · 781 · 699 ms** |
| 500 | 6 666 ms | **959 · 895 · 964 ms** |

## Realtime recovery — the window is gone

WO-035 measured **47 of 48 SSE subscribes refused at +20 s** after a burst,
recovering by ~60 s. Re-measured with `frust-e2e/sse-recovery.mjs`, which
reproduces that scenario exactly (burst first, then probe at intervals):

```
baseline (unloaded): 48/48 accepted
  + 0s : 48/48 accepted   503 0   other 0
  + 5s : 48/48 accepted   503 0   other 0
  +20s : 48/48 accepted   503 0   other 0
  +40s : 48/48 accepted   503 0   other 0
  +60s : 48/48 accepted   503 0   other 0
RECOVERY WINDOW: recovered by +0s
```

**0 s — there is no impairment window**, because the shed keeps workers free so
subscribe is never starved. The contention run agrees from the other side: 48
SSE streams held *through* 200-concurrent overload, 0 refused, streams still
ticking (344 events against ~384 expected — not starved).

> **The probe's first run was wrong and said so.** It used `/live/{table}`; the
> route is `/live/sse/{table}`, so it read 404 everywhere — including the
> unloaded baseline — and reported "recovered by +0 s" on `0 >= 0`. A vacuous
> pass. The probe now **aborts with no verdict** if the unloaded baseline is
> zero: the instrument's own control, because a recovery test that cannot
> subscribe at all will always report instant recovery.

## A3 re-recorded — the number moved, honestly

Criterion 4 as amended: admission gets **no credit** for the ceiling, but the
structural fix did move the Desk tier, so A3 is re-recorded rather than left to
drift. Medians of three samples:

| rung | WO-035 (A3) | **WO-038** |
|---|---|---|
| 10 | — | 145.6 req/s |
| **50 (the knee)** | **~135 req/s** | **146.4 req/s** |
| 200 (served) | — | 137.5 req/s |
| 500 (served) | — | 142.8 req/s |

**A3: ~135 → ~146 req/s.** The gain is from `spawn_blocking` removing the
16-worker cap so the kernel is fed properly — **not** from admission control,
which only sheds excess. The ceiling remains DB-bound; beating it is still the
WO-026 batching/sharding conversation.

## Files

`frust-desk/src/main.rs` (semaphore admission, `spawn_blocking` kernel calls,
shared `Agent`, honest status propagation, `GET /admission`) ·
`topcoat/crates/topcoat-router/src/error/service_unavailable.rs` (**new —
carried patch**) + `error.rs` (module + downcast list) ·
`frust-e2e/sse-recovery.mjs` (new — the post-burst recovery probe)

## Related
[[WO-038 Desk Admission Control]] · [[2026-07-28 WO-035 desk concurrent load]]
(the finding, and A3) · [[2026-07-29 WO-041 connection reuse]] (the same
per-call-client defect, found again in the Desk) ·
[[2026-07-25 WO-013 tenant fairness]] (typed refusals) ·
[[2026-07-26 WO-026 surrealdb write concurrency]] (the DB ceiling) ·
[[Topcoat vendored]] (the carried patch) · [[v2.0 Deployability Gate]]
