---
tags: [frust, work-order, docs, rest, api, milestone-5]
status: DELIVERED (2026-08-01) — surface documented from the code (31 routes), 41 examples executing against a live kernel, route table guarded against drift in BOTH directions (watched red both ways). Evolution policy normative. **ONE ESCALATION — G1: bad credentials answer 500 with an internal transport detail, contradicting the route's own source comment; reported, deliberately NOT documented as a promise, fix is one match arm and needs a ruling.** Rider (a) corrected its own premise by measurement. See [[2026-08-01 WO-054 rest surface docs]].
created: 2026-08-01
---

# WO-054: REST Surface Docs + Evolution Policy

## Why

ADR-016 ratified "BYO-frontend is first-class supported" — and priced it: *"supported" is only true once the surface is documented and its stability promised.* This pays the first installment. The house discipline applies to docs too: **an example you can't re-run is an anecdote**, so every documented example executes.

## Criteria

1. **Inventory from the code, not memory:** every route in `rest.rs` (+ app routes' dispatch) — method, path, auth tier (none/session/manager), request/response shape, typed error codes (`FRUST:E_*`), the throttle/admission semantics (429/503 + `Retry-After`). Documenting always finds warts: **inconsistencies found = named, not papered** (a gaps section, PM decides which get fixed).
2. **Every example executes.** A committed harness (`frust-e2e/` home, the WO-039 precedent) runs each documented request against a live `frust serve` and asserts the documented response shape — docs whose examples are green in CI, not prose. Include the conventions a consumer must know: decimal-as-string money (never floats), the tenant hint at `/login` + `<TenantId>.<random>` bearer discipline, dataless realtime ticks (refetch through the read door), SSE endpoints.
3. **The evolution policy, formally stated** (the ADR-016 text made normative): the documented surface grows **additive-only**; breaking changes = versioned majors with deprecation notice; undocumented routes carry no promise. One page, linked from the docs' front.
4. **The BYO quickstart:** a minimal external client — plain `curl`/`fetch`, no Topcoat, no Desk — driving login → list → read → write → workflow transition end-to-end. This is the "supported pattern" proven rather than claimed, and it doubles as the doc a Vue-frappe-ui consumer would start from.
5. **Home: in-repo** (`frust-kernel/docs/`), where a BYO consumer finds it; the vault gets the link + the gaps list. Suites green (docs riders below are the only code).
6. **Two hygiene riders** (small, in-tree, from WO-053's findings): (a) the registry round-trip defect — a stored manifest containing a lone surrogate is rejected by its own `/app/update` door; validate JSON-serializability at intake so the registry can never hold a manifest it can't re-emit; (b) delete the stale `wasm-spike/wit` fork (drift hazard, superseded by the kernel-owned WIT).

## Boundaries

- Documentation of what IS, not redesign — route changes (beyond rider (a)'s intake validation) are findings for the gaps list, not edits.
- No new deps; the harness reuses the existing e2e tooling.

## Escalation

- If the inventory finds a route whose behavior can't be documented honestly without fixing it (a contradiction, not a wart), stop and report — that's a surface bug, and the docs shouldn't launder it.
