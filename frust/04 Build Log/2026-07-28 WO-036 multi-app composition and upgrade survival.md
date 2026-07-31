---
tags: [frust, build-log, apps, upgrade, hooks, v2.0-gate, work-order]
created: 2026-07-28
status: complete — A1 and A2 both closed; the gate's assumption bucket is empty
work-order: "[[WO-036 Multi-App Composition and Upgrade Survival]]"
---

# Build Log — WO-036: Multi-App Composition + Upgrade Survival (A1, A2)

The last two gate blockers. Both were expected to be confirmations; the gate
refuses "expected," and A1 turned out to have an answer nobody had written down.

## A1 — multi-app hook composition (P-2.2)

**The question the gate forced:** two apps, each declaring a server script on
the *same* DocType, one write. Does composition work?

**The structural prediction was wrong — in the safe direction.**
`sync::load_server_script` reads a single `doctype.server_script` field
(`LIMIT 1`), so I predicted the second install would silently overwrite the
first: last-writer-wins, the classic quiet disaster. What actually happens:

```
beta install -> Err(InvalidValue {
  detail: "bundle is not installable:
           - server_script targets 'shared_doc', which this bundle does not declare" })
```

**The manifest validator refuses it at install time, loudly.** Alpha's hook
keeps running (`{"alpha":"alpha-ran", ...}`). There is no silent overwrite
because the second app never gets that far.

So the honest answer to P-2.2 is not "lightly exercised" — it is a **design
boundary**: *one DocType = one owning app = one server script*, enforced at the
door. Cross-app extension of another app's DocType is **not possible**, and that
is a stated trade rather than an untested hope. P-2.2 becomes
**bounded-by-architecture**.

**The composition that IS possible, also proven.** The realistic multi-app case
— two apps installed *together*, each hooking its own DocType — was the other
half of P-2.2's sentence and equally untested. Now tested: each DocType runs its
own app's hook, and **neither runs the other's**. The pooled `ScriptSource` is
keyed `(tenant, doctype)`; that keying had never been exercised with two apps
live at once, and a hook leaking across apps would have been a serious bug.

*Method note:* the A1 test is written to assert **whichever reality holds** —
three explicit branches (refused-loudly / both-ran / silent-overwrite-panic) —
rather than encoding my prediction. A test that only asserted my expectation
would have failed and looked like a defect, instead of reporting the design.
And because a green test with three passing paths does not say *which* path ran,
the result was read off `--nocapture`, not inferred from the pass.

## A2 — major-upgrade survival (P-7.3, the founding pain)

`accept_meta_migrations_two_step` proved the meta gate with `NoUserSync` and
**no app installed**. This installs a real app — DocType, hook, rollup
declaration, and live data — then drives the ADR-008 two-step upgrade:

1. meta version forced to 0 (a database written by an older binary)
2. un-acked boot → **refused**, `MetaMigrationPending { db_version: 0 }`
3. acked boot (`--accept-meta-migrations`) → **applied**, version back to 4

**Post-upgrade the app is still functional, not merely present:**

| checked | result |
|---|---|
| DocType metadata | survives |
| app data rows | survive (1/1) |
| `installed_app` registry | still registered, still `enabled` |
| **the app's hook fires on a new write** | **yes — `stamp = "hook-ran"`** |

The last row is the load-bearing one. A test that counted rows would have passed
over an app whose hooks had stopped firing — the WO-027 lesson (*a verification
that only checks what it expects cannot catch what was silently destroyed*)
applied to the founding upgrade pain.

**Residual, stated honestly:** this proves survival across *a* major meta
upgrade. It cannot prove survival across *years* of them — longitudinal evidence
cannot be manufactured. P-7.3 moves from assumption to
**bounded-by-measurement**, with that residual named rather than papered.

## Result for the gate

Both assumptions closed by measurement. With WO-035's A3, the gate's
**bounded-by-assumption bucket is empty**: final classification
**4 measurement / 8 architecture / 0 assumption**, and
[[v2.0 Deployability Gate]] **passes on its second run**.

The first-pass failure is deliberately preserved in the gate document. A capstone
that showed only its final green state would hide the reason it exists.

## Verification

`app_composition_and_upgrade` — 3 tests, all green, committed and re-runnable:
- `two_apps_declaring_hooks_on_one_doctype` (A1a — the refusal)
- `two_installed_apps_each_hook_their_own_doctype_without_crosstalk` (A1b — isolation)
- `an_installed_app_survives_a_major_meta_upgrade` (A2)

## Related
[[v2.0 Deployability Gate]] · [[2026-07-28 WO-035 desk concurrent load]] · [[ADR-006 Plugin Capability Surface]] · [[ADR-008 Data Shape]] · [[Frappe Pain Points]] (P-2.2, P-7.3)
