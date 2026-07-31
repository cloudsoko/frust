---
tags: [frust, adr, permissions, security, surrealdb]
status: ACCEPTED 2026-07-26 (WO-020; PM ratified — invariant split is the load-bearing argument; docstatus-0 constraint on clerk transitions promoted to a workflow design rule)
---

# ADR-012: Row-Write Permission — Who May Update a Row

## Context

Since WO-005 the permission compiler has emitted, for every DocType:

```
FOR update, delete WHERE $auth.role = 'manager'
```

That is a coherent policy — *only managers mutate existing rows* — but it was
never a **decided** one. It was a compiler default that shipped unexamined for
fifteen work orders, and it did not match what the WOs built on top of it:
WO-009 gave clerks a save button, WO-014 gave them dynamic-form edits, WO-017
gave them client scripts that mutate their own drafts. Every one of those was
UI over a write the database refused.

It stayed invisible because of a second defect (**Finding A**): a refused
`UPDATE` in SurrealDB returns an *empty result set*, not an error, and the
broker returned `Ok(Null)` for it. A permission-denied write looked like a
successful one. Discovered when the WO-018 workflow's canonical proof — a clerk
submitting their own draft — could not run.

## Decision

**Update permission is:**

```
FOR update WHERE (owner != NONE AND owner = $auth.id AND docstatus = 0)
                 OR $auth.role = 'manager'
```

on **submittable** DocTypes, and

```
FOR update WHERE (owner != NONE AND owner = $auth.id) OR $auth.role = 'manager'
```

on **non-submittable** ones (which have no `docstatus` field — gating on a
field that does not exist would evaluate `NONE = 0`, false, and lock owners
out).

**Delete stays manager-only.** The WO ruled update; delete is destructive and
unruled, so the conservative default holds until evidence demands otherwise.

## The invariant split (why this is not duplicated enforcement)

Two different questions, two different enforcers:

- **The row permission gates WHO may write** — an owner (of a draft) or a
  manager. This is REQ-3.1.2, row-level security, enforced by the DB under the
  caller's own session.
- **The lattice EVENT gates WHICH docstatus moves are legal** — 0→1→2, no
  skips, no resurrection from 2 (ADR-009). This fires for *everyone*, managers
  included.

A manager passes the row permission and is still refused a 0→2 jump by the
lattice. An owner passes neither the manager clause nor (for a submitted doc)
the draft clause. The two never enforce the *same* invariant, which is the
whole reason **option 2 beats option 1** (owner-writes-always): option 1 would
have made the row permission responsible for immutability, duplicating what the
lattice already guarantees — P-3.2's four-layer validation creep, reborn.

## Empirical basis (probed on v3.2.0 before deciding — WO-020)

1. **`docstatus = 0` is accepted in an update permission.** No syntax barrier.
2. **Update permissions evaluate against the AFTER-state**, not the before.
   Proven: an owner setting `docstatus` 0→1 on their own draft is *refused* —
   a before-state evaluation would have allowed it.

The after-state behavior is load-bearing and has a named consequence:

> **An owner cannot advance docstatus at all.** Editing a draft's fields keeps
> docstatus 0 and is allowed; any write that would leave the row at a non-zero
> docstatus is refused. Advancing the lattice is therefore a **manager** act
> (or the lattice's own), never an owner's direct write.

This aligns with approval semantics — you do not approve your own expense by
setting `docstatus = 1`; a manager does — and it means a workflow's
**clerk-driven transitions must stay at docstatus 0** (the WO-018 expense flow
does exactly this: `Draft → Submitted for Approval` is 0→0; only the manager's
`Approve` is 0→1). A workflow that has a clerk move docstatus directly will be
refused by this permission, correctly.

## Finding A, generalized (the fix that stands regardless of policy)

A write that changes **zero rows** returns a typed error
(`E_WRITE_NO_ROWS`, surfaced as `PermissionDenied`), never `Ok`. It names both
possibilities — the record does not exist, or the caller's role may not write
it — because the caller cannot distinguish them and guessing sends an operator
to the wrong place. This lives in `db_write_inner`, the single write path, so
it covers every write (plain, child-table, workflow transition) uniformly. It
is correct under *any* permission policy: a write that silently does nothing is
the failure this project exists to refuse.

## Deferred, with a trigger

**Owner edits of a submitted doc's allowlisted fields** (`allow_on_submit`) are
NOT built. v1 is manager-only post-submit. The field-level-PERMISSIONS path is
expensive and speculative; it is deferred until a concrete case (expected from
the accounting seed, WO-022) forces it with evidence. Recorded here so the next
session finds the reasoning, not a surprise.

## Consequences

- The WO-009 Desk save works for a clerk on their own draft for the first time.
- Everything built over the silent-refusal since WO-005 now actually writes.
- Perf: the update clause adds one indexed field comparison; measured within
  the WO-018 baseline (submit 27–38 ms / 60 ms gate).

## Related

[[ADR-009 Execution Model]] (the lattice half of the split) · [[ADR-003 Tenancy Model]] · [[SRS]] (REQ-3.1.2, REQ-4.1.1) · [[SurrealDB]] (Finding A caveat; after-state update-permission evaluation) · [[WO-020 Row-Write Permission]]
