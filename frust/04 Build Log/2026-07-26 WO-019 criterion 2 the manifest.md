---
tags: [frust, build-log, apps, plugins, work-order]
created: 2026-07-26
work-order: "[[WO-019 App Lifecycle]]"
status: criteria 1-2 done — WO active
---

# Build Log — WO-019 criterion 2: The Manifest

One file format for an App, validated before anything applies, with REQ-6.6's
gate discipline **reused literally rather than imitated**.

## The format

```
manifest_version, name, version (MAJOR.MINOR.PATCH), label
doctypes[]        — full DocTypeDef, so aggregate declarations ride along free
client_scripts[]  — { doctype, hook, script }
server_scripts[]  — same shape (criterion 6 delivers them)
routes[]          — { path, component } → served at /app/{app}/{path}
components[]      — bare .wasm filenames
workflows[]       — RESERVED for WO-018
```

Route paths are namespaced by app name, so two apps cannot collide however
carelessly they are written. `DocTypeDef` already carried an `app` field from
WO-005 — ownership had a slot before it had a use.

**The reserved slot is reserved, not designed.** The test asserts a workflow
object survives a parse/serialize round trip *verbatim* and nothing more. A
format that could not carry workflows would force WO-018 to change the format;
one that pretended to define them would force WO-018 to fight a guess.

## The gate discipline extends — literally

`Manifest::plan()` does not implement migration. It builds the **same
`ResourceSpec`s** a plain metadata sync builds and hands them to the **same
`ResourceMigrator`** with the **same `MigrationOptions`**. Dry-run-before-
install is not a new feature; it is REQ-6.6.1 machinery surfaced as UX.

Proven:

| Assertion | Result |
|---|---|
| dry run yields a plan naming the table | `planned=1`, contains `acct_invoice` |
| dry run applies nothing | `applied=0`, `INFO FOR DB` has no such table |
| the *same call shape* applies when told to | `applied` non-empty, table exists |
| the preview is the whole truth, not just DDL | routes `["/app/acct/ledger"]`, client scripts, workflow count all previewed |

Plan and apply differ only by `MigrationOptions` — one call shape, which is
what stops "what you were shown" from drifting away from "what ran". There is
no second migration path for bundles, for the same reason there is no second
permission compiler.

## Validation reports everything at once

Modelled on `MigrationReport::errors`, which collects per-resource failures
rather than halting: peeling one error per attempt is its own kind of
hostility. A deliberately broken bundle produced **nine** errors in one pass:

```
manifest_version 99 is not supported (this kernel speaks 1)
app name '9bad name' is not an identifier
version '1.0' is not MAJOR.MINOR.PATCH
ok_one.bad field is not an identifier
doctype 'ok_one' is declared twice
client_script targets 'not_in_bundle', which this bundle does not declare
server_script on 'ok_one' uses hook 'before_save'; only 'validate' exists today
route 'fine' names component 'missing.wasm', which the bundle does not ship
component '../../etc/passwd.wasm' must be a bare .wasm filename
```

Two of those deserve naming. **A script pointing at a doctype the bundle does
not ship** is the classic half-installed app, caught at validation rather than
at first use. **A hook point that does not exist** is refused rather than
silently ignored — the engine exports only `validate` today, and accepting
`before_save` into metadata would create a script that never runs and never
says why.

And an invalid bundle **never reaches the migrator**: the refusal names the
problems, and `INFO FOR DB` confirms nothing was created.

## Notes

- Version comparison is numeric, not lexical — `10.0.0 > 9.0.0` holds, and a
  malformed version returns `None` rather than a silent `false`. Needed by
  criterion 3's update detection, cheap to get right now, expensive to discover
  later.
- `app.rs`, like `routes.rs`, is **absent from `surql_monopoly`'s allowlist**.
  A manifest is data; turning it into schema is the sync engine's job.
- Metadata types gained `Serialize` (they were `Deserialize`-only) so a
  manifest can be written back out, not merely read.

## Suite state

**29 binaries green, zero failures** (the 29th is `app_manifest`), stack
stopped. 50 scratch databases dropped at close.

## Next

Criterion 3: install / enable / disable / update through the Desk, with this
plan shown as the preview. That is where the app registry needs persistence,
and where a ruling is due on which module owns app-registry writes — `app.rs`
is query-free today and I would rather keep it that way than quietly add it to
the allowlist.

## Related
[[WO-019 App Lifecycle]] · [[2026-07-26 WO-019 criterion 1 the door probe]] · [[ADR-006 Plugin Capability Surface]] · [[SRS]] (REQ-2.1.1, REQ-6.6.1, REQ-6.6.2) · [[WO-018 Workflow Engine]]
