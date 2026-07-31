---
tags: [frust, work-order, skeleton]
status: completed — all criteria passed 2026-07-24; changefeed datetime-SINCE bug confirmed real & filed (surrealdb#7433); → [[2026-07-24 Architecture skeleton (WO-002)]]
created: 2026-07-24
---

# WO-002: Architecture Skeleton (Walking Skeleton, Composition Form)

> [!info] PM work order — builder: results to `04 Build Log/`, live vault path verified first.

## What this WO is — and is not (claim scoping)

This is the **architecture skeleton**: every pillar and every ADR exercised once, end to end, on this machine, as a **three-process composition** — NOT the final single-binary shape, and NOT an integration test of the real engine.

- **Disclosed sliver:** `framework-core` is not on this machine; DocType→`DEFINE` sync is re-implemented inline (~50 lines against the HTTP `/sql` endpoint). This tests the *architecture*, not the engine's sync code.
- **WO-003 (planned, not authorized):** engine-integration pass on the machine/repo where `framework-core` lives — the inline sliver gets **deleted** in favor of `framework-orm-adapter`. The build log for WO-002 must not claim what WO-003 exists to prove.

## Composition (fits the 7.3 GB disk)

1. `surreal.exe` — schema, data, `PERMISSIONS`, `CHANGEFEED`
2. frust-proto Desk (existing Topcoat workspace, small delta) — reads DocType metadata over HTTP; expect the ~5–10 min dev-server rebuild
3. Hook-runner — **reuses the built wasmtime host target** (1.5 GB preserved) running `plugin_demo.wasm` *and* `script_engine.wasm` on the same `validate` (ADR-005 + ADR-007 on one hook)

## Exit Criteria

1. **The sentence, verbatim:** *create a DocType at runtime, submit a document through it, and show the audit trail — without restarting anything.*
2. Both hook classes fire on the same `validate` — compiled plugin and Tier-2 JS script, ADR-006 typed envelope both ways.
3. **Named criterion — first live exercise of `PERMISSIONS` and `CHANGEFEED` (SurrealDB v3.2.0):**
	- A restricted role's `db-read` returns permission-filtered rows enforced **by the DB**, not the app (REQ-3.1.2).
	- The changefeed contains the full mutation history of the submitted document (REQ-3.2.1), and survives a `surreal.exe` restart.
	- ⚠️ Any misbehavior here is a **SurrealDB-risk finding with its own build-log section** (planner bug #7432 says version-skepticism is warranted) — escalate to PM before working around it; it feeds [[ADR-002 SurrealDB Lock-In]]'s watch-items.
4. Telemetry captured for the SRS gap-fills: end-to-end submit latency (Desk → hooks → DB → changefeed), hook overhead share, and one scheduled-script run (first real datapoint for job semantics, REQ-6.3).

## PM Ruling — criterion-3 escalation (2026-07-24)

Builder escalated per the stop rule: `SHOW CHANGES … SINCE d'<datetime>'` silently empty while versionstamp-SINCE returns the full feed. Docs check (PM): datetime-SINCE is **documented** to require a datetime after changefeed creation — before-feed empties are by design; same-day empties suspected timezone edge.

**Ruled:** (1) versionstamp-SINCE is the *correct API* for audit, not a workaround — **datetime-SINCE is banned in Frust code**; (2) builder runs one explicit-UTC postdating check — still empty = real bug upstream, works = file as DX issue (silent empty should warn/error); (3) ADR-002 watch-item as "changefeed DX hazard" unless the UTC check fails (then promoted); (4) proceed to task 5.

## Escalations

- Criterion 3 fails → stop, report, PM decision (this is a pillar behavior, not an implementation detail).
- Composition can't satisfy criterion 1 without touching `framework-core` → WO-002 is mis-scoped; stop and say so.

**Related:** [[Frust Hub]] · [[ADR-002 SurrealDB Lock-In]] · [[ADR-005 Plugin Isolation]] · [[ADR-006 Plugin Capability Surface]] · [[ADR-007 Tier-2 Script Architecture]] · [[WO-001 Script Engine Spike]]
