---
tags: [frust, adr, realtime, desk, surrealdb]
status: accepted
decided: 2026-07-25
---

# ADR-011: Realtime — Per-Session LIVE, Focused-View Posture, Writer-Budgeted

**Context:** The push transport exists (Topcoat #195, pin governance in [[Topcoat]]); [[WO-011 Live-Query Scale Spike]] measured the last unmeasured risk-list behavior ([[2026-07-25 WO-011 live-query scale spike]]). REQ-6.5; kills P-2.4 outright.

## Decision

**Kernel-side per-session `LIVE SELECT` → websocket forward.** Each subscriber's live query runs under *their own record session* — row permissions enforced by the DB per subscriber, the property Frappe's socket.io never had. Proven: **zero leaks across ~8,800 notifications** (clerks received exactly their clause, through churn and a restart).

**Posture: live-for-focused-view.** Subscribe on view focus, unsubscribe on blur. Not live-for-everything — and the reason is the spike's central finding: **the constraint is the writer, not the reader.** Parked subscriptions are free at idle (0.02 s CPU/15 s at N=1000) and push latency never degrades to polling-competitive (p50 92 ms at N=1000 vs 30 s poll staleness) — but **every write pays ~70 µs per parked subscription on its table**, eroding the 25 ms submit floor from ≈50 subscriptions per hot table.

**The budget: ~~50~~ → 20 live subscriptions per table** *(amended 2026-07-25 — WO-012 priced the tax into CI kernel-side and the number moved to fit the measurement: +1 ms at 5–20 subs, +2 at 30, +4 at 40, bracketed against drift; the spike's client-side ~70 µs/sub was optimistic)*, kernel-enforced, with transparent poll fallback. This is REQ-6.5.2's trigger, a measured number twice over. Focused-view lifecycle keeps realistic deployments under it naturally; the budget is the backstop, not the mechanism.

**Hardening (WO-012, ratified): ticks carry `{action, id}` only — never row data.** Clients refetch through the normal read door; the push path cannot become a second data path with its own permission story. A leak can't ride a payload that doesn't exist.

**CI structure (WO-012 judgment call, ratified):** REQ-6.1.1's floor gate runs *without* subscriptions, exactly as specified; the realtime tax has its **own** gate asserting delta-over-in-run-baseline ≤ 2 ms at budget. Drift moves both halves together, so the gate judges realtime alone — and its failure remedy is *lower the budget, never widen the allowance*. Parking subs under the single 25 ms gate would have silently re-spec'd the write-path contract to absorb an optional feature.

**Recovery: reconnect = refetch** under the subscriber's own session. Per-subscriber changefeed replay is rejected *by the DB's own posture* — record sessions are refused `SHOW CHANGES` with a loud typed IAM error, and routing replay through the kernel would create the exact filter surface the multiplexing clause warns about. The changefeed bridge stays what WO-004 built: a root/worker tool. Reconnect storm measured: 100 sessions in 0.7 s, zero post-restart loss.

## Rejected

- **Multiplexing (one LIVE per shape, kernel fans out with per-session filtering)** — rejected because *unneeded at Desk-realistic scales*, and therefore the kernel-side permission-filter surface it requires never has to exist. Stated as the reason, not slid past: if a future scale demand resurrects this, it re-opens this ADR **and** weakens the one-door property — that trade gets made in daylight.
- **Live-for-everything** — writer-tax math above.
- **Polling as the primary** — never latency-competitive; remains the fallback (REQ-6.5.2) past the budget or on socket failure.

## Evidence base

Zero loss at every rung (10/100/500/1000) + 20-drop/20-join churn + full restart with fleet attached; at-commit delivery held at fleet scale (notify ≈ write-ack); ~150 ms session setup amortized on the existing kernel session layer; substrate probe run before every rung (with the per-client-stack calibration refinement now in the caveat).

**Related:** [[Frust Hub]] · [[WO-011 Live-Query Scale Spike]] · [[2026-07-24 Live-query and event fidelity (WO-004)]] · [[ADR-009 Execution Model]] · [[SRS]] (REQ-6.5) · [[Topcoat]]
