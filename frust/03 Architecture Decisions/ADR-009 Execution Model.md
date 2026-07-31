---
tags: [frust, adr, execution, surrealdb]
status: accepted — logic placement 2026-07-25; queue half completed on WO-004 evidence
---

# ADR-009: Execution Model — Logic Placement & the Job Queue

## Half 1 — Logic Placement (ACCEPTED, as amended by grill)

**Default: the kernel.** Debuggable, versionable, traced (REQ-6.4.1). The DB tier admits an invariant only by passing the **two-clause admission test**:

> 1. *If the kernel has a bug, must this still hold?* — AND —
> 2. *Is it expressible as a pure record-local invariant?* (`$value`/`$before`/`$auth` only — no lookups, no workflow metadata, no session context, no cross-document state)

**The EVENT tier has exactly one resident: the docstatus lattice** — 0→1→2, no edits at 1 except allowlisted fields, no resurrection from 2. Fixed, universal, record-local, tiny.

**A1 — Audit stamps are written OUT of the EVENT tier.** The authoritative audit is the changefeed (REQ-3.2.1) — it survives kernel bugs by not involving the kernel. `modified`/`modified_by` stamps are queryable conveniences expressible as `DEFINE FIELD … VALUE` — field definitions, not events. (Changefeed retention is finite; the stamps are the long-lived queryable trace — still fields.)

**A2 — Lattice ≠ workflow, split explicitly:** *EVENTs enforce the docstatus lattice; workflow transition rules (REQ-4.1.2 — role-based, per-DocType, per-tenant runtime metadata) are kernel logic evaluated before the kernel attempts the transition.* The EVENT is the floor, not the workflow engine. Workflow rules in EVENT bodies = unversionable business logic in DB strings = Server Scripts with extra steps; treat as an incident.

**A3 — Error surface:** EVENT rejections `THROW` stable machine codes (`FRUST:E_DOCSTATUS:<reason>`); the kernel maps codes to typed errors. Verified as WO-004 criterion 5.

**A4 — Threat model, named so the tier doesn't grow:** EVENTs defend against **kernel bugs, ops consoles, and root-credential migrations** — nothing else (plugins are already brokered per ADR-006; Tier-2 scripts have no raw writes). *The list is expected to stay at one or two entries; growth is a smell* — this is P-3.2's four-layer validation creep, pre-named.

**A5 — WO-003 scope consequence:** the orm adapter's snapshot/diff vocabulary is `TABLE | FIELD | INDEX`; syncing docstatus EVENTs adds `EVENT` to it — parse, snapshot, diff, classify, including event-body definitional drift. Flagged in [[WO-003 Engine Integration]].

## Half 2 — Job Queue (ACCEPTED — WO-004 verdict: **viable-with-bridge**)

**Decision: table-as-queue.** `job` DocType + workers; `enqueue` (ADR-006) writes a record; job states queryable as data (REQ-6.3.3); zero extra processes. Evidence: [[2026-07-24 Live-query and event fidelity (WO-004)]] — 3,200/3,200 delivered across bursts, subscription drops, and a mid-subscription restart; delivery is **at-commit** (98–100% of notifications beat the writer's own commit-ack); p50 32 ms batch-inclusive upper bound (REQ-6.3 datapoint).

**Architecture: the bridge IS the worker, not a fallback.** The worker loop is *replay-from-cursor (versionstamp changefeed) → LIVE tail → advance cursor*. LIVE SELECT is a **latency optimization over a changefeed-backed log** — never the source of truth. (A naive LIVE-only worker missed 300/300 dark-window inserts; the bridge recovered every one, including 330 that provably crossed a server restart.)

**The two design items WO-004 left to this ADR, decided:**
1. **Claiming: delivery ≠ claim.** Delivery (LIVE or replay) is advisory; the serialization point is an **atomic conditional claim** — single-statement `UPDATE … SET status='claimed', worker=$w WHERE id=$job AND status='queued'` — winner runs, losers move on. This makes duplicate delivery harmless by construction, which is what makes viable-with-bridge *safe*, not just workable.
2. **Retention is a queue parameter, and the table is the source of truth.** Changefeed retention on `job` = max *bridgeable* worker downtime (efficiency bound, not correctness bound): a worker beyond retention **rescans the table** (`status='queued'`) instead of replaying — jobs are records, so cold-start recovery is a query. Retention on `job` is therefore sized for ops (worker outage tolerance), independent of audit retention policy.

**Result:** REQ-5.1.1/REQ-6.3 land on the same two-process footprint as everything else. No Redis, no broker, no fourth process — P-2.3's stack-collapse thesis holds through the last subsystem that traditionally breaks it.

> [!success] Rulings executed (WO-005 module 5, 2026-07-24)
> **Claim ruling:** 6 workers × 200 jobs, every worker attempting every job — exactly-once = 200/200, double-claims = 0 across 1,200 contended attempts; **attempts-per-claim ≈ 1.01** (optimistic concurrency is a non-event at queue pressure). **Retention ruling:** cursor-less worker drained the queue by `status='queued'` rescan alone. **Authority ruling (ADR-006):** revoked-before-run principal → typed `Denied`, terminal, never requeued. [[2026-07-24 Module 5 close — worker loop]]

**Related additions:** [[2026-07-24 Live-query and event fidelity (WO-004)]]

**Related:** [[Frust Hub]] · [[ADR-002 SurrealDB Lock-In]] · [[ADR-006 Plugin Capability Surface]] · [[ADR-008 Data Shape]] · [[WO-004 Live-Query and Event Fidelity]]
