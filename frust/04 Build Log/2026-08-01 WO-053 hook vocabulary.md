---
tags: [frust, build-log, hooks, plugins, scripts, adr-006, milestone-5]
created: 2026-08-01
work-order: "[[WO-053 Hook Vocabulary]]"
status: DELIVERED — REQ-2.2.1's four lifecycle classes reach server scripts, each contract stated and tested. ADR-006's edge-1 evolution policy spent for the first time and REFINED by measurement: "additive" has exactly one unit, the WORLD. Plugin subscription to the new classes is proven-possible but unbuilt, and said so.
---

# WO-053 — Hook Vocabulary: REQ-2.2.1's full lifecycle set

## Criterion 1 — the census, sourced

The WO-049 discipline: find out what is actually wired before building anything.

| surface | what exists | wired? |
|---|---|---|
| `HookClass` (`contract.rs`) | `Validate`, `OnWrite`, `Scheduled` | `Validate` only. `OnWrite` appears solely in the cycle-rule unit tests; **`Scheduled` appears in exactly one place — its own declaration** |
| WIT world (`frust-kernel/wit/plugin.wit`) | `hooks.validate` + hostile `spin`/`hog`; worlds `plugin`, `route-plugin` | one lifecycle export, ever |
| script engine (`script-engine/src/lib.rs`) | `impl Guest { fn validate }` | one export |
| stored subscription (`doctype.server_script`, meta v7) | `{app, script}` | **no `hook` field** — the manifest's `hook` was validated to be `"validate"` and then *discarded* at storage |
| manifest door (`app.rs`) | `hook != "validate"` refuses | the door existed; the vocabulary behind it did not |
| scheduled work (WO-007) | `RollupWorker` + `Box<dyn Contrib>` | **real and running — but kernel handlers, not guest code** |
| notification events (WO-043, `mail.rs`) | `after_insert`, `on_update`, `on_transition`, `on_submit`, `on_cancel`, validated at the door | **fully wired — for mail** |

Two findings changed the build:

1. **The event vocabulary already existed, one subsystem over.** WO-043's notification layer already computes and names the docstatus edges. So this WO delivers those events to *scripts*; it does not invent them. The wire names are deliberately shared, so `on_submit` means the same thing to a mail rule and to a script.
2. **`Scheduled` is a declared-and-unwired enum variant**, and WO-007's scheduled work is `Contrib` handlers compiled into the kernel. The WO's own boundary anticipated this: nothing to rebuild, and the class stays declared because the cycle trap's key space is the honest place to record that it exists.

Two hygiene items the census turned up, neither load-bearing:
- `wasm-spike/wit/plugin.wit` is a **stale fork** of the canonical WIT (still carrying the WO-001-era `record doc {id, status, total: f64}`), which the canonical file explicitly forbids — *"CANONICAL COPY: `frust-kernel/wit/` ... do not fork it."* Nothing ships from it; the three `wasm-spike/host` spike binaries still bind against it.
- The owner-script writer used `.find()` where it should use `.filter()`, so a second subscription on one DocType would have been **silently dropped**. Invisible while one class existed; fixed here before it could bite.

## Criterion 2 — ADR-006's edge-1 policy, spent for the first time

The policy was written on day two and never tested against the toolchain. Three shapes, **predictions stated before running**, all three confirmed:

| shape | prediction | result |
|---|---|---|
| **P1** add funcs to the existing `hooks` interface | old components fail to instantiate | **CONFIRMED** — `instance export 'frust:plugin/hooks' does not have export 'before-insert'`. And it broke the **shipped** artifacts too, not just the old fixtures |
| **P2** new interface, added to the existing world's exports | still fails — a world's export is a requirement | **CONFIRMED** — `no exported instance named 'frust:plugin/lifecycle'` |
| **P3** new interface in a **new world**, base world untouched | old components load *and run* | **CONFIRMED** — 3/3 green |

**The refinement, and it is worth having: ADR-006's "additive" has exactly one unit — the WORLD.** Growing an interface is a breaking change; adding an export to an existing world is a breaking change. Only a new world beside the old one is additive. This is the same conclusion `route-plugin` reached for routes in WO-019 — but that was *reasoned*, and this is *measured*, with the two failure messages on the record.

The compatibility proof is committed as `hook_world_evolution.rs` and is deliberately not vacuous: it loads the pre-WO fixtures (`artifacts-old-world/`, snapshotted by SHA before the WIT was touched) **and runs them**, because a component that links but dispatches to nothing would satisfy a load-only check. Both runtimes fire in the passing run.

**No STOP was reached** — the evolution policy survived contact with reality, in one shape.

### The design this bought

The probe reshaped the build in a useful direction: **server scripts need no engine rebuild at all.** The script engine is a *text runner* — which script runs for which event is a host-side routing decision, so the vocabulary reaches scripts without the engine changing world. Only WIT plugins need the new world. The standing "script engine = one source, two artifacts" hazard never had to be paid.

## Criterion 3 — each event's contract, stated

| event | fires | may mutate | may reject |
|---|---|---|---|
| `before_insert` | CREATE only, ahead of `validate` | **yes** | yes |
| `validate` | every write | **yes** | yes |
| `on_submit` | docstatus crosses to 1, **before the write commits** | **no** — host discards its output | **yes** |
| `on_cancel` | docstatus crosses to 2, same | **no** | **yes** |

The edges are computed pre-commit from `baseline` vs the document about to be written — the pre-commit twin of the post-commit derivation the notification path already does. Pre-commit is forced by the contract: a rejection has to *prevent* the transition, not annotate it afterwards.

**The escalation clause was not reached.** A rejected 0-to-1 leaves docstatus 0 and the lattice EVENT never runs — because the *write* never happens. Nothing in this WO is near ADR-009's one resident.

## Criteria 4-5 — composition per class, and the door

Composition generalizes rather than duplicates: the class filters which entries run, and the WO-050 loop (owner-first, per-app pool key now including the class, per-app attribution, veto-names-its-app) is otherwise unchanged. WO-049's silent-override scenario is re-run **per class** — `ran == "AB"` where `"A"` alone means the extension never ran and `"B"` alone is P-2.2 reborn.

The door grew with the vocabulary from one source: `HookClass::SUBSCRIBABLE` feeds both the refusal message and the dispatcher, so they cannot disagree about what exists. `on_sumbit` is a 400 that names the typo *and* the real list.

Storage grew to `{app, script, hook}` — **meta v7 to v8**, backfilling `hook: 'validate'` because everything written before this WO subscribed to the only class there was. Stated rather than defaulted at read time: an entry whose hook was lost must not silently become a `validate`.

## Three instrument failures, all mine, all caught

1. **A vacuous pass I wrote myself.** `on_submit_may_not_mutate_the_document` asserted the stamp was absent — which is equally true of a host that discards the mutation and of a hook that never ran. Added the control the standing rule demands: the **same script text** moved to a class that *may* mutate. It stamps there (`stamp: "SMUGGLED"`) and not under `on_submit`, so the absence is the contract being enforced rather than the hook being missing.

2. **An unrepresentative probe population — the WO-052 lesson, immediately repeated.** I verified the v8 migration against a population where *every* row had an array `server_script`, so the type guard was never asked to do anything. Real doctypes have `server_script` as NONE, and the migration failed the **app-upgrade test** (`an_installed_app_survives_a_major_meta_upgrade` — the WO-036 test that closed the v2.0 gate's A2 assumption) with *"no such method found for the none type."*

   **New SurrealDB caveat, measured:** `UPDATE ... SET expr WHERE cond` **evaluates `expr` for every row** and only *applies* it to the matching ones — so a `WHERE type::is_array(x)` does **not** protect a `.map()` in the SET from rows where `x` is NONE. The guard has to be *inside* the expression (`IF type::is_array(x) { ... } ELSE { x }`). Re-probed across all four real shapes (NONE / pre-v7 string / array / array-already-hooked), run twice for idempotence, then re-ran the upgrade test green.

3. **I truncated this build log by describing a corrupt character.** Writing the finding below with `open(path,'w')` truncated the file *before* the encoder rejected the lone surrogate — so the log went to zero bytes. Rewritten whole. The lesson is small and general: `open(...,'w')` destroys before it can fail, and a file you are about to write is not safe just because the string exists.

## Criterion 6 — live through `frust serve`, and in the browser

A real business rule, expressible only now, published through the app door as `acct` **v1.1.0**: *invoices over 500.00 need a second approver.*

**The naming point, stated because it will surprise someone:** in this workflow the clerk's **Submit** action moves `Draft` to `Submitted for Approval` and leaves docstatus at **0**, so `on_submit` does *not* fire there. It fires on the manager's **Approve**, the transition that crosses docstatus into 1. The class is named for the docstatus edge, not for a button label.

| arm | through `frust serve` | in the browser |
|---|---|---|
| 25.00 invoice, Approve | `docstatus 1`, `workflow_state: Approved` | chip reads **Approved**, fields frozen by the lattice, no error |
| 750.00 invoice, Approve | **refused**: `invoices over 500.00 need a second approver (this one is 750) (rejected by the owner, app 'acct')` | error banner carries the script's own message **and its app** |

State asserted from the DATABASE, not the response: the refused invoice is still `Submitted for Approval` at `docstatus 0`. Traces carry the class: `hook: on_submit, ok: true` on the allowed arm, `hook: on_submit, error_code: E_HOOK_REJECTED, ok: false` on the refused one. Screenshot: `frust-e2e/wo053-on-submit-refusal.png`.

The dev store also proved the migration on real data: booted to **meta v8** with the existing `acct` script backfilled to `hook: 'validate'` and still firing (its `E_INVOICE_UNBALANCED` refusal fired during setup, attributed to the owner), and WO-052's `sales_invoice.crm_followup` orphan still named.

### Finding: a stored manifest its own door will not accept

Republishing `acct` failed with `bad json body: lone leading surrogate in hex escape`. The stored manifest carries a **lone surrogate (U+DC9D)** — the remains of an em-dash that lost a byte — inside a JS *comment*. Harmless to execution, fatal to round-tripping: **the registry holds a manifest that cannot be re-submitted through `/app/update`.** Repaired in place for the dev store (the comment now reads `--`); the general defect — install accepted bytes the update door rejects — is **not fixed here** and is reported as its own item.

## Criterion 7 — suites, gates, hygiene

- **Fresh-store gates, both auth modes**, on a dedicated scratch data dir (the live store was never swapped), 3 samples each, converged:

  | | hook chain | submit | realtime tax |
  |---|---|---|---|
  | jwt | 0 ms (gate 30) | **2 / 2 / 2 ms** (gate 25) | 0.15 ms (allowance 2) |
  | basic | 0 ms | **2 / 2 / 2 ms** | 0.52 ms |

  Faster than WO-052's 4-5 ms, and **no improvement is claimed**: a brand-new scratch store on a quiet machine is the most favourable case there is, and machine-sensitivity is the standing caveat. What the gates say is that the write path grew two pre-commit hook classes and did not move the floor.

- **jwt suite: 58 binaries, 354 passed, 2 failed.** Both failures were tests pinning things this change legitimately invalidated; both fixed and re-run green in isolation:
  - `validation_collects_every_problem_at_once` asserted the refusal says *"only 'validate' exists today"* — a sentence that stopped being true. Now asserts the stronger shape: it names the typo **and** the real vocabulary.
  - `the_script_pool_is_keyed_per_app` (a WO-050 source guard) matched the pool key as an exact 3-tuple. WO-053 **widened** it to include the class, and an exact-tuple match cannot tell a widening from a narrowing — only the narrowing is the bug. Re-written to match the prefix, so it still fails if the key narrows back to `(tenant, doctype)`.
- **basic suite: 58 binaries, 356 passed, 0 failed** — run with every fix in, which is
  also the confirming evidence for the two jwt failures above (the counts differ by
  exactly those two, which pass here).
- **Hygiene flag (done):** the perf-shaped mail check that flapped in WO-052's jwt suite is `#[ignore]`-gated to its own invocation, with the measured evidence in its doc comment, and shown **still passing standalone** — gated, not disabled.
- SRS REQ-2.2.1 annotated with the honest new truth, including the remainder.
- Scratch store dropped.

## What is NOT delivered, stated

- **Plugins do not subscribe to the new classes.** The `lifecycle-plugin` world exists and old components are proven to keep loading beside it, but no component exports `lifecycle` yet. The mechanism is proven; the subscription is unbuilt.
- **`HookClass::Scheduled` stays unwired**, per the census and the WO's boundary.
- **No new verbs.** The ADR-007 profile table is untouched — this widened *when* scripts run, never *what they may call*.
- The stale `wasm-spike/wit/plugin.wit` fork and the three spike host binaries binding against it are left alone (out of scope, named above).
- The manifest round-trip defect above.

## Dev-store state, stated

meta **v8**; `acct` at **v1.1.0** carrying the `on_submit` rule; the mojibake comment repaired; four new `sales_invoice` rows from the proof (two clerk-owned at 25.00 and 750.00, approved and refused respectively, plus two manager-owned created before the row-ownership lesson); `sales_invoice.crm_followup` still a named orphan.

## Related
[[WO-053 Hook Vocabulary]] · [[ADR-006 Plugin Capability Surface]] (edge-1, now measured) ·
[[ADR-015 Cross-App Extension Model]] · [[ADR-009 Docstatus Lattice]] · [[SRS]] REQ-2.2.1 ·
[[2026-07-31 WO-050 extension mechanism]] · [[2026-08-01 WO-052 orphan columns]]
