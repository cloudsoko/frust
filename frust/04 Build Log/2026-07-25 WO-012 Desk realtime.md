---
tags: [frust, build-log, realtime, desk, work-order]
created: 2026-07-25
work-order: "[[WO-012 Desk Realtime]]"
---

# Build Log — WO-012: Desk Realtime

ADR-011's measured design is now product: kernel-owned live subscriptions, focus-scoped from the Desk, with polling still doing the job underneath. **Proven end-to-end in a browser: an externally-written row made the list go from "3 row(s)" to "4 row(s)" on its own.**

## First act — the ruled relocation

The Desk lives at **`D:\Dev\rust\frust-desk`** as its own crate, depending on the vendored Topcoat by path (`websocket` feature on). It is gone from the vendor tree (`examples/frust-proto` removed, vendor `main` = `6ef5a36`) — Desk source is no longer an "example" inside someone else's repo. Carried-patch manifest shrinks accordingly: the vendor now carries runtime work only (dynamic signals, Decimal, signal methods, #192 fix, bridge example).

## What shipped

**Kernel (`realtime.rs` + `wsrpc.rs`, zero new crates):**
- `wsrpc.rs` — a hand-rolled RFC 6455 client (text frames, ping/pong, masking, close) for SurrealDB's `/rpc`. Scope disclosed in the module docs: no fragmentation (SurrealDB sends whole messages), no TLS and no accept-key verification (loopback, same trust domain as every other kernel→DB connection).
- `realtime.rs` — the subscription registry. **Session-before-upgrade**: each subscription authenticates to SurrealDB with *the subscriber's own record JWT*, pulled from the session record, so **the DB filters the push path** — the kernel never does. The `/subscribe`, `/events`, `/unsubscribe` routes sit behind the same bearer discipline as every other route; a socket is a session.
- **Ticks carry `{action, id}` only** — no row data. Clients refetch through the normal read door, so the push path can never become a second data path with its own permission story. This is a deliberate hardening beyond the WO: a leak can't ride a payload that doesn't exist.

**Desk (`frust-desk`):** focus-scoped subscription — opened when the list is visible, closed on `visibilitychange`/`pagehide` via `sendBeacon`, so the kernel's parked count tracks **focused views, not open tabs**. The browser never holds a kernel token: `/live/*` are Desk routes that attach the session bearer server-side. Polling stays first-class — the 60 s meta-refresh renders *above* the realtime script and is never removed; with JS off, a dead socket, or a 429 budget refusal, the list still updates.

## Criteria

| Requirement | Evidence |
|---|---|
| Leak partition re-proven on **shipped** code | `realtime_e2e::shipped_push_path_partitions_and_budgets`: writes through the broker; c1's subscription ticks for exactly c1's row, c2's for exactly c2's, manager for both; c2 reading c1's subscription = 403; a tick asserted to be exactly 2 keys |
| Budget observable **before** it's hit, transparent when it is | `/metrics` shows `frust_live_subscriptions{table,tenant}` while healthy; over budget returns **429 + `FRUST:E_SUB_BUDGET`** (a capacity answer, not an error), client stays polling |
| Parked count tracks focused views | idle-reap (30 s without an `/events` poll) + dead-connection reap on the serve tick; `frust_live_reaped_total` observed incrementing in the live demo |
| Reconnect-refetch through a restart | `realtime_e2e::restart_reports_dead_then_refetch_recovers` (run alone): after a surreal bounce `alive:false`, then resubscribe + refetch under a fresh session |
| Floor gate runs **with** parked-subscription load | `gate_submit_with_live_subscriptions` — see below |
| Browser proof | list self-refreshed 3 → 4 rows on an external write; `frust_live_ticks_total = 1`, subscription auto-opened on focus |

## The writer tax, measured through the kernel (and a budget correction)

WO-011's spike estimated ~70 µs/sub client-side. The kernel-side curve (`live_tax_curve`, **bracketed** — baseline re-measured at the end to separate tax from machine drift) confirms the order and sharpens it: **+1 ms at 5–20 parked subs, +2 ms at 30, +4 ms at 40**, i.e. ~50–100 µs/sub.

**Budget corrected 50 → 20**, because the gate said so: at 40–50 the floor gate flapped (24/26/24, then 26/27/25/25). At 20 the realtime tax measures **0 ms** against the same run's baseline. The WO's instruction was to price the tax into CI, and pricing it revealed the spike's estimate was optimistic — the number moved to fit the measurement, not the other way round.

**How the tax is gated, and a governance note.** The floor has only ~1–3 ms of headroom over a 22–24 ms baseline, so a naive "gate at 25 ms with subs parked" would have quietly widened REQ-6.1.1 to absorb an optional feature. Instead there are now **two gates**:
- `gate_submit_latency` — REQ-6.1.1 exactly as specified: the write path alone, ≤ 25 ms.
- `gate_submit_with_live_subscriptions` — parks the full budget and asserts the **delta over the same run's own baseline** stays inside `LIVE_TAX_BUDGET_MS = 2`. Measuring the delta in-run means machine drift moves both halves together, so the gate judges realtime and nothing else. Its failure message names the only correct remedy: *lower the budget, do not widen the allowance.*

Final run on a fresh instance, single-threaded: hook 0 ms, **submit 24 ms (gate 25)**, **tax 0 ms at budget 20 (allowance 2)** — all green.

## Findings

1. **The surql monopoly gate caught this WO's own code.** `LIVE SELECT` composed in `realtime.rs` failed `surql_monopoly`. Fixed the honest way — `surql::render_live_select()` — rather than widening the allowlist. The monopoly earns its keep by catching the author, not just strangers.
2. **The latency gates were self-interfering.** Adding a second submit-measuring gate made both flap: cargo runs tests in parallel, so they measured each other's contention. Serialized with the same mutex pattern `permission_proof` uses. Any future latency gate must take that lock.
3. **`view!` escapes inline `<script>` text** (`=>` became `=&gt;`). Rather than add an asset bundle the Desk deliberately does without, the live script is written with no `<`, `>`, or `&` (function expressions, nested ifs). Noted in-code so the next editor doesn't "clean it up" into breakage. A `Js` content wrapper (mirroring the vendor's `Css`) is the tidier long-term option.
4. **Self-inflicted fixture pollution, caught by CI:** the browser demo wrote a manager-owned row into the shared WO-002 fixture, breaking `permission_proof`'s "clerks partition the table" invariant. Deleted; suite green. Demo writes against the shared fixture need a scratch doctype next time.

## Boundaries kept

Kernel owns sessions (no topcoat-session; 🚫 bucket intact). Polling is first-class and un-removable. Realtime is an enhancement: every failure path (budget refusal, dead socket, JS off) degrades to the same working list.

## Suite state
**23 test binaries green, zero failures**, including the two new realtime files and both perf gates.

## Related
[[WO-012 Desk Realtime]] · [[ADR-011 Realtime]] · [[2026-07-25 WO-011 live-query scale spike]] · [[2026-07-25 WO-010 Observability]] · [[SRS]] (REQ-6.5) · [[Topcoat]]
