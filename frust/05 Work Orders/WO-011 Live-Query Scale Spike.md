---
tags: [frust, work-order, realtime, surrealdb, spike]
status: COMPLETED 2026-07-25 — verdict: per-session LIVE latency-viable to N=1000; governing constraint is the WRITER (~70 µs/sub/write → ~50/table budget); zero leaks in ~8,800 notifications; zero loss through churn+restart; multiplexing rejected-as-unneeded; reconnect=refetch. → [[ADR-011 Realtime]] · [[2026-07-25 WO-011 live-query scale spike]]
created: 2026-07-25
---

# WO-011: Live-Query Scale Spike (Gates ADR-011 Realtime)

> [!info] PM work order — results to `04 Build Log/`, live vault path verified first. Spike, not feature: no Desk socket code ships from this WO.

## Why

The push transport exists (Topcoat #195) and the design position is on record: **kernel-side per-session `LIVE SELECT` → websocket forward** — realtime with row permissions enforced by the DB per subscriber, the property Frappe's socket.io never had (P-2.4, REQ-6.5.1). What's unmeasured is the only thing that matters: WO-004 proved *one* worker's LIVE fidelity; a Desk fleet means **a LIVE query per open list view per session**. The risk list's last unmeasured behavior. No ADR-011 until these numbers exist.

## Exit Criteria

1. **Fleet scaling curve:** N concurrent record-user sessions, each holding a `LIVE SELECT` on a permission-filtered list shape, N ∈ {10, 100, 500, 1000}. Measure: notification latency p50/p95 under a steady write load, kernel + surreal CPU/memory per rung, connection setup time. **Run the substrate probe before every rung** (WO-010 caveat).
2. **Permission enforcement at the notification layer, proven:** clerk1's live subscription must NOT receive events for rows outside their permission clause (the WO-002 partition, now on the push path). A single silent leak here is worse than no realtime — treat any leak as a stop-everything finding.
3. **Fidelity under fleet conditions:** the WO-004 sequence-accounting harness rerun with a fleet attached — zero loss for every subscriber through subscription churn (sessions joining/leaving mid-burst) and one surreal restart (the bridge/replay story per subscriber, or an honest "reconnect = refetch" verdict if per-subscriber replay is the wrong shape).
4. **The degradation boundary, characterized:** the rung where latency/CPU makes polling competitive again — that number decides ADR-011's *default posture* (live-for-focused-view vs live-for-everything vs hybrid). REQ-6.5.2's transparent-fallback gets its trigger threshold from this.
5. **Verdict line:** *per-session LIVE viable to N=___ / viable-with-multiplexing (one LIVE per shape, kernel fans out with per-session filtering) / dead → polling stays.* If multiplexing is the verdict, state what the kernel-side filter re-check costs — it moves permission enforcement from DB to kernel for the push path, which weakens the WO-002 property and must be said out loud, not slid into.

## Escalations

Standard rules — silent misbehavior = instance #3 = stop, ADR-002 re-read. Criterion 2 leak = stop regardless of loudness.

**Related:** [[Frust Hub]] · [[Topcoat]] (#195, pin governance) · [[2026-07-24 Live-query and event fidelity (WO-004)]] · [[SRS]] (REQ-6.5) · [[SurrealDB]] (risk list) · [[2026-07-25 WO-010 Observability]] (/metrics reads the rungs)
