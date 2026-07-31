---
tags: [frust, build-log, topcoat, vendor, reactivity]
created: 2026-07-25
---

# Build Log — Topcoat Vendored; Signal Utility Methods (#175)

Two decisions land together: **Topcoat is now a vendored fork we own**, and issue [#175](https://github.com/tokio-rs/topcoat/issues/175) (typed signal mutators) is implemented into it — not PR'd.

## The pivot: vendor, don't contribute

Operator call, and the day's evidence backs it: the upstream signal system is being fully reworked (maintainer closed #198/#200 as "reworking everything"), so upstream contribution to the runtime is low-yield right now. We fork, own the trunk, move at our own pace, and adopt upstream's creative advances occasionally rather than continuously.

- **Remote `vendor`** = `github.com/AmeinEskinder/frust` (private); `local-dev` pushed as `main` and tracks it.
- **`origin`** stays `tokio-rs/topcoat` — the well we dip into for their advances (websocket #195, wasm bundling #199, etc.).
- **Default posture: fix locally, no upstream PRs** unless directed. (The earlier #201 fix-branch for #192 remains open on our old PR-fork, but the fix itself now lives in our vendored trunk regardless.)
- Vendor trunk `main` = upstream main + Frust Desk v1 (WO-009) + fix #192 + feat #175.

## #175: signal utility methods

Typed convenience mutators on signals, implemented on **both sides of Topcoat's language boundary** (the surrogate pattern: a Rust type + its TS mirror, kept in sync):

| Method | Signal type |
|---|---|
| `toggle()` | `Signal<bool>` |
| `increment()` / `decrement()` | `Signal<f64>` |
| `push_str(...)` | `Signal<String>` |

- **Rust** (`crates/topcoat-runtime/src/surrogate/signal.rs`): methods on the type-specialized `SignalSurrogate<T>` impls, each `panic!`-bodied for server-side (writes only run client-side — same contract as the existing `set`). `push_str` takes `impl Deref<Target = StrSurrogate>` so borrowed literals **and** owned strings (e.g. `e.target.value`) both pass — the expression language has no `&` operator.
- **Browser** (`crates/topcoat-runtime/browser/src/surrogate/signal.ts`): `WriteSignal` gains the four methods, each reading maverick's current value, applying the surrogate op, writing back. `dist/index.js` rebuilt.
- **Docs**: runtime guide + `expr.md` vocabulary list.

## Verification

- Rust view-macro translation tests (`crates/topcoat-view/macro/tests/signal_methods.rs`, 2): the methods compile in `$(...)` expressions and emit `.toggle()`/`.increment()`/`.decrement()`/`.push_str(` in the generated JS; `push_str` accepts `e.target.value`. Full macro suite 74/74.
- Browser vitest (`signal.test.ts`, 3): `toggle` flips, `increment`/`decrement` step by one, `push_str` appends. Suite 6/6 (with the #192 tests).
- **End-to-end in a real browser** on the merged trunk: `toggle` false→true, `increment`/`decrement` 10→11 / →2, and `push_str` building `hi!!!!` — the String-render path that **froze the tab pre-#192**, now smooth because both changes ride the same trunk. Confirmed no hang, reactive throughout.

## Notes for next time (in [[Topcoat]] memory)
- Browser bundle builds with pnpm here (`pnpm approve-builds --all` once for esbuild); upstream tracks `yarn.lock`, so don't commit pnpm lockfiles — just rebuild and commit `dist/index.js`.
- Rust `view!` translation tests need `cx =>` as the first token.
- The playwright renderer freezes hard on any pre-#192 String-render bug; kill the frozen *renderer child of the playwright chrome root* by CPU, never the user's own Chrome.

## Related
[[Topcoat]] · [[2026-07-25 Topcoat 192 string-deref hang fix]] · [[2026-07-25 Dynamic signals - upstream issue and PR]] · [[ADR-004 Topcoat for Desk v0]]
