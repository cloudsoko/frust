---
tags: [frust, build-log, kernel, correctness, milestone-5]
created: 2026-08-01
work-order: "[[WO-057 Refused-Create Silent Success]]"
status: DELIVERED — a CREATE that persists nothing is now a typed 403 E_WRITE_NO_ROWS instead of 200 "created" with a null record. Control watched RED on the exact WO-056 repro, then green. Both sides tested: the refusal is typed AND a legitimate create still returns its record. Live through `frust serve`.
---

# WO-057 — a refused CREATE reported success

## The defect, and why it survived

WO-020 found that **a write the database refuses returns an EMPTY RESULT SET,
not an error** — so returning `Ok` made a permission-denied write look like a
completed one. It closed that with a typed `E_WRITE_NO_ROWS`. But the guard was
written as one match arm:

```rust
match (op, &row) {
    (WriteOp::Update, None) => Err(...E_WRITE_NO_ROWS...),
    _ => { /* success path: stored = row.unwrap_or(Null) */ }
}
```

`(Create, None)` fell through the catch-all and became `Ok(Null)`. Six
milestones later, WO-056's dogfood clicked "New" on a write-closed rollup and
got `200 {"action":"created","created":null,"record":null}` with no row created.

**Two things made it visible, and both are worth noting.** It was found by
*using the app*, not by a test — no test had ever created into a write-closed
table. And WO-055's additive `action`/`record` keys are what made the lie
legible: `action:"created"` beside `record:null` is self-contradicting in a way
`{"created":null}` alone was not.

## The fix

The sibling arm, not a redesign:

```rust
(WriteOp::Create, None) => Err(BrokerError::PermissionDenied {
    detail: format!("E_WRITE_NO_ROWS: create in {} stored nothing: the table is \
                     write-closed (maintained by the kernel, not by record users), \
                     or your role may not create in it", meta.name),
}),
```

**A zero-row CREATE has one meaning, unlike UPDATE's two.** The update message
says "the record does not exist, *or* your role may not write it" because it
genuinely cannot tell. A create has no missing-record case: a create that
succeeds always returns its record, so zero rows means the insert was refused.
The message says that directly rather than hedging.

## Criterion 2 — the control, watched red

Against the pre-fix binary, on the WO-056 repro:

```
create into a write-closed rollup -> Ok(Null)
```

then green:

```
create into a write-closed rollup -> Err(PermissionDenied {
  detail: "E_WRITE_NO_ROWS: create in party_total stored nothing: ..." })
```

The test builds its own write-closed table the way the platform does — a source
DocType declaring a Tier-1 aggregate, which is what compiles the rollup
write-closed (ADR-010) — rather than depending on the dev store's fixture.

## Criterion 3 — both sides, because this fix could go wrong in two directions

The WO-055 lesson applied: a guard that turns a false success into a false
failure has not improved anything.

| side | asserted |
|---|---|
| refused create | typed `E_WRITE_NO_ROWS`, names the table, **and still zero rows** — the DB's refusal is untouched; only the response changes |
| legitimate create | returns its record, and the row really is stored |
| the rollup still works | the EVENT-maintained aggregate is populated on the legitimate write — "record users may not write it directly" never meant "the ladder is broken" |
| UPDATE guard | unchanged, still `E_WRITE_NO_ROWS`, still `PermissionDenied` |

Those last two matter: without them a "fix" that broke Tier-1 rollups or
rewrote WO-020's arm would pass the headline test.

## Criterion 5 — live through `frust serve`

WO-056's exact repro, over HTTP, on the shipped binary:

```
POST /write/ar_outstanding   →  HTTP 403
{"error":{"detail":"E_WRITE_NO_ROWS: create in ar_outstanding stored nothing: ...",
          "kind":"permission-denied"}}
```

with the row count unchanged (still none), and the control alongside it:
`POST /write/customer` → `200 action=created record=customer:jn9hb…`.

## Criterion 4 — docs and harness followed

- `rest-api.md`'s `/write` section documents the refused-write answer explicitly
  ("a write that stores nothing is refused, never reported as done").
- `gaps.md`: the WO-056 **ESC** entry moved to a **"Fixed in WO-057"** section —
  kept, not deleted, with the note that it was found by using the app.
- `docs.spec.mjs`: **47 → 49 checks**, asserting both the 403/`E_WRITE_NO_ROWS`
  and that the refusal is not dressed as a success (`action !== 'created'`).

## One self-caught slip

The first version of the message used a Rust `\` line continuation that
collapsed into a run of literal spaces in the user-facing string
(`write-closed                      (maintained…`). Caught on the live HTTP
response, not in the test — the test asserted the code and the table name, both
of which were present. Rewritten as a clean single line. A reminder that
asserting *substrings* of a message does not assert that the message reads well.

## Verification

`refused_create.rs` 3/3 · docs harness 49/0 · both auth modes and the fresh-store
gates appended below · the Desk-glue half (not offering "New" on a write-closed
DocType) stays in WO-056's list as application-layer work, per this WO's own note.

## Related
[[WO-057 Refused-Create Silent Success]] · [[2026-08-01 WO-056 complete the dogfood]] (where it was found) ·
[[WO-020]] (Finding A, the UPDATE half) · [[ADR-010 Aggregate Ladder]] (write-closed rollups) ·
[[2026-08-01 WO-055 rest surface corrections]] (whose `action`/`record` keys made it legible)
