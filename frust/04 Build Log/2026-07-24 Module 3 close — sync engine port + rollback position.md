---
tags: [frust, build-log, kernel, work-order, migrations]
created: 2026-07-24
work-order: "[[WO-005 Metadata Kernel v0]]"
---

# Build Log — Module 3 Close: Sync Engine Port + Rollback Position Paper

## State at close

- **Suite: 95 (frust-orm) + 15 (frust-kernel) tests green** against live SurrealDB v3.2.0. The adapter's suite ran for the first time in its existence (was SDK/kv-mem, never executed); ported to a live-server testkit with a uniquely-named database per call (parallel-safe by construction).
- **The WO-002 sliver is dead.** Schema DDL emission + application is the kernel's job: DocType metadata -> `ResourceSpec` -> the ported engine (diff / classify / gate / history / lock). Both Desk call sites removed (`frust-proto` rebuild green); the Desk is now a pure `(metadata, record JSON)` renderer per [[ADR-004 Topcoat for Desk v0]]. `frust` boots against the skeleton and reports `2 doctype(s)` through the real A7 sync path.
- **Five static fixes: 5/5 proven.** Fix #5's dedicated test (`fleet::tests::failed_checkpoint_warns_and_continues_then_resume_checkpoints`) landed — the `Conn` trait made failure-injection possible where the SDK made it nearly impossible. Fixes #1-4 reported in the prior checkpoint.
- **EVENT joined the sync vocabulary** (ADR-009 DB tier): `ObjectKind::Event`, parsed/diffed/ordered-last, drift reads it live. Required a statement splitter (`schema::split_statements`) so EVENT-body semicolons inside `{ }` don't shatter the parse — `schema::tests::event_body_semicolons_do_not_split_the_statement`.

## Two integration findings — classified: loud, documented quirks (silent counter stays at 2)

1. **`FLEXIBLE` on `array<object>` does not reach elements (v3.2.0).** An embedded child field needs *two* defines: `field TYPE option<array<object>>` **and** `field.* TYPE object FLEXIBLE`. Surfaced as a loud coercion error ("no such field exists for table"), never silent. Handled in `sync::doctype_ddl`.
2. **`owner` must be `option<record<app_user>>`, not `record<...>`.** Root/system sessions have no `$auth`, so the `DEFAULT $auth.id` yields NONE; a required record type refuses it loudly. Record users still get stamped.

Both are loud, documented behaviors, not silent misbehavior. Cross-referenced in [[SurrealDB]]. Tripwire unfired; the [[ADR-002 SurrealDB Lock-In]] instance counter stays at **2** (#7432, #7433).

---

## Rollback / Dry-Run Position Paper (last SRS gap → REQ-6.6)

Written from *executed* gate/revert semantics. Every claim cites a test that ran.

### Q1 — What happens on a failed mid-sync?

**A sync is a sequence of per-resource transactions, not one transaction.** Each resource's DDL + its history row commit together, atomically (`tests::failed_transaction_rolls_back_ddl_and_history`: a THROW mid-transaction rolls back both the DDL *and* the history row — fix #3, now corroborated on the production backend by a second independent suite). But resource B's failure does **not** roll back resource A's already-committed transaction.

Concretely, on failure at resource B:
- Resources ordered before B (toposort): **applied and history-recorded.**
- Resource B: **fully rolled back** — no partial DDL, no history row (the atomicity guarantee).
- The run does **not** halt: every resource is attempted, errors collected per-resource (`MigrationReport::errors`), so operators see every problem at once rather than one-at-a-time.
- **Next boot re-diffs against recorded history**, so applied resources are no-ops and only B (and anything after it) retries. The sync is *resumable by construction* — this is the same property `fleet::tests::failed_checkpoint_warns_and_continues_then_resume_checkpoints` proves at the tenant-fan-out level.

**Honest limit:** there is no cross-resource transaction. A sync that half-completes leaves the schema in a valid intermediate state (each resource all-or-nothing), never a torn one — but not the *target* state until re-run.

### Q2 — What does a destructive field-type change do today?

The classifier (`field::classify_field_change`, `classify::classify_diff`) grades every field change, and the gate (`decision::decide`) acts on the worst:
- **Field drop** → `Blocked`, and separately flagged destructive: refused unless `allow_destructive` (`schema::tests::removed_field_is_destructive_drop`).
- **Narrowing / incompatible type change** (e.g. `string`→`int`, `float`→`int`) → `Blocked` (`field::tests::string_to_int_is_blocked_narrowing`, `float_to_int_is_blocked`).
- **Optional→required, or required-add-without-default** → `NeedsBackfill` (`field::tests::optional_string_to_string_is_not_safe`, `added_required_field_needs_backfill`): existing rows would violate it.
- **Widening** (`int`→`float`), **required→optional**, **new optional field** → `Safe` (`field::tests::known_numeric_widening_is_safe`, `int_to_optional_int_is_not_blocked`).
- **Opaque/unrecognized type** → conservatively `Blocked` (`field::tests::opaque_type_change_is_conservatively_blocked`).

**The gate is environment-aware and fail-closed** (`decision::tests::severity_environment_matrix`): in `Prod`, `NeedsBackfill`/`Blocked` **refuse** without acknowledgment; in `Dev`/`Test` they **warn and proceed**. `Environment::default()` is `Prod` — unspecified means strict.

### Q3 — What is dry-run/preview, honestly?

**Dry-run is the diff+classify pipeline run to completion with the apply step skipped and no lock taken** (`MigrationOptions::dry_run`; `tests::dry_run_revert_plans_without_mutating` proves the schema and history are untouched — `pinned` still accepted, history still at version 2 afterward). It returns:
- the exact DDL statements that *would* run (`PlannedMigration::statements`),
- the destructive operations and warnings,
- the full field-change classification.

**What it promises:** the plan is exactly what apply would emit from the *current* recorded snapshot. **What it cannot promise:** that apply will succeed — a dry-run diffs metadata, it does not test the DDL against live data (a `NeedsBackfill` field can pass dry-run and fail apply because real rows violate the new `ASSERT`). Dry-run is a *plan preview*, not a *success predictor*. Reviewable `.surql` artifacts (`write_artifacts`) make the plan a PR-reviewable file.

### Q4 — What can "rollback" honestly mean?

The engine has **revert-from-snapshot** (`revert_platform`/`revert_tenant`), and it is genuinely useful, but its honest scope is narrow and must be stated:

- **Revert is `diff(current, target)` run backward** — schema only. `tests::revert_to_version_jumps_directly` and `revert_of_an_added_field_is_destructive_and_gated` prove it: reverting an added field *drops* it (destructive → gated exactly like a forward drop); reverting a dropped field *re-adds* it (`revert_of_a_dropped_field_re_adds_it_and_is_safe`). History is an append-only event log, so a revert is a new forward event whose target is a past snapshot — the version number always increases.
- **Revert restores SCHEMA, never DATA.** Reverting a field-drop re-adds the column definition; the values that were in it are gone. This is inherent to any schema rollback and is the bright line: **schema revert ≠ data restore.**
- **The corrupt-snapshot asymmetry** (fix #2) bounds what revert can even attempt: `load_full_history` (revert's source) refuses on *any* unparseable snapshot (`tests::corrupt_history_snapshot_is_a_hard_error`), while forward migrate tolerates a corrupt *superseded* row as long as the latest parses (`corrupt_superseded_snapshot_does_not_block_forward_migrate`). Revert needs every intermediate snapshot intact; forward migration needs only the current one.

**The honest line:** the engine offers *schema revert to any recorded version*, transactional per-resource, gated for destructiveness. It does **not** offer point-in-time data recovery — that is `surreal export`/backup territory, and the changefeed (unbypassable audit, [[ADR-002 SurrealDB Lock-In]]) is the record of what data *was*, not a mechanism to put it back. A production "undo a bad migration" runbook is: **revert schema (this engine) + restore data (backup), in that order** — two tools, named honestly, not one pretending to be both.

### Proposed requirement (draft for grill → REQ-6.6)

> **REQ-6.6 (Migration Safety & Reversibility).** The schema-sync engine MUST provide: (a) **dry-run preview** returning the exact planned DDL, destructive operations, and per-field change classification without mutating schema, history, or taking locks — understood as a *plan preview, not a success predictor*; (b) an **environment-aware, fail-closed gate** that refuses destructive (field-drop) and unsafe (narrowing, needs-backfill) changes in production absent explicit operator acknowledgment, while warning-and-proceeding in development; (c) **schema revert to any recorded snapshot version**, transactional per-resource and subject to the same destructive-change gate as forward migration. The system MUST NOT represent schema revert as data recovery: restoring lost row data is out of scope for the sync engine and belongs to backup/export tooling. Operators MUST be able to distinguish, from engine output alone, a *plan* (dry-run) from an *applied change* (history-recorded) from a *revert* (a new forward event targeting a past snapshot).

Two sentences of that are the load-bearing ones: **dry-run is a preview not a predictor**, and **schema revert is not data recovery** — both are lines the engine's executed behavior draws, not aspirations.

## Related
[[WO-005 Metadata Kernel v0]] · [[ADR-002 SurrealDB Lock-In]] · [[ADR-008 Data Shape]] · [[ADR-009 Execution Model]] · [[SRS]] (REQ-6.6) · [[2026-07-24 Live-query and event fidelity (WO-004)]]
