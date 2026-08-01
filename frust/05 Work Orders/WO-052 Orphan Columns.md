---
tags: [frust, work-order, kernel, boot, apps, milestone-4]
status: DONE (2026-08-01) — all 7 criteria; evidence in [[2026-08-01 WO-052 orphan columns]]. The fix is larger than the refusal it removes: tolerating the destructive refusal would have booted the kernel and **silently frozen the DocType** (the migrator abandons a whole resource on a refused diff), so the orphan is **carried** back onto the desired schema from migration history rather than merely tolerated. Watched RED twice — fixture and **the real dev store** — then green. No `BootOptions` flag, pinned by a test. M4's verdict re-runs in [[2026-08-01 M4 close-out re-run]].
created: 2026-07-31
---

# WO-052: Orphan Columns

## Why

WO-051's gate refused to close M4: an extension uninstall (WO-050, correct per its own contract) leaves an undeclared column, and the next boot's sync classifies it destructive and **refuses to start with no operator remedy**. The ADR-008 amendment rules the reconciliation; this builds it.

## Exit criteria

1. **Orphan tolerance at boot:** an undeclared-but-present user-doctype column never blocks boot. Sync applies nothing to it; boot proceeds; the orphan is **named** in the boot report and exported in `/metrics` (count + identities). Meta fail-closed behavior untouched — `boot_discipline` and the keyguard suites stay green.
2. **THE MISSING REGRESSION, as a first-class test:** install extension → exercise → uninstall → **restart the kernel** → boots green with the orphan named. This is the test whose absence let the blocker ship; it exists before the fix is trusted (watch it fail against the pre-fix behavior — red on `E_BOOT_DB` — then go green).
3. **Re-adoption proven:** re-install the extension after a restart → the orphan column is re-declared and **its data is back** (assert a pre-uninstall value survives the full uninstall→restart→reinstall cycle — the enable-restores semantics, extended, and a count-only check can't prove it; assert the value).
4. **Reclaim as an explicit act:** dropping an orphan goes through the online update path with the REQ-6.6.2 acknowledgment shape — refusal names the column and its data, ack applies. No boot flag added; `BootOptions` stays `{holder, accept_meta_migrations}`.
5. **Never silent:** the orphan appears in the boot report, `/metrics`, and (cheaply, if the seam is already there) the app-registry or doctype meta so an operator can list orphans without reading logs. An orphan nobody can see is the P-2.2 failure shape in ops clothing.
6. **The dev store's hand remediation migrated to the mechanism:** WO-051 re-declared `crm_followup` with `orphaned_from:'crm'` by hand to revive the store — one store repaired, not a fix. Convert it: remove the hand-declared field, restart, confirm the store boots with `crm_followup` as a *proper* named orphan (this is also criterion 2 run against real history rather than a fixture).
7. Full suite both auth modes; fresh-store gates (boot path touched); scratch dropped.

## Boundaries

- The destructive guard's *apply* semantics don't weaken — this WO changes what boot does with a plan it was never going to apply, not what acknowledgment destructive DDL requires.
- No new `BootOptions` flag (the amendment's ruling: drift = orphan, so the boot-time ack path isn't needed — adding one anyway would be the ADR-013 footgun shape).

## Exit

M4's close-out verdict re-runs (short form — the blocker's row flips or it doesn't; the rest of WO-051's table stands).
