---
tags: [frust, work-order, desk, production, hardening, backlog]
status: COMPLETE (2026-07-29) — the Desk sheds with intent; every criterion measured. FIX (as re-ruled): `Semaphore::try_acquire` → `spawn_blocking` reusing the WO-041 shared Agent — unpinning the worker is what makes the bound VISIBLE, moving concurrency from 16 invisible workers to **N=64 explicit permits** (N sized from the measured plateau; at N=24 it shed ~88% at 50 concurrent — right mechanism, wrong bound, and the measurement said so). FRAMEWORK GAP FIXED IN THE VENDORED TRUNK: the first working run's sheds FIRED and still answered **HTTP 500** (`shed: 32464`, `codes {"500":...}`) because Topcoat maps errors to statuses via a CLOSED downcast list and had no 429/503 constructor at all — added `ServiceUnavailableError` / `service_unavailable(retry_after_secs)` as a carried patch (Topcoat suite green: 288 + 67). RESULTS (3 samples, fresh store, WO-035 driver): 200-rung **9938 / 10874 / 12145 × HTTP 503**, 500-rung **13717 / 13872 / 11582 × 503** — ZERO 500s, zero timeouts, zero dropped connections; rungs 1/10/50 clean. `shed` moved **0 → 23655**, matching the 503 count EXACTLY (9938+13717). p99 at 500 concurrent **6666 ms → 959/895/964 ms**. REALTIME RECOVERY **20–60 s → 0 s**: 48/48 subscribes accepted at +0/+5/+20/+40/+60 s (WO-035: 47/48 REFUSED at +20 s), baseline 48/48 so non-vacuous — the probe's first run was WRONG (`/live/{table}` vs the real `/live/sse/{table}`) and reported a vacuous "recovered by +0s" on `0>=0`, so it now ABORTS with no verdict if the unloaded baseline is zero. A3 RE-RECORDED **~135 → ~146 req/s** (median of 3 at the knee) — the gain is `spawn_blocking` removing the 16-worker cap, NOT admission control; ceiling stays DB-bound. See [[2026-07-29 WO-038 desk admission control]].

Prior status — RE-RULED 2026-07-29. First-pass finding: handler-level admission control is STRUCTURALLY INERT (shed:0 measured two ways at 500-concurrent) — the queue forms in the tokio scheduler upstream of any handler code, because each kernel call blocks a worker inside `async fn` (the F1/rt-multi-thread finding come due). The prerequisite (unpin the worker) is folded IN: `Semaphore::try_acquire` → `spawn_blocking` reusing the WO-041 shared Agent; the semaphore bound IS the shed point. Two premise corrections banked (failure mode is dropped-connections not 500s; WO-035's "DB-bound" was throughput-only). Landed en route & ratified: shared Agent in Desk (WO-041-class fix), err500→typed 503+Retry-After, GET /admission, inert gates labelled ⚠ MEASURED INERT (no build log, no criteria claimed — honoured).
created: 2026-07-28
---

# WO-038: Desk Admission Control (Shed With Intent, Don't 500 By Accident)

> [!info] PM work order — **queued, not a v2.0-gate blocker.** WO-035 measured A3 and closed the assumption (Desk concurrency = ~135 req/s, DB-bound); this is the *quality* finding it surfaced. The gate passes with this named-and-measured, because the failure mode is now known, not assumed — but it's real production-hardening work for Milestone 4.

## The finding (WO-035)

Past the ~50-concurrent knee, the Desk returns **HTTP 500 for ~45% of requests at 200 concurrent, ~85% at 500** — it *fails* users under a spike rather than slowing them, the wrong failure mode for a UI tier. Measured details:
- **Cumulative:** a fresh stack driven straight at 200 gave *zero* failures; the escalating sweep gave 781. The failures are DB saturation propagating up, not a fixed per-request defect.
- **Realtime impaired >20 s post-load:** SSE subscribe 47/48 refused at +20 s after a load burst; 48/48 unloaded; recovered by ~60 s.
- Root cause is downstream (SurrealDB write saturation, the WO-026 ~150 w/s ceiling), surfacing as 500s at the Desk.

## The 2026-07-29 finding — admission at the handler is STRUCTURALLY INERT (measured, not mistuned)

The builder built handler-level admission control **two ways** and measured both at 500-concurrent across three sweeps. `shed` stayed **0** every time. The cause is not a threshold — it is the layer:

| observation | value |
|---|---|
| requests inside the Desk (Little: 157.8 req/s × 2.476 s p50) | **~391** |
| in-flight kernel calls a handler counter can see | **≤ 24** |
| kernel round-trip latency the Desk observed (EWMA) | **29 ms** |
| Desk page p50 at that same moment | **2 376 ms** |
| kernel errors, all sweeps | **0** |

~2 350 ms of every slow request queues **inside the Desk's tokio scheduler**, upstream of any handler code. Both candidate signals are blind: in-flight kernel calls are *already capped by the tokio worker count* (~16), and kernel latency stays low *because the kernel isn't the bottleneck*. Overload arrives as **dropped connections** (`code 0`), not a counter going up — so nothing at the handler can shed. (Same failure shape as WO-032: a `shed:0` that reads identical whether the gate works or *cannot possibly* work — the builder proved it can't, and refused to report the improved numbers as if the gate caused them.)

**Root cause = the blocking `ureq` call inside each `async fn`** (the F1 / rt-multi-thread finding, flagged since WO-032, now come due): every kernel call pins a tokio worker for the whole round trip → 16 workers cap concurrency → the 17th+ request queues in the scheduler where no handler code runs. Until the call stops blocking a worker, no handler-observable overload signal exists.

### Two premise corrections (WO-035)
1. **The failure mode is not 45%/85% HTTP 500s.** On today's stack it is dropped connections (`code 0`) — 22 at 200-concurrent, 6–91 at 500. Arguably worse: the client gets *nothing*. The 500s WO-035 saw were the Desk's *own* per-call-`Agent` transport defect (a WO-041-class leak), not the kernel.
2. **WO-035's "DB-bound, NOT the blocking-ureq structure" was correct FOR THROUGHPUT and is being over-read as "the structure is fine."** It is fine for the *ceiling* (~135, DB-bound) and *fatal for overload behaviour* (queue-in-scheduler → drop → admission-blind). Same shape as WO-041↔WO-026: measure the dimension you're claiming, not the adjacent one.

### Landed en route & ratified
- **Shared `ureq::Agent` in the Desk** — the identical WO-041 defect (fresh Agent per kernel call) in the tier this WO is about; would have contaminated the measurement. Fixed.
- **`err500` no longer collapses the kernel's typed 429/503 into a 500** — a kernel correctly saying "busy" reached users as "the Desk is broken"; capacity answers now survive the hop as **503 + `Retry-After`**.
- **`GET /admission`** exposes `inflight/served/shed/kernel_latency_ms`, dependency-free so it answers while the Desk is too busy to serve a page.
- The two inert gates are kept but **labelled `⚠ MEASURED INERT`** with the evidence table — tested-seam-not-wired applied to the builder's own code. Honoured: no build log, no criteria claimed met.

## Exit Criteria (RE-RULED 2026-07-29 — prerequisite folded in)

The finding makes admission and the async-structure fix **inseparable**: you cannot shed while a blocking call pins a worker, and the mechanism that unpins the worker is *also* the bound to shed on. One WO, stacked criteria.

1. **Unpin the worker (structural fix, lazy-correct):** wrap each kernel call in `tokio::sync::Semaphore::try_acquire` → `tokio::task::spawn_blocking` (reusing the WO-041 shared `Agent`, **no new HTTP-client dependency**). The async worker awaits the handle instead of blocking on the round trip, so the accept loop keeps accepting and the bound moves from "16 invisible workers" to "N explicit permits." `spawn_blocking`'s pool (default 512) sits above N by construction.
2. **Shed on the bound:** a full semaphore → `try_acquire` fails → **503 + `Retry-After` immediately** (typed, honest busy). `/admission` now reads live permit state — a *real* signal, not the inert one; every shed attributable in `/metrics`.
3. **Bounded recovery:** the post-load realtime-impairment window (WO-035: SSE subscribe 47/48 refused at +20 s) is bounded and stated.
4. **The number stays honest — amended:** criterion 4's *forbiddance of touching throughput was written on a now-falsified assumption* (that admission is orthogonal to the async structure). Reframed: the ceiling stays **DB-bound ~135 and admission is not credited with raising it** — but the structural fix's throughput effect is a **measured number, reported honestly**, not suppressed and not forbidden. If it moves the Desk-tier (A3) bound, **re-record that bound — don't let it drift.** ≥3 samples, fresh store, WO-035 driver.

## Boundaries

- **Lazy-correct first:** `spawn_blocking` + `tokio::sync::Semaphore` reusing the shared `Agent` — **NOT** a reqwest/hyper async-client migration. Reach for an async HTTP client only if the semaphore+spawn_blocking measurement proves it insufficient, and report the number that justifies it.
- Still failure-*mode* work: shed 503, don't drop/500. The ~135 ceiling is the WO-026 DB conversation, separate.
- The prerequisite is now IN this WO (the finding made them inseparable) — no separate WO-042.
- Prove it live/browser through the shipped Desk, not only a constructed test (the tested-seam≠wired standing check).

**Related:** [[Frust Hub]] · [[2026-07-28 WO-035 desk concurrent load]] (the finding) · [[2026-07-25 WO-013 tenant fairness]] (typed refusals) · [[2026-07-26 WO-026 surrealdb write concurrency]] (the DB ceiling) · [[v2.0 Deployability Gate]]
