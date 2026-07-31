---
tags: [frust, build-log, apps, plugins, scripts, work-order]
created: 2026-07-26
work-order: "[[WO-019 App Lifecycle]]"
status: criteria 1-6 done — criterion 7 remains
---

# Build Log — WO-019 criteria 5 + 6

Uninstall answers the `bench remove-app` question honestly, and the kernel
finally runs per-DocType server scripts — closing the WO-017 item-3 finding.

## Criterion 5 — uninstall is honest

The response states what survived, per table, at the moment the operator acts:

```json
{"action":"uninstalled","metadata_removed":["acct_note"],"routes_removed":0,
 "data_retained":[{"table":"acct_note","rows_retained":3}],
 "note":"Metadata detached and the manifest removed. Tables and rows remain —
         dropping them is a separate, explicitly acknowledged act."}
```

**Metadata detaches. Data remains. The manifest that described the metadata is
itself removed — uninstall is the one operation that genuinely forgets
something.**

That last clause is now a *test*, not a description: `enable` after uninstall
fails `UnknownDoctype`, because there is no stored manifest left to restore
from. Disable is reversible precisely because the manifest survives it; that
asymmetry is the design, and it is asserted in both directions.

An app's data outlives the app deliberately. REQ-6.6.3 draws the line — schema
revert is not data recovery — and the same line holds here: this is not a
delete button wearing a lifecycle costume.

## The 404 ruling, and the subtlety it implied

A disabled app's route answers **404, indistinguishable from an unknown one**:

```
disabled: UnknownDoctype { name: "route /app/acct/ledger" }
unknown:  UnknownDoctype { name: "route /app/acct/nosuch" }
```

The test asserts the two carry the **same discriminant**, and separately that
the response does *not* contain the word "disabled". A distinguishable refusal
is itself the leak — it confirms to an outsider that the route is
real-but-forbidden. This is the HTTP echo of "metadata detached".

The reason is not lost, only moved: `route_refused_app_disabled` is emitted to
the log. **The server keeps what the response withholds.**

## Criterion 6 — server scripts, delivered

Before this, every server-side write ran the engine's *built-in default*, for
every DocType, since WO-001. Now:

| Behaviour | Proof |
|---|---|
| a bundled script runs on a write | `{"flag":"server-script-ran", …}` |
| a script can reject a write | `HookRejected { message: "Error: notes must be approved first" }` |
| **an edited script takes effect next write, no restart** | `v1` → edit metadata → `v2` |
| a DocType with no script runs **nothing** | `flag` is null; the default does not leak in |

**Scripts are data, live-mutable — server-side, at last.** The load-bearing
detail is that the instance pool compares the script **text**, not merely its
`(tenant, doctype)` key. A pool keyed alone would serve a stale script forever
and make "scripts are data" a slogan: data in name, configuration in practice.

**No-script means no script.** A DocType that declares none runs none, rather
than silently inheriting the built-in default — "silently inherits someone
else's validation" is the WO-017 finding wearing a new costume, and it only
stays fixed if the negative is asserted.

**Delivery is by seam, never env inheritance.** The guest's world stays empty
apart from the single `FRUST_SCRIPT` variable the host chooses to put there;
inheriting the kernel's environment would hand a sandboxed guest every secret
the process holds.

### The trait was wrong

`HookDispatch::validate` did not receive the doctype, so a hook could not know
what it was validating — and therefore could not do per-DocType anything. Nine
implementors changed. That is the right blast radius: the trait was the defect,
and working around it (a thread-local, say) would have hidden the defect rather
than removed it.

## Findings

1. **A shared-state test that asserts before restoring cascades.**
   `a_disabled_app_stops_serving_routes` asserted on the old "disabled" message
   between its disable and its enable. When the 404 ruling changed that
   message, the assertion failed *before* the re-enable — leaving the app off
   and turning one real failure into **four unrelated ones**. Restructured to
   observe → restore → assert. The serial mutex prevents interleaving; it does
   not survive a panic.
2. **`surreal.exe` had died** from the perf-chase kills earlier in the session,
   which is why the first run of the new tests showed seven identical
   `Connection refused` failures. Worth recognising the signature: *every* test
   failing identically is an environment fact, not a code fact.

## Suite state

`app_uninstall_scripts` (7 tests) and `app_routes_e2e` (6 tests, re-verified in
default parallel mode) green. Full-workspace tally at close.

## Next — criterion 7 closes the WO

The demo app end to end: installed from a bundle, exercised, disabled,
re-enabled, updated to v2, with a server script that demonstrably runs and a
route that serves — no restarts anywhere, on a **fresh store** for the perf
pass per the third clause of the hygiene rule.

## Related
[[WO-019 App Lifecycle]] · [[2026-07-26 WO-019 criterion 3 the install story]] · [[2026-07-26 WO-019 criterion 4 routes over REST]] · [[2026-07-26 WO-017 item 3 decimal NaN-catch]] · [[ADR-007 Tier-2 Script Architecture]] · [[SRS]] (REQ-2.1.1, REQ-2.2.3, REQ-6.6.3)
