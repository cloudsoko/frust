---
tags: [frust, build-log, skeleton, work-order]
created: 2026-07-24
work-order: "[[WO-002 Architecture Skeleton]]"
---

# Build Log — Architecture Skeleton (WO-002)

**Composition (three processes, as scoped):** `surreal.exe` v3.2.0 (surrealkv, `:8899`) · hook-runner on the ADR-005 wasmtime host (`:8787`, `wasm-spike/host/src/bin/hookrunner.rs`) · frust-proto Desk (Topcoat, `:3000`). Repro: `D:\Dev\rust\wasm-spike\` (hook-runner + components), `D:\Dev\rust\frust-skel\` (SurrealDB setup + data), `D:\Dev\rust\topcoat\examples\frust-proto\` (Desk).

## Criterion 1 — the exit sentence, verbatim ✅

`purchase_order` did not exist anywhere when all three processes started. Through the running Desk: metadata record created → table `DEFINE`d live (SCHEMAFULL, ASSERTs from field metadata, `PERMISSIONS`, `CHANGEFEED`) → form rendered on the next request → documents submitted → audit trail rendered from the changefeed. **Zero process restarts** (the one surreal.exe restart was the deliberate survival test, after creation).

## Criterion 2 — both hook classes on one validate ✅

Compiled plugin (ADR-005) then Tier-2 JS script (ADR-007), chained through the WIT `result<doc, string>` envelope:
- Mutation trail persisted to storage: a 25,000 Draft came back `Needs Approval` @ 28,750 (plugin flag + script 15% tax) and that is what the DB holds.
- Typed reject: negative total → `Rejected by plugin hook` with the hook's own message; nothing written.
- Hook share of a warm submit: **1.8–2.9 ms of 7–8 ms** (~25%). Pooled instances per ADR-007; per-call epoch deadline (500 ms) armed by the runner.

## Criterion 3 — PERMISSIONS + CHANGEFEED, first live exercise ✅ (with one upstream bug)

**Row-level security, enforced by the DB (REQ-3.1.2):** the identical `SELECT * FROM purchase_order` under three different record tokens returned **2 / 1 / 3** rows (clerk1 / clerk2 / manager). clerk2 reading clerk1's record **by exact id** → `[]`. The Desk contains no filtering code at all; `DEFAULT $auth.id READONLY` stamps ownership in-DB.

**Changefeed (REQ-3.2.1):** complete history — every create and update with full record states — readable via `SHOW CHANGES … SINCE <versionstamp>`; **survives a surreal.exe restart** intact.

**Finding A (first-class, per PM):** repeated `DEFINE TABLE OVERWRITE` re-syncs do **NOT** wipe changefeed history (re-defines appear in the feed as `define_table` events; record history continues). Schema sync and audit retention do not fight — load-bearing for WO-003's real engine, which uses the same OVERWRITE idempotency.

**Upstream bug (PM ruling in [[WO-002 Architecture Skeleton]]):** `SHOW CHANGES … SINCE d'<datetime>'` silently returns `[]` **even for an explicit-UTC timestamp taken strictly after changefeed creation and before the mutations** (ruled UTC check — mutation seconds old, versionstamp form returns it, datetime form doesn't). Filed as a real bug: [surrealdb/surrealdb#7433](https://github.com/surrealdb/surrealdb/issues/7433). Per ruling: **datetime-SINCE is banned in Frust code** (versionstamp-SINCE is the correct API, not a workaround), and the ADR-002 watch-item is **promoted** (silent-empty on a documented API form, second planner-adjacent silent misbehavior after [#7432](https://github.com/surrealdb/surrealdb/issues/7432)).

## Criterion 4 — telemetry for the SRS ⏳ items ✅

| Measurement | Value |
|---|---|
| Submit end-to-end, warm (form parse → hooks → authenticated write) | **7.1–8.2 ms** |
| — hooks share (plugin + JS script) | 1.8–2.9 ms (~25%) |
| — authenticated DB write share | 5.3–5.9 ms |
| Submit, cold (first per process: signin round-trip included) | 40–43 ms |
| **Finding B (first-class, per PM): scheduled-script run, fresh isolated instance per run** | **~2.5 ms** (2.45–2.81 ms over 3 runs) — REQ-6.3's first real number |

## Disclosed shortcuts (unchanged from WO scoping)

Inline sync sliver (~45 lines, deleted by WO-003) · blocking HTTP inside async handlers · root credentials inline · the hook envelope carries the toy `{id, status, total}` doc, not ADR-006's dynamic-value encoding (engine work, not skeleton work). Two skeleton-code bugs found and fixed during E2E (path-param segment naming; record-id URL round-trip with underscored table names) — both app-level, no pillar involvement.

## Verdict

Every pillar did its job in one flow, and the numbers the architecture was betting on showed up: runtime metadata is real, hooks are cheap, permissions live in the database, audit is storage-level and restart-proof. The one misbehavior found was caught by this WO's named criterion doing exactly what it was designed to do.

## Related

[[WO-002 Architecture Skeleton]] · [[ADR-002 SurrealDB Lock-In]] · [[ADR-005 Plugin Isolation]] · [[ADR-006 Plugin Capability Surface]] · [[ADR-007 Tier-2 Script Architecture]] · [[2026-07-23 SurrealDB report benchmark]]
