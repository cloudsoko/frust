---
tags: [frust, work-order, surrealdb, production, performance, milestone-4]
status: COMPLETE (2026-07-29) - the sustained-load port ceiling is closed. PROBED FIRST: SurrealDB 3.2.0 sends no `Connection: close`, so the connection was always reusable and the churn was entirely client-side. A/B (`examples/connprobe.rs`, 200 sequential queries): bare `ureq::post` leaves **200 TIME_WAIT sockets @ 54.8 req/s**; one persistent Agent leaves **1 @ 63.7 req/s** - NOT a trade, reuse removes a handshake per query so it is faster AND bounded. FIX: `agent_for(endpoint)` in db.rs, a process-global registry keyed by ENDPOINT (not tenant - N tenants add no sockets); `Db` resolves its agent ONCE at construction so the query path never touches the registry (WO-024/025's don't-trade-a-resource-win-for-a-lock). Pool sized to the REST worker clamp(2,16) + 8 headroom. `broker.rs`'s legacy WO-002 hook runner FIXED rather than excepted - one-agent-per-endpoint is process-wide. CRITERION 1 - 3-MINUTE SOAK, ~26k requests, 0 errors: TIME_WAIT 1/1/1/1/0/0/0, final 0 (the old model would have burned ~26k sockets against a ~14k port range). Checked for vacuity WHILE RUNNING: kernel log +2791 lines/6s and **22 connections ESTABLISHED** (pool 24) - reuse visible directly, ruling out "flat because nothing happened". CRITERION 2 - throughput c=10 six 30s samples: 133.2/142.3/146.8/140.9/143.7/156.2, median **143.0** vs Chunk C's 138.0 and the 124 baseline - median IMPROVED; submit 3 ms (gate 25), hook 0 ms, realtime tax 0.25 ms; suite 309 passed / 0 failed / 51 groups. CRITERION 3 - 22 concurrent ESTABLISHED connections + throughput up and p50 down = no serialization. CRITERION 4 - footprint 65.6/65.4/67.2 MB at 1/10/50. GUARD: `connection_reuse.rs` = behavioural (200 queries left **0** new sockets) + structural (no bare ureq free functions in src/ - it caught the broker.rs caller on its first run); failure mode stays reproducible via `connprobe -- bare 200`. RECONCILES WO-026: `bare ~= pooled within 10%` was true FOR THROUGHPUT; the resource dimension was never measured. See [[2026-07-29 WO-041 connection reuse]].
created: 2026-07-29
---

# WO-041: SurrealDB Connection Reuse (the Sustained-Load Port Ceiling)

> [!info] PM work order — a real production constraint surfaced by WO-040 Chunk B's load leg. **Not a Chunk B blocker** (it broke the load *generator*, not the kernel; `/health` answered in 64 µs mid-"stall"), but a genuine sustained-load ceiling that must close before "turnkey under load" is claimable. Governing: [[2026-07-26 WO-026 surrealdb write concurrency]] (the connection model this revisits + the hot path a fix must not regress).

## The finding (WO-040 Chunk B)

The kernel opens **~1 TCP connection to SurrealDB per query** (`ureq::post`, the bare free function — WO-026's deliberate model). TIME_WAIT climbs at request rate: **118 → 1252 → 4274 → 6150 sockets** across three benches. On Windows's ~14k ephemeral-port range with a ~4-min TIME_WAIT, sustained ~50 req/s (the WO-035 Desk knee) reaches ~12k sockets at steady state → **port exhaustion in minutes**.

**Reconciles, doesn't contradict, WO-026:** WO-026 exonerated the connection model *for throughput* (`bare ≈ fresh ≈ pooled within 10%`) — true, and it stands. The **resource-exhaustion dimension was never measured.** Both hold: per-request connections are throughput-neutral AND port-exhausting under sustained load. (Another instance of *measure the dimension you're claiming, not an adjacent one.*)

## Exit Criteria

1. **Connection reuse** in `db.rs` — a bounded pool or a persistent `ureq::Agent`, so TIME_WAIT stays bounded under sustained load rather than climbing at request rate. Prove: a sustained-rate run (minutes, not seconds) holds TIME_WAIT flat.
2. **The WO-026 hot path is not regressed** — this touches the exact path WO-026 optimized to 124 req/s. Re-measure: **124 req/s and the 25 ms floor must hold**, the WO-026 caches (session + doctype generation-invalidation) still correct, the conflict-retry still works. A pool that trades port-churn for throughput or breaks the cache model is a worse deal — STOP+report.
3. **Concurrency-safe under the WO-025 worker pool** — 16 workers sharing the pool must not serialize on it (the WO-024/025 lesson: don't trade a resource win for a lock). If a shared `Agent` contends, size the pool to cores.
4. **Footprint held** — a connection pool adds state; `tenantmem`-class check that it stays in the tens-of-MB class.

## Boundaries

- This is the *connection* model, not the *query* model — don't touch the WO-026 caches' logic, only how the transport connects.
- Interacts with tenancy: all tenant queries hit the same SurrealDB endpoint, so the pool is per-endpoint not per-tenant — the count is per-query-rate, not per-tenant-count.

## Escalations

If a persistent `Agent` or pool measurably regresses the 124 req/s throughput (WO-026's own probe found bare ≈ pooled within 10%, so a regression here would be surprising and worth understanding before shipping), report the number.

**Related:** [[Frust Hub]] · [[2026-07-26 WO-026 surrealdb write concurrency]] · [[2026-07-25 WO-024 load and footprint benchmark]] · [[2026-07-29 WO-040 chunk B strategy parity]] (where it surfaced) · [[v2.0 Deployability Gate]]
