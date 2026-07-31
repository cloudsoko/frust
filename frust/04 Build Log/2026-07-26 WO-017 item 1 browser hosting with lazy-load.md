---
tags: [frust, build-log, tier2, sandbox, desk, work-order]
created: 2026-07-26
work-order: "[[WO-017 Client Script Sandbox]]"
status: item 1 of 4 complete — WO still open
---

# Build Log — WO-017 item 1: Browser hosting with lazy-load

The engine now runs in the Desk, and the 2-request posture for scriptless forms
is measured rather than asserted. Items 2–4 of the WO remain open.

## The non-negotiable, measured

Counted in a real browser, not reasoned about:

| Form | Requests | Engine fetched? |
|---|---|---|
| `/form/purchase_order` (scriptless) | **2** — document + `runtime.js` | no |
| `/form/travel_claim` (scripted) | 12 — + boot module, 6 shim files, 232 KB host shim, 4.06 MB core | yes |

A scriptless DocType is **indistinguishable on the wire from a Desk with no
script engine at all**. The gate is one server-side decision — `dt.script()` in
`form_page` — so nothing on the client can defeat it, and no client-side
"should I load this?" branch exists to get wrong.

## What was built

- **`Wasm` content wrapper** in the vendored Topcoat (`topcoat-router`,
  mirroring `Js`/`Css`, with a colocated test). Not decoration:
  `WebAssembly.compileStreaming` rejects any response whose media type is not
  exactly `application/wasm`, so without it the engine cannot load at all.
- **`/engine/{file}`** serves the transpiled assets, embedded via
  `include_bytes!`/`include_str!` so the Desk stays one binary (the WO-009
  no-asset-bundle posture holds). **"Lazy" is a claim about requests, not
  binary size** — the bytes ship, they are simply never fetched.
- **`/engine-boot/{doctype}`** generates the per-DocType boot module: loader
  plus that DocType's script in one request. Generated rather than embedded in
  the page because user-authored JS in the document would have to survive
  `view!`'s HTML escaper — a trap already paid for once in WO-014.
- The script reaches the engine through **`_setEnv({FRUST_SCRIPT})`**, the same
  seam the kernel host fills. Neither host is privileged; they feed the same
  artifact the same way.

**No import map.** The transpiler emits Node-shaped bare specifiers; these are
rewritten to sibling URLs on the way out. An import map would have been a JSON
blob inside the document, i.e. more escaping risk, for no gain.

**No kernel change.** `/meta` is `SELECT *`, so `client_script` reached the Desk
as ordinary metadata the moment it existed on the record.

## Proven end to end

Against `travel_claim`, in Chrome:

- domestic → `approver_note` = `Auto-approved`
- international, no visa → **rejected**, form-level message, no save
- international + visa, title `"Berlin summit"` → `Route to regional manager
  (title has 13 chars)` — real computation, not a declarative rule

## Findings

### 1. `strip_trace` leaked engine internals on the same line — in BOTH hosts

The first reject rendered as:

> `Error: International trips need a visa reference before saving. (unknown at :2:9)`

`strip_trace` dropped trailing *lines*, but Boa appends the source position to
the **first** line, so it survived. A line/column inside a script the user
cannot see is engine internals by any reading — this was a live **ADR-007
hygiene defect**, and because it lives in the shared artifact, the **kernel had
it identically**. It was never browser-specific; the browser host is simply
what surfaced it.

Fixed precisely, not bluntly: the suffix is stripped only when it really is a
position (`(… at <digits>:<digits>)`). Verified both ways — the position is
gone, and a reject message legitimately ending in parentheses
(`Blocked by travel policy (see clause 4.2)`) survives intact.

**One rebuild fixed both hosts.** That is criterion 3's "one dialect, two
hosts" paying out as maintenance, which is the only place a shared-artifact
claim can actually be tested.

### 2. WO-016's Currency→decimal migration silently broke two kernel tests

`permission_proof.rs` asserted money with `as_f64().unwrap()`. `Currency` is
`decimal` since WO-016, SurrealDB serialises decimals as JSON **strings**, so
`as_f64()` returns `None` on exactly the fields that matter most.

The reason WO-016 reported green: the dev DB had not been migrated yet. Booting
`frust serve` today applied the pending `ALTER FIELD total` and the assertions
broke immediately. Pre-existing latent defect, surfaced — **not** caused by the
sandbox work, and worth naming because rows written before the migration are
still floats, so **both shapes are live in one table at once**.

Fixed with a `num()` helper reading either shape. f64 is correct *there* — those
assertions are about row visibility and sort order, not monetary exactness,
which is asserted decimally against the DB in `decimal_rollups.rs`.

### 3. 94 scratch databases had accumulated again

Same drift as WO-016 (76 that time). Dropped at close; one database remains.
The hygiene rule holds, but it is being re-applied by hand every WO — the
tests that create scratch DBs still do not drop them.

## Suite state

**26 binaries green, zero failures.** Perf gates in their own invocation per
standing practice — the first run flagged `gate_submit_with_live_subscriptions`
purely because a live kernel and Desk were competing with it, which is precisely
the contention the isolation practice exists for. Green isolated.

## Carried patch

`Wasm` in `topcoat-router` joins the vendored trunk. Recorded, not PR'd, per the
standing fix-locally posture.

## What remains in WO-017

2. **Worker + watchdog containment** (criterion 5) — the epoch budget does not
   transfer to the browser; the genuinely new design.
3. **Decimal NaN-catch** (criterion 4) — rides the same host shim.
4. **Desk authoring UI** (criterion 6).

## Related
[[WO-017 Client Script Sandbox]] · [[2026-07-26 WO-017 client script sandbox (partial)]] · [[ADR-007 Tier-2 Script Architecture]] · [[ADR-001 UI Extension Tiers]] · [[2026-07-26 WO-016 decimal rollup accumulation]] · [[2026-07-25 WO-014 Desk v2 dynamic forms]]
