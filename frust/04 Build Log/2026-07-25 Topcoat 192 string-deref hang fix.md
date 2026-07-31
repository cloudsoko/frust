---
tags: [frust, build-log, topcoat, upstream, fix]
created: 2026-07-25
---

# Build Log — Topcoat #192: Browser Hang on Owned-String Text Render (PR #201)

Operator-directed engagement on [tokio-rs/topcoat#192](https://github.com/tokio-rs/topcoat/issues/192) — an open, unclaimed browser-runtime hang, taken up hours after the signals-rework rejection ([[2026-07-25 Dynamic signals - upstream issue and PR]]) because it sits in the lane that stays open during the rework: **bug fixes in existing behavior**, the same category as our three merged fixes.

## The bug (reporter's diagnosis, verified in our clone byte-for-byte)

`toText` in the browser runtime unwraps ref-like values in a loop (`while (isRefLike(current)) current = current.deref()`), but the owned-string surrogate derefed to **itself** — so the first client-side re-render of any text expression producing an owned `String` (e.g. `$(string_signal.get())`, since `Signal.get()` returns an owned clone) spun the main thread forever. Tab frozen, permanently.

**Reproduced before fixing**: the issue's exact code, current `main`, Playwright click → the click never returned; the renderer process burned **136 CPU-seconds** in the loop before we killed it (parent-PID-verified as Playwright's renderer, not the user's Chrome).

## The fix (PR [#201](https://github.com/tokio-rs/topcoat/pull/201), option 1 of the reporter's three)

```ts
// before: always ref-like -> unwrap loop never exits
deref(): Str { return this; }
// after: mirrors Rust's Deref<Target = str>
deref(): Str { return new Str(this.v); }
```

Chosen over the symptom-guards (reorder `toText`'s checks; bail on same-object) because it fixes the cause: the Rust side of the surrogate pair already derefs `StringSurrogate → StrSurrogate` — the target type, never itself — so the JS mirror was simply unfaithful. Returning the borrowed form terminates *every* unwrap loop structurally, keeps compiled `*expr` derefs correct (the grammar maps unary `*` to `.deref()`; callers get a `Str` with the full borrowed vocabulary), and leaves `Signal.get()`'s `Ref.deref()` path untouched. Swept for siblings: `string.ts` was the only self-deref surrogate.

## Verification

- Two vitest regression tests (`src/surrogate/string.test.ts`): deref yields the borrowed form (a `Str`, not a `String`, not the same object), and ref-unwrapping an owned string terminates in bounded steps. Suite 3/3.
- `dist/index.js` rebuilt (`pnpm build`), committed as the repo does.
- End-to-end re-run of the identical repro on the fixed dist: click returns instantly, `Message: hello` renders and stays reactive.

## Status + local notes

- PR #201 **open**, CI green, `Fixes #192` — issue closes on merge. Remaining is maintainer review; deliberate omissions a reviewer might request (each trivial): defense-in-depth same-object bail in `toText`, manual changelog entry (their release-plz should pick up the `fix:` prefix).
- `local-dev` (and therefore the Desk) is unaffected either way — Desk v1 uses zero runtime features (the standing posture while the signal rework is in flight). The fix arrives with the next pin move once merged.
- Machine-local: pnpm 11 blocks esbuild's postinstall until `pnpm approve-builds --all`; approval lives outside the repo (nothing leaked into the PR). Fix branch: `fix/string-deref-hang` (local + fork).

## Related
[[Topcoat]] · [[2026-07-25 Dynamic signals - upstream issue and PR]] · [[2026-07-25 Topcoat pin moved to upstream main]]
