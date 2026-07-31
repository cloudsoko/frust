---
tags: [frust, work-order, data-integrity, hooks, P0]
status: COMPLETED 2026-07-27 — P0 killed. Hooks receive `baseline ∪ delta`, baseline read under the CALLER's session (sql_as — no read bypass; unseeable row → empty baseline → same E_WRITE_NO_ROWS). Persist iff caller-wrote OR hook-changed (diff vs shown doc) → MERGE's atomic partial-write preserved. Concurrency PROVEN: two writers, disjoint fields, same record, both survive + lines intact (lost-update race stayed shut). 2 diff traps handled: `same_value` compares NUMERICALLY (50.98 echo not dirty — WO-016 lesson); baseline typed from DocType (SurrealDB returns decimals as JSON strings → untyped would re-type money as text; required widening broker FieldMeta with fieldtype). Cost: CREATE reads nothing (WO-026 create-heavy bench untouched); only hooked updates pay one indexed read. Test gap closed: accounting_seed_e2e asserts lines==2 after EVERY state change. 38 groups/37 binaries green. Lesson: *a test that only checks what it changed cannot catch what it silently destroyed.* → [[2026-07-27 WO-028 full-document hooks]]
created: 2026-07-26
---

# WO-028: Full-Document Hooks (Partial-Update Data Loss)

> [!danger] P0 silent data loss, found by [[WO-027 Backup Restore DR]]'s probe. Any validate hook that derives or echoes a field silently destroys that field on any partial update — the accounting seed's reconciliation script (`doc.lines = doc.lines || []`) wiped embedded invoice lines on every workflow transition. The seed is just the first app with *both* a child table and a workflow; every future app of that shape hits it.

## The bug

`db_write_inner`'s comment claims *"the FULL document goes to the hooks now (ADR-006 edge 3)"* — **false for partial updates.** `hook_doc` is whatever the caller passed. On a workflow transition the validate hook sees only `{workflow_state, docstatus, id}`; a script reading `doc.lines || []` gets `[]`, writes it back, and `UPDATE … MERGE` faithfully empties the field. Reproduced: 2 computed lines after CREATE → `[]` after one Submit.

**Classification: implementation bug against a CORRECT ADR.** ADR-006 edge 3 ratified the full-document contract; the code doesn't deliver it. The fix restores the ADR's intent — no ADR amendment, but ADR-006 gets a note that full-doc-on-partial-update is load-bearing for embedded data.

## The ruling (fix direction decided; the concurrency handling is the work)

Rejected: "hooks receive only the delta, must never echo" — makes correctness depend on script-author discipline where the failure is silent data loss (the class this project refuses). Adopted: **make the full-document contract true.** But naïve read-modify-write reopens a lost-update race under WO-025's concurrent loop, so:

## Exit Criteria

1. **Hooks see the full document on partial updates:** the kernel reads the current record and merges the caller's delta onto it *before* invoking the hook, so the hook's view is the complete document (embedded children present). The seed's reconciliation script becomes correct unchanged.
2. **Persistence stays field-scoped — no lost-update race:** persist only the fields that actually changed (diff the hook's output against the pre-hook merged doc; MERGE just that set). Unmentioned-unchanged fields are never rewritten. **Prove with a concurrent-partial-update test:** two workers updating *different* fields of the same record concurrently → both survive (the race the naïve full-doc-write would open).
3. **The no-hook fast path is preserved:** the read+merge happens only when the DocType has a hook that consumes the doc — a no-hook DocType keeps WO-026's throughput. State the measured cost on the hooked path (one indexed record read); both perf gates green on a fresh store; the 124 req/s no-hook figure must not regress.
4. **The round-trip regression test that would have caught it:** `accounting_seed_e2e` (and the discipline generally) must **re-assert the whole document survives after every state change**, not just the field under test. The suite missed this because it checked AR (derived from `total`, unaffected) and never re-read `lines` after the transition. Add mutate-then-assert-whole-survives; this generalizes past the seed.
5. **Full suite green** — embedded children survive create → partial update → workflow transition → cancel; no other hook path regressed.

## Escalations

Standard rules + full hygiene set. **If the diff-and-MERGE-changed approach can't cleanly express a hook that legitimately rewrites an embedded array, report it** — there may be a nested-value diff subtlety (decimal scale, array identity) worth an explicit rule rather than a silent guess.

**Related:** [[Frust Hub]] · [[ADR-006 Plugin Capability Surface]] (edge 3 full-doc contract) · [[ADR-008 Data Shape]] (embedded children — the data at risk) · [[2026-07-26 WO-025 concurrent serve loop]] (the race context) · [[WO-027 Backup Restore DR]] (found it)
