---
tags: [frust, work-order, doctype, desk, milestone-5]
status: IMPLEMENTATION EXISTS, UNRECONCILED (2026-08-01) — the cancelled worktree agent HAD built it (found UNCOMMITTED in its worktree — NOT "nothing"): `sync.rs` +90 (kernel single-record sync), `main.rs` +238 (Desk single-form), new `tests/single_doctype.rs`. **NOT copied to the main tree** — it's base-254d812, the main tree has moved, and a concurrent session may also be doing `is_single`. **Boss decides:** reconcile this implementation, or let a main-tree Single-DocType session own it (confirm which — don't ship two). Recoverable from the worktree until pruned; unreviewed by PM (the *compiler-unchanged* gate needs checking).
created: 2026-08-01
---

# WO-061: Single DocType

## Why

Frappe's Single DocType (`issingle`) holds exactly one record — settings-style (Company defaults, Accounting Settings). Frust has no equivalent; a config singleton today is a one-row table with a nonsensical list view. A small, low-risk fill — **the alpha is untouched: a single is a normal table + a metadata flag + Desk UX**, not a new storage mode and not a compiler change.

## Gates (exit criteria)

1. **`is_single` flag; exactly-one-record invariant** at a stable, well-known id — a second create is refused or resolves to the one record (an `ASSERT` or fixed-id upsert; a metadata property, NOT a kernel write-path special case — if it needs one, **escalate**).
2. **The compiler is UNCHANGED — this is the load-bearing gate** (proves it's sugar, not a special path): a single syncs via `DEFINE TABLE` like any doctype, one row, and the alpha (DB-enforced permissions under the caller's session) applies identically. **Do NOT build Frappe's `tabSingles` key-value store** — one row in a real table is cheaper in SurrealDB (a MariaDB table-count workaround we don't inherit).
3. **Desk: a direct edit-this-record form — no list view**, a Settings-style page reachable from home. Field-level permissions apply (asymmetric read/write by role — asserted, since settings usually are).
4. Live through `frust serve` + browser; regression green; zero kernel-correctness risk.

## Boundary

Desk UX + a metadata flag + the single-record invariant. Not a settings *framework*, not defaults-inheritance, not a settings-registry. One singleton, editable, permission-respecting.
