---
tags: [frust, build-log, topcoat]
created: 2026-07-23
---

# Build Log: Topcoat Prototype

**Goal:** pass/fail the four exit criteria in [[Topcoat#Prototype Exit Criteria — ✅ all passed 2026-07-23]].
**Where:** `D:\Dev\rust\topcoat\examples\frust-proto` — workspace member inside the Topcoat clone (reuses built deps; disk was tight). ~280-line `main.rs` + `doctype/supplier.json` loaded at request time. Server: `http://127.0.0.1:3000` (`/` form, `/grid`, role-switch nav links).
**Scope call:** metadata from a JSON file, not SurrealDB (~3 GB dep tree, full disk). The metadata loader is the swappable part — deliberately proves the headless contract.

## Results

| # | Criterion | Outcome |
|---|---|---|
| 1 | Form from runtime metadata | ✅ Added `payment_terms` to the JSON **while the server ran** — rendered on reload, zero recompilation. Binary predates the field. |
| 2 | Dependent field via signals | ✅ Checkbox toggles Credit Limit both directions, **zero network requests** |
| 3 | Grid through a shard | ✅ 500 rows, 24/page: **14–18 ms click-to-swap** loopback, ~0.4 ms server render |
| 4 | Server-side field permissions | ✅ Clerk DOM contains no manager-only field/column — initial render *and* every shard re-render |

## Upstream bugs found

1. **Windows/MSVC asset stripping** (known) — linker strips `#[used]` asset markers; pages panic until `/OPT:NOREF`.
2. **`e.target.checked` poisons signals** — arrives as raw JS boolean; next `.get()` throws (`this.read(...).deref(...).clone is not a function`). Workaround: toggle with `!sig.get()` instead of writing the event value.
3. **Windows hot-reload broken** — running server locks the exe; every rebuild fails until the process is killed manually.

## Design finding

**Signals are compile-time items** — can't declare one per metadata field in a loop. General `depends_on` graphs need one generic mechanism: re-render the form section via shard on driver-field change (~15 ms, acceptable). Constrains the Tier-2 bridge in [[ADR-001 UI Extension Tiers]].

## Follow-ups

- [ ] Swap JSON metadata loader for [[SurrealDB]] (needs disk space first)
- [ ] File the three bugs upstream at tokio-rs/topcoat
- [ ] Stress the shard path beyond loopback (realistic RTT) before trusting the 15 ms figure
	- ✅ 2026-07-24: the **620 ms per interaction at Slow-4G** figure (originally a lost write) was **reproduced under WO-001**: 617–628 ms across 5 samples (original 615–624), same methodology. Corroborated; cited by [[ADR-007 Tier-2 Script Architecture]] as the reason dependent-field logic stays client-side.
- [ ] Spreadsheet-style per-cell editing remains unproven — test at the grid-bulk-edit screen

## Related

[[Frust Hub]] · [[Topcoat]] · [[ADR-001 UI Extension Tiers]] · [[SRS]]
