---
tags: [frust, build-log, tier2, sandbox, desk, work-order]
created: 2026-07-26
work-order: "[[WO-017 Client Script Sandbox]]"
status: PARTIAL — feasibility + containment proven, integration not built
---

# Build Log — WO-017: Client Script Sandbox (PARTIAL)

**Status: not closed.** The load-bearing unknowns are resolved and two criteria are proven; the integration work (browser hosting, Desk authoring, in-browser containment) is **not built**. Reporting where it actually stands rather than claiming a finish — the remaining work is well-defined, not blocked.

## What is proven

### The engine runs outside wasmtime (criterion 1's feasibility)

`script_engine.wasm` is a WASM **component** (layer=1, 4.06 MB) and browsers run core modules, so this was the WO's real gate. Resolved: `jco transpile` produces browser-shaped artifacts —

| artifact | size |
|---|---|
| `script_engine.core.wasm` | 4.05 MB |
| `script_engine.js` (host shim) | 232 KB |

— with `frust:plugin/host-api` mapped to our own `log` implementation, exactly the seam the kernel fills for wasmtime. **The transpiled engine executes**, verified end-to-end.

### Criterion 3 — one dialect, two hosts, executable

The *same* `script_engine.wasm`, the *same* script dialect, run through a **non-wasmtime host**, produced semantically identical output to the kernel's server-side execution:

```
in:  status "Draft",  total 20000.00
out: status "Needs Approval",  total "23000.00"
```

Both rules fired (the >10k approval flag, the 15% tax) and **the money came back as an exact decimal string** — `20000.00 × 1.15 = 23000.00`, no float artifact. ADR-007's founding sentence is now a thing that runs, not a plan.

### Criterion 2 — the capability surface, probed hostilely

A hostile script injected through the engine's `FRUST_SCRIPT` seam:

```js
doc.status = "probe:" + (typeof fetch) + "/" + (typeof document) + "/" + (typeof window)
           + "/" + (typeof XMLHttpRequest) + "/" + (typeof globalThis.process)
           + "/" + (typeof require) + "/" + (typeof WebSocket);
```

Result: **`probe:undefined/undefined/undefined/undefined/undefined/undefined/undefined`.**

Every host capability is absent — network, DOM, globals, module loader. The important structural point: **the boundary is the Boa engine's global object, not the host's configuration.** The same component runs in both hosts, so this containment property is carried *by the artifact*, not re-established per host — which is why it will hold in the browser by construction rather than by careful wiring. There is no `eval`/`Function` path because the engine is the only executor and it exposes no such global.

## What is NOT built (the honest remainder)

1. **Browser hosting + lazy load** (criterion 1's product half). The artifacts exist and execute; serving them from the Desk, gated on "this DocType has client scripts", is not done. The WO-014 2-request posture is therefore still intact but *untested against* the script path.
2. **In-browser containment** (criterion 5). Boa's server-side epoch/fuel budget does not transfer to the browser host; this needs a worker thread + watchdog (the WO anticipated this tension). Not designed.
3. **Decimal NaN-catch** (criterion 4). The engine already re-imposes types on the way out (the WO-001 shell property), and the decimal survived exactly in the proof above — but the *loud rejection* of a float-mangled Currency is not implemented.
4. **Desk authoring UI** (criterion 6). No `client_script` DocType, no editor.

## Findings

1. **Machine toolchain blocker, worked around non-invasively.** `npm install` fails for *every* project-scoped install on this machine: an environment-injected `allow-scripts` config makes npm reject the install outright (`EALLOWSCRIPTS`), and `--ignore-scripts`, `--userconfig` and `--globalconfig` overrides do not clear it. **`pnpm` is unaffected and was used instead.** No user config was modified. Worth recording in the environment notes — anything needing npm on this machine will hit it.
2. **The component→browser path costs a build step, not a rewrite.** No "browser-fit sibling" of the engine is needed (the WO offered that latitude): `jco transpile` + an import map is the whole adaptation, so the two hosts genuinely share one artifact rather than two builds that must be kept honest with each other.
3. **The 4 MB figure is confirmed as a product constraint, not a rounding error** — lazy-loading is correctly specified as a product property. A scriptless form must never pay it.

## Suite state
No kernel changes were made in this WO, so the suite is unchanged from WO-016 (**26 binaries green**). New tooling lives in `wasm-spike/` (`browser-engine/` transpiled output, `host-api.js`, `engine-test.mjs`); nothing is wired into the Desk or kernel yet.

## Related
[[WO-017 Client Script Sandbox]] · [[ADR-001 UI Extension Tiers]] · [[ADR-007 Tier-2 Script Architecture]] · [[2026-07-24 Script engine spike (WO-001)]] · [[2026-07-25 WO-014 Desk v2 dynamic forms]]
