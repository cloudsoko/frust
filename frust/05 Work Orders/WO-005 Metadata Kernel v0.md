---
tags: [frust, work-order, kernel, milestone]
status: COMPLETED 2026-07-24 — all 7 exit criteria executed as tests, 15/15 binaries green; the kernel exists (frust serve + surreal.exe, two processes). → [[2026-07-24 Module 6 + WO-005 close — frust serve]]
created: 2026-07-25
---

# WO-005: Metadata Kernel — Frust Core v0 (`frust serve`)

> [!info] PM work order — the milestone build. Results to `04 Build Log/`, live vault path verified first. Every architecture question this WO needs is already answered; if you find one that isn't, that's an escalation, not a judgment call.

## PM Ruling — kernel identity (2026-07-25, pre-line-one)

**There is exactly one kernel: this one.** `framework-core` (other machine) is a *source of salvage, never a second kernel* — its accumulated decisions have no standing until grilled and ratified here, whatever it turns out to be. Consequence: [[WO-003 Engine Integration]] is **dissolved into this WO** — the sync engine's source (`D:\Dev\rust\orm`, full `framework-orm-adapter`, reviewed + 5 static fixes) is already on this machine; its parent surface is six imports the kernel was going to define anyway. Residue from WO-003 that survives: verify [[ADR-003 Tenancy Model]] against `framework-core` source when that machine is next available (checklist item, off the critical path).

## What this builds

The component every ADR assumes: one Rust binary, `frust serve`, that replaces WO-002's composition. Target shape: **two processes total** — `frust serve` + `surreal.exe`.

**Transport (contract line, ponytail):** kernel v0 speaks SurrealDB's wire protocols directly — HTTP `/sql` + WebSocket RPC, the skeleton's `db` module grown up (the skeleton proved this surface covers auth, queries, LIVE, changefeed). The official SDK crate is admitted later **only if typed responses earn their multi-GB build cost**. This also keeps ADR-002 honest: locked to SurrealDB the *system*, not a client library's API churn.

**Build discipline: the WO-002 flow is the acceptance test from week one, not the last.** The composition's processes fold into the kernel one at a time — hook-runner first (wasm-spike/host was written to be absorbed), Desk stays external until last — and **every fold-in ends with the exit sentence still true.** No phase completes with the flow broken.

## Modules, in build order

1. **The broker first — everything else consumes it** ([[ADR-006 Plugin Capability Surface]]). Structured filter contract, verb dispatch (`db-read`, `db-write`, `db-aggregate`, `db-named-query`, `enqueue`, `log`), the permission compiler (role metadata → `PERMISSIONS` clauses + field-level envelope filtering — **one implementation** serving Desk, REST, and plugins), hook-chain cycle trap (`(record-id, hook-class)` key, depth cap 8), index-hint rules from [[2026-07-23 SurrealDB week-1 benchmark]] (range indexes opt-in, `NOINDEX` for range+sort shapes, attached `TIMEOUT`).
2. **Metadata store & boot** ([[ADR-008 Data Shape]]): hardcoded minimal meta-schema; boot sequence exactly A7 (advisory lock → meta sync from binary → re-read from DB → user-DocType sync → boot-check verdict); fail-closed on newer-DB meta; `--accept-meta-migrations` ack path. Meta-DocTypes closed to customization.
3. **Schema sync — port the adapter, don't rewrite it:** `D:\Dev\rust\orm` moves into the kernel workspace; the kernel defines the six-import interface (`registered_resources`, `AppContext`, `ResourceRegistration`, `FieldKind`, `StorageLocation`, `ANALYZER_SQL`); the resource source swaps from compile-time registry to **runtime DocType metadata** (the diff/classify/gate/apply/revert/fleet engine is source-agnostic — only the input constructor changes). **Run its full test suite for the first time** — the session's five static fixes have been waiting for it. Additions per ADR-008/009: EVENT in the sync vocabulary (docstatus lattice only, `FRUST:E_DOCSTATUS:` codes), children embedded-default with immutable flag, `OVERWRITE` re-syncs preserve changefeed history (WO-002 Finding A — assert in tests). The WO-002 sliver dies here; no second sliver is ever born. **The migration rollback/dry-run position paper (last SRS gap) is produced by this module's port**, from the adapter's actual gate/revert semantics.
4. **Hook dispatcher**: plugin WASM + script-engine WASM behind one lifecycle (reuse both spike artifacts); pooled per `(tenant, script-set)` for scripts; fresh-instance-per-run for job handlers ([[SRS]] REQ-6.3 note).
5. **Worker loop** ([[ADR-009 Execution Model]] Half 2, verbatim): replay-from-cursor → LIVE tail → advance cursor; atomic conditional claim as the only serialization point; cold-start = rescan `status='queued'`.
6. **REST surface**: metadata-generated endpoints speaking the same filter contract (the headless contract from [[ADR-004 Topcoat for Desk v0]] — Desk becomes a client, not a special case).

## Exit Criteria

1. **The WO-002 flow, verbatim, on two processes:** create a DocType at runtime, submit a document through both hook classes, show the audit trail — no restarts, composition deleted.
2. **One permission compiler, proven:** the same role metadata produces identical row/field filtering via REST call, Desk render, and plugin `db-read` — three consumers, one test, byte-equal results.
3. **REQ-6.1.1 gates hold in CI:** submit ≤ 25 ms warm; hook overhead within spike budgets. Regression = build failure (REQ-6.1.2 becomes real in this WO).
4. **Boot discipline demonstrated:** newer-DB meta refuses boot with a named error; `--accept-meta-migrations` two-step works; two racing nodes can't double-apply meta sync (lock held per A7).
5. **Queue end-to-end:** `enqueue` verb → worker claims atomically → job runs with re-derived authority (deny after revocation = typed non-retryable failure, ADR-006) → state queryable as data.
6. **ADR-009 ruling #1 executed, not just written:** atomic claim under concurrent workers — two+ workers, contested jobs under burst, **exactly one winner per job**, measured. (One evening against WO-004's machinery.)
7. **ADR-009 ruling #2 executed:** retention-as-efficiency-bound — kill a worker past a short test retention; it cold-starts via `status='queued'` rescan, not replay, and misses nothing.

## Escalations

- Any SurrealDB behavior not covered by prior logs misbehaving silently → instance-count conversation, stop.
- Any architecture question without an ADR answer → stop and ask; do not decide architecture inside a build WO.
- `framework-core` machine comes online mid-build → nothing re-sequences (kernel-identity ruling above); the only action is the ADR-003 source verification, off the critical path. Salvage proposals from that repo go through the grill like any external code.

**Related:** [[Frust Hub]] · ADR-001…009 · [[2026-07-24 Architecture skeleton (WO-002)]] · [[2026-07-24 Live-query and event fidelity (WO-004)]]
