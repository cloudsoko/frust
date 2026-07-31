---
tags: [frust, srs, requirements]
status: draft-v1
created: 2026-07-23
---

# Frust — Software Requirements Specification

> [!info] Purpose
> Technology-agnostic SRS for a high-performance, metadata-driven ERP framework inspired by Frappe, built in Rust. Motivated by [[Frappe Pain Points]]. Gaps flagged there (⚠️ in the coverage table) need requirements added here.

## 1. Dynamic Schema & Data Management System

### 1.1 Schema Definition & Persistence

- **REQ-1.1.1 (Metadata Specs):** The system MUST define all data models ("DocTypes") purely as runtime metadata (JSON/YAML) rather than compiled code structures.
- **REQ-1.1.2 (Live Schema Mutation):** Schema alterations (adding, removing, or changing fields and field types) MUST take effect immediately at runtime without requiring system restarts or binary recompilation.
- **REQ-1.1.3 (Database Synchronization):** The engine MUST automatically detect changes in metadata definitions and issue appropriate DDL queries to sync the underlying database tables and indexes safely.

### 1.2 Data Access & Query Engine

- **REQ-1.2.1 (Dynamic Querying):** The data layer MUST provide a runtime query builder capable of executing CRUD operations, filtering, joining, sorting, and aggregating across standard fields and dynamic custom fields. The builder MUST expose **index hints** (force/suppress index use) — required from day one per the planner behavior measured in [[2026-07-23 SurrealDB week-1 benchmark]].
- **REQ-1.2.2 (Data Type Validation):** The system MUST enforce field-level constraints (required fields, regex patterns, min/max values, unique constraints, foreign key relationships) dynamically based on the current DocType metadata.

## 2. Dynamic Plugin Architecture & Hot Swapping

### 2.1 App Packaging & Installation

- **REQ-2.1.1 (Zero-Compilation Extension):** The core system MUST support loading, updating, and disabling third-party extension modules ("Apps") at runtime without rebuilding the core binary. → *loading half* satisfied by [[ADR-005 Plugin Isolation]]; *lifecycle half* (versioned bundle, install/enable/disable/update) → [[WO-019 App Lifecycle]] *(audit correction 2026-07-26: the earlier "satisfied" annotation overclaimed)*.
- **REQ-2.1.2 (Isolated Execution):** Extension logic MUST run inside an isolated execution environment to prevent third-party code from corrupting the core process memory or crashing the host system. → satisfied by [[ADR-005 Plugin Isolation]] (wasmtime memory isolation + fuel/epoch limits; verified in [[2026-07-24 WASM isolation spike]]).

### 2.2 Event Hooks & Extension Points

- **REQ-2.2.1 (Lifecycle Hooks):** Extensions MUST be able to subscribe to document lifecycle events (`before_insert`, `validate`, `on_submit`, `on_cancel`, …).
- **REQ-2.2.2 (API Endpoint Registration):** Extensions MUST be able to declare and register custom REST or gRPC API routes at runtime.
- **REQ-2.2.3 (UI & Schema Overrides — tiered):** Extensions MUST be able to extend UI and schema without modifying core files, via three tiers (see [[ADR-001 UI Extension Tiers]]):
	- **Tier 1 — Metadata (runtime, v0):** dynamic fields, layouts, field configs, UI views injected purely as metadata. The metadata schema MUST define named lifecycle hook points (`on_load`, `on_change`, `validate`, …) **from v0**, even before anything attaches to them — retrofitting hook points into production metadata is the expensive version.
	- **Tier 2 — Sandboxed scripts (runtime, later):** user-authored client scripts attach to the Tier-1 hook points via a sandboxed engine. The sandbox needs no DOM access — only a bridge to a small verb set (~6: get/set field value, toggle visibility/read-only, validate, call server method) defined in metadata terms, mapping to signal writes and shard/procedure calls.
	- **Tier 3 — Compiled widgets (recompile):** novel widget types ship as Rust code via a curated marketplace; recompile-and-redeploy is acceptable for this tier only.

## 3. Security & Fine-Grained Access Control

### 3.1 Role & Field-Level Authorization

- **REQ-3.1.1 (Field-Level Permissions):** The engine MUST enforce read/write access per field based on user roles.
- **REQ-3.1.2 (Row-Level Security):** The system MUST evaluate user permission rules and dynamically inject filter conditions directly into data access queries before execution.

### 3.2 Auditability

- **REQ-3.2.1 (Change Tracking):** The system MUST track and log field-level mutation histories (who changed what value, and when) for auditing purposes.

## 4. Document State & Workflow Machine

### 4.1 Document State Transitions

- **REQ-4.1.1 (Docstatus Progression):** The engine MUST enforce a formal three-tier transactional state machine for submittable documents:
	- **State 0 (Draft):** Full editability.
	- **State 1 (Submitted):** Read-only / immutable (except fields explicitly designated as editable post-submission).
	- **State 2 (Cancelled):** Permanently revoked; no further edits allowed.
- **REQ-4.1.2 (Workflow Engines):** The system MUST support dynamic, multi-step approval workflows that govern permitted status transitions based on user roles.

## 5. Background Jobs & Asynchronous Processing

### 5.1 Distributed Task Queue

- **REQ-5.1.1 (Async Task Dispatch):** Long-running tasks, scheduled cron jobs, and heavy extension logic MUST be offloaded to an asynchronous task queue.
- **REQ-5.1.2 (Hook Execution):** Async workers MUST inherit the exact metadata context, dynamic schemas, and permission rules present in the main process.

## 6. Non-Functional & Cross-Cutting Requirements
*Gap-fills drafted 2026-07-24. Items marked ⏳ get finalized with WO-002 skeleton telemetry.*

### 6.1 Performance Targets ⏳
- **REQ-6.1.1 (Floors from measurement):** The system MUST sustain, at 100 k-document scale: indexed point lookup ≤ 5 ms; list/register query ≤ 100 ms; plugin hook overhead ≤ 50 µs warm; script hook overhead ≤ 1 ms warm; Desk interaction (shard round-trip) ≤ 50 ms on LAN; **end-to-end document submit ≤ 25 ms warm** (measured 7.1–8.2 ms in [[2026-07-24 Architecture skeleton (WO-002)]], hooks ≈ 25% — budget set at ~3× measured for headroom under real DocType complexity). Sources: [[2026-07-23 SurrealDB week-1 benchmark]], [[2026-07-24 WASM isolation spike]], [[2026-07-24 Script engine spike (WO-001)]]. *1 M-row scaling claims still blocked on the pending re-run.*
- **REQ-6.1.2 (Regression gates):** These numbers are CI gates, not aspirations — a change that regresses a floor fails the build.

### 6.2 Money & Decimal Arithmetic
- **REQ-6.2.1 (No floats, ever):** Monetary values MUST be stored, transmitted, and computed in decimal form. A float representation of money crossing any boundary is a defect (encoding already enforced at the plugin boundary — [[ADR-006 Plugin Capability Surface]]).
- **REQ-6.2.2 (Explicit rounding):** Rounding scale comes from currency metadata; rounding mode is explicit per money field (default: half-even). Intermediate results carry extended precision; rounding happens once, at defined points (line → tax → total), never implicitly. → **[[WO-021 Money Arithmetic]] (active 2026-07-26)** implements this: mul/div with explicit rounding, half-even proven against half-up, defined-points discipline load-bearing.

### 6.3 Background Job Semantics
*First real datapoint: scheduled-script run ~2.5 ms with a fresh isolated instance per run ([[2026-07-24 Architecture skeleton (WO-002)]]) — fresh-per-run is affordable, so job handlers get maximum isolation by default.*
- **REQ-6.3.1 (At-least-once + idempotency):** Delivery is at-least-once; every enqueue carries an idempotency key; handlers MUST be safe under redelivery. Duplicate keys within a configurable window are rejected at enqueue.
- **REQ-6.3.2 (Retry & dead-letter):** Failed jobs retry with exponential backoff up to a per-job-class limit, then land in a queryable dead-letter state. Permission-denied failures are non-retryable ([[ADR-006 Plugin Capability Surface|ADR-006]]: identity captured, authority re-derived).
- **REQ-6.3.3 (Visibility):** Job states (queued/running/failed/dead) are queryable like any DocType — no `bench doctor` archaeology (P-6.1).

### 6.4 Observability ✅ *(implemented WO-010, 2026-07-25 — trace reconstruction proven, floor held +0–2 ms)*
- **REQ-6.4.1 (Structured everything):** Structured logs with a trace ID propagated across request → hooks (plugin and script) → jobs → DB calls (P-7.4).
- **REQ-6.4.2 (Metrics endpoint):** Latency histograms per verb, hook timings, queue depths, live-query counts — scrapeable without shell access.
- **REQ-6.4.3 (Per-tenant attribution):** Resource usage (query time, hook fuel, job time) attributable per tenant — the measurement prerequisite for solving P-8.2.

### 6.5 Realtime ✅ *(implemented WO-012, 2026-07-25 — per-session LIVE → socket, DB-filtered push path, budget 20/table with transparent poll fallback; [[ADR-011 Realtime]])*
- **REQ-6.5.1 (Permission-aware subscriptions):** Clients subscribe to record/list changes; the engine backs subscriptions with `LIVE SELECT` so row-level permissions apply inside the DB ([[SurrealDB]] §4). No separate realtime process (P-2.4).
- **REQ-6.5.2 (Graceful degradation):** Subscription failure degrades to polling transparently; realtime is an enhancement, never a correctness dependency.

### 6.6 Migration Safety & Reversibility
*Ratified 2026-07-24 from the executed-semantics position paper in [[2026-07-24 Module 3 close — sync engine port + rollback position]] — every clause backed by a named test.*

- **REQ-6.6.1 (Dry-run):** The schema-sync engine MUST provide a dry-run preview returning the exact planned DDL, destructive operations, and per-field change classification — without mutating schema or history or taking locks. Dry-run is a *plan preview, not a success predictor*, and MUST be presented as such.
- **REQ-6.6.2 (Fail-closed gate):** An environment-aware gate MUST refuse destructive (field-drop) and unsafe (narrowing, needs-backfill) changes in production absent explicit operator acknowledgment; development environments warn-and-proceed. Unspecified environment = production strictness.
- **REQ-6.6.3 (Schema revert):** Revert to any recorded snapshot version MUST be available, transactional per-resource, gated identically to forward migration, and recorded as a new forward history event. The system MUST NOT represent schema revert as data recovery — data restoration belongs to backup/export tooling.
- **REQ-6.6.4 (Mid-sync failure semantics):** Each resource's DDL and history record MUST commit atomically together; a failed resource rolls back fully while prior resources stand; the run collects all per-resource errors rather than halting; and a re-run MUST resume idempotently from recorded history. A half-completed sync is a valid intermediate state, never a torn one.
- **REQ-6.6.5 (Distinguishability):** Operators MUST be able to distinguish, from engine output alone: a plan (dry-run), an applied change (history-recorded), and a revert (a forward event targeting a past snapshot).

---

## Open Requirement Gaps

> [!todo] From the coverage table in [[Frappe Pain Points#Pain Point → Requirement Coverage]]
> - [x] ~~Performance targets~~ → **REQ-6.1** ⏳ (submit budget + 1 M-row claims pending WO-002 / re-run)
> - [x] ~~Decimal money arithmetic~~ → **REQ-6.2** (encoding was already decided in [[ADR-006 Plugin Capability Surface]])
> - [x] ~~Migration rollback / dry-run diff~~ → **REQ-6.6 ratified 2026-07-24** from executed semantics ([[2026-07-24 Module 3 close — sync engine port + rollback position]])
> - [x] ~~User-script sandboxing model~~ → **decided:** [[ADR-007 Tier-2 Script Architecture]]
> - [x] ~~Job retry, idempotency semantics~~ → **REQ-6.3** ⏳ (first datapoint from WO-002 scheduled-script run)
> - [x] ~~Observability~~ → **REQ-6.4**
> - [x] ~~Multi-tenancy model decision~~ → **decided:** [[ADR-003 Tenancy Model]] (P-8.2 resource starvation remains open; REQ-6.4.3 is its measurement prerequisite)
> - [x] ~~Realtime requirement~~ → **REQ-6.5**
> - [x] ~~Clarify REQ-2.2.3 scope~~ → **decided 2026-07-23:** tiered model, see [[ADR-001 UI Extension Tiers]]
>
> **ALL SRS GAPS CLOSED (2026-07-24).** Every requirement in this document is either satisfied by a ratified ADR, gated by measured CI floors, or specified from executed semantics.

## Related

- [[Frust Hub]] — project home
- [[Frappe Pain Points]] — motivation
