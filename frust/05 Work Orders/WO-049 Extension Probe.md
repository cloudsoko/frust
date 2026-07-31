---
tags: [frust, work-order, apps, extension, probe, milestone-4]
status: ACTIVE (2026-07-31) — probe-first; gates ADR-015's final ratification. NO shipped relaxation of `owns` in this WO.
created: 2026-07-31
---

# WO-049: Extension Probe

## Why

The Boss approved ADR-015's direction (declared extension as manifest content). Before it ratifies, its three questions carry *leans*, not evidence — and the house convention is spike-gates-ADR. This probe produces the evidence. The WO-019 door-probe is the template: **falsifiable predictions stated before running.**

## Criteria

1. **The vocabulary verification (the pre-grill task, first).** Is validate-only for app server-scripts [[ADR-007 Tier-2 Script Architecture]]'s *ratified profile boundary* (widening = ADR amendment, standing convention) or simply unbuilt delivery? Read the profile table AND the code path (`app.rs::validate`'s `hook != "validate"`, the dispatch/delivery seams). Deliverable: a **sourced answer** — because it decides whether extending the hook vocabulary is an ADR-007 amendment or ordinary build work, and REQ-2.2.1's lifecycle list (`before_insert/validate/on_submit/on_cancel`) is the vocabulary an extension could eventually claim.
2. **The composition probe — predictions first.** In a *test build only* (planted bypass, never shipped — the `owns` refusal is a boundary), hand-carry a minimal extension: app B declares a namespaced field + a `validate` hook on app A's doctype. State predictions, then observe: does the `(tenant, doctype)` script pool serve one script or can it compose two? Does the owner's hook still run, first, un-overridable? What does the trace attribute each hook to? Does uninstall of B detach cleanly while A's doctype survives? This names exactly what the real build touches — pool keying, dispatch order, manifest schema, registry rows — instead of guessing.
3. **The owner-evolution seam (identify, don't build).** ADR-015's lean for question (c) is the WO-019 update gate extended cross-app — refusal-names-the-extension-casualty from its declared dependency surface. Verify the seam exists: when app A updates, can the gate *see* app B's registered extensions against A's doctypes (registry rows are per-app — does the plan phase consult other rows)? Deliverable: the seam named with file/fn, or the gap named honestly.
4. **Position paper → final ADR-015.** Each lean — (a) no-owner-opt-in + vocabulary, (b) refuse-ambiguity incl. veto semantics, (c) update-gate-extended — confirmed or overturned *by the probe's evidence*, with the WO-036 discipline: the probe asserts whichever reality holds, never encodes the prediction.

## Boundaries

- **Probe only.** No shipped change to `owns`, no manifest-schema change lands, no new hook delivery built. Planted bypasses live in test builds and die there.
- Scratch stores, standing perf hygiene if any number is taken, both auth modes if suites run.
- **Escalation:** if composition genuinely cannot fit the `(tenant, doctype)` pool without redesign, that is a *finding for the ADR* — the cost gets priced in the ratification, not worked around in a probe.

## Exit

ADR-015 ratifiable: every lean has evidence or an honest overturning, the vocabulary question has a sourced answer, and the build WO (WO-050, the real mechanism) can be scoped from what the probe touched rather than from the armchair.
