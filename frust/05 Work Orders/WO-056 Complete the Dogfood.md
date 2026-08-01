---
tags: [frust, work-order, dogfood, desk, maturity, milestone-5]
status: DELIVERED (2026-08-01) as a SURVEY — full lifecycle driven in a browser EXCEPT email-to-customer (not completable). **23 gaps categorized; 2 fixed as cheap Desk glue; core untouched.** **ESCALATION: a write refused by the database reports HTTP 200 "created" with a null record** — WO-020's Finding A alive on the CREATE path, reported not fixed. The alpha held silently throughout; no containment intrusion found. Maturity read: platform mid-beta, application early-alpha. See [[2026-08-01 WO-056 complete the dogfood]].
created: 2026-08-01
---

# WO-056: Complete the Dogfood — the Full Invoicing App Experience

## Why

Frust has **55 individually-green features** and has never been asked whether they *add up to a product a person can use*. WO-022 proved mechanisms work (a browser write posts to a rollup); it never asked whether someone could run their business on this. That gap — the glue *between* features, the flows, the edges — is where maturity lives, and it is only findable by **living in the app**. This is the first order that says *use it like a product*, not *make feature N work*.

**Grade this as a survey, not pass/fail.** The deliverable is TWO things: the working end-to-end experience **and** the experience-gap list. A run that surfaces many gaps is a **successful** survey — it measures the distance from "55 green features" to "a product," the number this project has never had. Grading it green/red wastes it.

Domain: **complete the accounting/invoicing app** already seeded — build on WO-022 (seed), WO-043 (email), WO-046 (print), WO-031 (workflow buttons), WO-050 (crm extension). Don't restart.

## The complete flow — clicked, no curl on the user path, live through `frust serve` + browser

1. **Land in a home/workspace** — the navigation a real user needs. Nothing built this; today it's direct URLs to `/list` and `/form`. This is the biggest missing-glue item.
2. Create a **Customer**, create an **Item** (with the empty-state a first-run user actually sees).
3. Draft a **Sales Invoice** with line items; money computes server-side (WO-021).
4. **Submit** (clerk) → **Approve** (manager) via the real workflow buttons (WO-031).
5. Record a **Payment**; watch **AR update** (the rollup, WO-007/016), staleness-visible.
6. **Print** the invoice (document view, WO-046).
7. **Email** it to the customer (WO-043).
8. Read an **AR aging / rollup report**.
9. **Live the edges:** empty states (no customers yet), error recovery (unbalanced invoice, wrong-role action), realtime (a second view self-updating).

## Deliverables

1. **The working experience** — the full lifecycle above, driven in a real browser end to end, no curl on the user-facing path. Assert the *experience* (a user can complete the flow), not just "the page renders."
2. **The experience-gap list — first-class**, everything missing or clumsy, categorized honestly:
   - **missing-glue** — navigation, home, empty states, "New" buttons, breadcrumbs, list filters (Desk-buildable; likely the bulk).
   - **missing-feature** — a report, a bulk op, a field (app-level, buildable as content).
   - **missing-in-core** — needs a kernel capability Frust doesn't have → **its own WO/ADR, NOT built here.**
   - **clumsy-but-works** — friction, not a blocker.
3. **An honest maturity read:** the distance from feature-set to product, plainly — and the **top 3–5 things** standing between here and "a real person could use this."

## The alpha, tested by silence

While living in the app — authoring a script, extending with crm, running the workflow — the containment (permissions, sandbox, tenancy) should **just hold**, never demanding thought. Note where it holds silently (the alpha working — the strongest result it can have) **and** anywhere it *intrudes* on a legitimate user flow (a first-class finding: the moat getting in the user's way).

## Boundaries (ponytail + the WO-022 rule)

- **Do NOT invent kernel features to make the experience green.** Where the flow needs something the platform lacks, that's the **highest-value finding** — log it, don't silently patch core. A survey that patches its own gaps measures nothing. (WO-022 caught two of these by refusing to.)
- Desk-level glue (home, navigation, empty states) **is in scope** and Desk-buildable — that's the missing-glue the survey exists to find and fill where cheap. `frust_ui.rs/.css` stays Desk-local.
- Bounded to the **core invoicing flow + honest edges** — not a shippable ERP, not every ERPNext feature. Build until the core flow is *livable* and you've learned what breaks.
- Dev-store mutations stated in the log (real customers/items/invoices created).
- Regression: the existing suites stay green — this assembles + adds Desk glue; it must not break the 55.

## Escalation

- If completing the flow **genuinely requires a new kernel capability** (a real platform gap, not glue), STOP and report it as a finding for its own WO/ADR. Don't build a platform feature inside a survey.
- If the **alpha intrudes** on a legitimate user flow (containment blocking something a real user must do), report it — don't work around it. That's the moat's honest cost, and it's exactly what this survey is for.

## Exit

Full lifecycle driven in a browser, live through `frust serve`, no curl on the user path; the categorized gap list; the honest maturity read with its top 3–5. Then the PM decides which gaps become the next orders.
