---
tags: [frust, work-order, doctype, integration, probe, milestone-5]
status: PROBE DONE (2026-08-01) — the cancelled worktree agent HAD completed the probe; found UNCOMMITTED in its worktree and **recovered to the main vault**. **Verdict: Frust does NOT do live virtual DocTypes — SYNC-TO-REAL-TABLE instead** (STOP on gate 1: permissions on non-DB data can only be bypassable in-connector `IF $auth` filtering → breaks the one-compiler alpha; grounded in SurrealDB evidence — `http::get` refused as a network boundary, `DEFINE FUNCTION` returns all rows to any caller with no permission surface). → [[ADR-018 Virtual DocType]] **PROPOSED** (PM recommends ratifying; the SurrealDB evidence was NOT independently re-run — offer to verify before ratifying a strategic direction). Sync-mirror worker + any ADR-006 outbound-HTTP amendment = future Boss-gated WOs.
created: 2026-08-01
---

# WO-062: Virtual DocType Probe

## Why

Frappe's Virtual DocType (`is_virtual`) has fields + an API but **NO table** — data comes from an external source (API, another DB) via controller data-access methods. It's the **integration primitive** (wrap external logistics/payment/CRM systems behind the DocType interface) — strategically valuable (the ecosystem/M5 theme, the Fleetbase-style SaaS-consumer context). BUT it is in **direct tension with the alpha**, so it is a **probe→ADR, not a build.** The probe decides whether Frust does this at all, and in what shape. Predictions stated before running (WO-019 template).

## The gates (falsifiable; a STOP returns to the Boss)

1. **THE LOAD-BEARING GATE — permissions on data the DB doesn't hold.** Frust's alpha is row/field permissions enforced BY THE DATABASE under the caller's session. External data isn't in SurrealDB, so it can't run `PERMISSIONS`. Probe: is there ANY structural shape where a virtual doctype's rows respect permissions *without* hand-written per-connector filtering? **STOP CONDITION: if the only enforcement is app-code filtering in the connector (bypassable, un-compiled, per-connector), that BREAKS the one-compiler alpha — report it as the trade; the Boss decides whether integration is worth sacrificing the guarantee.** This gate decides viability.
2. **Capability / containment gate — where the fetch runs.** App-authored connector → needs an **outbound-HTTP capability the sandbox deliberately lacks** (ADR-006's surface is db-read/write/enqueue/log; adding network is a **containment-boundary expansion = ADR-006 amendment**, profile tables are security boundaries). Kernel-native connector → needs a **recompile per connector** (breaks the metadata-driven / no-recompile thesis). Probe both, price each. STOP if outbound-network can't be contained acceptably.
3. **Request-path gate.** An external fetch on the read/write path is a blocking network call (latency, failure — the WO-024/038 blocking-in-request-path lesson). Probe: request-path (pins a worker, fails the floor) vs async/worker/cached. Name the shape.
4. **Honest-limits gate.** State plainly what BREAKS for virtual doctypes: **no changefeed audit** (REQ-3.2.1 — data isn't in the DB), **no LIVE realtime**, and whether external data flows through hooks / validation / the typed-decimal envelope (external money → decimal safety). A virtual doctype that *silently* loses the audit trail is worse than none — the limits must be explicit, not discovered.
5. **The alternative gate — virtual-proxy vs SYNC-TO-REAL-TABLE.** Frust could integrate by a worker **syncing external data into a real SurrealDB table** → permissions/audit/realtime/LIVE all keep working, the alpha HOLDS (cost: staleness, storage). Compare live-proxy (Frappe's virtual doctype — sacrifices the alpha's guarantees) vs sync-mirror (keeps them). **For Frust specifically, whose alpha IS those guarantees, sync-mirror may be the better fit** — and the environment's own Fleetbase guidance leans "consume/sync, don't live-mirror" in cases. Recommend which, with reasons. A valid finding: *"Frust should NOT do live virtual doctypes; it should do sync-mirror + the consumer pattern."*

## Deliverable

Position paper → **ADR-018**: does Frust support virtual doctypes; if so, live-proxy or sync-mirror; the permission + capability + honest-limits rulings. **NO build until the ADR.** Experiments in scratch only — **zero shipped kernel edits** (WO-022 rule).

## Isolation

Mostly analysis + scratch experiments; deliverable is a paper, not code. It reasons about the kernel/capability surface, so if run alongside the WO-057 kernel builder, **coordinate to avoid contention** — experiments stay in scratch, nothing shipped. If launched as a parallel agent, pin `model: opus`.
