---
tags: [frust, adr, topcoat, desk]
status: accepted
decided: 2026-07-23
---

# ADR-004: Adopt Topcoat for Desk v0 — Behind an Absolute Headless Contract

**Context:** [[2026-07-23 Topcoat prototype]] passed all four exit criteria. This ADR formalizes the verdict already recorded in [[Topcoat]].

**Decision:** Topcoat renders Desk v0, under these terms:
- **Absolute headless contract:** the engine's UI boundary is `(DocType metadata, record JSON) → render`. Nothing below that boundary imports or knows Topcoat. Swapping to Leptos/htmx later touches zero engine code.
- **Pinned revision** — 0.x with promised breaking changes; upgrades are deliberate events, not `cargo update` side effects.
- **Measured interaction budget:** shards for data operations (14–18 ms click-to-swap on loopback), client-side signals for visibility/format-only toggles. Per the compile-time-signals constraint in [[ADR-001 UI Extension Tiers]], `depends_on` graphs go through generic shard re-renders, not per-field signals.

**Revisit triggers (any one re-opens this ADR):**
1. Spreadsheet-grade screens (grid bulk edit, report builder) exceed what shards + thin client runtime can deliver.
2. Realistic-RTT measurements blow the interaction budget (current numbers are loopback-only).
3. Upstream breaking changes make the pin unmaintainable.
4. Tier-2 sandbox needs client capabilities the thin runtime can't host.

**Clarifying note (WO-042, 2026-07-30) — "no second UI runtime" ≠ "no JavaScript."** This contract governs the **engine↔Desk boundary** (engine never imports the UI; UI stays swappable) and the interaction *budget*, NOT the Desk's internal JS. Web-platform primitives (`<dialog>`+`showModal()`, `<datalist>`, `<details>`) and progressive-enhancement one-liners are **in-bounds** — they are the browser's own behavior, not a runtime, and the engine never sees them. What the pin and the headless contract guard against is a second UI **framework/SPA runtime** (Vue/React/etc.), not JS per se. **Ruling: native `<dialog>`+`showModal()` is approved for modals that must trap focus + honor Escape** (accessible blocking decisions); CSS-only `:target` stays the default for dismissible/informational surfaces. WO-042 found the one frappe-ui interaction unreachable by CSS-only / server-round-trip / the six-verb bridge without disproportionate cost is modal focus-trap+Escape — and the correct resolution is the *most* native option (rung 3), not the bridge. A hand-rolled CSS focus-trap is banned: it claims containment it cannot deliver.

**Related:** [[Frust Hub]] · [[Topcoat]] · [[ADR-001 UI Extension Tiers]]
