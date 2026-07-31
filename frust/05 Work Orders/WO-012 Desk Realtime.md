---
tags: [frust, work-order, realtime, desk]
status: COMPLETED 2026-07-25 — realtime is product (browser-proven self-refreshing list); Desk relocated to D:\Dev\rust\frust-desk; budget re-measured 50→20 (ADR-011 amended); gate split ratified (floor as-specified + separate ≤2 ms tax gate); ticks carry {action,id} only; per-subscriber record JWT to the DB; monopoly gate caught its own author. → [[2026-07-25 WO-012 Desk realtime]]
created: 2026-07-25
---

# WO-012: Desk Realtime (ADR-011, Implemented)

> [!info] PM work order — results to `04 Build Log/`, live vault path verified first. Governing contract: [[ADR-011 Realtime]] — every design question is already answered there; deviations are escalations.

## Scope

The measured design becomes product: kernel websocket endpoint + Desk subscription lifecycle. Also the ruled Desk relocation executes here (first act, per WO-009 close-out): **`frust-proto` moves to `D:\Dev\rust\frust-desk`** — own workspace, pinned topcoat dep, REST+socket contract only.

## Exit Criteria

1. **Kernel socket endpoint:** session auth *before* upgrade (the #195 extractor property — a socket is a session, same bearer discipline as REST); per-session `LIVE SELECT` under the subscriber's own record session; subscription protocol scoped to (doctype, filter-shape) the filter contract can express.
2. **Focused-view lifecycle in the Desk:** subscribe on view focus, unsubscribe on blur/navigation — verified by metrics: parked-subscription count tracks *focused views*, not open tabs.
3. **The ~50/table budget, kernel-enforced with transparent fallback:** subscription #51 on a hot table gets a clean "budget" response and the Desk degrades that view to polling *without user-visible ceremony* (REQ-6.5.2). `/metrics` exposes per-table subscription gauges — the budget is observable before it's hit.
4. **Reconnect = refetch:** socket drop or kernel/surreal restart → Desk refetches the focused view under its own session and resubscribes; prove zero stale render and zero leak with the WO-011 harness pattern (a fleet-lite: 2 roles × several views through a restart).
5. **The leak partition, re-proven at the product layer:** clerk1's browser DOM never receives a manager row via socket — the WO-011 zero-leak result, now asserted against the actual shipped code path.
6. **The floor and the budget's premise hold in CI:** 25 ms release gate green with a parked-subscription load ≤ budget on the hot table (the ~70 µs/sub tax priced *into* the gate scenario, not around it).

## Boundaries

- No Topcoat auth/session machinery for sockets (🚫 bucket) — the kernel owns the session; the socket carries the same bearer.
- Realtime is enhancement-only: every view must remain fully functional with sockets disabled (polling path stays first-class — REQ-6.5.2 is a correctness requirement, not a courtesy).

## Escalations

Standard rules. Any push-path leak = stop-everything, same as the spike.

**Related:** [[Frust Hub]] · [[ADR-011 Realtime]] · [[WO-011 Live-Query Scale Spike]] · [[2026-07-25 WO-009 Desk v1]] · [[Topcoat]]
