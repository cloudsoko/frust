---
tags: [frust, work-order, permissions, security]
status: COMPLETED 2026-07-26 — all 7 criteria; ADR-012 ratified. Four-way enforcement proven through the broker per principal; **clerk-edits-own-draft persisted in the BROWSER** (WO-009 regression fixed and seen, not just tested) + clerk-sees-not-another's counter-check; after-state eval discovered by probe (owner can't advance docstatus → clerk transitions stay at 0); conditional clause for non-submittable tables; Finding A general in db_write_inner; permission_proof + rest_surface self-seeding (3-session landmine gone); blast radius = 1 instructive interaction (layers compose in depth); floor held on a dedicated scratch dir (submit 23–28/60, tax 0.48–0.66/2). → [[2026-07-26 WO-020 row-write permission]]
created: 2026-07-26
---

# WO-020: Row-Write Permission (Finding B — the Compiler's Update Policy)

> [!info] PM work order. Escalated out of [[WO-018 Workflow Engine]] (criterion 6 blocked). Governing: REQ-3.1.2 (row-level security), the permission compiler (kernel), [[ADR-009 Execution Model]] (lattice as the immutability floor). **Ruling below is the direction; the WO implements and proves it, and produces the decision record.**

## The finding (broke closed, not open)

Since WO-005 the permission compiler has emitted `FOR update, delete WHERE $auth.role = 'manager'`. **No non-manager can update any row, including their own draft.** The DB enforced this correctly the whole time (nothing unsafe shipped); Finding A ([[SurrealDB]] caveat — refused UPDATE returns `Ok([])`) swallowed the refusal, so it was invisible. Consequences masked since: WO-009 Desk save for clerks, WO-014 dynamic-form edits, WO-017 client-script mutations — all built over a write the DB refused silently.

## The ruling (option 2, PM-decided 2026-07-26)

Update permission becomes: **`(owner = $auth.id AND docstatus = 0) OR $auth.role = 'manager'`** — owners edit their own drafts; managers write anytime. Rejected: option 1 (owner-writes-always → dual enforcement of immutability with the lattice EVENT, P-3.2 reborn); option 3 (privileged transition write → second authority path, forbidden by the door-probe's authority-is-not-a-parameter result).

## Exit Criteria

1. **The policy compiles and enforces, proven three ways:** clerk edits own draft → OK; clerk edits own *submitted* doc → refused typed (not swallowed); clerk edits another's draft → refused; manager edits anything → OK. Each asserted through the broker under the caller's own session.
2. **Finding A stays fixed and general:** every write path returns a typed error when zero rows are affected — not just the transition verb. `E_WRITE_NO_ROWS` (or successor) names both possibilities (row absent / role may not write) since the caller can't distinguish from there.
3. **The `allow_on_submit` sub-question, answered with a stated default:** v1 = post-submit writes are manager-only; owner-`allow_on_submit` is **deferred** and noted in the decision record. If a later WO (the accounting seed) produces a concrete owner-post-submit-edit case, that reopens it with evidence — do not build the field-level-PERMISSIONS path speculatively.
4. **Blast-radius audit:** enumerate everything silently broken since WO-005 (from Finding A's suite run) — table in the log. Re-verify the WO-009 Desk save works for a clerk in the browser (the user-visible regression). Blast radius already measured at escalation = **zero tests hit `E_WRITE_NO_ROWS`** (Finding B was dormant because every test writes as manager — untested surface is where dormant bugs live); the browser re-verify is the load-bearing check, not the suite.
5. **Make `permission_proof` + `rest_surface` self-seeding — do it HERE, not later.** They are the only two binaries depending on a hand-seeded ambient `skeleton`; that landmine has now been tripped three times (WO-010, WO-016, WO-018). WO-020's criterion-1 four-way proof *needs* controlled seed data (clerk1/clerk2/manager + owned rows) under known docstatus — so self-seeding isn't scope creep, it's a prerequisite. The canonical rows are recovered and pinned in `frust-skel/setup.surql`; fold them into the test's own setup so no test depends on ambient dev state again.
6. **Decision record:** produce the ADR (or amendment) documenting the update-policy as a deliberate decision — undocumented compiler default for 15 WOs; it stops being implicit here. Row-permission gates *whether the write exists*; the lattice EVENT independently gates *the docstatus value* — different invariants, not duplicated enforcement (this is why option 2 beats option 1).
7. **Floor holds:** full hygiene set, perf gates on a **dedicated scratch data-dir** (never the dev store — new caveat), WO-018 baseline submit 27–38/60, tax ~1.1/2.

## Escalations

Standard rules. If option 2's DDL can't express `docstatus = 0` in a row permission at v3.2.0, that's an empirical finding — report before working around.

**Related:** [[Frust Hub]] · [[WO-018 Workflow Engine]] · [[ADR-009 Execution Model]] · [[SRS]] (REQ-3.1.2, REQ-4.1.1) · [[SurrealDB]] (Finding A caveat)
