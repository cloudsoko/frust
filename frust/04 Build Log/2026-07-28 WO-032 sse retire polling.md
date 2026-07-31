---
tags: [frust, build-log, sse, realtime, desk, concurrency, work-order, v1.1]
created: 2026-07-28
work-order: "[[WO-032 SSE Retire Polling]]"
status: complete — SSE replaces browser polling; Desk-tier concurrency MEASURED with a failing control (160 subscribers = 10x cores, no stall)
---

# Build Log — WO-032: SSE, and the Desk Concurrency Measurement

The SSE wiring took an afternoon. The measurement is the work — and it produced
a control that fails, a corrected metric, and two findings.

## The design, and why it does not pin a thread (criterion 2)

**The probe that decided the design:** the kernel's `/events/{sub}` is a
**non-blocking drain** — `realtime.rs::drain` locks a mutex, empties a `Vec` and
returns. There is no long-poll to hold open. That single fact is what makes a
cheap SSE possible: the stream sleeps with `tokio::time::sleep` (which *releases*
the worker) and only occupies a thread for the ~1 ms drain call itself.

```rust
let events = stream::unfold(Some(guard), move |guard| async move {
    tokio::time::sleep(Duration::from_millis(LIVE_DRAIN_MS)).await;   // releases the worker
    let (code, body) = kernel::call(.., &format!("/events/{sub}"), ..); // ~1 ms
    ...
});
Ok(Sse::new(events).keep_alive(KeepAlive::new()))
```

A `std::thread::sleep` there — the naive shape — pins one OS worker for the
subscription's entire lifetime. That is the failure the PM ruled in as a design
constraint, and it is real: see the control below.

`SubGuard` unsubscribes on drop, so a closed browser connection returns its slot
to the WO-012 per-table budget immediately rather than waiting out the kernel's
30 s idle reaper.

## THE MEASUREMENT (criterion 2)

16 cores → 16 tokio workers. Subscribers spread across 8 tables because the
kernel budget is 20 subs **per table** (WO-012).

| | shipped (`tokio::time::sleep`) | control (`std::thread::sleep`) |
|---|---|---|
| subscribers | **160** (10× cores) | **24** (1.5× cores) |
| streams live / stalled | **160 / 0** | **1 / 24** |
| ordinary GET p50 | 24.7 → **26.5 ms** | 25.5 → **10,009 ms** |
| timeouts | **0 / 20** | **20 / 20** |
| p50 ratio | **1.07×** | **392×** |
| verdict | **DESK STILL SERVING** | **STALLED** |

**160 concurrent SSE subscribers, 1031 events delivered, no measurable cost to
ordinary requests.** The control — same code, one line changed, behind a
`naive-blocking-sse` feature that is never shipped — stalls the entire Desk at
24 subscribers. The measurement can fail, so its pass means something.

### The metric that was decorative — and how it was caught

My headline metric was **OS threads per subscriber**. It read **0.000 in the
stalled control too.**

Tokio does not grow its worker pool for *blocked* tasks — it has a fixed pool
and blocking simply starves it. So thread count is invariant under exactly the
failure it was chosen to detect: a fully-stalled Desk and a healthy one report
the identical number. Had the control not been run, this WO would have shipped
"0.000 threads per subscriber — criterion satisfied" as a **meaningless pass**.

The discriminating measures are *streams stalled* and *ordinary-request
timeouts*. The bench now leads with those and keeps the thread count only as a
labelled non-discriminator, with the reason written next to it.

That is the **sixth instance** of the standing check — *assert the outcome you
need, not the operation you performed*. I measured a proxy (threads) when the
outcome I needed was "can the Desk still serve." The correction came from the
control, which is the general lesson: **a metric you have never seen fail is not
yet a metric.**

Note on the criterion's own wording: "must not pin an OS thread per subscriber"
describes the *mechanism*; the failing build satisfies it literally (0 threads
per subscriber) while being completely stalled. The intent — *the Desk keeps
serving* — is what the bench now asserts.

### One false alarm, correctly diagnosed

A first run at 160 died with `ECONNREFUSED`. The Desk was **still alive and
LISTENING** and the kernel was still serving: it was the *load generator*
exhausting Windows ephemeral ports opening 160 sockets in a tight loop
(`TIME_WAIT` pileup). Staggering the opens by 25 ms fixed it. Reporting that as
"the Desk caps at 160" would have been a fabricated finding; the bench now
counts client connect errors separately from Desk refusals so the two can never
be conflated.

## Criteria 1, 3, 4 — the committed browser proof

Per the standing methodology ruling, sealed as a re-runnable spec
(`wf-proof/sse.spec.mjs`), not an MCP observation. All checks pass:

- **1 — polling retired:** on a focused list, **exactly 1 SSE connection and 0
  `/live/events` polls in 6 s** (the old interval was 3 s, so ≥2 polls would
  have appeared).
- **1b — push works:** an out-of-band kernel write (no browser involvement)
  makes the watching page refresh itself.
- **3 — permission-aware push preserved:** a clerk's stream ticks for a
  clerk-owned write but **does not tick** for a manager-owned row it cannot
  read. The two halves are the control for each other — silence alone would
  have proved nothing, so the same stream is shown ticking and not-ticking.
- **4 — graceful degradation:** with the SSE route aborted at the network
  layer, the page falls back to polling and keeps updating (REQ-6.5.2 — realtime
  is an enhancement, never a correctness dependency).

## Findings

### 1. SSE relocates polling, it does not eliminate it (honest bound)

The browser now holds one long-lived stream — criterion 1 as written. But the
kernel's realtime API is **drain-based**, so the *Desk* still polls the kernel
once a second on each subscriber's behalf. End-to-end push would need a
streaming/long-poll kernel endpoint. That is a kernel change and outside this
WO's boundary, so it is **reported, not built**. The win is real (the browser
round trip and its latency/battery cost are gone, and the transport is now the
one ADR-011 specified) but it should not be described as "polling is gone."

### 2. WO-014 criterion 5's dirty-guard has no tick source (pre-existing)

`window.__frustOnTick` is **defined** only in `dirty_guard()` — rendered on
`/doc/{doctype}/{key}` — and **called** only in `live_updates()` — rendered on
`/list/{doctype}`. The two never appear on the same page. So on the doc form the
guard is installed and never invoked: the `stale-note` banner cannot appear, and
the "a tick must not stomp unsaved input" protection has never actually run in
the shipped Desk.

This is **not a regression from this WO** (my change routes both transports
through the same `onTick`, preserving the contract exactly) and it is **not a
data-loss risk** — no tick reaches the form, so nothing can be stomped. The real
cost is a *missing* feature: doc forms have no realtime and no staleness
warning at all.

It is another **tested-seam-not-wired** instance: the guard was built and proven,
but its two halves live on pages that never co-occur. Fixing it means giving doc
pages realtime, which is a behaviour change beyond "retire polling" — **reported
for a ruling, not shipped.**

## Verification

- **Concurrency bench:** `wf-proof/sse-bench.mjs` — 160 subs healthy (exit 0);
  naive control 24 subs stalled (exit 1).
- **Browser spec:** `wf-proof/sse.spec.mjs` — 8/8, committed and re-runnable.
- **Kernel suite:** unchanged by this WO (Desk-only), re-run for the close tally.
- Desk builds clean; the control lives behind a never-shipped feature flag.

## Files

- `frust-desk/Cargo.toml` — `topcoat/sse`, `futures-core`, `futures-util`,
  explicit `tokio/time`, and the `naive-blocking-sse` control feature.
- `frust-desk/src/main.rs` — `live_sse` route + `SubGuard`; `live_updates`
  client rewritten to EventSource with a polling fallback sharing one `onTick`.
- `wf-proof/sse-bench.mjs`, `wf-proof/sse.spec.mjs` — the measurement and the proof.

## Related
[[WO-032 SSE Retire Polling]] · [[ADR-011 Realtime]] · [[2026-07-25 WO-012 Desk realtime]] · [[2026-07-26 WO-024 load and footprint benchmark]] (the kernel-tier precedent) · [[2026-07-28 WO-029 topcoat v0.5.0 adoption]] (brought SSE in-tree) · [[tested-seam-not-wired]] · [[assert-outcome-not-operation]]
