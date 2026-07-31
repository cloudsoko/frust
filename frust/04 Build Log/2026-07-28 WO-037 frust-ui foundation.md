---
tags: [frust, build-log, desk, ui, design, wo-037]
date: 2026-07-28
wo: WO-037
status: DONE (foundation delivered; not wired into Desk pages by design)
---

# WO-037 — Frust UI Foundation (frappe-ui aesthetic, Topcoat-native)

> Parallel design track, deliberately isolated from WO-035 (Desk load) / WO-036
> (kernel). All code work in an isolated copy; the vault log is a new file.
> Governing: [[ADR-004 Topcoat for Desk v0]], [[Topcoat]], WO-012.

## Outcome

A Desk-local design system — **frappe-ui visual language, rendered natively in
Topcoat** (`view!` + a hand-authored token stylesheet). No Vue, no SPA runtime,
no second CSS toolchain. Builds clean, serves, and renders both light and dark
in one browser proof.

**Exit criteria — all met:**
1. Tokens (light + dark) sourced from the **real** frappe-ui language. ✅
2. Core component set renders in Topcoat; a gallery shows all of them. ✅
3. `cargo build` clean; gallery renders; screenshot captured. ✅
4. Zero conflict proven (below). ✅
5. NOT wired into existing Desk pages (deliberate follow-on after WO-035 seals). ✅

## Isolation mechanism (frust-desk is not a git repo)

The WO assumed a `git worktree` of `frust-desk`, but `frust-desk` is **not** a
git repository (only `topcoat` is). Honored the *intent* instead: made an
isolated working copy at **`D:\Dev\rust\frust-desk-ui`** (source only, no
`target/`), `git init` + branch `frust-ui`, committed a baseline snapshot, then
did all work there. The `../topcoat` path dep resolves the same from the copy,
so nothing in the shared tree was touched.

## What was built

Two new files in the copy + one additive line:

- **`src/frust_ui.css`** — design tokens (light + dark) as CSS custom
  properties, plus component classes. Tokens are **lifted verbatim** from
  frappe-ui's published design tokens (`frappe/frappe-ui`
  `espresso-v2-design-tokens/`: `Colour primitives` + semantic `Styles.Light` /
  `Styles.Dark`), resolved to concrete hex — **not guessed**. Key anchors:
  - surface-base `#ffffff` / `#171717`; ink ladder gray-9…gray-4
    (`#0f0f0f…#999999` light, `#f8f8f8…#575757` dark); outline hairlines
    `#ededed / #e2e2e2 / #c7c7c7`.
  - brand accent blue `#0d8ef8` (blue-500) / hover `#077ddf`; feedback
    red `#e03434`, green `#268c5c`, amber `#ca7e0c`, each with tinted
    surface + outline for banners/badges.
  - type scale 11–20px (Inter), tight line-heights for labels; radii
    sm 4 / control 6 / card 8 / dialog 12; three subtle shadow tiers.
  - Theming is compositional: tokens live on `:root`/`[data-theme="light"]`
    and `[data-theme="dark"]`, so a light panel and a dark panel sit side by
    side on one page with **no JS**.
- **`src/frust_ui.rs`** — the Topcoat-native component set + routes:
  - `#[component]`s: `fui_button` (primary / secondary / ghost / accent /
    danger × sm/md/lg, optional leading icon, block, disabled), `fui_input`,
    `fui_textarea`, `fui_select` (options via child slot), `fui_checkbox`,
    `fui_badge` (gray/blue/green/red/amber × subtle/solid/outline, dot, pill),
    `fui_card` (header + actions slot + body child), `fui_form_control`
    (label / required / description / error + control slot), `fui_list_row`
    (avatar + title + meta + trailing slot), `fui_alert` (info/success/
    warning/danger with icon), `fui_toast`, `fui_dialog` (surface).
  - `#[route(GET "/frust-ui.css")]` — serves the stylesheet via Topcoat's
    `content::Css` (`text/css`), embedded with `include_str!` (Desk's
    no-external-asset posture holds).
  - `#[route(GET "/ui-gallery")]` — the committed proof: a **standalone full
    document** (its own `<head>` + stylesheet link) rendering every component
    twice, light and dark, side by side.
- **`src/main.rs`** — one additive block: `mod frust_ui;` (+5 lines incl.
  comment). No existing `#[page]`/`#[route]` handler touched. Route/component
  discovery is `inventory`-based (`.discover()`), so the new module is picked
  up with zero edits to the router wiring.

## Aesthetic decisions

- **Primary button = solid gray** (frappe-ui's documented canonical primary,
  DESIGN.md rule 4), which **auto-flips** high-contrast: near-black on light,
  near-white on dark (bound to the ink-9/surface-base pair). The **brand blue
  is a distinct `accent` variant** so the accent is visibly showcased without
  overriding frappe's gray-first primary.
- **Gray-first, color-encodes-meaning** throughout (DESIGN.md principle 1):
  badges/alerts are the only saturated surfaces; everything else is ink-on-gray.
- Gallery is a `#[route]`, **not** a `#[page]` — layouts wrap pages by path
  prefix, so a page would inherit the Desk's `root_layout` chrome + inline body
  styles. A route is unwrapped → clean canvas, own `<head>`.
- Minor documented deviation: control radius 6px (skill documents 8 for
  buttons/inputs). Chosen for a crisper Desk look; still within "small radii".

## Build + proof

- `cargo build` (isolated copy): **clean**. The only 2 warnings are
  pre-existing dead-code in `main.rs` (`DocField.fetch_from`, `FetchFrom`),
  not from the new module.
- Served on `PORT=3097` (distinct from the parallel agent's default 3000).
  `/frust-ui.css` → 200 `text/css` 24 KB; `/ui-gallery` → 200 `text/html` 20 KB.
- Rendered HTML check: 1 light pane + 1 dark pane, 20 real `<svg>` icons,
  **0 escaped `&lt;svg`** (the trusted-raw-markup path works), all component
  classes present. Only console message: a favicon 404 (harmless).
- Screenshot (light + dark, full page):
  `06 Attachments/wo037-frust-ui-gallery-light-dark.png`.

## Finding — the Topcoat ceiling (named, not a reason to reach for Vue)

The **aesthetic** — every token, every component's *look* — is fully expressible
in `view!` + CSS. Two seams are worth banking:

1. **`view!` escapes interpolated text and `(expr)` node content**, so inline
   SVG / any raw markup cannot be passed as a string (camelCase `viewBox`,
   `<path/>` would be escaped — the same class of trap as WO-014's import map).
   Solved with a 5-line `NodeViewParts` wrapper (`Raw`) calling
   `push_str_unescaped`, fed only compile-time icon constants. **This is the
   standing pattern for icons/SVG in the Desk** — no Vue, no asset bundle.

2. **The behavior layer, not the aesthetic, is the real ceiling.** The
   components render perfectly server-side, but the frappe-ui *interactions* —
   dialog open/close + focus-trap + escape-key, toast auto-dismiss/stacking,
   combobox typeahead, hover tooltips — are genuine client behaviors. In this
   foundation the Dialog is rendered as a **static surface** and Select uses the
   native `<select>`. A follow-on that wants those behaviors has three
   Vue-free options, in order of preference: (a) CSS-only toggles
   (`:target` / `<details>` / checkbox-hack) for open/close; (b) a server
   round-trip to a `?dialog=…` state; (c) the **six-verb client bridge / dynamic
   signals** (WO-014, ADR-001) for zero-round-trip open/close/typeahead. This is
   a *behavior* gap, not an aesthetic one — naming it per the WO's escalation
   clause. None of it justifies a second UI runtime.

## Zero-conflict proof

- All code changes are commits on branch `frust-ui` inside
  `D:\Dev\rust\frust-desk-ui` (baseline `d876c87`, WO commit `b873906`;
  3 files: 2 new + `main.rs` +5).
- **Primary `D:\Dev\rust\frust-desk\src\main.rs`**: sha256 (CR-normalized)
  `de364148…c7d00` — **identical** to the WO-start snapshot. The WO-035 load
  target was never edited. Primary `src/` still contains only `main.rs`
  (my `frust_ui.rs`/`.css` exist solely in the copy). `Cargo.toml`/`Cargo.lock`
  identical.
- **Shared `topcoat` tree** (a git repo): `git status` clean — I made no
  changes to the vendored dependency.
- **`frust-kernel`** (WO-036): never a write target of this agent. Every write
  went only to `D:\Dev\rust\frust-desk-ui` and this vault (new files only).

## Follow-on (out of scope here, per WO)

- Re-skin existing Desk pages onto Frust UI — **after WO-035 seals** so it
  doesn't collide with the load test.
- Interactive behavior layer (dialog/toast/combobox) via CSS-only or the
  six-verb bridge (see finding #2).
- Bundle Inter (InterVar) as a served asset rather than the Google Fonts
  `<link>` (currently a graceful progressive enhancement; falls back to
  system-ui offline).

**Related:** [[WO-037 Frust UI Foundation]] · [[ADR-004 Topcoat for Desk v0]] ·
[[Topcoat]] · [[2026-07-25 WO-009 Desk v1]] · frappe-ui / ui.frappe.io
