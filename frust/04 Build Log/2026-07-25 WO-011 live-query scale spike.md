---
tags: [frust, build-log, realtime, surrealdb, spike, work-order]
created: 2026-07-25
work-order: "[[WO-011 Live-Query Scale Spike]]"
---

# Build Log — WO-011: Live-Query Scale Spike

**Verdict line, up front (criterion 5):** *Per-session LIVE: latency-viable to N=1000, but the write-side tax (~70 µs per parked subscription per write on the subscribed table) erodes the 25 ms submit floor from ≈N=50 subscriptions per hot table. ADR-011 default posture: **live-for-focused-view** (subscribe only the visible list, unsubscribe on blur — bounding subs/table to concurrent viewers), polling elsewhere. **Multiplexing: not needed, therefore rejected** — the one-door weakening it would entail (kernel-side permission filtering on the push path) never has to happen at Desk-realistic scales, and that is the reason to refuse it, stated out loud per the WO's honesty clause.*

**Harness:** `D:\Dev\rust\frust-skel\wo011\fleet.mjs` (Node 22, zero deps — WO-004's bones grown into a fleet). Record-user WS sessions (argon2 signin, the kernel identity posture), each holding `LIVE SELECT * FROM ticket` under the kernel's null-safe permission clause; root writer round-robins rows across 50 clerk identities; 2 manager sessions per rung as fan-out-heavy subscribers. Substrate probe before every rung (threshold recalibrated per client stack: node-fetch healthy ≈ 20-25 ms; the Rust release gate at its historic 22 ms cross-confirmed the substrate — the post-reboot machine also retro-closed WO-010's reboot-recheck item, floor green at 22 ms).

## Criterion 1 — the fleet scaling curve

200 writes at 20/s per rung; latencies in ms, node-client overhead ≈ 20 ms included (deltas are the signal):

| N sessions | setup/session p50 | write ack p50/p95 | notify p50/p95/p99 | owed notifications | loss | idle CPU (15 s) | surreal WS |
|---|---|---|---|---|---|---|---|
| 10 | 123 | 21.6 / 26.7 | 21.8 / 26.9 / 40 | 432 | **0** | +0.03 s | ~120 MB |
| 100 | 161 | 27.2 / 36.8 | 27.5 / 36.8 / 44 | 792 | **0** | +0.02 s | 212 MB |
| 500 | 149 | 57.6 / 98.6 | 58.4 / 99.2 / 122 | 2,392 | **0** | +0.02 s | 223 MB |
| 1000 | 143 | 91.3 / 182.8 | 92.2 / 172.7 / 352 | 4,392 | **0** | +0.02 s | 913 MB |

**The cost model, clean:** parked LIVE queries are FREE at idle (0.02 s CPU over 15 s even at 1000) — the entire cost is **write-time matching, linear in subscriptions on the written table**: (91.3−21.6)/990 ≈ **70 µs per subscription per write**. WO-004's at-commit property held at fleet scale at every rung: notify ≈ write-ack (the notification adds nothing on top of the taxed write).

## Criterion 2 — permission enforcement on the push path: ZERO leaks

Every notification at every rung and phase was checked against the receiving session's clause: **0 leaks in ~8,800 delivered notifications** across 4 rungs + churn + restart. Clerk sessions received exactly their own rows; manager sessions everything — the WO-002 partition, now proven where Frappe never proved it (REQ-6.5.1). The stop-everything finding did not occur.

## Criterion 3 — fidelity under fleet conditions: zero loss, everywhere

- **Steady rungs:** 0 missing of 432 / 792 / 2,392 / 4,392 owed.
- **Churn (N=100, 20 dropped + 20 joined mid-burst):** 0 missing of 1,168 owed to continuous sessions; joiners owed only post-live-ack rows by design.
- **Surreal restart with the fleet attached:** all sockets drop; **reconnect storm of 100 sessions completes in 0.7 s**; post-restart 0 loss of 198 owed.
- **The per-subscriber replay question, answered by the DB itself:** a record session running `SHOW CHANGES` is **refused loudly** (`IAM error: Not enough permissions`, typed, 54 µs). Per-subscriber changefeed replay therefore cannot be DB-direct — and routing it through the kernel would put kernel-side permission filtering on the recovery path. **Verdict: reconnect = refetch** — on reconnect the client re-runs its list query under its own session (DB-enforced permissions, zero new trust surface); the changefeed-replay bridge remains what WO-004 built it as: a root/worker tool. No silent-misbehavior instance #3: every behavior did what its documentation says.

## Criterion 4 — the degradation boundary

Polling never wins on **latency** (push p50 at N=1000 is 92 ms; a 60 s poll averages 30 s stale). The boundary is the **writer's floor**: submit budget headroom over the 22 ms baseline is ~3 ms, and the tax model `+0.07 ms × subs_on_table` spends it at **≈N=50 subscriptions per hot table** (≈100 ⇒ +7 ms, floor breached; 500 ⇒ 2.6× floor). So REQ-6.5.2's fallback trigger is a **per-table subscription budget (~50)**, enforced where subscriptions will live (the kernel), with overflow sessions degrading to polling transparently. Live-for-focused-view keeps real deployments under the budget naturally — subs/table ≈ users actively viewing that list.

## ADR-011 inputs (beyond the verdict line)

1. Subscription lifecycle: subscribe on list focus, unsubscribe on blur; budget ~50 live subs per table, kernel-enforced, poll fallback above it.
2. Recovery: reconnect = refetch under the subscriber's own session. No per-subscriber replay, no kernel filter surface on the push path.
3. Setup cost ≈ 150 ms/session (argon2 signin dominates) — amortized per session, not per view; the kernel's session layer already holds the JWT, so Desk view-switches reuse the socket.
4. Memory at the extreme: ~0.9 GB surreal working set at 1000 connections — fine for the rungs that matter (≤100/table).
5. Client-stack honesty: absolute numbers include ~20 ms node overhead; the kernel's own socket forward (Rust) will sit below these curves, not above.

## Housekeeping
Spike only — no Desk socket code shipped, per the WO. Harness kept for re-runs (`node fleet.mjs <N>` / `churn`). The `wo011` scratch database remains in the dev store (small); substrate probes passed before every rung.

## Related
[[WO-011 Live-Query Scale Spike]] · [[2026-07-24 Live-query and event fidelity (WO-004)]] · [[SRS]] (REQ-6.5) · [[SurrealDB]] · [[2026-07-25 WO-010 Observability]]
