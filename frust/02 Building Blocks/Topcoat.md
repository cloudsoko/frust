---
tags: [frust, building-block, topcoat, frontend]
status: adopted-desk-v0
created: 2026-07-23
---

# Building Block: Topcoat

> [!success] Status: **adopted for Desk v0** — all four prototype exit criteria passed 2026-07-23. See [[2026-07-23 Topcoat prototype]]. Still 0.x/experimental: pin the version, expect breaking changes, keep the core headless so it stays swappable.

## What It Is

[Topcoat](https://github.com/tokio-rs/topcoat) — a batteries-included Rust framework for full-stack reactive web apps, from the **tokio-rs org** (same team as Tokio/Axum). MIT licensed.

- **SSR + reactivity without WASM** — `view!` macro templates; `$( … )` expressions cross-compile a type-checked subset of Rust to JavaScript. No Leptos/Dioxus-style WASM bundle.
- **Signals** — client-side state, no server round-trip.
- **Shards** (`#[shard]`) — server components that auto-expose an API endpoint and re-render on the server, swapping only changed DOM.
- **Components query the DB directly** — async data fetching in components, `#[memoize]` for request-level dedup, auth logic in-component. Kills API boilerplate.
- **Module-based routing**, `asset!` content-hashed assets, Tailwind/htmx/Alpine AJAX integrations, Topcoat UI component library.
- Complements Axum (raw API endpoints) rather than replacing it.
- Roadmap: Toasty ORM integration, validations, email.

## Why It's Interesting for Frust

| Frust need | Topcoat fit |
|---|---|
| Desk UI without Frappe's jQuery legacy ([[Frappe Pain Points#7. Developer & Operator Experience|P-7.5]]) | Rust end-to-end UI, server-rendered, one language for the whole stack |
| Stack collapse philosophy (P-2.3) — same bet as [[SurrealDB]] | No Node build chain, no SPA framework, no separate API layer for the Desk |
| Metadata-driven forms/lists | SSR is the *right* rendering model: one generic form/list component renders any DocType from runtime metadata. WASM frameworks bake UI at compile time — wrong shape for us |
| Server-side permission enforcement in UI ([[SRS#3. Security & Fine-Grained Access Control|REQ-3.1.1]]) | Shards render on the server → field-level permissions applied before HTML leaves; nothing leaks to the client |
| Components + [[SurrealDB]] | Components query SurrealDB directly; `LIVE SELECT` → shard re-render is a natural realtime pipeline |

## Tensions / Open Questions
*Sharpened 2026-07-23 after a source-level review.*

- [x] **Runtime UI injection vs compile-time macros — the data/code boundary.** → **Decided:** tiered model, [[ADR-001 UI Extension Tiers]]. Topcoat has **no dynamic loading path**: `topcoat ui add` vendors component *source* into your tree; views are proc-macro expanded. So the honest line for [[SRS#2.2 Event Hooks & Extension Points|REQ-2.2.3]]:
	- Plugins contributing **data** (metadata, layouts, field configs) → fully runtime. One generic component walks DocType metadata and emits fields — "forms appear when you define a DocType" works with zero recompilation.
	- Plugins contributing **code** (novel widget types) → recompile-and-redeploy, or a WASM/iframe escape hatch.
	- ⚠️ **Decide now** which side REQ-2.2.3 actually needs: recompile-per-plugin is viable for a curated marketplace, **fatal for user-authored scripts**. → flagged in [[SRS#Open Requirement Gaps]].
- [x] **Reactivity ceiling — shard boundary = network round trip.** → **Measured:** 14–18 ms click-to-swap (500 rows, 24/page, loopback; ~0.4 ms server render). Fine for list views. Spreadsheet-style per-cell editing still unproven — revisit at the grid-bulk-edit screen.
- [x] **Maturity.** → **Confirmed the hard way:** 3 upstream bugs in one prototype day (see [[2026-07-23 Topcoat prototype#Upstream bugs found]]). Budget accordingly; pin the version.
- [x] **Coupling — the headless contract.** → **Held in practice:** prototype's metadata loader is a JSON file, swappable for [[SurrealDB]] without touching render code. The boundary *"here's DocType metadata + record JSON, render it"* is real.

## Prototype Exit Criteria — ✅ all passed 2026-07-23

1. [x] **Form whose DocType didn't exist at compile time** — `payment_terms` field added to JSON while server ran; rendered on reload, zero recompilation. REQ-2.2.3 Tier 1 settled empirically.
2. [x] **Dependent field via signals** — checkbox toggles Credit Limit both directions, zero network requests.
3. [x] **Grid via shard** — 14–18 ms click-to-swap, ~0.4 ms server render (500 rows, 24/page).
4. [x] **Server-side field permissions** — clerk's DOM contains no manager-only field/column, in initial render *and* every shard re-render (REQ-3.1.1).

> [!important] Design finding that outlives the prototype
> **Signals are compile-time items** — you cannot declare one per metadata field in a loop. A general `depends_on` graph needs one generic mechanism instead: simplest is re-rendering the form section through a shard on any driver-field change (~15 ms, acceptable). This directly constrains [[ADR-001 UI Extension Tiers]]'s Tier-2 six-verb bridge: the verbs must compile down to *generic* signal/shard operations, not per-field signals.

## Verdict

**Adopted for Desk v0.** Core engine stays headless; Topcoat is a renderer of `(metadata, record JSON)`, never a dependency of the engine. Prototype: `D:\Dev\rust\topcoat\examples\frust-proto` (~280-line `main.rs` + runtime `doctype/supplier.json`). Windows dev: `/OPT:NOREF` linker workaround; kill the server before rebuilds (exe lock breaks hot-reload).

## Upstream Roadmap Watch (reviewed 2026-07-24)

### 🎯 Load-bearing for Frust — these unblock named gaps/triggers
- **WebSockets** — ✅ **SHIPPED upstream (#195, pin moved 2026-07-25** — [[2026-07-25 Topcoat pin moved to upstream main]]**).** `WebSocketUpgrade` as an ordinary route extractor → kernel-owned session auth runs *before* the upgrade, our model unchanged. The REQ-6.5 push pipe exists; the realtime layer is now an ADR decision (queued behind WO-010): kernel-side per-session `LIVE SELECT` → websocket forward, gated on a live-query *scale* spike — the risk list's last unmeasured behavior. Polling remains the shipped posture until that ADR.
- ~~SSE / WebTransport~~ — superseded by shipped WebSockets.
- **(More) reactivity (`topcoat-runtime`) + Islands** — aimed directly at [[ADR-004 Topcoat for Desk v0]] revisit-trigger #1 (spreadsheet-grade screens) and could relax ADR-001's compile-time-signals constraint (runtime signals → per-field wiring might become possible).
- **Streaming SSR / Suspense + client-side navigation & prefetching** — attacks perceived latency on slow links; the corroborated 620 ms Slow-4G number is the benchmark any improvement gets measured against.
- **Validations** — display-layer only for us: REQ-1.2.2 validation truth lives in metadata/engine/DB `ASSERT`s. Useful if it can *render* our engine's validation errors nicely; rejected if it wants to *own* rules.

### 🙂 Conveniences — adopt passively
`topcoat new`, more UI components/blocks (Desk build velocity), deploy docs, image optimization, compression middleware.

### 🚫 Refuse — headless-contract violations (coupling temptations, P-7.5 reborn)
- **Better Toasty integration** — Toasty is an ORM; our data path is [[ADR-006 Plugin Capability Surface]] → SurrealDB. Desk components must never query through Topcoat's data story.
- **Authentication** — identity lives in the engine + SurrealDB `DEFINE ACCESS` (REQ-3.x). Topcoat auth would put a second identity system in the UI layer.
- **Background jobs** — `enqueue` (ADR-006) is the *only* job door; a Topcoat job system would fork REQ-6.3 semantics.
- **Emailing** — engine battery, not UI-framework battery.
- **OpenAPI endpoints** — our REST surface is engine-generated from metadata; Desk isn't an API host.
- Static export / sitemaps / pre-rendering — irrelevant to Desk; re-evaluate only if a public-website module ever exists.

> [!warning] Standing rule
> Each Topcoat release, re-sort its changelog into these buckets. The 🚫 bucket is the headless contract's early-warning system: the day Desk code touches Topcoat auth, ORM, or jobs, ADR-004's boundary has been breached.
> **Pin governance (added 2026-07-25):** moving the pin is a deliberate event — cause named, verification run (rebuild + browser smoke + suite), pre-move state branch-preserved, PM ratification. The 2026-07-25 move (websocket + our 3 fixes merged upstream, #168/#169/#170) met all four and is **ratified**; future moves get the PM ack *before* the rebase, not after. The **2026-07-28 move to v0.5.0** ([[2026-07-28 WO-029 topcoat v0.5.0 adoption]]) met all four: cause = WO-029; verification = 1125-pass suite + browser smoke (runtime.js + Wasm engine served live) + full rebuild; pre-move state on branch `pre-v050-20260728` (`b1e7039`); PM issued the WO before the rebase.
> **Carried patches (governance; ledger updated 2026-07-28 post-WO-029):**
> - ~~Dynamic signals / signal utilities~~ — **merged upstream** `17ec3f4` (#214), taken wholesale (its tests are a strict superset of ours); deleted from ledger
> - ~~#192 owned-string render-hang fix~~ — **merged upstream** `c8fb66f` (#201); deleted from ledger
> - **`push_str` widening** (NEW carried patch) — upstream #214 typed it `&StrSurrogate`, which rejects an OWNED `StringSurrogate` (`message.push_str(e.target.value)`); we keep `impl Deref<Target = StrSurrogate>`. Regression test `signal_methods.rs`. PR candidate — *this is the cost of adopting #214: it fixed one thing and narrowed another.*
> - **`ServiceUnavailableError` / `service_unavailable(retry_after_secs)`** (NEW carried patch, WO-038 2026-07-29 — `topcoat-router/src/error/service_unavailable.rs`) — Topcoat maps errors→statuses via a **closed downcast list** and had **no 429/503 constructor at all**, so an admission-shed error fell through to HTTP 500 (`shed:32464` answered as 500). A framework gap fixed in the trunk, not worked around in the Desk. Topcoat suite green (288+67). **Strong upstream-PR candidate** — every service needs a "busy" status, this isn't Frust-specific. *Finding banked: Topcoat's error→status mapping is a closed list; a bespoke error type silently degrades to 500.*
> - Six-verb bridge reference impl (`examples/frust-form-bridge`)
> - **`Js` response wrapper** (mirrors `Css` — single-binary no-asset-bundle posture with real signals; PR candidate)
> - **`Str::to_decimal_or_zero()`** + the **`Decimal` surrogate** (parse-without-arithmetic — the compare-never-compute money guarantee; browser `dist/index.js` must be rebuilt after any merge or the Rust side exposes `Decimal` with no browser impl; PR candidate)
> - **`Wasm` content wrapper** (WO-017 item 1 — `compileStreaming` requires `application/wasm`; without it the engine can't load; PR candidate, same shape as `Js`) — now **committed** (`b1e7039`), was working-tree-only before WO-029
> - Desk v1 moved OUT of the vendor tree (lives at `D:\Dev\rust\frust-desk`)
> Every carried patch is upstream-PR-or-recorded; unlisted divergence is drift. **Net after WO-029: 4 patches retired, 1 new (`push_str`), 1 promoted from uncommitted to committed.** **WO-038 (2026-07-29) adds `service_unavailable` — a genuine framework gap (no 503 constructor), strong PR candidate.** Real content divergence from upstream v0.5.0 is 18 files / +685 lines, all ours-by-design.
>
> **UPSTREAM TRACK (ruled 2026-07-31, one-time, parallel to WO-047):** runs in a **separate clone of `tokio-rs/topcoat` from origin/main — NEVER the vendored trunk at `D:\Dev\rust\topcoat` while WO-047 is in flight** (frust-desk path-depends on it; a mid-suite mutation contaminates the hygiene bundle's regression evidence). Priority order: **(1) upstream the carried patches** — highest value, zero new code, pure drift-reduction (every landed patch = one less hazard per pin move): `service_unavailable` first (no 429/503 constructor exists upstream at all), then `Js`+`Wasm` wrappers as a pair, `push_str` widening (tiny), `to_decimal_or_zero` + Decimal surrogate offered honestly (Frust-flavored, may not land — that's fine); ~~check PR #203's status~~ **ANSWERED (2026-07-31): #203 and #200 are both CLOSED — the dynamic-signals feature landed via #214 (merged 2026-07-27), which WO-029 already absorbed; there is NO live deletion-path PR and none is needed. Third stale ledger entry caught by this track.** **PROGRESS: PR #266 OPEN (service_unavailable, verified on GitHub)** — re-authored against current upstream idiom (module moved since v0.5.0, ratified over cherry-pick), with the regression on the *downcast mapping* not the type (mutation-tested: removing the arm reproduces WO-042's 500-for-503 symptom exactly). If merged → carried patch retires at next pin move. **429 ruled: separate tiny sibling PR, same idiom, not widening #266.** **QUEUE COMPLETE (2026-07-31, all 5 GitHub-verified OPEN): #266 service_unavailable (mutation 500→503) · #267 too_many_requests (mutation 500→429; merge-order overlap w/ #266 flagged in-body) · #268 Js+Wasm wrappers (media-type + no-parameters pins) · #269 push_str widening (compile-fail regression; root cause of the #214 slip named: upstream's test passed a *borrowed literal* — the only case the narrow signature accepts — so the owned path had zero coverage) · #270 Decimal + to_decimal_or_zero (offered honestly w/ three acceptable outcomes incl. decline; Rust↔TS comparators pinned by a SHARED comparison table asserted on both sides, so drift fails a test not a hydration mismatch).** Every carried patch now has an open upstream PR — each merge retires drift at the next pin move. **TRACK COMPLETE (2026-07-31): + #271 OPEN (`&&`/`||` thunk, ratified design — `OpKind::Logical` → `a.and(|| b)`/`a.and(() => b)` mirroring lazy `Bool::then`; the short-circuit `unwrap`-on-`None` case renders `false` instead of panicking, and it's the PR's motivating example) + the #187 advocacy comment posted (metadata-driven-Desk case; the `Some("")`→`None` semantic change named as changelog material; direction left to the maintainer). #271's test shape is the keeper: eager evaluation returns the CORRECT boolean, so result-only tests pass while semantics are wrong — the browser tests give the thunk a ran-flag, assert it did NOT run on short-circuit AND that it DOES run when it decides the result, all verified failing against an eager implementation first. NOT carried locally (arrives at a pin move). Posture now: MONITORING — merges retire patches at pin moves; the trunk-touch bundle (clippy fixes + stale comment) waits for the next legitimate touch.**
>
> **CAVEAT (banked 2026-07-31): a carried patch that never faces upstream CI accumulates lint debt invisibly.** `_decimal.rs` fails upstream clippy `-D warnings` with 3 findings (`map_or(true,…)`→`is_none_or`, 2 dropped `#[must_use]` in `should_panic` tests) — the vendored tree tolerated them because the fork only runs the checks the fork runs. Fixed in #270. **Trunk-touch queue (next legitimate `local-dev` touch, NOT mid-WO-047): (a) take the `_decimal.rs` clippy fixes downstream; (b) fix the stale async-closure workaround comment at the bridge example's `main.rs:205`.** **Ops fix en route (PM, 2026-07-31): the vendored trunk's WO-038 patch was UNCOMMITTED working-tree state (the WO-029 Wasm-wrapper hazard verbatim) — committed as `fa05b66` on `local-dev`; zero content change.** **(2) `&&`/`||` as a NEW upstream PR, thunk design ratified** — and **NOT carried locally**: we've lived with `else if` since WO-014, so it arrives at a pin move after upstream merges; a 7th carried patch for a papercut is drift we don't buy. **(3) #187 (empty form values → `Option<T>` 400s): HOLD + ADVOCATE** — the maintainer hasn't picked a fix direction, so building now risks the wrong shape; instead comment upstream with the strongest real-world case (a metadata-driven Desk where *every* optional field submits blank), and build only when a direction is blessed.

> [!done] v0.5.0 ADOPTED (WO-029, 2026-07-28). The review below (WO-025, 2026-07-26) held on every point **except one it could not have seen from the changelog** — a fifth breaking change.
> Upstream was **v0.5.0** (v0.4.0 base + our 9 commits). **All four *named* breaking changes (`895cded` layout Result, `9044ef0` bool attrs, `4b5650c` router error module, `a554e7a` AssetConfig arg order) were ALREADY in our tree** from the 2026-07-25 pin-to-main. **But v0.5.0 carried a FIFTH the release notes did not flag:** `topcoat-router` changed `mod content; pub use content::*;` → `pub mod content;`, so `Form`/`Js`/`Wasm` moved to `router::content::`. `frust-desk` adapted its import (didn't carry a patch to undo the namespacing). *Lesson: "already absorbed the breaking changes" is only ever "the ones upstream labelled" — the merge is the probe that finds the rest.*
> **The two redundant patches were dropped by taking upstream wholesale** — but only after checking upstream's replacements were **supersets**: its signal tests assert our three behaviours more strongly plus a fourth; its `write_in_browser_only()` replaces our repeated `panic!`. The one place upstream *narrowed* (`push_str` arg) became a new carried patch, caught by our own regression test.
> **Three roadmap features now IN:** SSE (#218 — ADR-011's missing push transport; retires the Desk polling loop, a v1.1 item); mail prototype (#216 — `topcoat-mail` compiles + tests green, a "batteries" head-start); improved asset linking (#217). **Adoption verified live:** the browser was served the rebuilt `runtime.js` (Decimal surrogate present) and the real 4.26 MB wasm engine via our `Wasm<T>` (`application/wasm`, valid magic bytes).

**Expression-vocabulary gaps (bridge findings 2026-07-25, re-verified 2026-07-31 by the upstream-track survey):** no `||`/`&&` in the `$()` language — **still true** (`$(a.get() && b.get())` → unsupported operator), workaround stays `else if`. **DESIGN RULING (2026-07-31): the vault's own `Bool.and/.or` sketch is WRONG and superseded** — `expr_binary.rs` compiles operators to *eager* method calls on both sides, so `a.and(b)` evaluates both operands where Rust's and JS's `&&` short-circuit; `$(opt.is_some() && opt.unwrap() > 0.0)` would panic on `None`. The correct shape is a **thunk** mirroring the already-lazy `Bool::then`: a third `OpKind::Logical` emitting `a.and(|| b)` / `a.and(() => b)`. Same size, correct semantics. *Honest payoff limit: `$()` is proc-macro expanded, so this never unlocks runtime-metadata `depends_on` — it clears the bridge's nested-`else if` papercut, ergonomic not architectural.* ~~async closures can't capture external signals~~ **STALE — retired 2026-07-31:** an async closure capturing a `Signal` from a runtime-built `Vec` now compiles and renders (likely fixed by #214); the bridge's workaround comment at the example's `main.rs:205` is stale (fix at next vendored-trunk touch, not mid-WO-047).

## Related

- [[Frust Hub]] · [[SurrealDB]] · [[Frappe Pain Points]] · [[SRS]]

## Sources

- [Announcing Topcoat (tokio.rs blog, 2026-07-22)](https://tokio.rs/blog/2026-07-22-announcing-topcoat)
- [github.com/tokio-rs/topcoat](https://github.com/tokio-rs/topcoat)
