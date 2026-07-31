---
tags: [frust, build-log, surrealdb, queue, work-order]
created: 2026-07-24
work-order: "[[WO-004 Live-Query and Event Fidelity]]"
---

# Build Log — Live-Query & Event Fidelity (WO-004)

**Verdict line, up front: `table-as-queue: viable-with-bridge`.** And the headline nobody should bury: **no silent-misbehavior instance #3.** Both never-exercised behaviors did exactly what their documentation says under every abuse this spike threw at them. The ADR-002 tripwire does not fire.

**Repro:** `D:\Dev\rust\frust-skel\wo004\` — `live.mjs` (criteria A1–A3), `event.mjs` (B4–B5). Node 22, zero dependencies (native WebSocket + fetch against `surreal.exe` v3.2.0, surrealkv, ns `frust` db `wo004`). Disclosed deviation: consumers are Node scripts, not the wasmtime host — even fewer dep trees than the WO suggested; nothing in these criteria touches WASM.

## A — LIVE SELECT fidelity

| Test | Result |
|---|---|
| A1c: concurrent burst — 2 writers × 1,000, batches of 50 | **2,000/2,000 delivered via LIVE alone, zero missing** |
| A1a: subscription drop + 300 dark-window inserts | Naive resubscribe delivers **0** of them (see A2); bridge recovers **300/300** with writers writing *through* the seam — no hole |
| A1b: `surreal.exe` restart mid-subscription, writer riding through with retries | **330 inserts crossed the outage gap; all 330 recovered via changefeed replay** — final accounting 3,200/3,200, zero missing |
| A2: catch-up characterization | **LIVE is not a log — confirmed and documented behavior**, not a bug: a returning subscriber sees only new events. The architecture answer is therefore mandatory infrastructure, not an error path: **cursor = changefeed versionstamp; on (re)connect open LIVE first, replay `SHOW CHANGES SINCE cursor`, dedupe by id, then consume the tail.** Proven holeless twice (reconnect and restart), reusing exactly the primitive WO-002 validated. |
| A3: delivery latency under burst | insert-send → notify: **p50 32 ms / p95 62 ms / p99 69 ms** (upper bound — includes batch HTTP round-trip). The sharper number: **98–100% of notifications arrived *before* the writer's own commit-ack** (p50 −0.5 ms) — LIVE push is effectively at-commit. REQ-6.3's queue-latency datapoint: delivery adds no meaningful latency on top of the write itself. |

## B — DEFINE EVENT fidelity (docstatus lattice)

| Test | Result |
|---|---|
| B5: error surface | `THROW 'FRUST:E_DOCSTATUS:…'` reaches the client **verbatim** (wrapped: `Error while processing event docstatus_lattice: An error occurred: FRUST:E_DOCSTATUS:ILLEGAL_TRANSITION_1_0` — stable substring, kernel mapping trivial). The rejected write **did not persist** — THROW aborts the transaction, which was the load-bearing claim. |
| B4a: deterministic legal sequence | Zero false rejects (edit@0, 0→1, 1→2 all admitted) |
| B4b: concurrent burst — 4 writers × 150 random transition attempts | 180 admitted, 420 rejected, **420/420 rejections carried the machine code**, zero other errors |
| B4c: truth audit — changefeed replay of every persisted transition, checked pairwise | **48 persisted transitions, zero illegal — no silent admits under concurrency** |

## Consequences for ADR-009 (queue half, now unblocked)

1. **The bridge is the worker, not a fallback.** A Frust queue worker's loop is: replay-from-cursor → LIVE tail → advance cursor. LIVE alone is a latency optimization on top of a changefeed-backed log — that mental model, written into the ADR, is what "viable-with-bridge" means.
2. Job *claiming* (atomic `claimed_by` update so two workers can't take one job) is ADR-009 design work the spike deliberately did not cover — delivery fidelity and claim atomicity are separate problems.
3. The `FRUST:E_DOCSTATUS` convention works end-to-end under load; the kernel's typed-error mapping can key on the stable substring (ADR-009 A4 satisfied empirically).
4. Changefeed retention (`CHANGEFEED 1d` here) bounds the maximum worker downtime that replay can bridge — retention policy is now a *queue* parameter, not just an audit parameter (feeds REQ-6.3's job semantics and the ADR-008 retention notes).

## Process notes

- One harness bug during the run (latency samples raced the commit-ack) — fixing it *produced* the at-commit finding. One re-run to make P4 honest: the first pass's restart landed after the writer finished, so no inserts actually crossed the outage; the second pass proved the real thing (330 gap-crossers). Logged because the difference between "restart happened during the phase" and "writes crossed the outage" is exactly the kind of gap this WO exists to close.
- Nothing to file upstream: both features behaved as documented. The 2-for-2 silent-failure record stays at 2-for-4 after this spike.

## Related

[[WO-004 Live-Query and Event Fidelity]] · [[ADR-009 Execution Model]] · [[ADR-008 Data Shape]] · [[ADR-002 SurrealDB Lock-In]] · [[2026-07-24 Architecture skeleton (WO-002)]] · [[SRS]] (REQ-6.3, REQ-6.5)
