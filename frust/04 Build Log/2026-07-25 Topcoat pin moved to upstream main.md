---
tags: [frust, build-log, topcoat]
created: 2026-07-25
---

# Build Log — Topcoat Pin Moved to Upstream main (websocket lands)

**A deliberate pin event per ADR-004, operator-ordered.** `local-dev` rebased onto tokio-rs `origin/main` (19 commits). Pre-rebase state preserved on branch `local-dev-pre-ws`.

## What came in

- **`feat: websocket support (#195)`** — the push transport. `WebSocketUpgrade` is an ordinary route extractor: it composes with `cookies(cx)`/session checks *before* upgrading, so the Desk's kernel-owned auth model carries over unchanged; feature-gated (`websocket`); message limits + subprotocols built in. **This trips WO-009's boundary clause** ("polling until Topcoat ships push"). Adopting push for Desk list refresh is now unblocked — a PM/boundary decision, deliberately NOT built in this pass.
- **Our three Windows fixes are upstream** (#168 bool surrogates, #169 dev exe lock, #170 MSVC asset stripping) — the rebase auto-dropped the local patches; `local-dev` is now `origin/main` + exactly one commit (Desk v1, now committed: `feat(frust-proto): Desk v1 …`). The `/OPT:NOREF` workaround era is fully closed.
- Also relevant: customizable page HTTP methods (#181 — could merge the Desk's `/login` GET + `/login-submit` POST split), tower service mounting (#184), wasm builds (#191).

## Breaking changes absorbed

- `refactor!: dedicated router error module (#183)` — `redirect`/`see_other`/`bad_request`/`internal_server_error` moved to `topcoat::router::error`. One import block in the Desk.
- `feat!: boolean attribute behavior (#179)` and `refactor!: AssetConfig::hosted_at` — no impact (the Desk uses legacy string-form attributes and no asset bundle).

## Verification

`cargo build -p frust-proto` clean on the new base; browser smoke test green: login (session + 303 flow), home, Tier-2 report with lag indicator. Desk re-serving on :3000.

## Related
[[2026-07-25 WO-009 Desk v1]] · [[ADR-004 Topcoat for Desk v0]] · [[Topcoat]]
