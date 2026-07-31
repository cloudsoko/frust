---
tags: [frust, build-log, data-integrity, hooks, concurrency, work-order]
created: 2026-07-27
work-order: "[[WO-028 Full-Document Hooks]]"
status: complete — silent data loss fixed, concurrency-safe, round-trip assertion added
---

# Build Log — WO-028: Full-Document Hooks

Silent data loss that shipped through the entire platform milestone, found by a
DR probe that was looking for something else, fixed without reopening a
lost-update race under the concurrent loop.

## The bug

`db_write_inner` carried this comment:

> *"The FULL document goes to the hooks now (ADR-006 edge 3)."*

**It was not true on partial updates.** `hook_doc` was the caller's partial
payload, so on a workflow transition a hook saw only
`{workflow_state, docstatus, id}`. The accounting seed's reconciliation script
then did what any reasonable script does:

```js
var lines = doc.lines || [];   // field absent → []
doc.lines = lines;             // writes [] back
```

`UPDATE … MERGE` is correct in itself — it does not touch unmentioned fields.
But the hook had **turned an unmentioned field into an explicitly-emptied one**,
and MERGE applied it faithfully. The embedded child rows were destroyed.

The general form is worse than the instance: **any validate hook that derives
or echoes a field silently destroyed that field on any partial update.** The
seed was simply the first app with both a child table and a workflow.

**Classification (PM ruling): an implementation bug against a *correct* ADR.**
ADR-006 edge 3 ratified the full-document contract; the code claimed it; the
implementation did not deliver it. The fix is to honor the ratified contract,
not to choose a new one. The alternative — "hooks get the delta, authors must
not echo" — was rejected: it makes correctness depend on every script author
remembering, and the failure mode is silent data loss.

## The fix — and the half that is the actual work

**1. Hooks see truth.** On an update the broker reads the current record
**under the caller's own session** (`sql_as`, never root — reading a record to
hand to a hook must not become a permission bypass; a caller who cannot see the
row gets an empty baseline and the same `E_WRITE_NO_ROWS` refusal ADR-012 would
have given) and merges the delta over it. Creates need no read: there is no
prior document.

**2. The write stays partial.** Handing hooks the full document means their
*output* is a full document. Persisting all of it would turn every hooked
partial update into a whole-record write — and under WO-025's parallel loop two
disjoint updates would then clobber each other (both read version A, both write
their own merge, last writer wins, one delta lost). **That would have traded one
silent data loss for another.**

So a field is persisted **iff the caller explicitly wrote it, or the hook
actually changed it** against the document it was shown. MERGE's atomic
partial-write property — the thing that made the old path safe — is preserved;
the bug was letting the hook *widen* the field set, not the merge itself.

### Two traps inside the diff

- **Representation vs value.** A hook echoing a decimal back must not count as a
  change, or every hooked update rewrites everything and the race reopens.
  `same_value` compares **numerically** via `Decimal`, not textually — WO-016's
  lesson landing in a new place.
- **Typing the baseline.** SurrealDB returns a decimal as a JSON *string*, so a
  `Currency` field would have re-entered the hook as text and left as text,
  quietly re-typing money. The baseline is typed from the DocType, which
  required adding `fieldtype` to the broker's `FieldMeta` (it carried only
  `fieldname` and `perm_role`).

## Proof

| test | result |
|---|---|
| partial update does not destroy unmentioned child rows | ✅ lines survive |
| a hook SEES the full document on a partial update | ✅ hook-derived `memo` still reads `lines=2` |
| **concurrent disjoint updates both survive** | ✅ `customer=writer-A`, `memo` intact, lines untouched |
| echoing an unchanged decimal is not a change | ✅ `total = 50.98dec` exact |

Also verified live through the running kernel: 2 lines through
CREATE → Submit (0→0) → Approve (0→1), `total` exact, docstatus advancing — and
two concurrent HTTP writers to disjoint fields on one record both landing
(`customer=writer-A`, `workflow_state=TouchedByB`, `lines=2`).

## The test gap that hid it — nameable, and mine

`accounting_seed_e2e` asserted the fields it cared about (AR, derived from
`total`, which happened to be unaffected) and **never re-read `lines` after the
transitions**. It is a cousin of *tested-seam ≠ wired-in-serve*:
**tested-the-output-I-expected, not the-invariant-that-held.**

The generalization, now carried: **mutate, then assert the whole thing survived
— not just the field under test. A test that only checks what it changed cannot
catch what it silently destroyed.** `accounting_seed_e2e` now asserts
`lines == 2` after *every* state change.

## Cost

CREATE is untouched (no prior document to read), so WO-026's create-heavy
throughput benchmark is unaffected. Updates pay one indexed record read, on the
path where the guarantee is bought.

## Suite

**38 test-result groups across 37 binaries, 0 failed, exit 0** — including the
new `hook_document_fidelity` binary.

## Related
[[WO-028 Full-Document Hooks]] · [[ADR-006 Plugin Capability Surface]] (edge 3, the ratified contract) · [[ADR-012 Row-Write Permission]] (the silent-wrong class) · [[2026-07-26 WO-025 concurrent serve loop]] (the race this had to not reopen) · [[2026-07-26 WO-022 accounting seed]] (where the gap was) · [[WO-027 Backup Restore DR]] (paused; resumes against data that now survives its own backup)
