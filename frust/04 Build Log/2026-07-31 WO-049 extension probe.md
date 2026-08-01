---
tags: [frust, build-log, apps, extension, probe, position-paper, milestone-4]
created: 2026-07-31
work-order: "[[WO-049 Extension Probe]]"
status: COMPLETE — ADR-015 is ratifiable. **The vocabulary question has a sourced answer: validate-only is UNBUILT DELIVERY, not a ratified boundary** — ADR-007's profile table's axis is *verbs*, it has no lifecycle row, and ADR-006's own evolution policy is additive-only while its cycle rule already keys on a plural `(record-id, hook-class)`; `HookClass{Validate,OnWrite,Scheduled}` exists in code with only `Validate` wired. So widening the vocabulary is ordinary build work against REQ-2.2.1, NOT an ADR-007 amendment. **The composition probe returned the sharpest possible result: with `owns` bypassed, app B SILENTLY REPLACED app A's hook — `owner_ran = null, ext_ran = "B"` — the owner's invariant stopped running and nothing said a word.** That is P-2.2 exactly, and `owns` is the only thing standing between the product and it. Leans (a) and (b) CONFIRMED with a cost the probe priced; lean (c) CONFIRMED — the update-gate seam exists end-to-end with **no new storage**. One unpredicted finding: an extension's fields must be declared in the doctype envelope or its writes are silently stripped (found by my own instrument error).
---

# WO-049 — Extension probe: the evidence ADR-015 ratifies on

No production bypass was written. `owns` remained a live boundary in every
artifact, including the test build — the probe **hand-carries the effect** a
relaxed `owns` would have, which answers the runtime questions without ever
weakening the refusal.

## Criterion 1 — the vocabulary question, sourced

**Answer: `validate`-only is unbuilt delivery, not a ratified profile boundary.
Widening it is ordinary build work against REQ-2.2.1, not an ADR-007
amendment.** Five sources, read rather than recalled:

1. **ADR-007's profile table's axis is VERBS, not hook points.** Its rows are
   `db-read`, `db-write`, `db-aggregate`, `enqueue`, `log`, the six-verb bridge,
   mutate-own-doc. Its columns are execution classes. **There is no lifecycle
   row at all**, so the ratified sentence — *"widening any cell is an ADR
   amendment"* — binds *what a script may call*, and `hook != "validate"` is not
   a cell of it.
2. **ADR-006 explicitly places hook signatures inside the typed layer**
   ("the typed layer is the verbs and hook signatures … not the fields"), and
   its **edge 1 evolution policy is additive-only + two-major host support**.
   A new hook export is additive under a policy this project already ratified.
3. **ADR-006's cycle rule already keys on `(record-id, hook-class)`** — a
   plural concept in the ratified text.
4. **`HookClass { Validate, OnWrite, Scheduled }` exists in `contract.rs`
   today.** Only `Validate` is ever pushed on the real path
   (`broker.rs:722`); the other two appear solely in the cycle-trap tests. The
   vocabulary was designed for and is simply unwired.
5. **REQ-2.2.1 requires the broader set** (`before_insert, validate, on_submit,
   on_cancel, …`), and ADR-001 Tier-1 warns that *"retrofitting hook points into
   production metadata is the expensive version."*

The refusal's own words agree: *"only 'validate' exists today."*

**What IS a real constraint:** the WIT world exports exactly one lifecycle func
(`validate: func(doc) -> result<doc, string>`), and `HookDispatch` has exactly
one method. Adding a hook is therefore a WIT + trait change across the
implementors — real build work, additive, no amendment.

## Criterion 2 — the composition probe

Predictions were stated before running. All five held; one behaviour nobody
predicted appeared.

| # | prediction | outcome |
|---|---|---|
| P1 | composition not representable — `server_script` is a single scalar | **CONFIRMED** — storage reports `single scalar string` |
| P2 | pool keyed `(tenant, doctype)`, serves one script | **CONFIRMED** — key is a 2-tuple; WO-048's cache is the same width |
| P3 | no per-app trace attribution | **CONFIRMED** — `hook_dispatch` carries `runtime`+`doctype`, no `app` |
| P4 | no clean detach — no per-app row to remove | **CONFIRMED** — 1 row an uninstall could target, still `app = app_a` |
| P5 | the doctype record already carries an owner | **CONFIRMED** — `app: 'app_a'` is already there |

### The result that matters

```
BASELINE:  owner_ran = "A"
AFTER B:   owner_ran = null,  ext_ran = "B"
OBSERVED:  B REPLACED A silently.
```

**App B's hook did not compose with app A's — it replaced it, and the owner's
invariant stopped running with no error, no log line, and no trace of what
happened.** That is P-2.2 in its original form: *hooks are global mutable
magic*. The v2.0 gate scores P-2.2 as bounded-by-architecture precisely because
`owns` refuses this at install; the probe shows what sits directly behind that
refusal.

**This is the cost ADR-015 must price, and it is larger than "relax a
predicate".** A naive relaxation of `owns` does not enable composition — it
enables silent override. The mechanism must land *before* the boundary moves:

- **metadata shape** — `server_script` becomes a list of `{app, script}`; a
  scalar cannot hold two contributors (this is a `DocTypeDef` change, and the
  ADR-008 migration discipline applies)
- **pool + cache keys** — `(tenant, doctype)` → `(tenant, doctype, app)` in both
  `hooks.rs::ScriptSource::pool` and WO-048's `script_cache`
- **dispatch order** — a loop with the owner first and un-overridable, which
  today's single-slot dispatcher has no place to express
- **attribution** — an `app` field on the `hook_dispatch` span, or a composed
  failure cannot be blamed
- **detach** — a per-app extension row, so uninstalling B has something of its
  own to delete while A's doctype survives

### The unpredicted finding

The probe's first run reported *"NEITHER hook ran"* — worse than either
prediction. It was my instrument: B's script wrote `doc.ext_ran`, an
**undeclared** field, and WO-009's envelope filter strips fields the doctype does
not declare. B *had* run; its output was silently dropped.

**So an extension's namespaced fields are load-bearing, not decoration** — an
extension whose manifest fails to declare a field it writes gets no error and no
data. That is a second silent-wrong lying in wait for the build WO, found only
because the probe reported an impossible-looking result instead of the expected
one.

## Criterion 3 — the owner-evolution seam: it exists, end to end, with no new storage

Lean (c) is **confirmed and cheaper than the ADR assumed.**

| piece | where | already used for |
|---|---|---|
| whole-tenant view at plan time | `app.rs::Manifest::plan_unchecked` → `load_doctypes(db)` | **already cross-app** — rollup targets "come from the whole world, not just this bundle" |
| each doctype's owner | `sync.rs::DocTypeDef::app` | install attribution |
| every app's declared surface, verbatim | `installed_app.manifest` (meta.rs `app_registry_ddl`) | route dispatch re-parses it live at `rest.rs:937` via `Manifest::parse` |
| the casualty list | `InstallPlan::destructive()` → `schema.planned[].destructive` | the WO-019 refusal that already names `REMOVE FIELD memo` |

So when app A updates, the gate can already: load every `installed_app` row,
`Manifest::parse` each, read other apps' `server_scripts[].doctype` and declared
fields, and intersect that with its own destructive list. **Nothing new needs
storing** — the manifest is on record and is already re-parsed in production for
another purpose.

**The gap is only that nothing calls it.** `destructive()` flattens the
migration engine's list without consulting other apps. That is the build WO's
work, and it is small.

## What ADR-015 should now say

- **(a) no owner opt-in — CONFIRMED as workable, with the safety it depends on
  now specified.** The lean's safety clause ("owner's hooks always run and
  cannot be overridden") is not a property the system has — it is a property the
  build must *create*, because today the owner's hook is exactly what gets
  replaced. State it as a build obligation, not an assumption.
- **Vocabulary — settled: ordinary build work.** An extension may claim the
  REQ-2.2.1 lifecycle vocabulary as it is delivered; no ADR-007 amendment.
- **(b) refuse-ambiguity — CONFIRMED, and the probe sharpens "ambiguity".** The
  pool's one-script assumption is real (P2), so *v1 composition is
  owner-first-then-extensions in a defined order with refusal on conflict*. On
  veto semantics: an extension rejecting a write the owner accepted is
  expressible today (a hook returns `Err` and the write fails), so a veto is not
  new machinery — it is the default unless suppressed. Worth ruling explicitly
  rather than inheriting.
- **(c) update gate extended — CONFIRMED, seam named, no new storage.**

## Boundary compliance

Probe only. `owns` untouched in every artifact; no manifest-schema change; no
hook delivery built. `kernel/tests/extension_probe.rs` is committed as the
evidence (it asserts whichever reality holds and will fail loudly if a future
build changes any of it). Scratch database `wo049_compose` is self-dropping via
the fixture's `REMOVE DATABASE IF EXISTS`.

## Related
[[WO-049 Extension Probe]] · [[ADR-015 Cross-App Extension Model]] ·
[[ADR-007 Tier-2 Script Architecture]] · [[ADR-006 Plugin Capability Surface]] ·
[[ADR-001 UI Extension Tiers]] · [[v2.0 Deployability Gate]] (P-2.2's score) ·
[[2026-07-28 WO-036 multi-app composition and upgrade survival]]
