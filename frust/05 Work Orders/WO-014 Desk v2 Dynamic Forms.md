---
tags: [frust, work-order, desk, tier2, bridge]
status: COMPLETED 2026-07-25 — all 6 criteria; runtime DocType with 4 behavioral rules, zero recompile; 2-request network log (page + runtime.js); lattice outranks dynamism; money rules compare-never-compute (type-enforced); dirty-guard reconciliation rule; PR #203 open + #192 merged upstream. → [[2026-07-25 WO-014 Desk v2 dynamic forms]]
created: 2026-07-25
---

# WO-014: Desk v2 — Dynamic Forms (the Bridge Becomes Product)

> [!info] PM work order — results to `04 Build Log/`, live vault path verified first. Governing: [[ADR-001 UI Extension Tiers]] (as amended — per-field signals), [[2026-07-25 Tier-2 six-verb form bridge]] (the reference impl), [[ADR-007 Tier-2 Script Architecture]] (client half).

## Scope

The six-verb bridge graduates from `examples/frust-form-bridge` into the real Desk form renderer: **metadata-declared field behavior, zero-round-trip, for any DocType**. This is Tier-1 client dynamics (declarative rules from metadata) — the untrusted-script-text sandbox stays deferred, as ADR-001 phases it.

## Exit Criteria

1. **Metadata vocabulary for client behavior:** DocType field metadata gains declarative rules — `depends_on` (visibility), `read_only_when`, `required_when`, simple validate expressions, and `fetch_from` (the `call-server` verb against a kernel procedure/read) — compiled at render time into per-field dynamic signals + bridge verbs. No recompile for a new rule on a new DocType (the ADR-001 Tier-1 property, now for *behavior*, not just fields).
2. **Zero-round-trip proven on a real form:** the WO-009 `expense_claim`-style form with dependent fields — browser network log shows no requests for visibility/readonly/validate interactions; exactly one for each `fetch_from`.
3. **The lifecycle still governs:** dynamic rules compose with docstatus affordances (a `depends_on`-shown field is still frozen at Submitted; allowlisted fields still editable) — the lattice stays the floor under the dynamism.
4. **Decimal discipline holds in the client rules:** expressions over Currency fields go through the Decimal surrogate — the structural no-client-arithmetic guarantee re-proven where users will actually hit it (a validate rule comparing money must work; a rule *computing* money must be impossible or server-routed).
5. **Realtime + dynamics coexist:** a socket tick refetching a focused form doesn't stomp in-progress field edits (the classic Frappe annoyance) — state the reconciliation rule and prove it (dirty-field guard at minimum).
6. **The dynamic-signals upstream PR is submitted** — the largest carried patch starts its deletion path (pin governance: the PR is the record; link it in the log).

## Boundaries

- Expression vocabulary stays inside what the `$()` language + bridge verbs express — the `||`/`&&` gap is a *vendor follow-up*, not a reason to invent a DSL. If a rule can't be expressed, it's server-side (shard/procedure), stated in the metadata docs table.
- No untrusted script text. Rules are metadata authored by builders, same trust tier as DocType definitions.

## Escalations

Standard rules + the measuring-WO hygiene set (substrate probe, serialize latency gates, drop scratch DBs at close).

**Related:** [[Frust Hub]] · [[ADR-001 UI Extension Tiers]] · [[ADR-007 Tier-2 Script Architecture]] · [[2026-07-25 Tier-2 six-verb form bridge]] · [[2026-07-25 WO-012 Desk realtime]] · [[Topcoat]]
