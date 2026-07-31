---
tags: [frust, work-order, tier2, sandbox, desk]
status: COMPLETED 2026-07-26 — all 6 criteria proven across 4 items + the initial gate session. The composition argument closed it: the authoring UI adds NO trust — a hostile script through the front door is exactly as contained as an injected one, because items 1–3 never asked where scripts came from. No eval path exists in either host. ADR-001's three tiers are ALL SHIPPED. → [[2026-07-26 WO-017 item 4 Desk authoring — WO closed]]

*(History below: initial gate session, then items 1–4.)* PROVEN: the feasibility gate (jco transpile → one artifact, two hosts — no sibling build needed); criterion 3 executable (same script, same engine, outside wasmtime, exact decimals); criterion 2 hostile-probed (7 escape vectors all undefined — boundary is the engine's global object, held by construction). Item 1 DONE 2026-07-26 (scriptless = exactly 2 requests, measured; server-side gate, no client branch; strip_trace position-suffix hygiene defect fixed in BOTH hosts via one artifact rebuild — [[2026-07-26 WO-017 item 1 browser hosting with lazy-load]]). Item 2 DONE 2026-07-26 (ADR-005 bar met in-browser hostile-first: loop/hog/hostile-user-script all killed, form alive, breaker trips, sticky kill notices; worker = thread not document — single-doc posture holds, validate goes async because main-thread sync is *uncontainable*; 0.3 ms healthy vs 250 ms budget — [[2026-07-26 WO-017 item 2 worker watchdog containment]]). Item 3 DONE 2026-07-26 (money a script corrupts is refused typed in BOTH hosts from one rebuild — E_MONEY_FLOAT/E_FIELD_NAN/E_MONEY_TYPE/E_MONEY_NOT_NUMERIC; NaN caught before stringify erases it; fractional-refused-even-when-exact rule; the catch also stopped a runaway self-feeding script at 3.7e20 — [[2026-07-26 WO-017 item 3 decimal NaN-catch]]). FINDING ROUTED: kernel never runs per-DocType server scripts (empty-WASI is correct posture; delivery unbuilt since WO-001) → server-script delivery added to [[WO-019 App Lifecycle]]. REMAINING: Desk authoring (criterion 6) — closes the WO. → [[2026-07-26 WO-017 client script sandbox (partial)]]
created: 2026-07-26
---

# WO-017: Client Script Sandbox (ADR-001's Last Unbuilt Piece)

> [!info] PM work order — results to `04 Build Log/`, live vault path verified first. Governing: [[ADR-007 Tier-2 Script Architecture]] ("two hosts, one dialect" — this is the second host), [[ADR-001 UI Extension Tiers]], [[2026-07-25 Tier-2 six-verb form bridge]] (the target surface, now in product via WO-014).

## Scope

Untrusted user-authored client script *text* executes in the browser inside `script-engine.wasm`, with **the six verbs as its entire world**. Scripts are metadata (a `client_script` DocType — stored, live-mutable, zero deploys), same JS dialect as server scripts.

## Exit Criteria

1. **The engine in the second host:** `script-engine.wasm` (the WO-001 Boa component or its browser-fit sibling) loads in the browser and executes script text against the six-verb bridge. **Lazy-loaded only when the DocType has client scripts** — the 2-request no-bundle posture of WO-014 is a product property; forms without scripts must not pay the multi-MB engine.
2. **The capability surface is exactly the six verbs:** no DOM, no network, no cookies, no globals beyond the bridge — probe it hostilely (attempt `document`, `fetch`, `window` escape from inside a script; each must fail typed, not silently).
3. **One dialect, two hosts, proven:** a single `validate`-shaped script runs server-side (WO-001 engine) and client-side unchanged where verbs overlap. The ADR-007 sentence, finally executable.
4. **Money discipline in user scripts:** the decimal-as-string reality (ADR-007 docs item) gets its helper — scripts get a documented decimal-safe path; a script that float-mangles a Currency field must be *caught* (NaN/type-change → loud typed error, the WO-014 suggestion made real), never stored.
5. **Hostile containment in the browser:** infinite loop and allocation bomb — terminated (worker thread + watchdog or engine-internal budget; design yours, prove it), form stays alive, error surfaced with engine internals stripped.
6. **Authored in the Desk:** create/edit a client script through the UI (manager-tier permission), attached to a DocType + hook point (the metadata-schema hook points from ADR-001, v0 promise kept); next form load runs it. The Frappe client-script UX, sandboxed for real (P-5.1's client-side cousin dies).

## Boundaries

- No `eval`/`Function` fallback path, ever — the engine is the only executor.
- Script errors: loud, stripped, and *attributed* (which script, which hook point) — debuggability is a feature of the sandbox, not a leak.
- Perf: script-present forms state their interactive-time cost honestly (first-load engine fetch + instantiate, then warm); the WO-001 cold/warm methodology applies.

## Escalations

Standard rules + full hygiene set. If criterion 5 can't be met without a worker-thread architecture that breaks the single-document posture — report the design tension before building around it.

**Related:** [[Frust Hub]] · [[ADR-001 UI Extension Tiers]] · [[ADR-007 Tier-2 Script Architecture]] · [[2026-07-24 Script engine spike (WO-001)]] · [[2026-07-25 WO-014 Desk v2 dynamic forms]]
