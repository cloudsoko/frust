---
tags: [frust, work-order, desk, ui, design, parallel-track]
status: COMPLETE (2026-07-28) — Frust UI foundation delivered, Topcoat-native (no Vue). Tokens lifted verbatim from frappe-ui's published `espresso-v2-design-tokens` (light+dark, concrete hex); 12 `#[component]`s (button/input/textarea/select/checkbox/badge/card/form-control/list-row/alert/toast/dialog) + `/ui-gallery` proof route; `cargo build` clean, gallery 200 both modes, 20 inline SVGs zero-escaped. **Zero-conflict VERIFIED by PM: primary `frust-desk/src` untouched (only main.rs), all work in isolated `frust-desk-ui` copy, frust-kernel never a target.** Screenshot [[06 Attachments/wo037-frust-ui-gallery-light-dark.png]]. → [[2026-07-28 WO-037 frust-ui foundation]]

> [!important] The Topcoat-ceiling finding (the valuable one): **the aesthetic is fully expressible; the ceiling is the BEHAVIOR layer.** Dialog open/close + focus-trap, toast auto-dismiss, combobox typeahead, hover tooltips are genuine client behaviors — this foundation renders Dialog as a static surface and uses native `<select>`. Addressable **Vue-free** (CSS-only toggles / server round-trip / the six-verb signal bridge, WO-014 / ADR-001), but real work. It is a *behavior* gap, not an aesthetic one — which is exactly the ADR-004 revisit-trigger territory (spreadsheet-grade interaction), now with a concrete list. Also banked: `view!` escapes interpolated text, so inline SVG needs a trusted-raw wrapper (`push_str_unescaped`) — the standing icon pattern, no asset bundle.
created: 2026-07-28
---

# WO-037: Frust UI Foundation (frappe-ui aesthetic, Topcoat-native)

> [!info] PM work order — a **parallel design track**, deliberately isolated so it cannot conflict with WO-035 (Desk load) or WO-036 (kernel). Governing: [[ADR-004 Topcoat for Desk v0]] (headless — the Desk is a swappable renderer; a design layer never touches the contract), [[Topcoat]] (Tailwind-based; carried-patch discipline), WO-012 (Desk lives at `frust-desk`, vendor tree = runtime patches only).

## The decision (PM, 2026-07-28)

**Frust UI = a Desk-local design system, Topcoat-native, frappe-ui-*inspired*.** Rejected: (a) the actual Vue `frappe-ui` — reintroduces the Node/SPA polyglot stack (P-2.3, P-7.5) the whole Desk thesis kills; the kernel being headless *permits* a Vue frontend but we don't want a second UI runtime. (b) building it *inside* vendored Topcoat UI — every change fights the rebase (carried-patch ledger); the Desk is already out-of-tree per WO-012. So: a `frust-ui` module in `frust-desk`, consuming Topcoat, owned by us. We take frappe-ui's **design language**, not its framework.

## Conflict-avoidance boundary (non-negotiable — a parallel agent owns WO-035/036)

- **Separate git worktree of `frust-desk`** (`git worktree add ../frust-desk-ui -b frust-ui`) — all code work in the worktree, never the primary working dir.
- **Additive only:** new files for the `frust-ui` module + one new standalone `/ui-gallery` route. **ZERO edits to existing Desk page handlers** (WO-035's load target) and **ZERO touch to `frust-kernel`** (WO-036).
- Vault: this WO's build log is a new dated file (no conflict with the primary track's separate files).

## Scope — FOUNDATION, not a re-skin

1. **Design tokens** (frappe-ui language): neutral gray scale + brand accent, type scale (Inter), spacing, radii (small), subtle shadows — as a Tailwind preset / CSS-variable file, **light + dark**. Use `extract-design` / `frappe-ui` skills to source the real tokens; use `frust-desk`'s existing Tailwind/asset pipeline (don't add a second one).
2. **Core components** in Topcoat's `view!` macro (study existing `frust-desk` components for the pattern): Button (primary/secondary/ghost/danger + sizes), Input, Textarea, Select, Checkbox, Badge, Card/Panel, FormControl (label+field+error), ListRow, Dialog/Modal, Alert/Toast — styled to frappe-ui.
3. **Gallery page** (new standalone route) rendering every component in light + dark — the committed, screenshot-able proof.

## Exit Criteria

1. Tokens defined (light + dark), sourced from the real frappe-ui language.
2. The core component set renders in Topcoat; gallery page shows all of them.
3. **Builds clean** (`cargo build` in the worktree); gallery renders (screenshot if a browser is drivable).
4. **Zero conflict proven:** all changes on the `frust-ui` branch/worktree, additive-only, `git status` on the primary `frust-desk` dir + `frust-kernel` unchanged.
5. **NOT wired into existing Desk pages** — that's a deliberate follow-on WO *after* WO-035 seals, so the re-skin doesn't collide with the load test.

## Boundaries

- Foundation, not full Desk re-skin. No Vue, no Node SPA, no new heavy deps. No vendored-Topcoat-tree edits. Machine caveats: pnpm not npm; Windows `/OPT:NOREF` linker + kill-server-before-rebuild; no `*install*`-named binaries (os-740).

## Escalations

If the frappe-ui aesthetic can't be expressed in Topcoat's `view!` + Tailwind without a real gap (a component that genuinely needs client reactivity beyond the six-verb bridge), name it — that's a finding about the Topcoat ceiling, not a reason to reach for Vue.

**Related:** [[Frust Hub]] · [[ADR-004 Topcoat for Desk v0]] · [[Topcoat]] · [[2026-07-25 WO-009 Desk v1]] (the current Desk) · frappe-ui / ui.frappe.io
