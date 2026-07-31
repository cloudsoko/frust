---
tags: [frust, build-log, permissions, security, work-order]
created: 2026-07-26
work-order: "[[WO-020 Row-Write Permission]]"
status: all 7 criteria done — WO-020 complete; WO-018 criterion 6 unblocked
---

# Build Log — WO-020: Row-Write Permission

Finding B closed: non-manager users can now edit their own drafts, the database
enforces it, and the policy is a documented decision (ADR-012) instead of a
fifteen-WO compiler default.

## The probe earned the design (empirical-first rule)

Before writing any DDL, probed the escalation risk the WO named — *can v3.2.0
express `docstatus = 0` in a row permission?* Two findings, both load-bearing:

1. **`docstatus = 0` is accepted** in an update permission. No syntax barrier.
2. **Update permissions evaluate against the AFTER-state**, not the before.
   Proven: an owner setting docstatus 0→1 on their own draft was *refused* — a
   before-eval would have allowed it.

The after-state result is why an owner can edit a draft's fields but cannot
advance docstatus. It also forced a correctness fix the naive version misses:
`docstatus = 0` on a **non-submittable** table references a missing field
(`NONE = 0`, false) and would lock owners out — so the clause is conditional on
`docstatus` existing. Submittable → draft-gated; non-submittable → plain
ownership.

## The four-way enforcement proof (criterion 1)

Each through the broker **under the caller's own session** — the only way "the
DB is the enforcer" means anything:

| case | result |
|---|---|
| owner edits own DRAFT | ✅ writes, and it lands |
| owner edits own SUBMITTED doc | ❌ `E_WRITE_NO_ROWS`, nothing written |
| owner tries to advance docstatus | ❌ refused (after-state) |
| clerk edits ANOTHER's draft | ❌ refused (can't even see it) |
| manager edits anything | ✅ draft or submitted |
| non-submittable, owner edits | ✅ ownership alone governs |
| write to a missing record | ❌ typed even for a manager |

## Finding A, general (criterion 2)

The zero-rows-affected fix lives in `db_write_inner`, the single write path —
so it was already general; verified rather than re-implemented. A write that
changes nothing returns `E_WRITE_NO_ROWS` (naming both possibilities, since the
caller can't distinguish record-absent from role-denied), never `Ok`.

## Blast-radius audit (criterion 4)

Full suite after the change surfaced exactly **one** real interaction, and it
was instructive rather than a regression: `layer_two` (WO-018) had a **clerk**
take a buggy 0→2 transition to prove the lattice catches it — but under the new
policy the clerk-owner can't write docstatus at all, so the **row permission**
refuses first, before the lattice is reached.

The fix makes the two-layer proof *more* honest: the buggy Leap is now taken by
a **manager**, who clears the row permission, so the lattice EVENT is the only
thing left to refuse the jump. **A principal who passed every gate above it,
still refused by the floor** — that is layer 2 in isolation. The layers compose
in depth (row-permission first for owners, lattice second for everyone).

### The load-bearing check: the browser, both directions

- **clerk1 edited their own "Alpha order" in the Desk and it PERSISTED** —
  verified in the database, not just the UI. This is the WO-009 regression
  (silent since the Desk shipped) fixed and *seen*.
- **clerk1 opening clerk2's document got "Can't do that — not yours to see"** —
  the row-select refusal, indistinguishable from non-existence (the WO-019
  not-a-leak property).

## Self-seeding (criterion 5)

`permission_proof` and `rest_surface` were the only two binaries depending on a
hand-seeded ambient `skeleton` — a landmine tripped in three separate sessions
(WO-010, WO-016, WO-018, the last one by me). Both now build the **exact**
recovered fixture themselves via `tests/common/mod.rs::seeded_broker`, in a
unique database per call (the counter is required — `rest_surface` runs its
tests in parallel and a shared name races). Neither touches ambient dev state
again. The landmine is gone by construction, not by care.

## Decision record (criterion 6)

**ADR-012** drafted for ratification. The invariant split, stated cleanly:

> The row permission gates WHO may write (owner-of-a-draft, or manager).
> The lattice EVENT gates WHICH docstatus moves are legal (0→1→2, no skips, no
> resurrection) — for everyone, managers included.

Different invariants, not duplicated enforcement — which is exactly why
**option 2 beats option 1**: option 1 (owner-writes-always) would have made the
row permission responsible for immutability, duplicating the lattice (P-3.2
reborn). `allow_on_submit` deferred with its trigger (the WO-022 seed).

## Floor holds (criterion 7)

Perf gates on a **dedicated scratch data-dir** (new caveat — the dev `data`
directory was never renamed or swapped; surreal pointed at a throwaway dir):

| run | submit (gate 60) | realtime tax (allowance 2) |
|---|---|---|
| 1 | 23 ms | 0.48 ms |
| 2 | 28 ms | 0.66 ms |

Inside the WO-018 baseline. The policy adds one indexed field comparison; no
measurable cost.

## Suite state

**32 kernel binaries green**; the 33rd (`perf_gates`) is green on a fresh store
and flaps only on a churned one — the standing substrate caveat, not a WO-020
regression. Dev store restored (6 rows, 3 users); 104 scratch databases dropped
at close.

## What this unblocks

WO-018 criterion 6 is green: the workflow suite's `the_expense_claim_flows...`
now runs end to end — **a clerk creates and submits their own draft**, a
manager approves it to docstatus 1. The canonical proof that was blocked at the
start of WO-018 works.

## Related
[[WO-020 Row-Write Permission]] · [[ADR-012 Row-Write Permission]] · [[WO-018 Workflow Engine]] · [[ADR-009 Execution Model]] · [[SRS]] (REQ-3.1.2, REQ-4.1.1) · [[SurrealDB]] (Finding A; after-state update-permission evaluation)
