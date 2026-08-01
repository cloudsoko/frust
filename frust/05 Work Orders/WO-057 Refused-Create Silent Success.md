---
tags: [frust, work-order, kernel, correctness, milestone-5]
status: ACTIVE (2026-08-01) — the WO-056 escalation: a correctness bug, not a gap. Jumps the queue.
created: 2026-08-01
---

# WO-057: Refused CREATE Reports Success

## Why

WO-056 found — and reproduced at the door — that `POST /write/{write-closed-rollup}` returns:

```
HTTP 200   {"action":"created","created":null,"record":null}
```

**and creates no row.** The containment is correct (the DB refused the write). The *response lies*: it says `created` when nothing was created. This is **WO-020's Finding A** — a write that changed nothing reporting success, the silent-wrong class this project exists to refuse — alive on the **CREATE** path. WO-020 closed it for UPDATE (`E_WRITE_NO_ROWS` in `db_write_inner`); the guard covers UPDATE, not CREATE. WO-055's `action`/`record` keys are what surfaced it (`action:"created"` beside `record:null` is self-contradicting).

This is not application polish and does not wait on the app-completeness direction call — it's a kernel correctness defect, and "a write that changed nothing must never report success" is a standing invariant.

## Exit criteria

1. **A CREATE that persists zero rows returns a TYPED refusal**, never `200 {"created":…}`. Same shape as the UPDATE fix — a code naming the likely cause (permission / write-closed), no response that claims `created` when nothing was.
2. **Failing control:** the WO-056 repro (`POST /write/ar_outstanding` as manager, a write-closed rollup) — watched **red** against current behavior (`200`/created-null) then **green** (typed refusal, still zero rows). The DB's refusal is unchanged; only the response the kernel synthesizes changes.
3. **Both-sides (the WO-055 lesson):** a *legitimate* create still returns its record — assert both, so the fix can't trade a false success for a false failure on real creates.
4. **Docs + harness follow:** `rest-api.md`'s `/write` section documents the refused-write response; `gaps.md`'s ESC entry moves to a "fixed in WO-057" line (kept, not deleted); `docs.spec.mjs` asserts the new shape.
5. **Both auth modes; live through `frust serve`** (a real refused create over HTTP returns the typed refusal); regression green.

## Boundary

- The CREATE analog of WO-020's UPDATE fix — same shape, don't redesign the write path.
- If a zero-row CREATE genuinely can't be distinguished from a legitimate no-op, STOP and report — but a write-closed table refusing is an unambiguous zero-rows case.

## Note for the record

The Desk *also* dead-ended the user on this (offered a "New" affordance for a write-closed rollup → 404 after the refused write). That's a Desk-glue gap in WO-056's list, not this WO — the kernel fix makes the refusal *honest*; the Desk not offering "New" on write-closed doctypes is separate application-layer work.
