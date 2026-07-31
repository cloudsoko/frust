---
tags: [frust, work-order, surrealdb, queue]
status: completed — verdict "viable-with-bridge", no instance #3 (record now 2-for-4); → [[2026-07-24 Live-query and event fidelity (WO-004)]], ADR-009 completed
created: 2026-07-25
---

# WO-004: Live-Query & Event Fidelity Spike

> [!info] PM work order — results to `04 Build Log/`, live vault path verified first. Runnable on this machine: `surreal.exe` + small consumers; reuse preserved artifacts, no new dep trees.

## Why

`LIVE SELECT` and `DEFINE EVENT` are the **last two never-exercised SurrealDB behaviors** in the architecture, against a 2-for-2 silent-failure record at v3.2.0 (#7432, #7433). LIVE SELECT gates ADR-009's table-as-queue bet — lost events in a queue aren't a bug report, they're lost jobs. EVENT gates ADR-009's one-resident DB tier (docstatus lattice). One spike closes both.

## Exit Criteria

### A — LIVE SELECT fidelity (gates the queue half of ADR-009)
1. **Zero loss, proven by sequence accounting:** a worker subscribed to a `job` table receives every insert (monotonic sequence numbers, gaps detectable) across: (a) subscription drop/reconnect, (b) `surreal.exe` restart, (c) concurrent-writer bursts (≥ 2 writers, ≥ 1 000 jobs).
2. **The catch-up story, characterized:** what does a worker down for 5 minutes see on resubscribe? (Expected: nothing — LIVE is not a log. The *architecture answer* — versionstamp-`SINCE` changefeed replay to bridge the gap, then LIVE for tail — should be exercised if so: prove the bridge overlaps or abuts, no gap between replay end and LIVE start.)
3. **Delivery-latency distribution, not a boolean:** p50/p95/p99 from insert to worker receipt under burst load — this is REQ-6.3's queue-latency datapoint arriving for free.

### B — DEFINE EVENT fidelity (gates the DB tier of ADR-009)
4. **Docstatus-lattice EVENT holds under concurrent-writer bursts:** an EVENT enforcing 0→1→2 legality (no edits at 1, no resurrection from 2) rejects **every** illegal transition and admits **every** legal one across concurrent writers — no silent admits, no false rejects.
5. **Error surface:** EVENT rejections `THROW` a stable machine code (`FRUST:E_DOCSTATUS:<reason>`); verify the code survives to the client error verbatim (feeds the kernel's typed-error mapping, ADR-009 A4).

## Escalation

- **Any silent miss (criterion 1 or 4) = SurrealDB silent-misbehavior instance #3** → STOP; per [[ADR-002 SurrealDB Lock-In]] the formal ADR re-read fires. That's a PM conversation, not a workaround.
- Loud failures (errors, disconnect notices) are findings, not escalations — characterize and continue.

## Deliverables

- [ ] Build log with A/B results, the latency distribution table, and the catch-up characterization
- [ ] Repro kept under `D:\Dev\rust\` if any bug found; upstream filing per prior practice
- [ ] Explicit verdict line: *"table-as-queue: viable / viable-with-bridge / dead"*

**Related:** [[Frust Hub]] · [[ADR-008 Data Shape#Parked for ADR-009 (recorded, not decided)]] · [[ADR-009 Execution Model]] · [[SRS]] (REQ-6.3, REQ-6.5)
