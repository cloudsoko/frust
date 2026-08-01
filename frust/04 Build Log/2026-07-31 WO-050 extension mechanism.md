---
tags: [frust, build-log, apps, extension, milestone-4]
created: 2026-07-31
work-order: "[[WO-050 Extension Mechanism]]"
status: COMPLETE — all 10 criteria, built in the ADR's binding order. **Two apps compose on one DocType, owner-first and un-overridable, dogfooded live in the browser against the accounting seed.** WO-049's silent-override scenario is inverted into a permanent failing control (`owner_ran="A"` AND `ext_ran="B"`; goes red if the owner is ever displaced — demonstrated). **Criterion 9 went better than the WO expected: `owns` never relaxed.** Extension arrives through its own `extends` declaration, so the A1 refusal stays live word-for-word for the owned-script path — the boundary was not moved, a second validated door was added beside it. **The build caught two silent-loss defects of its own**: `attach_extensions` never synced schema (fields existed in metadata and nowhere in the database), and — worse — the owner's `UPSERT … CONTENT` would have **silently wiped every extension's fields and chain links on an owner update**, P-2.2 one path over. Both fixed and pinned. **P-2.2 re-scored bounded → KILLED**, trade line updated from "not possible". Suite 55 binaries / 341 passed / 0 failed.
---

# WO-050 — The cross-app extension mechanism

Built in the ADR's binding sequence. `owns` was to move last; it turned out not
to need to move at all.

## What shipped, criterion by criterion

**1 · Storage.** `doctype.server_script` scalar → `[{app, script}]`, meta **v6 →
v7**. The migration rides `apply_meta`'s own transaction, so a bumped version can
never outrun the data it promises. **Recompute, not launder** (ADR-008): the
owner's entry is derived from the record itself — its existing script text paired
with its own `app` — never invented. Idempotent by `type::is_string`, verified by
running it twice. The read path still tolerates a scalar, which is what makes the
migration a non-event.

**2 · Dispatch: owner-first, un-overridable.** Every contributing app runs in
order, each seeing the previous one's output. The owner is first and cannot be
skipped, reordered or replaced.

> **The permanent failing control.** WO-049's probe measured the alternative:
> with one slot, `AFTER B: owner_ran = null, ext_ran = "B"` — silent replacement.
> That exact scenario is now the shipped test, asserting `owner_ran = "A"` **and**
> `ext_ran = "B"`. Deliberately breaking owner-first turns it red with
> `THE OWNER'S HOOK WAS DISPLACED — this is P-2.2`. Demonstrated, then restored.

**3 · Keys widened.** Pool `(tenant, doctype)` → `(tenant, doctype, app)`, so the
owner's pooled instance and each extension's coexist instead of evicting each
other. WO-048's cache keeps its doctype key but now carries the whole `HookPlan`,
so one generation check still covers every app's script. WO-048's correctness
suite re-run against the new shape: **8/8**, including live-mutability and the
out-of-band caveat.

**4 · Attribution.** `hook_dispatch` carries `app` and `owner`. P-2.2's actual
complaint — *which app changed this behaviour* — is a log field.

**5 · Veto, as ruled.** An extension's `Err` fails the write and the error names
the rejecting app and its role: *"crm says no (rejected by the extension, app
'crm')"*. No suppression mechanism. The owner rejects first because it runs
first.

**6 · Envelope loudness.** An app writing a field it never declared is now
`FRUST:E_FIELD_UNDECLARED`, naming the field and the app — checked per app, right
after its own hook returns, because that is the only moment the culprit is still
known. This was WO-049's unpredicted finding: silent stripping made a running
hook look like a dead one.

**7 · Manifest + registry.** `extends: [{doctype, fields, hook, script}]` as
manifest content, validated at the door in one pass (WO-019 discipline).
**Namespacing enforced**, not advised: an app may only add fields under its own
name, which is what turns a routine collision into a rare, real error.
Refuse-ambiguity: a collision names both apps. Fields are stamped `ext_app`, so
uninstall removes exactly one app's additions.

**8 · Owner evolution.** The seam WO-049 named, wired with **no new storage**:
the plan reads every other app's manifest from the registry (already stored
verbatim, already re-parsed live for route dispatch) and cross-references their
declared surfaces against this update's destructive list. A breaking update
refuses with `FRUST:E_BREAKS_EXTENSION` naming the app and field; acknowledge
proceeds, per WO-019's shape. Never silent.

**9 · `owns` never relaxed — and that is the better outcome.** The WO sequenced
the relaxation last. Building the mechanism made it unnecessary: extension
arrives through `extends`, its own declaration with its own validation, so
`server_scripts` targeting a foreign DocType is **still refused with the A1
message, word for word** (asserted). The boundary did not move; a second,
narrower, validated door was added beside it. A relaxed predicate would have
been strictly weaker.

**10 · The dogfood, live.** A second real app (`crm`) extended the accounting
seed's `sales_invoice` — a DocType `acct` owns — through the shipped install
path, exercised in a real browser:

```
record: {"customer":"Contoso","crm_followup":"call Contoso","total":"0"}
  the EXTENSION's hook ran   -> crm_followup = "call Contoso"
  the OWNER's hook still ran -> total = "0"
uninstall: sales_invoice survives with the owner's own fields (4: customer, lines, total, workflow_state)
           the extension's field is detached
           every row the extension touched SURVIVES
```

`pnpm extension`, 10/10.

## Two silent-loss defects the build found in itself

1. **Extension fields never reached the schema.** `attach_extensions` wrote them
   into the DocType record, but the install path syncs schema *before* metadata
   is attached — so the field existed in metadata and nowhere in the database,
   and the first write naming it failed with SurrealDB's *"no such field exists
   for table"*, which reads like a bug in the extension rather than a missing
   step in the installer. Fixed by syncing after attach.
2. **An owner update would have silently wiped every extension.** `attach_metadata`
   does a whole-record `UPSERT … CONTENT` built from the *owner's* manifest,
   which knows nothing about extensions. Publishing owner v2 would have deleted
   every extension's fields and chain links with no error — **P-2.2 exactly, one
   path over from the one this WO set out to close.** Fixed by preserving
   `ext_app`-tagged fields and foreign chain entries across the upsert, and
   pinned by a test asserting an ordinary owner update leaves the extension
   standing.

Both were found by tests failing, not by review.

## P-2.2 re-scored: bounded → **KILLED**

The trade line moves from *"cross-app extension of one DocType is not possible"*
to possible, declared, ordered and attributed. The honest residue, stated in the
row: composition is on `validate` only (the wider REQ-2.2.1 vocabulary is
ordinary build work, deliberately deferred so composition landed on one hook
first), and extension-to-extension ordering is out of v1.

## Regression

| | |
|---|---|
| full kernel suite (parallel) | **55 binaries / 341 passed / 0 failed** (was 331 — 10 new extension tests) |
| browser | workflow 18/18 · print 24/24 · **extension 10/10** |
| dev store | migrated to meta **v7**; `crm` installed and uninstalled during the dogfood, leaving `sales_invoice` with its own 4 fields |

## Boundaries held

Vocabulary widening (OnWrite/Scheduled) stayed **out**, as ADR-015 ruled —
composition landed on `validate` first rather than ballooning. Monopoly guards
untouched. No number claimed: the dispatch loop runs one hook per contributing
app, and with a single contributor it is the same single call as before.

## Related
[[WO-050 Extension Mechanism]] · [[ADR-015 Cross-App Extension Model]] ·
[[2026-07-31 WO-049 extension probe]] (the evidence, and the control this
inverts) · [[v1.0 Pain-Point Scorecard]] (P-2.2 re-scored) ·
[[v2.0 Deployability Gate]] · [[ADR-008 Data Shape]] (the migration discipline) ·
[[2026-07-31 WO-048 server script cache]]
