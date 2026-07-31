---
tags: [frust, build-log, plugins, routes, work-order]
created: 2026-07-26
work-order: "[[WO-019 App Lifecycle]]"
status: criterion 1 CONFIRMED — registry design may proceed
---

# Build Log — WO-019 criterion 1: The Door Probe

**The PM prediction holds. No escape found. The registry design may proceed.**

> *A plugin route handler is a profile of ADR-006 — a WIT export receiving the
> verb surface under the caller's session, in a world with no db handle to
> obtain.*

Every hostile attempt below was made **by the guest, through the real
boundary** — not simulated host-side, which would only have proven that the
host refuses what the host already refuses.

## The probe results

| Attempt | Result |
|---|---|
| honest structured read | `ok:[{"title":"clerk1 order",…}]` |
| **row-permission leak** (read clerk2's row) | **1 row, clerk1's own** — identical to the same caller's direct broker read |
| raw SurrealQL as the doctype | `refused: E_UNKNOWN_DOCTYPE { name: "purchase_order; REMOVE TABLE purchase_order" }` |
| raw SurrealQL through the filter | `refused: FRUST:E_BAD_FILTER … unknown variant, expected one of and/or/not/cmp` |
| read the identity table | `refused: E_UNKNOWN_DOCTYPE { name: "app_user" }` — no password material crossed |
| **open its own socket** | `refused: Permission denied (os error 2)` |
| **read the filesystem** | `refused: No such file or directory (os error 44)` |

The two rows in bold are the ones that would have ended the WO. A plugin that
can open a connection or read the disk makes the door decoration — it would
simply go around it. Neither is reachable: they are not in the guest's world.

Note *how* the raw-SurrealQL attempts failed. Not by string inspection or a
denylist — the doctype failed identifier validation and became an unknown
doctype, and the filter failed to parse as the ADR-006 `Filter` variant tree.
**Query text isn't rejected; it is un-representable.**

## Why the shape holds

The signatures contain no way to name a caller, a session, a connection, or a
query. **Authority is not a parameter.** The host binds a `Caller` into a fresh
store for the duration of one call and tears it down with the store, so a
handler cannot forge, cache, outlive or widen the authority it ran under.

And it is not a second permission compiler: the host implementation calls
`Broker::db_read` — the same one the Desk and REST use — so row rules are
enforced by the DB under the caller's own session (ADR-003). The test asserts
route-read and broker-read return *the same rows for the same caller*, which is
the one-compiler property stated as an equality rather than an intention.

**`routes.rs` is deliberately absent from `surql_monopoly`'s allowlist**, so CI
fails the build if query text ever appears on the route path. Both properties
re-verified with the route live: `surql_monopoly` green, all six
`permission_proof` tests green.

## Two findings from the WIT, both load-time

### 1. Adding `routes` to the shared world broke the script engine

`script_engine.wasm` is a hook provider, not a route provider, and a world that
demands a `routes` export made it unloadable. The WIT was right and the
modelling was wrong: **declaring a route is a distinct capability claim.**
Split into `world plugin` (hooks) and `world route-plugin` (hooks + routes +
door), which is ADR-006 edge 1's additive-evolution rule applied to worlds
rather than to operators.

### 2. A component that names the door won't load where the door isn't offered

Once `plugin_demo` imported `db-api`, the *hook* host refused it —
`imports instance frust:plugin/db-api, but a matching implementation was not
found in the linker`. This is exactly ADR-006 edge 1's promise working
unprompted: **incompatible plugins fail at load-link time, never at runtime.**

Resolved by having the base world import `db-api` too, with the hook host
answering `FRUST:E_NO_DOOR`. The alternative — a component that links as a
route but not as a hook — is a worse thing to explain than an explicit
refusal. A hook is handed its document; it has no door because it needs none,
and saying so out loud beats silently returning empty rows (which would teach
plugin authors that reads "sometimes don't work").

## Suite state

**28 binaries green, zero failures** (the 28th is `door_probe`), stack stopped.
47 scratch databases dropped at close.

## Next in this WO

Criterion 1 is confirmed, so the registry design is unblocked: the manifest
(2), install/enable/disable through the Desk with dry-run-as-UX (3), routes
served under bearer discipline + WO-013 throttling proven to apply (4), honest
uninstall (5), server-script delivery (6), demo app (7).

Carried forward for criterion 4: the route path currently serializes on one
instance and is reachable only in-process — REST dispatch, bearer discipline
and door-throttling are not yet wired.

## Related
[[WO-019 App Lifecycle]] · [[ADR-005 Plugin Isolation]] · [[ADR-006 Plugin Capability Surface]] · [[ADR-003 Permission Compiler]] · [[SRS]] (REQ-2.2.2, REQ-3.1.2)
