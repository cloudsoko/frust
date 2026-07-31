---
tags: [frust, build-log, desk, ui, design, topcoat, milestone-4]
created: 2026-07-29
status: COMPLETE — the shipped Desk wears Frust UI; the three behaviours land on rung (a)/(a)/(b) with ZERO JS added; 18 WO-031 + 8 WO-032 checks pass and the shed still fires (135 × 503). The predicted ceiling was REAL and is named: focus-trap + escape.
work-order: "[[WO-042 Frust UI Re-skin]]"
---

# Build Log — WO-042: Frust UI Re-skin + Behaviour Bridge

WO-037 solved the aesthetic in an isolated copy and deliberately never wired it
in. This makes it what `frust serve` renders, builds the three interactions
WO-037 named as the ceiling, and **finds that the predicted ceiling is real** —
in one specific, nameable place.

## 1. The re-skin, on the real pages

`frust_ui.rs` + `frust_ui.css` moved into `frust-desk/src/` (**Desk-local** —
verified nothing named `frust_ui*` exists anywhere in the vendored `topcoat`
tree, per ADR-004) and merged onto the **current** post-WO-038 `main.rs`, not
WO-037's baseline: the semaphore, `spawn_blocking`, shared `Agent` and SSE are
all intact underneath.

Re-skinned: **layout/nav**, **login**, **home/DocType directory**, **list**,
**new-record form**, **record/workflow page**, and every **field**. Browser-
proven on real pages with the real dev fixture — `purchase_order` (the WO-002
six-row dataset), `sales_invoice` under the `invoice_approval` workflow — not
the gallery.

The gallery route survives as WO-037's component proof and still renders.

### The component set had to grow to meet real pages

WO-037's components were built for a gallery, and it showed the moment they met
a form:

| gap | why it mattered |
|---|---|
| every `fui_*` was **private** | the gallery was in-module; real pages are not |
| `fui_button` was always `type="button"` | a form cannot submit |
| no `href` | the Desk's actions are navigations, not JS handlers |
| no `name`/`value` | the record page dispatches on `action=save\|submit\|cancel` |
| `fui_input` had no `required`, no `list` | validation and typeahead |
| `href`/`value` were `&str` | every real destination is a `format!` |

**Four token gaps, found by rendering rather than reading.** The chrome I wrote
referenced `--fui-space-*` and `--fui-font-sans`, **neither of which existed** —
so the first render came back in Times New Roman with collapsed spacing. Added a
4px spacing scale (matching the component paddings already in the file) and an
alias to the one real font token, rather than redeclaring the stack twice.

**And a variant that never existed.** I used `variant: "solid"` in eight places;
the real set is `accent | primary | secondary | ghost | danger`. `fui-btn--solid`
matched no CSS, so every primary button silently rendered as base-class-only.
Corrected to `primary`. A class name that does not exist fails *silently* in
CSS — which is exactly why the re-skin had to be looked at, not just compiled.

## 2. The behaviour layer — and the ceiling, named

The ranked order held. **No JavaScript was added for any of the three**; the
only script on a re-skinned page is WO-014's pre-existing `runtime.js`, and the
home page ships **zero scripts and zero inline handlers** (asserted in-browser).

### Dialog — rung (a), CSS-only via `:target`

Open is a link to `#id`, close is a link to `#`. Proven in-browser:
`display: none → flex` on target, `none` again after Cancel, `scripts_on_page: 0`.

**Free win worth naming:** the Back button closes it, because open-ness is a
history entry. A checkbox-hack has neither that nor reload-safety.

> ### ⚠ THE CEILING IS REAL — focus-trap and escape-key
>
> WO-037 predicted behaviour would be where Topcoat hits its limit. It is, and
> here is the exact interaction: **modal focus containment and Escape-to-close.**
>
> - **(a) CSS-only — cannot.** There is no selector for "keyboard focus is
>   leaving this subtree" and no CSS keyboard handler. `tabindex` juggling can
>   *reorder* focus but cannot *contain* it, and a trap that leaks is worse than
>   none because it claims a guarantee it does not keep.
> - **(b) server round-trip — cannot.** Both are sub-frame keyboard concerns;
>   there is no request to make. A round trip per keystroke would be absurd even
>   if it worked.
> - **(c) six-verb bridge — *could*, and I did not reach for it.** It would take
>   a keydown handler plus focus-cycle logic — i.e. shipping a client runtime to
>   the whole Desk for one modal's keyboard ergonomics, on a page that currently
>   ships **zero** scripts.
>
> **What exists natively and why it is not free:** `<dialog>` + `showModal()`
> gives focus-trap, Escape, and inert-background *correctly* — but
> `showModal()` is a JS call. The capability is one line of JS away and zero
> lines of CSS away.
>
> **Reported, not worked around.** This is an ADR-shaped question (as ADR-011
> was for realtime transport): *is one narrowly-scoped progressive-enhancement
> script an acceptable price for native modal semantics, or does the
> zero-script property win?* Not mine to rule. Today the dialog is
> mouse-and-Back-button complete and keyboard-incomplete, and it says so here
> rather than pretending.

### Toast — rung (a), CSS-only auto-dismiss + stacking

Flash messages now render as toasts. Stacking is flex-column; auto-dismiss is
one keyframe with `animation-delay: 6s` and `fill-mode: forwards` — **no timer,
no JS**. Asserted in-browser: `animation-name: fui-toast-in, fui-toast-out`,
`delay: 0s, 6s`, `fill: none, forwards`.

`prefers-reduced-motion` disables the animation **including the auto-dismiss** —
text that vanishes on a timer is precisely what that preference is about.

Proven with a real refusal: clerk1 attempting a manager-only `Approve` produced

> **Couldn't do that** — "'Approve' from 'Submitted for Approval' requires role
> manager; you are 'clerk'"

…which also re-proves WO-031's denial prose survives the re-skin verbatim.

### Combobox — rung (b), server round-trip typeahead

Link fields get a `<datalist>` populated by a kernel read on render. Rung (a) is
impossible (CSS cannot filter typed text); rung (c) would buy a client runtime
to save one round trip. **This is the one interaction that legitimately needed
(b)** — exactly where WO-037 predicted it would land.

**The label field comes from metadata, not a guessed name.** The first cut
probed `name`/`title` and fell back to the record id — which on the seeded
`customer` doctype (whose field is `cust_name`) would have offered
`customer:qfmpdzax2shlte1flelt` in the dropdown: technically correct and
useless, exactly what my own fallback comment warned against. It now asks the
target DocType for its first `Data` field.

Proven in-browser on a real Link field: 4 real customer names, `input[list]`
wired to the `<datalist>` id, only `runtime.js` on the page. The option set is
capped at 50 and **the UI says so when the cap is hit** — a silently truncated
option list is a combobox that lies about what exists.

## 3. Zero behaviour regression — asserted, not assumed

The committed suites, re-run against the re-skinned Desk:

| suite | result |
|---|---|
| **WO-031 workflow** (`workflow.spec.mjs`) | **18/18 PASS** — full clerk→submit→manager→approve→reject→reopen cycle |
| **WO-032 SSE** (`sse.spec.mjs`) | **8/8 PASS** — one stream, zero polls, out-of-band refresh, zero-leak, poll fallback |
| **WO-038 admission** (`shed-check.mjs`, new) | **135 × 503**, `shed` counter moved 0→135 (exact match), `Retry-After: 2`, 65 served against the 64 bound |

Plus in-browser assertions: WO-015 line tables (`lines.0..4` ×
item/qty/rate/amount/`__remove`), WO-014 dirty guard (`dirty-note` banner
logic), WO-014 reactive form (signal bindings preserved), light/dark
(`rgb(248,248,248) → rgb(31,31,31)`, `prefers-color-scheme` rule present in the
served sheet), and row permissions (clerk1 sees **3** `purchase_order` rows
where manager sees **6**).

### Two instrument failures caught, both mine

**The browser cannot prove the admission shed.** My first attempt fired 300
concurrent `fetch`es in-page and read `shed: 0` — which reads exactly like "the
re-skin broke admission". It did not: Chrome caps ~6 connections per host, so
300 fetches serialise and never put 64 in flight. I was measuring the
instrument. `shed-check.mjs` uses a Node agent with `maxSockets: Infinity`, the
same shape WO-038's driver used, and the shed fires immediately.

**Six workflow checks failed on a stale selector, not a broken behaviour.** The
suite read the state chip via `h1 span`; the re-skin moved it to a sibling
inside `.fui-page-head`. So it returned `""` for every state while *every
transition still worked* — the buttons, the role filtering, the refusal prose
all passed. Fixed the selector, and deliberately **did not** add an `h1 span`
fallback: a selector that matches two generations of markup cannot tell you the
next re-skin moved it.

## 4. The carried Topcoat patch

**Topcoat had no 429/503 constructor at all**, and its error→status map is a
**closed downcast list** — so a Desk-local `Busy` with its own `IntoResponse`
rendered as **HTTP 500**. That was WO-038's bug; WO-042 fixed it in the right
place rather than around it.

`ServiceUnavailableError` + `service_unavailable(retry_after_secs)` added to the
vendored trunk (2 files: the new module + the module list & downcast line).
Topcoat's own suite stays green — **288 + 67 passed**. Strong upstream-PR
candidate: every service needs a busy status, and nothing about it is
Frust-specific.

## 5. Merge + isolation discipline

`frust-desk-ui` is **retired, not deleted**, and the reasoning is recorded in a
`RETIRED.md` in that tree. The drift risk the clause exists to close — two live
Desks — is closed: the tree is marked dead and its content is superseded (the
real tree's copies have already evolved past it).

It still exists because **`frust-desk` and `frust-kernel` are not under version
control** (only `topcoat` is), so its two commits are the only versioned record
of the foundation's introduction. `target/` (1.01 GB) removed; 4.5 MB of source
plus `.git` remains. If the real tree is ever put under git, delete it outright.

## 6. Findings for the board

1. **The predicted ceiling is real and precisely one interaction wide** —
   modal focus-trap + Escape (above). Everything else WO-037 flagged reached
   rung (a) or (b) with no JS.
2. **The evidence harness had been silently unrunnable.** `pnpm workflow` and
   `pnpm sse` could not start: `node_modules/playwright` was a junction to
   `D:\Dev\rust\wf-proof\…`, the directory name from **before WO-039 renamed it
   to `frust-e2e`**. WO-039 kept the installed Playwright to avoid a re-download
   and the link never got repointed. Repaired against the local store; both
   suites now run. The README's own rule — *"evidence you cannot re-run is an
   anecdote"* — had quietly stopped being true for the two browser suites.
3. **A CSS class name that does not exist fails silently.** `fui-btn--solid`
   compiled, rendered, and looked *nearly* right. There is no type system on the
   class-name seam between Rust and CSS; the only guard is looking at it.

## Dev-store mutations (stated, not hidden)

The browser proofs ran against the **dev store**, as WO-020/WO-031 did before
them, and left real changes: a `wo042_order` DocType (a Link field was needed to
prove the combobox and no seeded doctype had one), four `customer` rows
(`Acme Corp`, `Beta LLC`, `Cyan Industries`, `Delta Partners`), and workflow
state advanced on the `sales_invoice` documents the suites drive.

## Files

`frust-desk/src/frust_ui.rs` (merged in, component set made public + extended) ·
`frust-desk/src/frust_ui.css` (spacing scale, font alias, `prefers-color-scheme`,
app chrome, the three behaviour layers, read-only field) ·
`frust-desk/src/main.rs` (layout, login, home, list, form, record, `field_row`,
`field_input`, `link_options`) ·
`topcoat/crates/topcoat-router/src/error/service_unavailable.rs` (**new, carried
patch**) + `error.rs` · `frust-e2e/shed-check.mjs` (new) ·
`frust-e2e/workflow.spec.mjs` (badge selector) · `frust-desk-ui/RETIRED.md` (new)

## Related
[[WO-042 Frust UI Re-skin]] · [[WO-037 Frust UI Foundation]] ·
[[2026-07-28 WO-037 frust-ui foundation]] (the predicted ceiling, now tested) ·
[[ADR-004 Topcoat for Desk v0]] · [[ADR-001 Client Extensibility]] (rung (c),
not reached) · [[Topcoat]] (the carried patch) ·
[[2026-07-29 WO-038 desk admission control]] · [[2026-07-28 WO-031 desk workflow
buttons]] · [[2026-07-28 WO-032 SSE retire polling]] ·
[[2026-07-28 WO-039 multi-db tenancy probe]] (`frust-e2e`'s rename)
