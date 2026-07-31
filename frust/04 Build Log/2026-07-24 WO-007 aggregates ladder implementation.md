---
tags: [frust, build-log, aggregates, kernel, work-order]
created: 2026-07-24
work-order: "[[WO-007 Aggregates Ladder Implementation]]"
---

# Build Log — WO-007: Aggregates Ladder (ADR-010 Tiers 1–2)

First post-kernel feature build. Both tiers implement ONE algebra — a document's **signed contribution**: subtract what the before-doc contributed, add what the after-doc contributes. Tier 1 evaluates it in-transaction (generated `DEFINE EVENT`); Tier 2 evaluates it in the kernel off the changefeed. Create, edit, key-move, delete, and cancel are all the same two operations.

## Exit criteria

| # | Criterion | Evidence | Result |
|---|---|---|---|
| 1 | Tier-1 counter generated from metadata; exact under burst; monthly report < 100 ms at 1 M | `aggregates_ladder::tier1_monthly_counter_exact_under_burst` + `tier1_counter_at_1m` | ✅ **16/28/51 ms vs 7.7 s live (~275×)**; reconciliation exact after concurrent bursts |
| 2 | AR counter with cancel-reversal | `tier1_ar_counter_reverses_on_cancel` | ✅ mixed submit/pay/cancel burst reconciles exactly; a fully-canceled customer's bucket reverses to n=0, AR=0 |
| 3 | Tier-2 worker rollup (2-hop), queryable lag, restart loses nothing | `tier2_group_revenue_rollup_with_restart` | ✅ reconciles after burst + mid-stream restart; cursor is a readable record; `lag()` derived from it |
| 4 | Item-wise rollup from embedded-line diffs (the no-live-door shape) | `tier2_item_rollup_from_line_diffs` | ✅ the report exists through the contract and reconciles against a root flatten |
| 5 | Rollups are DocTypes | asserts in tests 1 & 4 | ✅ declared in metadata, synced through the engine, read via `db_read` under a record principal; non-managers see zero rows; tamper writes match nothing |

**The named escalation is a NON-EVENT:** 6-thread contended bursts produced optimistic write conflicts (3–5 retries observed per run, all absorbed by the db-layer retry contract) and **zero lost increments** — reconciliation is exact every run. Tier 1's admission to ADR-010 stands on measured ground.

## What was built

- `sync.rs` — `aggregates:` declarations on DocType metadata. `kind: counter` compiles to a `DEFINE EVENT` through the same engine pipeline as all DDL (diff/gate/history); `kind: worker` marks the source `CHANGEFEED 7d INCLUDE ORIGINAL`. Any aggregate's `rollup` target compiles **write-closed** (`create/update/delete NONE`) and lands in the toposort as a dependency (an EVENT UPSERT into an undefined table would auto-create it permissionless). `backfill_counter()` recomputes a rollup in one transaction (4.3 s at 1 M).
- `aggregates.rs` (new; surql-monopoly allowlist ruling recorded in that test) — feed decoding with before-doc reconstruction, the `Contrib` trait, `GroupRevenue` (memoized 2-hop) and `ItemSales` (line diffing) handlers, and `RollupWorker`: replay from a persisted versionstamp cursor, fold contributions, then **apply deltas + advance cursor in ONE transaction** — crash-replay is exactly-once by construction, which is the whole restart proof.
- `main.rs` — `frust serve` builds rollup workers from metadata (`handler: group_revenue | item_sales`) and drains them on the resident tick; an unknown handler name refuses boot (a silently-not-running rollup would be unbounded staleness).

### Measured costs (1 M fixture, release)
- Counter EVENT on the write path: **6.0 ms vs 5.6 ms median without** (raw record-session writes) — ~0.4 ms buys transactional exactness.
- Backfill: 4.3 s one-time over 1,001,610 rows → 13 rollup docs.
- Post-run consistency spot-check: March = 77,757 in both live scan and rollup.

## Tier 0 rule table (documented here per WO scope; implementation rides with Desk v1)

| Report shape | Rule | Why |
|---|---|---|
| Range + sort (register) | Filter on the stored period field (`month = 'YYYY-MM'`), page within the bucket | Equality dodges #7432 structurally; the trap is range-only |
| Scoped entity reads (statement) | Equality index on the entity link at 10 M; stay live | 0.45 s live at 1 M; equality indexes were clean in WO-006 |
| Any range + explicit order through the broker | `WITH NOINDEX` stays (unchanged posture) | #7432 alive in pinned v3.2.0 (16× at 1 M) |
| Stored period fields | Part of the standard DocType shape | One field; Frappe-realistic; feeds both Tier 0 and Tier 1 |

## Findings (all probed on pinned v3.2.0)

1. **EVENT-body writes bypass table permissions.** A record user's submit maintains a `create/update/delete NONE` rollup; the same user's direct UPSERT matches nothing. This is what makes tamper-proof Tier-1 rollups possible — load-bearing, worth a regression canary if we ever bump the pin.
2. **`$auth` resolves differently by context.** In PERMISSIONS clauses `$auth.role` works regardless of `app_user` readability; in `DEFAULT`/query contexts `$auth.id` requires the user to be able to select their own record. With `app_user PERMISSIONS NONE` (the worker_queue test posture), `DEFAULT $auth.id` stamps **NULL** — and then `owner = $auth.id` row clauses pass via `NONE = NONE` for every non-manager. Skeleton's posture (`FOR select WHERE id = $auth.id`) is correct and is now what the WO-007 tests use. **Follow-up flagged: `app_user` DDL should become kernel-owned so this posture can't drift** — it is one config mistake away from a row-security hole.
3. **`INCLUDE ORIGINAL` does not give before-docs.** Updates carry `current` + a JSON-patch *undo*; `replace/add/remove` ops carry full old values (records, numbers, array elements), but plain strings degrade to diff-match-patch text ops. Implemented a strict `dmp_unpatch` (context-verified, loud on any mismatch — a reconstruction can fail, never lie). Deletes carry the full `original`; creates the full new doc.
4. **`SHOW CHANGES LIMIT` is not linear in entries** (LIMIT 5→1 entry, 15→3, 1000→24 on the same feed). Drains use 1000; small limits only appear in tests that deliberately want partial batches.
5. **`FOR $g IN (SELECT …)` fails to iterate** ("Cannot execute statement using value"); `LET $rows = SELECT …; FOR $g IN $rows` works. Backfill codegen uses the latter.
6. `type::thing()` is gone in v3; `type::record(table, key)` two-arg form is the replacement and handles non-ident keys (`roll:⟨2026-07⟩`).

None of these were silent — every one surfaced as a loud error or a probed, documented behavior. Silent-misbehavior counter stays at **2**.

## Housekeeping
- `scale_proof.rs` `DATA_DIR` updated to the relocated fixture (`D:/Dev/rust/frust-scale-data`).
- Full suite green: 41 tests across 13 binaries (incl. perf gates at the tightened 25 ms release floor); `tier1_counter_at_1m` runs `--ignored --release` against the fixture instance.
- Fixture left healthy: counter EVENT live on `sales_invoice`, rollup consistent; instance stopped.

## Related
[[WO-007 Aggregates Ladder Implementation]] · [[ADR-010 Materialized Aggregates]] · [[ADR-009 Execution Model]] · [[2026-07-24 WO-006 1M-row scale proof]] · [[SurrealDB]]
