---
tags: [frust, build-log, tenancy, quotas, work-order]
created: 2026-07-25
work-order: "[[WO-013 Tenant Fairness]]"
---

# Build Log — WO-013: Tenant Fairness (P-8.2)

**The headline, up front — the bound exists and is stated:**

> **Tenant B's submit median: 28 ms quiet → 50 ms with a noisy neighbour unthrottled (1.8×) → 38 ms with the neighbour shaped at the broker door (1.4×).**

Frappe's P-8.2 pain was never that neighbours get noisy — it was that **no bound existed to state**. Frust now has one, measured, with a knob that moves it.

## Phase 1 — door throttling

**`fairness.rs`** — token bucket per tenant (sustained rate + burst depth), checked at the broker door *before the verb runs*; round-robin ordering at the worker door. Policy is kernel-owned meta (`_tenant_policy`, DDL in `meta.rs` beside identity, loaded at boot). **Absent policy = unlimited**, so a deployment that writes no policy row keeps exactly its pre-WO-013 behaviour — fairness is opt-in.

| # | Criterion | Evidence |
|---|---|---|
| 1 | Per-tenant budgets, typed refusal | `tenant_fairness::broker_door_throttles_loudly_and_locally`: A over budget gets `E_TENANT_THROTTLED` with an actionable `retry_after_ms`; the code reaches logs via `telemetry::error_code`; REST answers **429**. B (no policy) is unaffected by A's exhaustion. **Refusals cost 2–4 µs** — measured in the noisy run — because the door rejects before the verb, so a shaped tenant stops consuming the shared DB almost immediately. |
| 2 | Worker-door fairness (500 vs 5) | `worker_door_interleaves_flood_and_trickle` through the **shipped** claim path: claim order `["A","B","A","B","A","B","A","B","A","B","A","A"]` — B's five jobs all land inside the first ten slots, not behind A's five hundred. |
| 3 | Noisy-neighbour bound | the numbers above; A completed 298,293 hammer loops during the run |

**Two design bugs the tests caught (both mine, both structural):**

1. **Window-based fairness is not fairness.** The first implementation ordered a 200-row scan window round-robin — but A's 500 queued jobs *fill* that window, so B never appears in it and "fair ordering" of an all-A window is FIFO with extra steps. Fixed by asking the DB for **per-tenant heads** (1 + T queries per round), which is correct at any backlog size.
2. **Round-robin without a cursor serves one tenant forever.** `claim_next` takes only the *first* job of each round, so a freshly-built round always started at the same tenant. Fixed with a rotating start index — real round-robin needs state, and the test that reads position zero is the one that proves it.

## Phase 2 — fuel-true hook accounting

Wasmtime fuel metering is on (`consume_fuel`), refilled per call to a known mark so consumption is *that call's*, and exported as `frust_hook_fuel_total{runtime,tenant}`. Wall-time says "slow"; fuel says "expensive" — quotas need the second. **The ADR-005 epoch deadline stays** as the wall-clock backstop (the allocation bomb ground 10.4 s before the memory cap caught it; wall-clock bounding is not optional).

**Criterion 5 — metering's cost, measured:** hook chain (plugin + script) **p50 80.6 µs / p95 91.8 µs** with metering, against the ADR-007 spike's ~55.7 µs baseline without it — call it **+25 µs, ~1.4×** on the hook path. Both remain ~12× inside REQ-6.1.1's 1 ms script-hook floor. Both submit gates green on a clean substrate: **floor 24/23 ms (gate 25)**, **realtime tax 2/0 ms (allowance 2)**.

## The escalation clause — NOT triggered, and the honest reading

Door throttling produced a stated bound (1.8× → 1.4×), so ADR-003 is **not** re-opened. But the residual is worth naming precisely: shaping the neighbour recovered **~55% of the degradation, not all of it**. The remaining ~0.4× is exactly the shared-DB contention the WO put out of scope — A's *admitted* traffic still competes with B's inside one surreal process. That is the ADR-003 trade sitting where the position statement said it sits, quantified rather than assumed. It is a finding, not a gap: if a future deployment needs a tighter bound than 1.4×, the lever is the ADR-003 amendment (per-tenant DB processes), and now there is a number to justify it with.

## Substrate discipline (the caveat earning its keep, twice)

Mid-WO the gates read 29–43 ms and the realtime tax read 8–11 ms — alarming until diagnosed: the dev instance had accumulated **76 databases** across the session, and the noisy-neighbour run alone wrote ~300k rows. Dropping the 75 scratch databases and restarting restored floor 24 ms / tax 0–2 ms. **New standing practice: drop scratch databases at the end of a measuring WO**, not just restart the instance — churn is cumulative across a session, and the substrate probe catches the symptom while the database count explains it.

## Findings

1. **v3.2.0 requires ORDER BY idioms in the projection** — `SELECT id, tenant … ORDER BY enqueued_at` is a parse error; `enqueued_at` must be selected. Sibling of the GROUP BY rule found in WO-007. Both now in the SurrealDB caveats.
2. **The surql monopoly caught this WO's author again** (policy DDL + policy SELECT in `fairness.rs`). Resolved by the project's own precedent rather than an allowlist entry: static kernel DDL moved to `meta.rs` beside identity, the load moved to `boot.rs`, leaving `fairness.rs` as pure logic with zero query text. The gate keeps making the code better, not just legal.
3. **Process-global state makes parallel unit tests lie** — two `fairness` tests shared the policy map and one's cap silently capped the other's flood. Fixed with distinct tenant names per test; the lesson generalises to any module holding a process-global registry.

## Suite state
**25 test binaries green, zero failures.**

## Related
[[WO-013 Tenant Fairness]] · [[ADR-003 Tenancy Model]] · [[ADR-005 Plugin Isolation]] · [[2026-07-25 WO-010 Observability]] (the position this executed) · [[Frappe Pain Points]] (P-8.2)
