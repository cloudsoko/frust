---
tags: [frust, work-order, engine]
status: DISSOLVED into WO-005 (2026-07-25) — the adapter source was on this machine all along (D:\Dev\rust\orm); kernel-identity ruling in [[WO-005 Metadata Kernel v0]] makes framework-core salvage-only. Surviving residue: ADR-003 source verification when that machine is available.
created: 2026-07-24
---

# WO-003: Engine Integration (Deletes the Sliver)

> [!info] PM work order — executable only on the machine/repo where `framework-core` lives. Results to `04 Build Log/`, live vault path verified first.

## What this WO proves that WO-002 could not

[[WO-002 Architecture Skeleton]] proved the *architecture* with a disclosed ~50-line inline DocType→`DEFINE` sync sliver. This WO proves the *engine*: the skeleton's sliver is **deleted** and `framework-orm-adapter` drives the identical E2E flow.

## Exit Criteria

1. **The sliver is gone** — `framework-orm-adapter` performs DocType→`DEFINE TABLE/FIELD/INDEX` sync in the WO-002 composition; the same exit sentence passes verbatim (*create a DocType at runtime, submit a document, show the audit trail, no restarts*).
2. **Finding A verified against the real engine:** the adapter's re-sync path (presumably `DEFINE TABLE OVERWRITE`) preserves changefeed history across repeated syncs — WO-002 proved this for the sliver; prove it for the engine's actual statements.
3. **ADR-003 caveat closed:** the tenancy strategy definitions in `framework-core` are read against source and [[ADR-003 Tenancy Model]] is corrected or confirmed (it was back-filled sight-unseen — flagged in the ADR itself).
4. **EVENT enters the sync vocabulary ([[ADR-009 Execution Model|ADR-009]] A5):** the adapter's snapshot/diff vocabulary (`TABLE | FIELD | INDEX`) gains `EVENT` — parse, snapshot, diff, classify, including event-body definitional drift (the case its docs punt on). Scope this before estimating.
5. **Migration rollback/dry-run investigation (last SRS gap):** report — with evidence, not opinion — what the adapter does today on: a failed mid-sync (partial DDL applied?), a destructive field-type change, and a sync preview request. Deliverable is a *position paper section* in the build log: what rollback/dry-run can honestly mean given SurrealDB's DDL semantics. I write the requirement from it.

## Standing rules that apply

- Versionstamp-`SINCE` only ([[WO-002 Architecture Skeleton#PM Ruling — criterion-3 escalation (2026-07-24)|WO-002 ruling]]; datetime form banned — check the adapter for violations).
- Every SurrealDB feature the adapter touches for the first time gets an empirical first-exercise before code trusts it (ADR-002 promoted watch-item discipline).
- Escalate, don't work around: any third silent-misbehavior instance at v3.2.0 stops work and triggers the ADR-002 re-read.

**Related:** [[Frust Hub]] · [[ADR-002 SurrealDB Lock-In]] · [[ADR-003 Tenancy Model]] · [[2026-07-24 Architecture skeleton (WO-002)]] · [[SRS]]
