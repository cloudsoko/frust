---
tags: [frust, work-order, hooks, kernel, plugins, scripts, milestone-5]
status: DELIVERED (2026-08-01) — REQ-2.2.1 satisfied FOR SERVER SCRIPTS, each contract stated and tested. ADR-006 edge-1 spent for the first time and REFINED by measurement: "additive" has one unit, the WORLD (growing an interface or a world export list breaks every component; only a new world beside the old is additive). No STOP reached. Plugin subscription to the new classes is proven-possible but UNBUILT and named as such. See [[2026-08-01 WO-053 hook vocabulary]].
created: 2026-08-01
---

# WO-053: Hook Vocabulary — REQ-2.2.1's Full Lifecycle Set

## Why

REQ-2.2.1 is a founding MUST: extensions subscribe to `before_insert`, `validate`, `on_submit`, `on_cancel`. Today only `validate` reaches the real delivery path (WO-049's sourced census: `HookClass{Validate, OnWrite, Scheduled}` exists in `contract.rs`, one wired). ADR-015 ruled the widening **ordinary build work** — the ADR-007 profile table's axis is verbs, not lifecycle, so no amendment. This is also **the first live exercise of ADR-006's edge-1 evolution policy** (additive-only growth + versioned-world host support) — the policy written on day two, never yet spent.

## Criteria

1. **Census before building** (the WO-049 discipline): what is *actually* wired today, per host — plugins (WIT world) vs scripts (engine) vs scheduled (WO-007's class ran real jobs — don't rebuild what exists). Sourced table first; the build fills only the real gaps.
2. **Additive delivery under the ADR-006 evolution policy.** New lifecycle exports grow the world *additively* — and the load-bearing proof is the compatibility one: **an existing component built against the old world still loads and runs**, receiving only the events it exports. If additive WIT growth cannot keep old components loading, STOP — that's the evolution policy meeting reality, an ADR-006 conversation, not an improvisation. (The WO-001/WO-005 artifacts are the natural old-component fixtures.)
3. **Each event's contract stated, not inherited:** when it fires relative to the write/transition; whether it may mutate (before_insert: yes, like validate); whether it may reject (on_submit/on_cancel: rejection blocks the transition — proven against the lattice: a hook-rejected 0→1 leaves docstatus 0, EVENT untouched). ADR-006's cycle rule already keys `(record-id, hook-class)` — plural in the ratified text; the trap extends per class.
4. **Extensions get the vocabulary too:** ADR-015's composition applies per hook class — owner-first, un-overridable, per-app-attributed, veto-names-its-app — the WO-050 dispatch loop generalized, with the silent-override control re-run per class.
5. **Manifest validation grows with the vocabulary** — the WO-019 rule stands: a hook point that doesn't exist is refused at the door; the *new* points validate; a typo is still a 400.
6. **Live proof through `frust serve` + browser** (tested-seam≠wired): a real `on_submit` script observably fires on a real workflow transition — the WO-043 notification or a script side-effect, content-asserted.
7. Both auth modes; fresh-store gates (write path touched); SRS REQ-2.2.1 annotation updated to the honest new truth; scratch dropped.

## Boundaries

- No new verbs (the profile table is untouched — this widens *when scripts run*, not *what they may call*).
- Scheduled-class rework out of scope if the census shows it already delivered (WO-007).
- Hygiene flag riding along, small: the one jwt-suite flap WO-052 hit was a **perf-shaped check inside the parallel suite** — move or `#[ignore]`-gate it per the standing own-invocation rule while you're in the tree. One test, not a project.

## Escalation

- Criterion 2's stop condition (old components must keep loading).
- If any event's semantics can't be stated without touching the docstatus lattice EVENT (ADR-009's one resident), stop — the lattice is the floor, not a hook.
