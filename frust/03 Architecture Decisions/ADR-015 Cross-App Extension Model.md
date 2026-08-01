---
tags: [frust, adr, apps, extension, milestone-4]
status: ACCEPTED 2026-07-31 — ratified on [[WO-049 Extension Probe]]'s evidence (all 5 predictions confirmed, both lean-corrections absorbed, see Ratification section). Build = [[WO-050 Extension Mechanism]].
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

## Ratification (2026-07-31, on WO-049's evidence)

**Vocabulary (the (a)-entanglement, sourced 5 ways):** validate-only is **unbuilt delivery, not a ratified boundary** — ADR-007's profile-table axis is *verbs* with no lifecycle row; ADR-006 already keys its cycle rule on a plural `(record-id, hook-class)` and its evolution policy is additive-only; `HookClass{Validate,OnWrite,Scheduled}` exists in `contract.rs` with only Validate wired. Widening the vocabulary is **ordinary build work under REQ-2.2.1**, not an ADR-007 amendment. (Deferred out of WO-050 — composition lands on `validate` first; vocabulary widening is its own follow-on.)

**The binding build order — THE MECHANISM LANDS BEFORE THE BOUNDARY MOVES.** The probe's headline: with `owns` bypassed, app B **silently replaced** app A's hook — `owner_ran = null`, no error, no log, no trace. P-2.2 in its original form, and `owns` is the only thing preventing it today. Therefore a naive relaxation enables silent override, not composition, and **`owns` is relaxed only as the LAST step of WO-050**, behind: `server_script` scalar→list of `{app, script}` (ADR-008 migration discipline), owner-first un-overridable dispatch, pool + WO-048 cache keys widened to `(tenant, doctype, app)`, `app` on the `hook_dispatch` span, per-app registry rows so uninstall detaches the extension and nothing else.

**(a) corrected per the probe:** "the owner's hooks always run and cannot be overridden" is **not a property the system has — it is one the build must create**, stated as an obligation with its own failing control: the probe's silent-override scenario becomes the permanent regression test (B may never replace A).

**(b) veto RULED:** extension hooks **may reject** — a validate that cannot reject is not a validate, and rejection is *tightening*, which is additive-only in spirit. Vetoes are attributed per-app in the typed error (the trace names the rejecting app). **No suppression mechanism in v1** (YAGNI). Refuse-ambiguity governs *conflicting mutations* (two extensions writing the same field → install-time refusal naming both apps), not vetoes — a veto is not ambiguous, it is a rejection with a name.

**(c) confirmed, cheaper than assumed:** the owner-evolution seam exists end-to-end with **no new storage** — `plan_unchecked` already loads the whole-tenant view (already used cross-app for rollup targets), `installed_app.manifest` holds every declared surface verbatim and is re-parsed live in production, `destructive()` already yields casualty strings. WO-050 wires the call: an owner update that breaks a declared extension surface **refuses naming the extension casualty**, never silently disables.

**Probe's unpredicted finding → build obligation:** the WO-009 envelope filter **silently strips undeclared fields** — an extension that writes a field it failed to declare gets no error and no data. In WO-050, an extension writing an undeclared field is a **typed, loud error**. Declare-or-lose-silently is the exact failure class this project refuses.

## Evidence

[[2026-07-31 WO-049 extension probe]] (`extension_probe.rs`, 3/3, asserts-whichever-reality-holds) · A1's refusal message · the accounting seed as owner-app test case · [[v2.0 Deployability Gate]] P-2.2 row (the trade re-priced) · topcoat #214 (the owner-evolution shape in the wild).
