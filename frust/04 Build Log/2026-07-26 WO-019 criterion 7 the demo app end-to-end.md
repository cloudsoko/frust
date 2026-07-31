---
tags: [frust, build-log, apps, plugins, work-order]
created: 2026-07-26
work-order: "[[WO-019 App Lifecycle]]"
status: all 7 criteria done — WO-019 complete
---

# Build Log — WO-019 criterion 7: The Demo App, End to End

Every piece built across this WO, running as one app, in **one kernel process
from the first install to the last assertion**.

## The run

```
STEP 1 plan ok: 1 planned
STEP 2 installed v1.0.0
STEP 3 small="small" large="large"
STEP 4 route served: ok:[{"amount":"12.5","band":"small","memo":"coffee"…
STEP 5 disabled: route 404, 2 rows intact
STEP 6 re-enabled: route serves, client script restored
STEP 7 updated to v2.0.0
STEP 8 post-update: band="large" reviewer="finance"
STEP 9 registry at v2, 5 audited lifecycle actions
```

Written as **one test, not seven**, because the criterion is the *sequence*.
Seven tests would prove seven things about seven kernels; the claim is one
thing about one app.

**STEP 8 is the line that matters.** `reviewer` is a field that did not exist
and a rule that was not written when the kernel started. It is present, correct
and live after an in-process update. REQ-2.1.1's zero-compilation claim is true
in the only way that counts.

Also proven along the way: money crossed as an exact decimal *string* (never a
float) through a script that classifies rather than computes; disable left both
rows untouched; re-enable restored the exact client script from the stored
manifest; the v1 rows survived the v2 update; and all five lifecycle actions
are in the changefeed.

## Finding — a test that was green and hollow

STEP 4 originally passed while the route read `purchase_order` — a name
hardcoded into `plugin_demo` during the criterion-1 probe. The app under test
declares `ledger_entry`. So the step proved *the route mechanism worked* and
said nothing about **an app reading its own data**, which is the actual claim.
It was green. It was hollow.

Fixed by having the handler take its doctype from the request; the test now
asserts the response contains the app's own row. **Distrusting a passing test
is harder than fixing a failing one**, and it paid immediately: the first
attempt returned `E_FIELD_NOT_READABLE: field "title"` — proving the **field
envelope reaches plugin surface**, which no other test asserted. Criterion 4
established `route == broker` for rows; this extends it to fields.

## Two more of the house style catching its own author

1. **`surql_monopoly` refused my `SELECT` in `hooks.rs`** (criterion 6's script
   lookup). The correct answer was to move the query to `sync.rs` beside
   `load_doctypes` — *not* to add `hooks.rs` to the allowlist. A guard that
   gets widened whenever it fires is not a guard.
2. **I asserted `amount == "4200.00"` and got `"4200"`.** SurrealDB normalises
   trailing zeros — the exact mistake WO-016's build log records, made again in
   the same session that quoted it. Worth stating plainly: **the caveat did not
   prevent the mistake; the test did.** That is the argument for tests over
   documentation, made accidentally and at my own expense.

## Suite state

**33 test-result groups green across 31 binaries, zero failures, exit 0** —
including all six WO-019 binaries (`door_probe`, `app_manifest`,
`app_lifecycle`, `app_routes_e2e`, `app_uninstall_scripts`, `demo_app_e2e`).

### Perf gates, fresh store per the third clause

| run | submit (gate 60) | realtime tax (allowance 2) | result |
|---|---|---|---|
| 1 | 43 ms | 2.75 ms | FAIL |
| 2 | 34 ms | 0.55 ms | pass |
| 3 | 26 ms | 0.00 ms | pass |

Run 1 fired immediately after the full suite, with the machine still settling;
runs 2 and 3 were consecutive on genuinely fresh stores and pass comfortably.
Reported rather than smoothed: the gate remains **sensitive to machine state at
the ~1 ms scale**, which is the standing caveat, not a new finding. Both
published numbers stand untouched — budget 20, allowance 2 ms.

Dev store restored intact afterwards (six doctypes, `travel_claim`'s client
script preserved).

## WO-019, complete

| # | Criterion | Outcome |
|---|---|---|
| 1 | Door probe | prediction **confirmed**; no escape found |
| 2 | The manifest | one format, validated before anything applies |
| 3 | Install story | validate → plan → gate → apply → record |
| 4 | Routes over REST | equality survived the wiring; throttle trips |
| 5 | Honest uninstall | metadata detaches, data remains |
| 6 | Server-script delivery | WO-017 item-3 finding closed |
| 7 | Demo app | whole lifecycle, one process, no restarts |

REQ-2.1.1's lifecycle half and REQ-2.2.2 are satisfied.

## Related
[[WO-019 App Lifecycle]] · [[2026-07-26 WO-019 criterion 1 the door probe]] · [[2026-07-26 WO-019 criterion 2 the manifest]] · [[2026-07-26 WO-019 criterion 3 the install story]] · [[2026-07-26 WO-019 criterion 4 routes over REST]] · [[2026-07-26 WO-019 criteria 5-6 uninstall and server scripts]] · [[SRS]] (REQ-2.1.1, REQ-2.2.2) · [[WO-018 Workflow Engine]]
