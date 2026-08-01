---
tags: [frust, build-log, milestone-4, kernel, boot, apps]
created: 2026-08-01
work-order: "[[WO-052 Orphan Columns]]"
status: **DONE — the M4 blocker is closed, and the fix is bigger than the refusal it removes.** Tolerating the destructive refusal would have booted the kernel and **silently frozen the DocType**: the migrator abandons a whole resource on a refused diff, so the owner's next field add would have been skipped with nothing but an "orphan" line to show for it. The orphan is therefore **carried** — re-appended to the desired schema verbatim from the migration history — so the diff is genuinely empty and the DocType keeps evolving around it. Reclaim runs through the migrator with `allow_destructive` scoped to one acknowledged column, so the history snapshot converges and the next boot does not report a phantom. Watched RED twice, once on a fixture and once **on the real dev store** (`E_BOOT_DB … refusing destructive change(s) ["REMOVE FIELD crm_followup"]`, exit 1), then green. No `BootOptions` flag added — pinned by a test.
---

# WO-052 — Orphan Columns

## The finding that changed the shape of the fix

The amendment says an undeclared column is an orphan, never a boot refusal. The
literal reading is a two-line change: classify the refusal as drift, don't fail
boot. That version passes criteria 1, 2, 3 and 5, and it ships a silent defect.

`ResourceMigrator::run` abandons the **whole resource** when its diff is
refused:

```rust
if !destructive.is_empty() && !opts.allow_destructive {
    report.errors.push(...);
    continue;          // <- the entire DocType, not just the drop
}
```

So a DocType with one orphan can never take another schema change. The owner's
next release adds a field, the sync skips the resource, boot is green, and the
only trace is an orphan line that was already there. That is the silent-wrong
shape wearing the fix's clothes.

**So the orphan is CARRIED, not tolerated.** `carry_orphans` reads the engine's
own history table, finds every field the last-applied snapshot holds that the
current metadata no longer declares, and appends its `define_sql` back onto the
desired schema verbatim. The diff is then genuinely empty: nothing is planned,
nothing is refused, nothing is skipped, and every *other* change to that DocType
applies normally. This is also, precisely, what WO-051 did to the dev store by
hand (`orphaned_from: 'crm'`) — the mechanism is the hand-fix generalised, which
is what criterion 6 asked for.

`is_orphan_refusal` (the drift/fatal partition) stays as the belt to that
braces: anything the carry does not anticipate still reports as an orphan
instead of killing boot, and every *other* migration error is still fatal,
unchanged.

## Criterion 2 — the missing regression, watched red first

`kernel/tests/orphan_columns.rs`. The step WO-050's suite never took:
install extension → exercise → uninstall → **restart the kernel**.

Red was demonstrated **twice**, both by inverting the change in place (never by
reverting the file — the standing rule):

| where | mutation | result |
|---|---|---|
| fixture | `is_orphan_refusal → false` | `E_BOOT_DB: refusing destructive change(s) ["REMOVE FIELD crm_tag"]` |
| **the real dev store** | carry disabled **and** drift-tolerance disabled | `E_BOOT_DB … ["REMOVE FIELD crm_followup"] — re-run with allow_destructive`, **exit 1, zero `boot_complete` lines** |

The second is the WO-051 blocker reproduced verbatim against real history and
real data, not a fixture — criterion 6 and criterion 2 in one run.

## Criterion 3 — re-adoption, asserted by value

`ORIGINAL-VALUE` is written before the uninstall, read back **while orphaned**,
and read back again after the extension is re-installed across a restart. A
count cannot tell "the data came back" from "an empty column was recreated";
the value can.

## Criterion 4 — reclaim as an explicit act

`POST /doctype/{name}/reclaim {column, acknowledge}`, manager-only.

```
refused: reclaiming orphan column 'inv.crm_tag' DROPS IT AND ITS DATA —
1 row(s) still hold a value. Re-send with "acknowledge": true if that is
what you mean.
```

- A **declared** field is refused separately: *"that is a DECLARED field of
  'inv', not an orphan — remove it through the owning app's update, which is
  where its acknowledgement belongs."* One door per act.
- The drop goes **through the migrator**, not by hand, with `allow_destructive`
  scoped three ways: only the target DocType's resource is migrated, every
  *other* orphan on it is still carried, and only the named column is excluded
  from the carry. A hand-issued `REMOVE FIELD` drops the column and leaves the
  history still claiming it — **the next boot then reports a phantom orphan for
  a column that no longer exists.** That is exactly what the first version did,
  and the test caught it.
- **No `BootOptions` flag.** Pinned by `boot_options_gained_no_destructive_flag`,
  which reads the struct's own source and fails if the word "destructive" ever
  appears in it. The ADR-013 posture, enforced rather than remembered.

**Live through `frust serve`, not only through a test broker** (the standing
tested-seam≠wired check). Against the running dev kernel over real HTTP, with a
manager session:

```
POST /doctype/sales_invoice/reclaim {"column":"crm_followup"}
→ refused: reclaiming orphan column 'sales_invoice.crm_followup' DROPS IT AND
  ITS DATA — 4 row(s) still hold a value. …

POST /doctype/sales_invoice/reclaim {"column":"total","acknowledge":true}
→ 'total' is a DECLARED field of 'sales_invoice', not an orphan — remove it
  through the owning app's update, …
```

Four real rows, named. **The acknowledged arm was deliberately NOT run against
the dev store** — applying it would drop the very orphan criterion 6 exists to
demonstrate; the applied path is proven on the fixture instead. Dev-store
mutation stated: a temporary `app_user:wo052_ops` manager was created to obtain
a session and **deleted afterwards** (users back to `clerk1, clerk2, manager, u`;
the four `crm_followup` values still present).

## Criterion 5 — visible without reading a log

Boot report (`orphan_columns`), and `/metrics`:

```
frust_orphan_column{column="sales_invoice.crm_followup",tenant="skeleton"} 1
frust_orphan_columns{tenant="skeleton"} 1
```

A gauge map never forgets a key, so a **reclaimed** column has to be zeroed by
name or `/metrics` sends operators hunting a column that no longer exists. The
reclaim path re-publishes the full list and zeroes the reclaimed series; the
test asserts the gauge reads `Some(0.0)`, not merely that reclaim returned OK.

## Criterion 6 — the dev store's hand remediation, migrated

The hand-declared `crm_followup` (`orphaned_from: 'crm'`) was **removed** from
`doctype:sales_invoice`, leaving the store in exactly the WO-051 state: an
undeclared column holding data (4 rows, values `'call Contoso'` ×2 and `''` ×2).

- mutated binary: **refuses, exit 1**, the blocker verbatim.
- shipped binary: `{"evt":"boot_complete","doctypes":10,"meta_version":7,`
  `"orphan_columns":["sales_invoice.crm_followup"]}`, REST listening, `/health`
  200, and all four values still readable.

The store now carries a **proper named orphan**. One repaired store became a
mechanism.

## Criterion 1 + 7 — suites and gates

**Fresh store** (a scratch data dir at `D:\Dev\rust\wo052-scratch`; the live dev
directory was never swapped, per standing policy — dropped afterwards), release,
quiet machine, ≥3 samples each:

| gate | jwt | basic | budget |
|---|---|---|---|
| hook chain warm median | 0 ms | 0 ms | 30 ms |
| submit warm median | **4 · 4 · 5 ms** | **5 · 5 · 5 ms** | 25 ms |
| realtime tax | 0.00 ms | 0.56 ms | 2 ms |

(The first jwt run read 6 ms and then converged at 4–5 — cold-store warm-up, the
WO-040 B2 lesson; the converged band is what is reported, and it matches
WO-051's 4 ms.)

Meta discipline untouched: `meta_remains_fail_closed` (a newer-than-binary meta
version still refuses boot) plus `boot_discipline` and the keyguard suites green
— `a_healthy_store_passes_the_keyguard_and_still_rejects_the_forged_key`,
`a_restored_store_is_still_refused_loudly`, `re_issuing_the_key_clears_the_refusal`,
`the_keyguard_probes_the_right_place_under_a_namespace_topology`.

**Full suite, both auth modes:**

| mode | binaries | passed | failed |
|---|---|---|---|
| `basic` | 56 | **347** | **0** |
| `jwt` | 56 | 346 | 1 → **re-run green** |

The one jwt failure was `the_save_floor_does_not_move_when_smtp_is_dead` — a
perf-shaped check (15.05 ms overhead against a 14.3 ms yardstick) running
*inside the parallel suite*, which the house rule says is the wrong place for
it. Re-run alone on a quiet machine: **6/6 green**; and the same check passed
inside the parallel `basic` run. Stated rather than buried: the number that
failed was measured under load, and WO-052 adds no query to the write path —
`history_fields` is called only from `MetadataSync::sync`, which a save never
enters.

Scratch dropped: the five `wo052_*` test databases removed, the scratch data
dir `D:\Dev\rust\wo052-scratch` deleted, the live dev store never swapped.

## What did NOT change

- **The destructive guard's apply semantics.** An app update that drops its own
  declared field is still refused and still needs acknowledgment
  (`an_owner_update_that_breaks_an_extension_refuses_naming_the_casualty` green,
  unchanged) — the app path builds its specs from the manifest and carries
  nothing. This WO changed what boot does with a plan it was never going to
  apply.
- `BootOptions` is still `{holder, accept_meta_migrations}`.

## Findings

1. **The migrator diffs against HISTORY, not the live schema.** So a hand-issued
   `REMOVE FIELD` cannot end an orphan's life — only a recorded migration can.
   Any future "just drop it" shortcut has the same phantom-orphan bug.
2. **A refused diff costs the whole resource.** Worth remembering anywhere else
   the engine's refusals get softened: the refusal is not the only consequence.
3. Small: the history table does not exist before the engine's first run, and
   SurrealDB makes reading a missing table an *error*, not an empty set —
   matched on that one condition so a genuinely broken read still fails loudly
   (the WO-043 lesson, third instance).

## Related
[[WO-052 Orphan Columns]] · [[2026-07-31 WO-051 milestone 4 close-out]] ·
[[2026-07-31 WO-050 extension mechanism]] · [[ADR-008 Data Shape]] (the
orphan-columns amendment this builds) · [[ADR-013 Signing Key Integrity]] (the
no-footgun-flag posture the reclaim path honours)
