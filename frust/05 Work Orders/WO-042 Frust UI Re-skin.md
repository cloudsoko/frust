---
tags: [frust, work-order, desk, ui, design, topcoat, milestone-4]
status: COMPLETE (2026-07-29) — the shipped Desk wears Frust UI, and the predicted ceiling is REAL and NAMED. RE-SKINNED (browser-proven on real pages with the real dev fixture, not the gallery): layout/nav, login, home, list, new-record form, record/workflow page, every field. Merged onto the CURRENT post-WO-038 main.rs (semaphore/spawn_blocking/shared Agent/SSE all intact underneath); frust_ui.rs/.css confirmed Desk-local, nothing named frust_ui* anywhere in the vendored topcoat tree. THREE BEHAVIOURS, ZERO JS ADDED (home page ships 0 scripts and 0 inline handlers, asserted in-browser; the only script on any re-skinned page is WO-014's pre-existing runtime.js): Dialog = rung (a) CSS-only `:target` (display none→flex on target, none after Cancel; Back button closes it free, because open-ness is a history entry). Toast = rung (a) CSS-only, `animation-delay: 6s` + `fill: forwards`, no timer; prefers-reduced-motion disables the auto-dismiss too, because vanishing text is what that preference is about. Combobox = rung (b) server round-trip `<datalist>` — the ONE interaction that legitimately needed (b), exactly as predicted; label field comes from METADATA (the first cut guessed `name`/`title` and would have offered `customer:qfmpdzax…` on the seeded doctype whose field is `cust_name`), capped at 50 with the cap STATED in the UI. **THE CEILING, NAMED — modal focus-trap + Escape-to-close:** (a) impossible (no selector for "focus is leaving this subtree"; tabindex can reorder focus, not contain it, and a leaky trap is worse than none); (b) impossible (sub-frame keyboard concern, no request to make); (c) COULD but NOT reached — it would ship a client runtime to the whole Desk for one modal's keyboard ergonomics on a page that currently ships zero scripts. Native `<dialog>`+`showModal()` does it correctly but `showModal()` is JS: the capability is one line of JS and zero lines of CSS away. ADR-shaped question, reported not worked around. ZERO BEHAVIOUR REGRESSION, ASSERTED: WO-031 workflow 18/18 PASS (full clerk→submit→manager→approve→reject→reopen), WO-032 SSE 8/8 PASS, WO-038 shed 135×503 with the counter matching exactly and Retry-After present, plus in-browser WO-015 line tables, WO-014 dirty guard + reactive form, light/dark (248,248,248 → 31,31,31 with prefers-color-scheme in the served sheet), and row permissions (clerk1 sees 3 rows, manager 6). CARRIED TOPCOAT PATCH: Topcoat had NO 429/503 constructor and maps errors via a CLOSED downcast list, so WO-038's Desk-local Busy rendered as HTTP 500 — added ServiceUnavailableError/service_unavailable(retry_after_secs) to the vendored trunk (suite green 288+67); strong upstream-PR candidate. THREE FINDINGS: (1) the ceiling above; (2) **the evidence harness had been silently unrunnable** — `node_modules/playwright` was a junction to `D:\Dev\rust\wf-proof\…`, the name from BEFORE WO-039 renamed it to frust-e2e, so `pnpm workflow`/`pnpm sse` could not start; repaired, both green — the README's own "evidence you cannot re-run is an anecdote" had quietly stopped being true; (3) a CSS class name that does not exist fails SILENTLY (`fui-btn--solid`, which I invented in 8 places, compiled and rendered nearly-right). TWO INSTRUMENT FAILURES, BOTH MINE: the browser CANNOT prove the shed (Chrome caps ~6 connections/host so 300 in-page fetches serialise and never reach the 64 bound — read `shed: 0`, which looks exactly like a broken re-skin; `shed-check.mjs` with maxSockets:Infinity fires it immediately), and six workflow checks failed on a STALE SELECTOR not a broken behaviour (`h1 span` → the chip moved to a `.fui-page-head` sibling; fixed with no fallback, because a selector matching two generations of markup cannot tell you the next re-skin moved it). frust-desk-ui RETIRED not deleted (RETIRED.md records why): drift risk closed, but frust-desk/frust-kernel are NOT under version control so its 2 commits are the only versioned record of the foundation; target/ (1.01 GB) stripped, 4.5 MB kept. Dev-store mutations stated in the log. See [[2026-07-29 WO-042 frust ui re-skin]].

Prior status — ACTIVE (2026-07-29) — Boss-selected M4 turnkey direction. Wire WO-037's Frust UI foundation into the REAL Desk pages (it was built isolated and deliberately unwired) + build the interactive behavior layer WO-037 named as Topcoat's real ceiling (dialog/toast/combobox), Vue-free, CSS-first. The load/scale spine (concurrency·memory·tenancy·conn-reuse·admission) is closed; this is the "make it look finished" half.
created: 2026-07-29
---

# WO-042: Frust UI Re-skin + Behavior Bridge (wire the foundation, cross the behavior ceiling)

> [!info] PM work order — the Boss-selected turnkey direction after the load thread closed (WO-041 sustained + WO-038 overload). Governing: [[WO-037 Frust UI Foundation]] (the foundation this wires in) · [[2026-07-28 WO-037 frust-ui foundation]] (what shipped + the named ceiling) · [[ADR-004 Topcoat for Desk v0]] (the headless/no-second-runtime boundary) · [[Topcoat]] · [[ADR-001 Client Extensibility]] (the six-verb bridge / dynamic signals — one of the Vue-free behavior paths).

## What already exists (WO-037), and what it deliberately left undone

WO-037 built a **Desk-local design system** — frappe-ui visual language, rendered natively in Topcoat (`view!` + a hand-authored token stylesheet, no Vue/SPA/second CSS toolchain):
- **`src/frust_ui.css`** — light+dark tokens **lifted verbatim** from frappe-ui's published design tokens (not guessed), plus component classes.
- **`src/frust_ui.rs`** — the Topcoat-native component set (`fui_button`, `fui_input`, `fui_textarea`, `fui_select`, `fui_checkbox`, `fui_badge`, `fui_card`, `fui_form_control`, `fui_list_row`, `fui_alert`, `fui_toast`, `fui_dialog`) + `/frust-ui.css` and `/ui-gallery` routes.
- **`NodeViewParts::Raw`** — the 5-line unescaped-markup wrapper (fed only compile-time constants) that is now **the standing pattern for inline SVG/icons** in the Desk.

**Deliberately undone** (criterion 5, held until the load work sealed):
1. It lives in an **isolated copy** `D:\Dev\rust\frust-desk-ui` (branch `frust-ui`, baseline `d876c87` → WO commit `b873906`), NOT in the shipped `frust-desk`.
2. It is **not wired into any Desk page** — the proof is a standalone `/ui-gallery` route, not the real list/form/workflow screens.
3. The interactions are **stubs**: Dialog is a static surface, Select is native `<select>`, Toast has no auto-dismiss.

## The named ceiling (WO-037 finding #2) — this WO's real risk

> "The behavior layer, not the aesthetic, is the real ceiling." The components *look* right server-side; the frappe-ui *interactions* — dialog open/close + focus-trap + escape, toast auto-dismiss/stacking, combobox typeahead — are genuine client behaviors.

WO-037 ranked three **Vue-free** paths, in order of preference — this WO holds that order:
- **(a) CSS-only** toggles (`:target` / `<details>` / checkbox-hack) for open/close — no JS at all.
- **(b) server round-trip** to a `?dialog=…` / `?q=…` state — natural for link-field options the kernel already holds.
- **(c) the six-verb client bridge / dynamic signals** (WO-014, ADR-001) for zero-round-trip open/close/typeahead.

None of it justifies a second UI runtime — that would breach ADR-004's headless boundary.

## Exit Criteria

1. **Re-skin the REAL Desk pages** — the shipped list, form, and workflow screens render in Frust UI (`fui_*` components + tokens), **wired into the live `frust-desk/src/main.rs`**, light+dark. **Browser-proven on real pages** — a real DocType list, a real form with real fields, a real workflow screen — **not the gallery** (the gallery was WO-037's proof; the tested-seam≠wired standing check bites here: prove it through `frust serve`/the browser, not a constructed route).
2. **Behavior layer, Vue-free, CSS-first** — the three interactions the Desk actually uses, each browser-proven:
   - **Dialog** — open/close (CSS-only preferred); focus-trap + escape-key as progressive enhancement (bridge only if must-have).
   - **Toast** — post-action feedback ("Saved"/"Submitted"), auto-dismiss + stacking.
   - **Combobox** — typeahead for **link fields** (native `<select>` stays fine for small enums; typeahead only where the option set is large — server round-trip is natural since the kernel holds the options).
   Take the **highest ranked path that works** for each (a → b → c); don't reach past CSS for what CSS does. **NAME any interaction none of the three paths can express** — that's a finding, possibly an ADR; STOP and report, do NOT reach for Vue/a second runtime.
3. **Zero behavior regression** — after the re-skin, everything still works in the browser: WO-038 admission (503 under overload), WO-032 SSE self-refresh, WO-031 workflow buttons (state→role transitions), WO-014 dynamic form (rule-driven fields), WO-015 line tables (child metadata), the WO-014 dirty-guard. The re-skin is *presentation*; it must not silently break *behavior* (assert the behavior, not just that the page renders).
4. **Merge + isolation discipline** — integrate onto the **current** `frust-desk/src/main.rs` (post-WO-038: semaphore/spawn_blocking, shared Agent, SSE), **not** the WO-037 baseline snapshot. Reconcile the `frust-desk-ui` copy back into the real tree and retire it (or state why it stays). `frust_ui.rs`/`.css` are **Desk-local** — confirm they live in `frust-desk`, never in the vendored `topcoat` tree (ADR-004: kernel/Topcoat stay lean; the Desk owns its skin).

## Boundaries

- **Presentation + interaction only** — no new data/behavior *semantics*, no kernel change, REST-only contract holds (ADR-004). The Desk stays a pure REST client.
- **Lazy-correct, ponytail-governed:** CSS-first, the bridge only where CSS/round-trip can't reach, **no second UI runtime, no new dependency, no component the Desk doesn't use.** Build the three interactions the shipped Desk needs, at the interaction depth it needs — not a general component-behavior framework.
- **Inter font** — WO-037 left it a Google Fonts `<link>` (progressive enhancement, system-ui fallback offline). Bundling it as a served asset is optional; do it only if offline-consistency is wanted, else leave the fallback and name the decision.
- **Light/dark** — the tokens already compose with no JS; keep that (a theme toggle, if any, is CSS/`data-theme`, not a runtime).

## Escalation

A frappe-ui interaction that **genuinely cannot** be expressed by any of the three Vue-free paths is an **architecture decision, not a workaround** — reaching for Vue would breach ADR-004. STOP, name the exact interaction and why each of (a)/(b)/(c) fails, and report; it may warrant an ADR (as ADR-011 did for realtime transport). WO-037 predicted BEHAVIOR is the ceiling — this WO tests that prediction; a real ceiling found is a *result*, not a failure.

**Related:** [[Frust Hub]] · [[WO-037 Frust UI Foundation]] · [[2026-07-28 WO-037 frust-ui foundation]] · [[ADR-004 Topcoat for Desk v0]] · [[ADR-001 Client Extensibility]] · [[Topcoat]] · [[2026-07-25 WO-009 Desk v1]] · [[2026-07-28 WO-031 desk workflow buttons]]
