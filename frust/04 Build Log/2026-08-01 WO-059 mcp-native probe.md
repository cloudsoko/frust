---
tags: [frust, build-log, mcp, api, probe, milestone-5]
work-order: WO-059
date: 2026-08-01
status: DONE — probe complete, adapter-over-REST CONFIRMED, containment PASS (byte-equal + provenance), ADR-017 PROPOSED
---

# 2026-08-01 — WO-059 MCP-Native Probe

**Verdict up front.** Adapter-over-REST **confirmed**. A TypeScript/Node MCP
server that is a *client* of the documented REST surface generated faithful
tools from `GET /meta`, and its `tools/call` under a **clerk** session returned
**exactly the clerk's rows — byte-equal to REST and provenance-correct**, with
every over-read attempt refused. Writes are opt-in/off-by-default and refusals
are typed. **21/21 proof checks pass.** The alpha holds for AI agents: the MCP
consumer is the **fourth consumer of the one permission compiler**, contained by
construction. Position paper → [[ADR-017 MCP Surface]] (PROPOSED — not
self-ratified). Five findings, all bounded; none breaks the thesis.

Artifact: **`D:\Dev\rust\frust-mcp\`** (standalone, non-Rust, additive-only).

---

## Isolation (criterion 4) — how the parallel-agent rule was honoured

The other agent holds `frust-kernel` and `frust-desk` (both git repos) with
WO-057/058 edits in flight. This probe made **zero Write/Edit calls to any path
under either** — the only reads there were source inspection.

- `frust-mcp/` is a **standalone sibling directory**, not inside either repo's
  worktree; **no kernel/desk source references it** (grep: none).
- The kernel I ran is a **COPY** of `frust.exe` at `frust-mcp/scratch/frust.exe`,
  so my running process never locks the other agent's `target/release`, and
  their rebuild never touches mine.
- **Constraint that shaped isolation (Finding 1):** `ConnConfig::default()`
  hardcodes the SurrealDB endpoint to `http://127.0.0.1:8899` with **no env
  override**. The shipped binary cannot be pointed at a separate surreal
  *process*/*port* without a kernel edit the probe forbids. So isolation is at
  the **database** level — a fresh, uniquely-named `frustmcp` database on the
  shared surreal (SurrealDB isolates databases), plus my **own kernel port
  8795** — which is exactly the kernel test-suite's own isolation model
  (`common/mod.rs` gives every test a unique database on the same 8899 surreal).
  The dev databases were never touched; `frustmcp` was dropped at close.

## Setup (all evidence re-runnable)

- Identities are write-closed (WO-008 — no REST door mints `app_user`), so
  `manager`/`clerk1`/`clerk2` were seeded directly as root (`scratch/seed.surql`,
  mirroring the proven `frust-skel/setup.surql`).
- **Everything else went through the REST door**, dogfooding ADR-016's BYO
  claim: `manager` installed a workflowed, submittable `expense_claim` app
  (`POST /app/install`), then each clerk created its own rows
  (`scratch/seed-fixture.sh`). Fixture: clerk1 owns 2, clerk2 owns 2, manager
  owns 1 — owner-based row perms (`FOR select WHERE (owner = $auth.id) OR
  $auth.role='manager'`), so clerk1 sees 2, clerk2 sees 2, manager sees all 5.
- Run recipe: `frust-mcp/README.md`. Proof: `node proof/containment.mjs`,
  driven through the **official MCP client SDK** (a real MCP consumer, stdio).

## Predictions vs results

Each block printed its prediction before running (house discipline). Full log in
the proof output; summary here.

### Criterion 1 — metadata → tool-schema fidelity
**Predicted:** read tools generate; with writes on, create/update/transition
generate; `amount` (Currency) → `type:string` + a decimal-string money note;
`required` carries `[purpose, amount]`; **labels unavailable** so descriptions
are synthesised. **Result: PASS**, with the label caveat (Finding 3). Money note
carries "decimal string, never a float". `required` = `["purpose","amount"]`.

### Criterion 2 — the fourth consumer (THE proof)
**Predicted:** clerk1 via MCP returns exactly clerk1's 2 rows, **byte-equal** to
clerk1's REST `/read`; every `owner == app_user:clerk1`; a clerk2 row is **never**
returned to clerk1 (not by `get`-id, not by a filter aimed at it); manager via
MCP == manager REST (all 5). **Result: PASS on every assertion.**

- `canon(mcp) === canon(rest)` for clerk1 (2==2) and manager (5==5) — byte-equal
  after normalising only key/element order (which the evolution policy
  unpromises).
- **Provenance:** clerk1's MCP rows are 100% `app_user:clerk1`.
- **Escalation gates (the whole thesis):** `get_expense_claim(<clerk2 id>)` →
  *not found*; `list` with `filter owner==app_user:clerk2` → **0 rows**. A filter
  cannot widen; the kernel filters the push under the caller's own session. **No
  over-read occurred — the escalation condition never fired.**

### Criterion 3 — write tools, gated
**Predicted:** writes off ⇒ no write tools; a create flows through the broker
(owner set, lattice holds); a refused write is a typed error, not a silent
created. **Result: PASS.**

- **Opt-in (structural):** with `FRUST_MCP_WRITES=off`, `tools/list` for clerk1
  is only `list_/get_` — the write tools do not exist.
- **Through the broker + lattice:** clerk1 `create` → `created`, persisted,
  `owner=app_user:clerk1`, money stored as decimal (`"33"`); clerk1 `Submit`
  advances `workflow_state` to *Submitted for Approval* but **docstatus stays 0**;
  manager `Approve` crosses to **docstatus 1**. Only the manager crosses the
  lattice — the kernel's rule, unchanged, reached through MCP.
- **Typed refusals, nothing silent:**
  - cross-owner update → **HTTP 403 `permission-denied`**
    `E_WRITE_NO_ROWS: … the record does not exist, or your role may not write it`
    → tool `isError`, and clerk2's row is **provably unchanged** (still "Hotel
    night", not "HACKED").
  - malformed money → **HTTP 400 `invalid-value`** "decimal must be a plain
    numeric string" → `isError`. (Cleaner than raw REST — see Finding 2.)
  - wrong-state transition on an approved doc → **HTTP 422 `workflow-denied`**
    → `isError`.

## Findings (all → ADR-017)

1. **DB endpoint is hardcoded, not env-configurable.** `ConnConfig::default()`
   → `http://127.0.0.1:8899`; no env reads it. A BYO/MCP/probe deployment can't
   select a separate store without a kernel edit. **Recommend an additive
   `FRUST_DB_ENDPOINT` env** (safe, transport-not-placement). Shaped this
   probe's isolation (see above).

2. **Money on WRITE: a bare decimal string is REFUSED — the money-safety wrinkle
   in a new place.** The docs' "money is a decimal string" holds on **read**;
   on **write** a Currency field rejects `"42.00"` with a `db`-kind coercion
   error *"Expected `decimal` but found `'42.00'`"*. Accepted forms: the typed
   `{"kind":"decimal","v":"42.00"}` or a bare integer. **The adapter wraps money
   strings into the typed form**, so (a) the agent sends a clean string and it
   works, and (b) malformed money surfaces as a clean `400 invalid-value` instead
   of the raw db coercion error. Without the wrap, an agent following the
   documented convention would get a confusing `db` error. This is both a
   doc/behaviour gap and an argument that money-typing belongs in the adapter (as
   built) or in a kernel write-path coercion.

3. **`GET /meta` omits field `label`.** The response exposes
   `fieldname/fieldtype/options/required` but not the human label (even though
   the manifest set one). "Labels → param descriptions" is therefore only
   partly satisfiable — descriptions are synthesised from `fieldname`+`fieldtype`.
   **Recommend `/meta` additively expose `label`** (better tool descriptions).

4. **`GET /meta` omits the workflow slot.** Only `submittable:true` is exposed;
   states/transitions/actions are not — despite rest-api.md describing "workflow
   slot". So a `transition_{doctype}` tool cannot enumerate its `action` enum
   from `/meta`; it takes a free-string action and relies on the typed refusal
   (or a per-record `GET /workflow/{dt}/{key}`). **Recommend `/meta` additively
   carry the workflow definition.**

5. **`/read` id-filter needs a typed record value.** A string
   `{path:"id",op:"eq",value:"expense_claim:x"}` matches nothing (SurrealDB `id`
   is a record link); the matching form is `value:{kind:"record",v:"…"}`
   (`kind:"thing"` is rejected — only `record`). Minor; the `get` tool uses the
   typed form.

## Honest scope — what this probe did NOT touch

One doctype (`expense_claim`), owner-based perms + a 4-state workflow. **Not**
exercised: MCP resources/prompts/**subscriptions/notifications**, the realtime
`/subscribe`+`/events` path (maps naturally to MCP resource subscriptions —
WO-060), child tables, Link/Select fields (none in fixture), and **multi-tenant
per-request auth** (this probe is one-server-one-principal). No streaming or
pagination-cursor semantics were needed for the tool shape, which is why
adapter-over-REST was *sufficient* here — but the untouched features are where a
future "REST can't express it" could still surface. Stated, not hidden.

## Cleanup
`frustmcp` database dropped; my kernel (port 8795) stopped. The dev store,
kernel (8790) and Desk were never touched. Reconstitute from `frust-mcp/README.md`.
