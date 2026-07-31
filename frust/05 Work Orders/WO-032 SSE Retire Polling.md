---
tags: [frust, work-order, realtime, sse, desk, v1.1]
status: COMPLETE (2026-07-28) - SSE replaces browser polling (1 stream, 0 polls, self-refresh proven); Desk concurrency MEASURED with a failing control: 160 subscribers (10x cores) healthy at 1.07x p50, naive std::thread::sleep control stalls at 24. Findings: (1) SSE relocates polling to Desk->kernel, does not eliminate it (kernel drain-based; streaming endpoint = kernel change, reported); (2) WO-014 c5 dirty-guard has no tick source - defined on /doc, called only on /list. See [[2026-07-28 WO-032 sse retire polling]].
created: 2026-07-28
---

# WO-032: SSE — Retire the Desk Polling Loop (+ Measure Desk Concurrency)

> [!info] PM work order. WO-029 brought SSE in-tree (ADR-011's push transport); this retires the Desk's polling loop for it. **Carries a ruled-in concurrency criterion** (below) because SSE long-lived connections are exactly where the Desk's blocking-`ureq`-in-async worker-thread cap stops being theoretical. Governing: [[ADR-011 Realtime]], [[2026-07-25 WO-012 Desk realtime]] (WS RPC + polling), [[Topcoat]] (SSE #218).

## Exit Criteria

1. **SSE replaces polling:** the Desk holds an SSE stream and receives `{action, id}` ticks (ADR-011: ticks carry NO row data — refetch through the read door under the subscriber's session). The 60 s meta-poll / tick-poll loop is retired. Prove via network log: a scriptless focused list shows **one long-lived SSE connection, zero poll requests**, and self-refreshes on an out-of-band write.

2. **THE CONCURRENCY CRITERION (ruled in 2026-07-28): SSE must NOT pin an OS thread per subscriber.** The Desk uses blocking `ureq` inside `async fn` handlers; an SSE handler that sources kernel events via a blocking call pins a tokio worker thread for the subscription's whole lifetime → core-count subscribers stalls the Desk. **Measure it the way WO-024 measured the kernel:** N > cores concurrent SSE subscribers all receiving events, and the Desk still serves ordinary requests at acceptable latency. State the measured concurrent-subscriber number. If the naive shape caps at core-count, the fix (`spawn_blocking`, async client, or a multiplexed shared subscription so one kernel stream fans out to many browsers) is **in scope** — SSE is what makes the cap load-bearing. This closes the Desk-tier's bounded-by-assumption concurrency for the v2.0 gate.

3. **Permission-aware push preserved:** ticks flow under the subscriber's own session; a clerk's SSE stream never carries another's row events (the WO-011 zero-leak property, re-proven on the SSE path, not assumed to transfer).

4. **Graceful degradation (REQ-6.5.2):** SSE failure falls back to polling transparently — realtime stays an enhancement, never a correctness dependency. Prove the fallback path still works.

5. **Browser proof — committed and re-runnable:** the list self-refreshes via SSE in a real browser. Drive via Playwright MCP if convenient, but **seal on a committed artifact** (a Playwright spec in the repo, ideally codegen'd from the MCP session). MCP-only is an unrepeatable observation; this WO's lineage is "keep the proof, not just witness it." Standing methodology ruling.

6. **No regression:** full suite green; WO-012's ~20-subs/table budget and the WO-014 dirty-guard (a live tick must not discard unsaved input) still hold on the SSE path.

## Boundaries

- Retire polling, don't rebuild the kernel realtime side (WO-011/012 per-session LIVE + budget stands). If SSE needs a kernel change, that's a finding — report it.
- If criterion 2's fix is bigger than `spawn_blocking` (a `frust-desk`-wide async refactor, or an async kernel client that ripples), **report before building** — that's a scope call, possibly its own WO.

## Escalations

Standard rules + full hygiene set. The concurrency measurement is the load-bearing new work; the SSE wiring is the easy part (same shape as WO-025: the concurrency is easy, the correctness/measurement under it is the WO).

**Related:** [[Frust Hub]] · [[ADR-011 Realtime]] · [[2026-07-25 WO-012 Desk realtime]] · [[2026-07-26 WO-024 load and footprint benchmark]] (the kernel-tier precedent) · [[Topcoat]]
