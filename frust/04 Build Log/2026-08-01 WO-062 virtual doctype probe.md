---
tags: [frust, build-log, probe, virtual-doctype, milestone-5]
date: 2026-08-01
wo: WO-062
status: DELIVERED — verdict is a STOP; ADR-018 PROPOSED (Boss ratifies)
---

# WO-062 — Virtual DocType Probe

**A probe, not a build.** No shipped kernel edits. All experiments in a scratch in-memory SurrealDB on `:8901` (isolated from the main tree's `:8899`, so the primary builder's data was never touched). Deliverable is [[ADR-018 Virtual DocType]] (PROPOSED).

## Verdict (one line)

**Frust should NOT do live virtual DocTypes. It should do sync-to-real-table (the consumer/mirror pattern).** Gate 1 — the load-bearing permission gate — **STOPS** the live-proxy shape: there is no structural way to enforce Frust's DB-level permissions on data the database does not hold. Sync-mirror preserves the whole alpha. This is the WO's own named "valid, valuable finding".

## Predictions (stated before running — WO-019 discipline)

1. **G1:** no DB-enforced permission shape for external data; only bypassable in-connector filtering → STOP.
2. **G2:** outbound-HTTP absent from the sandbox and gated in SurrealDB → an ADR-006 amendment to add.
3. **G3:** a live fetch on the read path blocks a worker and fails the ~25 ms floor.
4. **G4:** live-proxy silently loses changefeed audit, LIVE realtime, typed-decimal envelope.
5. **G5:** sync-mirror keeps permissions/audit/realtime/decimal at the cost of staleness + storage → recommend.

**All five confirmed.** Two by direct SurrealDB probe (G1, part of G2/G5), three by construction against the codebase's established facts.

## Gate 1 — THE STOP GATE (permissions on data the DB doesn't hold)

The alpha is `PERMISSIONS` evaluated under the caller's session, one compiler, byte-equal to REST/Desk/plugins. External data has **no table rows** for a `PERMISSIONS` clause to bind to. Two probes settle whether any DB primitive could stand in:

- **`http::get` is refused by default and is server-side when allowed.**
  `RETURN http::get('http://127.0.0.1:8901/health')` →
  `{ kind: NotAllowed, result: "Access to network target '127.0.0.1:8901' is not allowed" }`.
  SurrealDB gates outbound HTTP behind an explicit `--allow-net` server capability, and even enabled it runs **as the database process, not the caller**. So the DB cannot fetch external data *as this user* — the one thing the alpha needs.
- **A `DEFINE FUNCTION` returns rows with no permission surface.**
  `DEFINE FUNCTION fn::ext_rows() { RETURN [{owner:'clerk1',secret:'A'},{owner:'manager',secret:'B'}] }; RETURN fn::ext_rows();`
  returns **both rows verbatim** to any caller. There is no `PERMISSIONS FOR select` on a function result / computed value. The only way to make it role-aware is `IF $auth.role = …` **inside the body** — hand-written, per-connector, un-compiled, bypassable.

**That in-body filtering is the STOP condition.** It is exactly the P-5.4 / `db_set`-bypass culture the project already refused, one layer out. A live virtual DocType can only be permission-correct by re-implementing the permission compiler by hand in every connector, which abandons the one-compiler guarantee. **Do not build past this gate** (the WO's instruction, honoured).

## Gate 2 — capability / containment (where the fetch runs)

- **App-authored (WASM) connector** would need an **outbound-HTTP capability the sandbox deliberately lacks** ([[ADR-006 Plugin Capability Surface]]: db-read/write/aggregate/named-query/enqueue/log — no network). Adding it is a **containment-boundary expansion = an ADR-006 amendment** (profile tables are security boundaries). SurrealDB independently treating outbound net as a grant (G1a) is a second vote that network is a boundary, not a default.
- **Kernel-native connector** avoids the sandbox question but needs a **recompile per integration** — breaking the metadata-driven / no-recompile thesis.
- **Resolved by the recommendation:** the mirror fetcher's network lives in a **kernel worker** (the WO-043 mail-relay posture — `std::thread`, owns its own blocking), contained and **off the per-request surface**. The ADR-006 table is not widened. A **generic, metadata-driven** mirror worker (source URL, auth ref, field map, interval, target DocType) serves N sources with **no per-connector recompile** — the thesis is kept without touching the sandbox.

## Gate 3 — request path

A live external fetch on the read/write path is a blocking network call (100s of ms) against a ~25 ms floor (REQ-6.1.1), and — the standing async-blocking note — it pins a worker and blinds admission (WO-024/038). **Sync-mirror moves the fetch entirely off the request path** into the worker; reads hit the mirror table at ordinary DB speed. Decisive practical advantage, independent of gate 1.

## Gate 4 — honest limits

A **live-proxy** would silently lose:
- **changefeed audit** (REQ-3.2.1) — no DB rows, no feed;
- **LIVE realtime** — no changefeed to watch, no push;
- the **typed-decimal / hook / validation envelope** unless each connector re-implements it (external money arriving as untyped JSON is a float waiting to happen — the WO-016 door).

A virtual DocType that drops the audit trail *quietly* is worse than none. The recommendation sidesteps this: the **mirror is a real table and has all three.**

## Gate 5 — the alternative (live-proxy vs sync-to-real-table)

`INFO FOR TABLE company_settings` (the WO-061 single) confirms a synced DocType carries the ordinary `PERMISSIONS` + `CHANGEFEED`. A worker mirroring external data into such a table yields:

| Property | Live-proxy | **Sync-mirror (recommended)** |
|---|---|---|
| Row/field permissions (the alpha) | ✗ hand-rolled, bypassable | ✓ DB-enforced, one compiler |
| Changefeed audit (REQ-3.2.1) | ✗ none | ✓ every mirror write |
| LIVE / SSE realtime | ✗ none | ✓ works |
| Typed decimal / hooks | ✗ per-connector | ✓ on the way in |
| Request-path latency | ✗ network on the floor | ✓ DB-speed reads |
| Freshness | live | **stale by interval** |
| Storage | none | **a copy** |

For Frust, whose alpha *is* the top four rows, the two costs sync-mirror pays (staleness, storage) are the good trade. **Recommend sync-mirror; refuse live-proxy.**

## What was BUILT vs PROBED vs STOPPED-on

- **STOPPED-on:** the live virtual DocType (gate 1). No `is_virtual` flag, no connector API, nothing shipped.
- **PROBED (scratch only):** SurrealDB's outbound-HTTP posture and function permission model; corroborated that a real table keeps the alpha.
- **BUILT:** nothing in the kernel. The deliverable is [[ADR-018 Virtual DocType]] (PROPOSED) + this log. A sync-mirror **work order** is proposed for after ratification (templates already exist: ADR-010 Tier-2 workers, WO-043 mail worker).

## Discipline notes

- **Assert provenance, not operation** (the security-domain standing check): G1b's finding is that *both* rows come back to *any* caller — the identities returned, not merely "some rows returned". A weaker "did it return data" check would have missed that the function cannot tell clerk from manager.
- **Isolation:** scratch `:8901` in-memory, never the main builder's `:8899`.
- **Left PROPOSED:** ADR-018 is the Boss's to ratify or veto — a builder does not self-ratify a security-boundary decision.
