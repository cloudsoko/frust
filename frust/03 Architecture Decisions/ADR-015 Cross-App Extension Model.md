---
tags: [frust, adr, apps, extension, milestone-4]
status: DIRECTION APPROVED by Boss 2026-07-31 — declared-extension-as-manifest-content is the ratified direction; FINAL RATIFICATION GATED on [[WO-049 Extension Probe]] (the pre-grill verification task + an empirical probe of the three questions' leans — spike gates ADR, per house convention). The leans below are positions to be confirmed or overturned by the probe's evidence, not decided text.
created: 2026-07-31
---

# ADR-015 (proposed): Cross-App Extension Model — Declared Extension as Manifest Content

## Framing — a SCOPE EXTENSION, not a compliance gap

Stated precisely, because the first draft of this position got it wrong and the grill caught it:

- **REQ-2.2.3 is satisfied as scoped.** Its text is "extend UI and schema *without modifying core files*" — don't-fork-core, which Tier-1 metadata satisfies; its scope was deliberately clarified 2026-07-23 (→ ADR-001) and the SRS records ALL GAPS CLOSED. This proposal does not claim a breach.
- **The v2.0 gate states the trade explicitly** — *"cross-app extension of one DocType is not possible"* — in the same cell that scores P-2.2 bounded-by-architecture. Nothing was papered over.
- **What this proposes:** extend the scope, and *re-price a named trade* against evidence. The founding evidence: P-2.2's three defects are all **mechanism** — global mutability, order-dependent merging, opaque attribution — none is "apps shouldn't extend each other." And What-Frappe-Gets-Right lists *"customization without forking … layered over core"* (honestly noted: that line is app-over-*core*; the killer Frappe move — an app layering fields onto Sales Invoice — is app-over-*app*, adjacent to the line, not stated by it). A1 killed the chaos by killing the capability; this proposes restoring the capability without the chaos.

## Proposed mechanism — everything the platform already knows how to do

**Declared extension as manifest content.** An extending app declares: "I add fields X (namespaced to me) to doctype Y (owned by app Z); I hook event E on Y." Then:

- **Install-gated** — the WO-019 gate discipline literally: validated, plan-shown, refused loudly on conflict.
- **Registry-recorded** — the registry stays the system of record for what an app is, extensions included.
- **Trace-attributed per app** — "which app changed this behavior" is a log field, not archaeology. P-2.2's opacity dead *by construction*, not by refusal.
- **Namespaced fields, one owner, one lifecycle** — an extension field belongs to its app; uninstall detaches it (the honest-uninstall story extends). P-4.2's precedence hell dead: there are not three overlapping mechanisms, there is one, with a registry row.
- **The A1 refusal STAYS for undeclared hooking** — the wall becomes a door with a customs post.

## The three grill questions (the ADR's real content)

**(a) Does the owner opt in — AND what is the hook vocabulary? One question, not two** (builder report, 2026-07-31, from the code): the enforcement in `app.rs::validate` is *two adjacent checks* — the `owns` predicate (who may attach) and a `hook != "validate"` restriction (what exists to attach *to*), both running at install time against the bundle's own manifest. **Relaxing `owns` alone buys a permission system for a single hook.** So the grill must settle both together. Lean on the who-half: **no owner opt-in** — owner-declared points make the seed a bottleneck and recreate the rigidity (Frappe's power was extending DocTypes that never anticipated it); safety from invariants instead: the owner's schema constraints and hooks **always run and cannot be overridden**, extensions additive-only, names namespaced, collisions refused at install. The vocabulary-half needs a **pre-grill verification task, not an assertion** (the lesson of this ADR's own first draft): read whether validate-only is [[ADR-007 Tier-2 Script Architecture]]'s *ratified profile boundary* for doc-hook scripts (profile widening = ADR amendment by standing convention) or simply unbuilt delivery — REQ-2.2.1 names `before_insert/validate/on_submit/on_cancel` as the lifecycle vocabulary, and which of those an *extension* may claim is part of this decision either way.

**(b) Composition semantics when extensions disagree.** Lean (adopted from the grill): **refuse ambiguity instead of resolving it.** If two extensions' outcomes conflict, don't invent a precedence rule — make the conflict an error naming both apps. *Zero mechanisms with unclear precedence beats three*, which is P-4.2's actual complaint. Still to grill: rejection semantics (extension B rejects a write the owner accepted — is that a veto, and is a veto additive-only in spirit?), and the `(tenant, doctype)` pool's one-script assumption, which composition touches.

**(c) OWNER EVOLUTION — the question that decides whether this beats Frappe or is just Frappe with a customs post.** Install-time gating catches the install; it says nothing about the owner's next release. Extension reads `Y.z`; owner v2 renames it or changes `validate` semantics; *both apps' CI is green*. P-4.2 and P-7.3 both live in that gap. Evidence it's structural, not occasional: topcoat #214 — a parameter narrowing, both sides green, downstream broke, caught only by a downstream regression test. Cross-app extension makes that shape systemic.
  **Lean:** the answer is the machinery WO-019 already built — **the update gate, extended cross-app.** The destructive-update refusal already *names its casualties* ("REMOVE FIELD memo"); an owner update that breaks a declared extension surface refuses the same way: *"this update breaks extension E of app B (reads `Y.z`, which you renamed)"* — refuse or explicitly acknowledge-and-disable, **never silently**. The extension's manifest already declares its dependency surface (fields read, events hooked), so the gate has exactly what it needs to name the casualty. A silently-disabled extension is P-2.2 reborn; a loudly-refused update is the house style.

## v1 scope (ponytail)

Fields + lifecycle hooks. UI injection rides Tier-1 automatically — the Desk renders whatever metadata exists. Everything else (extension-to-extension dependencies, marketplace ordering) waits for evidence.

## Evidence base for the grill

A1's refusal message (the exact constraint to relax) · the accounting seed as owner-app test case · [[v2.0 Deployability Gate]] P-2.2 row (the trade being re-priced) · topcoat #214 (the owner-evolution shape in the wild).
