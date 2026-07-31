---
tags: [frust, adr, vision, desk, frontend, milestone-4]
status: ACCEPTED 2026-07-31 — Boss ratified as recommended. Follow-through: (1) the priced BYO obligation's first installment = REST-surface documentation + the additive-only evolution policy formally adopted (named backlog item — "supported" is only true once the surface is documented); (2) topcoat #275 on the watch list (M5 timing coupled to it); (3) sequenced after ADR-015 as written.
created: 2026-07-31
---

# ADR-016 (proposed): Frontend Posture — Headless Kernel + One SSR Desk, App-Building Tier Named Not Silent

## The fork this closes

Raised by the Boss 2026-07-30: Frappe's selling punch is low-code + DocTypes, *completed* by frappe-ui (components, theme, responsiveness, `createResource` data flow). Frust has no equivalent app-building frontend tier — and until now that was decided by omission, WO by WO, never ratified.

## Proposed decision

1. **Frust is a headless metadata kernel + one excellent SSR Desk.** Stated, not implied. No SRS requirement demands an app-building toolkit (verified: the frontend MUSTs are perf floors and Desk-shaped); none of the 34 founding pains is "can't build bespoke SPAs"; and every deep bet in the vault — stack collapse, ADR-004 headlessness, ADR-001 one-dialect SSR, WO-042's zero-script pages — is a bet *against* the second client-side stack that made frappe-ui powerful and gave Frappe its heterogeneity pain.
2. **BYO-frontend is a first-class supported story TODAY — with its cost priced, not hand-waved.** "Supported" converts the REST surface from implementation detail into **a product with compatibility obligations**. Scope of the obligation (proposed): the *documented* REST surface, under the **ADR-006 evolution-policy shape applied to REST** — additive-only growth, breaking changes = versioned majors with deprecation notice. Not a frozen API. Said with a straight face: running Vue frappe-ui against Frust's REST surface is a supported pattern — that's ADR-004 cashing out, not a consolation prize.
3. **The SSR-native app-building kit is a NAMED future milestone (M5 candidate), not a silent absence.** If pulled, it's built from proven primitives — `fui_*` tokens/components, shards, per-field signals, SSE, the six-verb bridge — one stack, no client/server dialect split: the differentiated version of frappe-ui, the thing Frappe wishes it had. A Vue clone is the one option the vision forbids (re-imports the exact stack-split pain) and the only option built from behind.
4. **M5's timing is explicitly coupled to topcoat #275** (opened 2026-07-31, verified: *"compile client code from real Rust to JavaScript instead of runtime expressions — working spike + benchmarks"*). If it lands, the interaction ceiling that motivates the kit **partly dissolves** and the kit's design space changes. That's a decision on someone else's schedule — named here so it's watched, not discovered.
5. **The Research-Index "stack heterogeneity → solved" row is reframed as a trade** (done in this pass): Frust *sidestepped* the pain by not offering the capability that caused it; this note is where the trade is priced.

## Sequencing

**After ADR-015 (extension model).** An app-building tier without a settled extension model builds the showroom before the chassis is final — what a bespoke frontend can do depends on what an app can compose.
