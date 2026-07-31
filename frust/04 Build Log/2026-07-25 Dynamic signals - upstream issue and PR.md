---
tags: [frust, build-log, topcoat, upstream, reactivity]
created: 2026-07-25
---

# Build Log — Dynamic Signals: the ADR-001 Constraint Is Dead (Upstream #198 / PR #200)

Roadmap engagement on the #1-ranked item, "(More) reactivity" — executed as spike → issue → PR, per the operator's direction.

## The finding (changes an ADR premise)

**"Signals are compile-time items" is false against current upstream.** The 2026-07-23 prototype's recorded constraint — one that shaped [[ADR-001 UI Extension Tiers]]'s Tier-2 bridge (generic shard re-renders, ~15 ms + RTT per dependent-field change) — does not hold. Source reading showed the substrate is fully dynamic (`SignalId` is a render-time UUID; the browser registry is runtime-keyed; expression captures serialize `&Signal<T>` per render), and only the `signal` *statement* is compile-time syntax. The value-level composition works today:

```rust
let signals: Vec<Signal<f64>> = fields.iter().map(|(_, d)| Signal::new(*d)).collect();
view! {
    for s in &signals { (SignalDeclaration::new(s)) }      // declare each to the browser
    for (i, _) in fields.iter().enumerate() {
        let sig = &signals[i];                              // view-`let` bridges into captures
        <button @click=$(|_e| sig.set(sig.get() + 1.0))>"+"</button> $(sig.get())
    }
    let a = &signals[0]; let b = &signals[1];
    <p>$(a.get() * b.get())</p>                             // cross-field dependency
}
```

**Browser-verified**: three runtime-metadata fields, per-field steppers, cross-field total — all reactive with **zero network requests** (2 requests total on the page: HTML + runtime script). The emitted markup shows the full mechanism: per-element `::topcoat::signal({id,v})` declarations, expressions hydrating by id (`cx.hydrate({"t":"Signal","id":…})`), one expression hydrating two collection signals.

## What went upstream (organic voice, no downstream context)

- **Issue [tokio-rs/topcoat#198](https://github.com/tokio-rs/topcoat/issues/198)** — "Creating signals dynamically (one per runtime collection item)": the generic use case (rows/form-builders from runtime data), the working pattern, the ask (intended API or accidental surface?), the offer.
- **PR [tokio-rs/topcoat#200](https://github.com/tokio-rs/topcoat/pull/200)** — docs + example + tests blessing the pattern, in repo style: a "Dynamic signals" section in the runtime guide (snippets are passing doctests), `examples/dynamic-signals` (order form, per-item steppers, cross-item total), and regression tests pinning declaration ids + expression hydration (`crates/topcoat-view/macro/tests/dynamic_signals.rs`, 2 tests, suite 74/74 green; the 5 pre-existing font/icon doctest failures on main are untouched/unrelated). Branch `feat/dynamic-signals` off `origin/main` — no Frust code near it.

## OUTCOME (same day): both filings closed by maintainer

> **#198**: "It's accidental, and the whole signal system will change so I'll close for now. We're reworking everything."
> **#200**: "…we want to completely change how signals work so this wouldn't be useful for long."

The pattern is officially **accidental surface inside a system being fully reworked**. The rejection is itself the highest-value output of the day: advance warning of a breaking-change wave through `topcoat-runtime`, straight from the maintainer. Public traces of the rework's direction: upstream branches `client-handle` ("Add Rust client handle" / "Add JS client handle" — a value-level handle API, plausibly the *intended* form of what we asked for) and `imperative-rendering` (prototype gutting the view runtime).

**Course correction:**
1. **Desk builds NOTHING on the dynamic-signals pattern.** ADR-001's shard-based Tier-2 bridge stays the ruling posture — shards are the stable primitive and survive the rework. The "constraint is dead" finding downgrades from ADR-amendment candidate to *watch item*: capability proven mechanically possible, API explicitly disowned.
2. **Reactivity contributions paused** (the ranked menu stands, but every item targets code inside the rework's blast radius — the "wouldn't be useful for long" rejection applies to all of it). Re-engage when `client-handle`/the rework surfaces for review; our use case is already on record in #198.
3. **Pin governance goes on alert** (ADR-004): "reworking everything" in the runtime means the next pin moves will carry breaking changes bigger than #183. The Desk currently uses zero runtime features — deliberately keep it that way until the rework stabilizes.
4. The regression tests and example remain on the local `feat/dynamic-signals` branch as documentation of the capability probe; not merged anywhere, wired into nothing.

## What this means for Frust (superseded by the outcome above — kept for the record)

1. **ADR-001's Tier-2 bridge premise needs a recorded amendment**: per-field client-side wiring for `depends_on` graphs is possible *now* — dependent fields need not cost a shard round-trip. The 620 ms Slow-4G pressure and the ADR-007 client/server split both shift.
2. **The ADR-007 boundary question sharpens** (flagged at the roadmap review): with per-field signals cheap, the line between "allowed client-side" (visibility/format toggles, derived *display* values) and "never client-side" (validation truth, stored-field computation) should be ruled *before* Desk v2 grows dependent fields.
3. Until #200 lands (or is redirected by maintainers), the pattern is unblessed upstream — our own regression tests ride the PR branch; if upstream stalls, the same two tests can be pinned in our clone.

## Housekeeping
- Desk/frust-proto: `topcoat/target` was cleaned for disk (6.8 GB of pre-rebase artifacts); the Desk binary needs one rebuild (`cargo build -p frust-proto`) before it serves again. Kernel `frust serve` on :8790 unaffected.
- `local-dev` restored (Desk commit intact); PR work isolated on `feat/dynamic-signals`.

## Related
[[Topcoat]] · [[ADR-001 UI Extension Tiers]] · [[ADR-004 Topcoat for Desk v0]] · [[ADR-007 Tier-2 Script Architecture]] · [[2026-07-25 Topcoat pin moved to upstream main]]
