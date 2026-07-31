---
tags: [frust, build-log, hygiene, testing, milestone-4]
created: 2026-07-31
work-order: "[[WO-047 Hygiene Bundle]]"
status: COMPLETE — three items, and two of them corrected published facts. (1) The `revoke_kills_` flake was a **KERNEL DEFECT, not a test artifact**: the login batch was `DEFINE TABLE IF NOT EXISTS …; CREATE …`, SurrealDB commits unbraced statements SEPARATELY, and the transport retries the WHOLE batch on conflict — so a DDL race between two first-logins re-ran the already-committed CREATE and duplicated a session row. `frust_conflict_retries_total 1` on every run. Fixed at root by moving `_frust_session` into boot-time `meta_ddl` (meta v5→v6); assertion untouched, no serialization. **10/10 consecutive parallel runs green, both auth modes** (was jwt 2/5, basic 3/5 failing). (2) The CSS guard is committed as two Desk unit tests and **found a live shipped bug first**: `fui-alert--error` does not exist, so EVERY error flash in the Desk drew with no colour and no icon. Both halves demonstrated failing. (3) The root-call list is delivered — and **corrects WO-044**: a steady-state write costs **1 root call, not ~3** (trace-attributed, 20/20 writes, zero variance), and idle is **9.0/s from exactly two named tickers**, not 19/s. Both WO-044 figures came from one instrument flaw: subtracting an *idle* baseline from a *loaded* window.
---

# WO-047 — Hygiene bundle

Three watch items. Two of them turned out to be corrections rather than chores.

## Item 1 — the `revoke_kills_` flake was a kernel defect

WO-044 proved it was not WO-044's (jwt 2/5 fail, basic 3/5, serial 0/5). It was
not the test's either.

**Diagnosis, by measurement.** A diagnostic dump of the session table at revoke
time, over six runs:

```
FAIL: boss@11.016  clerk@11.018  clerk@11.053  clerk@11.085  other@11.116  boss@11.147
PASS: clerk@27.807 boss@27.809   clerk@27.835  boss@27.844   other@27.865  boss@27.890
```

Six sessions where the binary's tests create five — and the extra one lands on
`clerk` (fail) or `boss` (pass). **One login wrote two rows.** In *every* run,
pass and fail alike: `frust_conflict_retries_total{tenant="kh_hygiene"} 1`.

**Root cause.** The login path issued:

```sql
DEFINE TABLE IF NOT EXISTS _frust_session SCHEMALESS PERMISSIONS NONE;
CREATE _frust_session SET token = '…', user = '…', …;
```

Three facts compose into the bug:
1. SurrealDB commits **unbraced statements in separate transactions**.
2. `sql_with_auth_inner` retries the **whole batch** when any statement reports
   a conflict — correct for a `BEGIN/COMMIT` batch, wrong for this one.
3. Two first-logins racing on the same `DEFINE` conflict on the DDL *after* the
   `CREATE` has already committed.

So the retry re-ran the CREATE and the user got a duplicate session row. The
retry re-sends identical text, so both rows carry the same token — which is why
this never broke a login and hid for so long. What it did break is any **count**
over the table, which is exactly what `revoke` returns.

**Fix at root, not at the symptom.** `_frust_session` was the *one* meta table
created lazily on the auth path; every other one comes from `meta_ddl()` under
the boot lock (ADR-008, binary-authoritative). It now does too — meta **v5 → v6**
— and login is a single `CREATE` with no DDL in it. `OVERWRITE` preserves rows
(WO-002 Finding A), so live sessions survive the migration.

The assertion was not weakened and the test was **not serialized** — the WO
allowed serialization only if the interference were by-design, and it was not.

**Exit criterion, the flake's own falsifying instrument:**

| | before (WO-044) | after |
|---|---|---|
| `FRUST_ROOT_AUTH=jwt` | 2/5 **failed** | **5/5 green** |
| `FRUST_ROOT_AUTH=basic` | 3/5 **failed** | **5/5 green** |

Bonus: login is now one statement instead of two, so it also stopped issuing a
DDL per authentication.

## Item 2 — the CSS guard, committed, and it found a live bug

Two Desk unit tests (`cargo test` in `frust-desk`, cannot be skipped):

- `every_custom_property_referenced_is_defined` — every `var(--fui-*)` the
  stylesheet reads must be one it defines.
- `every_component_variant_literal_has_a_class` — every `variant:`/`color:`
  literal passed to `fui_button`/`fui_alert`/`fui_badge` must be a modifier
  **that component** defines.

**It found a shipped bug before any plant.** `fui-alert--error` does not exist —
the alert vocabulary is `info|success|warning|danger`. Three call sites passed
`"error"`, and one of them was `flash_variant()`, which returns it for **every
non-success flash in the Desk**. The result: base alert styling, no colour, and
`icon("error")` fell through to `_ => ""` so **no icon either**. Compiles,
renders nearly right, says nothing — the WO-042 class exactly. Fixed to
`danger` at all three sites.

**Both halves demonstrated failing** (WO-032's rule):

```
plant --fui-does-not-exist  → FAILED: custom properties referenced but never defined: ["--fui-does-not-exist"]
plant fui_button(variant:"solid") → FAILED: fui_button(variant: "solid") — no `.fui-btn--solid` in the stylesheet
```

**And planting is why the guard works at all.** The first version passed the
planted bug **twice**:
1. It accepted a value if *any* component defined it — and `.fui-badge--solid`
   exists, so WO-042's actual `fui-btn--solid` bug sailed through. Fixed by
   binding each literal to its own component.
2. It derived the function name from the class stem (`fui-btn` →`fui_btn`), but
   the function is `fui_button`, so the button half matched no call sites and
   **checked nothing**. Fixed by stating the pairing explicitly.

A guard that cannot fail on the defect it was written for is worse than none,
and only the plant revealed that — twice, in one sitting.

**Class-name coverage: shipped as the variant half, and here is why that is the
right scope.** A blanket "every `fui-*` literal in Rust has a CSS class" scan
would miss the bugs that actually occur, because the failing names are
*composed at runtime* (`format!("fui-btn fui-btn--{variant}")`) and never appear
as literals. The variant check targets exactly the composed case. Static class
literals are already covered incidentally: a wrong one would have to be typed
somewhere the eye passes over, whereas a wrong *variant* is a legal-looking
argument.

## Item 3 — which root calls remain (identification only)

**The metric could not answer this; the trace could.** Attributing by trace id
is confound-free: a call either belongs to a request's trace or it does not.

### The write path: ONE root call

| calls per `/write` | count |
|---|---|
| root | **1** (20 of 20 writes, zero variance) |
| session | 1 (the write itself) |

And it is:

```
SELECT server_script FROM doctype WHERE name = '…' LIMIT 1;
```

`sync::load_server_script`, called by `hooks::dispatch_doctype_script` on
**every validate**, uncached — the leading suspect WO-044 named but did not
claim. It runs even for a DocType with no script, because the query is how you
find that out. (`load_doctype` and `notification_rules` are both
generation-cached and issue nothing in steady state.)

### The idle ticker: 9.0/s, two sources

| rate | query | source |
|---|---|---|
| **5.0/s** | `SELECT tenant FROM job WHERE status = 'queued' GROUP BY tenant` | `worker::claim_next` — resident worker tick |
| **4.0/s** | `SELECT id, enqueued_at FROM _frust_mail WHERE status = 'queued' …` | `worker::MailWorker::queued_ids` — WO-043 mail worker, 250 ms poll |

No Tier-2 rollup drains appear because the dev store's two aggregates are both
`kind: counter` (Tier-1, maintained by a DB EVENT inside the write transaction),
so no `RollupWorker` is wired. The list is complete for this deployment.

### Correction to WO-044

WO-044 reported **~3 root calls per write** and **19/s idle**. Both are wrong,
and both from one instrument flaw: **I subtracted an *idle* background rate from
a *loaded* measurement window.** A ticker with a backlog does more work per tick
than an idle one, so the background term was underestimated and the residual was
misattributed to the write. The trace-attributed numbers — 1 per write, 9.0/s
idle — supersede them. The *direction* of WO-044's conclusion is unaffected (the
124 req/s ceiling was still argon2), but the per-write figure was inflated ~3×.

### PM decision queued

`load_server_script` is the only per-request root call left. Collapsing it is a
WO-026-shaped generation cache (the script text already lives on the `doctype`
record, which the metadata cache holds). **Not done here** — the WO scoped item 3
to identification, and at ~297 µs the urgency is low. Noting only that it is a
one-query win on every write, and that the cache would have to invalidate on
`POST /doctype/{name}/script`, which already bumps the generation.

## A second flake, found by the exit criterion itself

Running the full suite in **both** auth modes — which the WO required and which
had never been done before — turned up three more failures, all in WO-044's own
`root_jwt_auth.rs`, and all mine:

1. **Three tests asserted JWT-path behaviour while floating on the process
   env.** Under `FRUST_ROOT_AUTH=basic` they failed for the entirely correct
   reason that there was no token to assert about. The *Basic* arm of the parity
   test was already pinned with `force_basic_for_test()`; I never pinned the
   other arm. Added `force_jwt_for_test()` — the missing half — so the file is
   mode-independent.
2. **`force_root_token_for_test` planted a token without pinning the arm**, so
   in Basic mode the planted token was never sent, no 401 came back, and the
   retry the test exists to prove **was never exercised — it passed by not
   running.** Planting now pins JWT by construction.
3. **`argon2_runs_once_not_per_request` read the process-global metric**, when
   WO-044 had added a per-handle `root_signins()` counter *for exactly this
   reason* and the test then read the global anyway. Once several tests each
   minted their own token in parallel, the global delta stopped being about this
   handle. Now `assert_eq!(db.root_signins(), 1)` — exact, and unraceable.

Same family as item 1, one level up: **a test that has only ever been run one
way is a test whose other way is unproven.**

## Regression

| | binaries | passed | failed |
|---|---|---|---|
| full kernel suite, parallel, `FRUST_ROOT_AUTH=jwt` | 53 | **330** | **0** |
| full kernel suite, parallel, `FRUST_ROOT_AUTH=basic` | 53 | **330** | **0** |

Plus `kernel_hygiene` 5/5 consecutive in each mode (10/10), Desk unit tests 4/4,
and browser suites green: workflow 18/18, SSE 8/8, print 24/24, mail 15/15.

No behaviour changed outside the flake fix and the alert-variant correction;
**no number moved** (the login path lost a statement, which can only help, and is
not claimed as a perf result).

## Dev-store note

The dev store migrated **meta v5 → v6** on restart (`--accept-meta-migrations`,
as the kernel already runs). `_frust_session` is now defined at boot;
`OVERWRITE` preserved the existing rows.

## Related
[[WO-047 Hygiene Bundle]] · [[2026-07-31 WO-044 root jwt auth]] (the corrected
figures) · [[2026-07-29 WO-042 frust ui re-skin]] (the CSS-seam class) ·
[[2026-07-31 WO-046 document view]] · [[2026-07-28 WO-033 revoke endpoint]] ·
[[ADR-008 Data Shape]] (meta is binary-authoritative)
