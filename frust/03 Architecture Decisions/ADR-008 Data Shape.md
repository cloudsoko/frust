---
tags: [frust, adr, data-shape, surrealdb]
status: accepted
decided: 2026-07-25
---

# ADR-008: Data Shape — Embedded Children by Default; Self-Hosted Metadata, Binary-Authoritative

**Context:** Two day-one open decisions from [[SurrealDB#Open Design Decisions]] became load-bearing for the [[Frust Hub#Next Foundational Milestone|Metadata Kernel]]. Positions grilled 2026-07-25; both survived with amendments.

## Half 1 — Child Tables

**Embedded arrays by default; per-child-DocType metadata flag for related storage.** Evidence: 500 k embedded lines aggregated in 0.9 s at 100 k invoices ([[2026-07-23 SurrealDB week-1 benchmark]]); atomic whole-doc writes; array order = line order for free; kills `parent`/`parenttype`/`idx` bookkeeping (P-1.3).

**A1 — The flag is immutable after first sync (v0 and v1).** Flipping it is a data migration wearing a checkbox costume (P-4.3, one abstraction up). Promotion ships later as its own work order: a checkpointed, fleet-fanout migration op designed against a real case. WO-003's sync engine is deliberately NOT required to express it.

**A2 — Storage-agnostic logical access (the sentence with teeth):** `doc.lines` always works; the flag changes the **physical plan only**, and the query layer compiles the difference. Enforcement point: [[ADR-006 Plugin Capability Surface]] path-segments address children identically in both storages — the fork exists in exactly one place (the broker's query compiler) and is *unexpressible* above it. If this sentence ever breaks, the flag has become a second schema system; treat as an incident.

**A3 — Decision rule (for DocType designers, in the ADR so it isn't rediscovered per incident):**

| Child needs | Storage |
|---|---|
| Identity from outside (cross-doc references to a line, per-line workflow status) | related |
| Unbounded cardinality (10 000-line telecom bill) | related |
| Everything else | embedded (default) |

Measured teeth for the second trigger: hook-envelope and changefeed costs scale with document size, and #7432's regression tracked document fatness — fat embedded docs tax *every* touch.

**A4 — Audit granularity, stated openly:** an embedded-line edit is a whole-document changefeed entry (REQ-3.2.1 remains authoritative). Per-field line diffs are a **presentation-layer computation** over before/after arrays — not a missing feature, a consequence of the shape. Expect the Frappe-parity bug report; point it here.

## Half 2 — Metadata Bootstrap

**Minimal hardcoded meta-schema in the binary; syncs itself to the DB; engine then operates from DB-resident metadata** (DocType-is-a-DocType — what lets the Desk edit DocTypes).

**A5 — The binary is authoritative for meta-tables. Sync is one-way, up. Fail closed:**
- DB meta-schema **newer** than the binary → **refuse boot**. The database never gaslights the binary; the failure is loud (the v3.2.0 lesson: silence is the expensive failure mode).
- Corollary — meta-migrations at prod boot get a named carve-out from "prod never auto-applies": they apply **only under an explicit ack** (`--accept-meta-migrations`); otherwise refuse boot naming the pending migrations. Binary upgrade in prod is a deliberate two-step: deploy, then ack.

**A6 — Meta-DocTypes are closed to customization (v0).** Extensions contribute metadata as their own records, never by mutating the meta-schema — no Custom-Fields-on-DocType second floor (P-4.2's precedence hell). Widening the meta-schema is an ADR amendment, per the [[ADR-007 Tier-2 Script Architecture|profile-table]] precedent.

**A7 — Boot sequence (explicit, five lines):**
1. Acquire advisory lock
2. Meta-schema sync from binary (under this lock — two nodes racing a meta-upgrade is the stale-lock cascade's uglier sibling)
3. Re-read meta from DB
4. User-DocType sync from DB metadata
5. boot-check verdict (prod: pending *user* migrations never auto-apply; pending *meta* migrations per A5)

## Parked for ADR-009 (recorded, not decided)

Table-as-queue via `LIVE SELECT` workers leans right, but live queries are this vault's last **unverified** SurrealDB behavior, and the v3.2.0 pattern (#7432, #7433) is *silent* misbehavior. Lost events in a queue aren't a bug report — they're lost jobs. The ADR-009 work order MUST carry a WO-002-criterion-3-shaped named criterion: **prove a LIVE SELECT worker misses nothing across reconnects and restarts** before the queue bet is accepted. Even odds it finds issue #3.

**Related:** [[Frust Hub]] · [[SurrealDB]] · [[ADR-006 Plugin Capability Surface]] · [[2026-07-24 Architecture skeleton (WO-002)]]
