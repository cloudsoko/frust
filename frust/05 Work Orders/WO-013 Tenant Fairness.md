---
tags: [frust, work-order, tenancy, quotas]
status: COMPLETED 2026-07-25 — P-8.2 has a stated bound: 1.8× unthrottled → 1.4× door-shaped (refusals at 2–4 µs, typed + retry_after_ms); 500-vs-5 = A,B,A,B; fuel = accounting truth (+25 µs, ~12× inside floor); residual 0.4× = shared-DB contention, quantified for any future ADR-003 amendment. → [[2026-07-25 WO-013 tenant fairness]]
created: 2026-07-25
---

# WO-013: Tenant Fairness — P-8.2, Phased as Positioned

> [!info] PM work order — results to `04 Build Log/`, live vault path verified first. Written from WO-010's P-8.2 position statement, phased exactly as it recommended. This closes the oldest deliberately-open item on the board ([[ADR-003 Tenancy Model]]'s honest caveat).

## Scope

Phases 1–2 only. **The DB-compute isolation trade (one shared surreal process) is explicitly OUT of scope** — that's an ADR-003 amendment decided in daylight if the door mechanisms prove insufficient, not an implementation detail of this WO.

## Phase 1 — Door throttling (the one-door property, weaponized)

1. **Per-tenant budgets at the broker door:** token-bucket (or equivalent) per tenant on verb execution, configured in metadata (a `_tenant_policy` shape — kernel-owned DDL like `app_user`), enforced before the verb runs. Over-budget = typed `E_TENANT_THROTTLED`, never a silent slow.
2. **Queue fairness at the worker door:** claim scheduling can't let one tenant's job flood starve another's — round-robin across tenants with queued work (or measured equivalent). Prove: tenant A enqueues 500, tenant B enqueues 5 → B's jobs don't wait behind all of A's.
3. **Proof shape:** the noisy-neighbor scenario measured — tenant A hammering (reads + writes + jobs) while tenant B runs the WO-002 flow; B's submit latency and job wait degrade by a *bounded, stated* amount vs quiet baseline. The bound is the deliverable — P-8.2's pain point was that Frappe had no bound at all.

## Phase 2 — Fuel-true hook accounting

4. **Wire wasmtime fuel** (the "small, known shape" from the position): per-hook-call fuel limits and per-tenant fuel consumption exported to `/metrics` — wall-time conflates slow-IO with compute-heavy; fuel doesn't. The ADR-005 epoch deadline stays (wall-clock backstop); fuel becomes the *accounting* truth.
5. **Floor check:** fuel metering's overhead on the hook path, measured (spike history: metering typically costs a few percent — get the real number); both submit gates green.

## Escalations

Standard rules + the WO-012 additions (latency gates serialize on the shared mutex; substrate probe per the calibrated caveat). If door throttling can't produce a stated bound in phase 1 — if the noisy neighbor leaks through the doors via shared-DB contention itself — that's the evidence that re-opens ADR-003, and it goes to the PM as a finding, not a phase-3 improvisation.

**Related:** [[Frust Hub]] · [[2026-07-25 WO-010 Observability]] (the position) · [[ADR-003 Tenancy Model]] · [[ADR-005 Plugin Isolation]] · [[Frappe Pain Points]] (P-8.2)
