---
tags: [frust, build-log, batteries, email, notifications, milestone-4]
created: 2026-07-30
work-order: "[[WO-043 Email Batteries]]"
status: COMPLETE — all 5 criteria met, browser-proven live. lettre direct/blocking per the PM ruling (+9 crates MEASURED, not the +10 estimated; zero tokio, verified in the graph). Notification = metadata record; rules added through `POST /notification` fire on the very next write and SURVIVE a restart. Dedicated std::thread mail worker. FOUR FINDINGS, one of them big — root Basic auth costs **16.5 ms/request** vs **0.79 ms** for a cached session Bearer (same query, same process): a pre-existing kernel-wide property that WO-043 is merely the first caller to put on the write path. ESCALATION CANDIDATE with a proven fix (root JWT via /signin works, 200 µs server-side).
---

# WO-043 — Email batteries: notifications as metadata, delivery as a contained side-effect

Transport ruled B before any code was written — see
[[2026-07-30 WO-043 mail transport decision]]. This log is the build.

## What shipped

| Piece | Where | Note |
|---|---|---|
| Notification rule shape, event matching, conditions, interpolation, recipients, transport | `kernel/src/mail.rs` (NEW) | **contains no query text** — `surql_monopoly`'s allowlist did NOT grow |
| Rule + address loading | `sync.rs` (`load_notifications`, `role_addresses`) | beside `load_workflow`, the module already sanctioned for metadata loading |
| Fire on lifecycle event + outbox enqueue | `broker.rs` (`fire_notifications`, `enqueue_mail`, `record_dead_mail`) | + a generation-keyed `notif_cache` |
| The dedicated worker | `worker.rs` (`MailWorker`) | claim / send / sent · retry · dead |
| Meta v5 | `meta.rs` (`mail_ddl`, `email` on `app_user`) | `notification` + `_frust_mail` tables |
| `POST /notification`, `POST /mail/outbox` | `rest.rs` | manager surface, validated before storing |
| Its own thread | `main.rs` | never the maintenance thread |
| 6 integration + 8 unit tests | `tests/mail_notifications.rs`, `mail.rs` | |
| Live browser proof | `frust-e2e/mail.spec.mjs` (NEW), `pnpm mail` | 15/15 |

**The dependency, measured not estimated.** The escalation predicted +10 crates;
the executed number is **+9** (231 → 240): `lettre`, `email-encoding`,
`email_address`, `quoted_printable`, `mime`, `nom`, `fastrand`, `base64 0.23`,
`getrandom`. `rustls`, `webpki-roots`, `url`, `socket2`, `uuid`, `httpdate` and
`percent-encoding` turned out to be **already in the graph** (ureq brings
rustls), so the marginal cost was smaller than the estimate I gave the PM.
`cargo tree -i tokio` still shows exactly one path — `wasmtime-wasi` — so the
mail path added none.

## The criteria

**1 · Email is metadata, not code.** A rule is a record in `notification`.
`POST /notification` validates the shape and bumps the metadata generation, so
the rule fires on the **very next write** — proven both ways: the integration
test does a write *before* the rule exists (warming the empty cache, which is
the half that breaks if invalidation is missing), and live, the rule created
mid-session fired on the next transition and then **survived a kernel restart**,
because it is a row, not configuration.

A rule that would never fire is refused at the door, live:

```
POST /notification {"event":"on_sumbit", ...}
→ 400 bad notification: event "on_sumbit" is not one of
  ["after_insert","on_update","on_transition","on_submit","on_cancel"]
```

That refusal matters more than it looks: a stored-but-never-firing rule is
indistinguishable from a broken mail transport, and would send an operator
looking in entirely the wrong place.

**2 · Delivery is contained.** Own `std::thread`, blocking `SmtpTransport`, no
async runtime, no second executor. The WO's stated comparison is *"save latency
with the mail worker healthy vs. with a dead/slow transport"*, measured with a
real worker thread draining concurrently in both arms:

```
SAVE FLOOR — no rule 2.99 ms | rule + HEALTHY transport 50.9 ms
            | rule + DEAD transport 39.3 ms | one blocked SMTP send 10.01 s
```

**The floor does not track transport health** — the dead arm is *faster*, and
one blocked send is 250× a whole save. (The healthy arm is slower only because
a delivering worker is issuing its own DB round trips and contending; that is
honest, and it is the opposite of the failure mode being tested for.)

Live, end to end, against a relay that refuses connections:

```
>>> Submit transition took 145 ms against a DEAD SMTP relay
outbox: {'status':'dead','attempts':5,'last_error':'Connection error: … actively refused it'}
```

**3 · File in CI, SMTP by config.** lettre's own `FileTransport` covers the
capture criterion — `.eml` plus an envelope `.json`, so **no `topcoat-mail`
FileTransport and no second file transport** were needed, exactly as the ruling
asked me to confirm. `FRUST_MAIL=file|smtp` selects at boot and an unusable
value **refuses the boot** rather than falling back:

```
FRUST_MAIL=smpt          → mail_config_refused (must be 'file' or 'smtp')
FRUST_MAIL_DIR=Z:/nope   → mail_config_refused (directory not usable)
```

A silent fallback to `file` would be a kernel that writes production invoices to
a directory nobody reads and reports success.

**4 · Failure is bounded, observable, never silent.** Transient → requeue with
an attempt counter; permanent (SMTP 5xx, unparseable address) → dead-letter
immediately; `MAX_ATTEMPTS = 5`. Unroutable outcomes get an outbox row too —
"nothing was written" and "it failed" must not look the same. Typed in
`/metrics`, live:

```
frust_mail_retry_total{kind="transient",tenant="skeleton"} 4
frust_mail_dead_total{reason="attempts_exhausted",tenant="skeleton"} 1
frust_mail_sent_total{mode="file",tenant="skeleton"} 3
frust_mail_enqueued_total{notification="invoice_submitted",...} 3
frust_mail_queue_depth{tenant="skeleton"} 0
frust_mail_transport{mode="file"} 1
```

`frust_mail_dead_total{reason=…}` distinguishes `permanent` ·
`attempts_exhausted` · `no_recipients` · `template`, because "the email never
arrived" has four different fixes.

**5 · Proven live through `frust serve` + the browser.** `pnpm mail`, 15/15, a
real clerk clicking Submit in a real Chromium:

```
From: frust@frust.local
Subject: Approval needed: Northwind Traders
To: approver@frust.local

Invoice sales_invoice:1imk543rx0bpgc4yjyco from Northwind Traders for 37.5
is awaiting your approval.
State: Submitted for Approval
```

Asserted on **content**: the approver resolved from `role:manager`, the subject
interpolated, the record id, the post-transition state, the envelope, and the
outbox row reaching `status: sent, attempts: 1`. Also asserted: a plain **save**
sends nothing — the rule is scoped to the transition.

## Findings

### 1 · Root Basic auth costs 16.5 ms per request (kernel-wide, pre-existing)

The save floor moved 2.99 → 50.9 ms when a rule was attached, and the WO's
criterion did not explain it. Chasing it down, with the changefeed and the
outbox both eliminated as suspects:

| same query, same process | median |
|---|---|
| `sql_root` (Basic root:root) | **16.56 ms** |
| `sql_as` (cached session Bearer) | **0.79 ms** |

**21×.** SurrealDB verifies the root password (argon2, deliberately slow) on
*every* request; a Bearer token is a signature check. This is not a mail
problem — it is a property of every `sql_root` in the kernel, and it is the same
shape as WO-026's finding that metadata-per-request was a third of the write
ceiling. WO-043 is simply the first feature to put a root query on the
**per-request write path**, which is why it surfaced here.

**The fix is proven and out of this WO's boundary.** `POST /signin` with root
credentials returns a JWT that `/sql` accepts, at **200 µs server-side**. A
cached root token in `db.rs` would take ~16 ms off every metadata read, job
claim, rollup drain and boot query in the kernel. It also touches ADR-013's
keyguard, whose self-forge probe deliberately drives the auth path — so it is an
escalation, not a thing to slip into an email WO. **Recommended as its own
order.**

Reported rather than absorbed: the notification path costs a couple of root
round trips (recipient lookup + outbox insert) and the test now bounds it
against a **measured** root RTT, not a constant.

### 2 · Approval emails say "37.5", not "37.50"

The template renders the stored decimal verbatim, per the WO's own boundary
(compare-never-compute, no arithmetic). But SurrealDB stores `37.50` as `37.5`,
so that is what the approver reads. **Correct by the rule as written, and
probably not what an ERP wants in customer-facing mail.** Money *formatting* is
not *arithmetic*, and the WO banned the second without ruling on the first —
so I did not invent a formatter. Flagged for a ruling.

### 3 · A missing `notification` table is an ERROR, not an empty set

SurrealDB answers `SELECT … FROM notification` on a pre-v5 database with
`The table 'notification' does not exist` — an error. Left alone, that would
have logged `lvl:error` on **every save** in any database predating meta v5,
which is precisely how operators are trained to ignore errors (WO-033). Mapped
that one condition to "no rules", the same surgical shape as the WO-040
`bodies()` precedent, with a test asserting a genuinely-broken query still fails
loudly.

### 4 · `app_user.email` has no kernel surface

`role:` recipients need addresses, and meta v5 adds the column — but `app_user`
is write-closed by WO-008's identity hardening, so an address can only be set
out of band. Pre-existing shape (users are already provisioned that way), now
load-bearing for a user-facing feature. A `POST /user` manager surface is its
own order; noting it rather than widening the identity table's permissions on my
own initiative.

## Instrument failures, all mine

1. **The first save-floor "control" was measuring nothing.** Sequential arms:
   3.7 ms control against 41.9 ms treatment — then the same no-rule operation
   measured 25 ms in a separate probe. Two contradictory numbers for one
   operation. Rebuilt as an **interleaved** design (no-rule → rule → no-rule →
   rule); it returned 2.9 / 26.4 / 2.9 / 26.4, reproducible in both directions,
   and only then was the number trustworthy.
2. **I curve-fitted a bound to a number I had already seen** (`< 60 ms`, chosen
   after observing 48 ms). It would have passed on any machine and caught
   nothing. Replaced with a yardstick measured in the same test — one root round
   trip — so the assertion means the same thing on different hardware.
3. **The e2e asserted against the wire encoding.** lettre wraps
   quoted-printable at 76 columns with a soft `=` break, which split
   `for 37.5 is awaiting` mid-assertion. The check was failing on the encoding
   while the message was correct — and it would have passed or failed depending
   on how long the customer's name happened to be. Decode the soft breaks, then
   assert.
4. **A filter that silently matched everything.** An empty record key made
   `endswith('')` true for every outbox row, reporting six cheerful `sent`
   rows that had nothing to do with the test. Added a guard that refuses to run
   the query with an empty key.
5. **Two SurrealDB caveats, one of them already written down two functions
   above the code I wrote.** `CREATE … CONTENT {…} SET x = …` does not parse
   (fixed by defaulting `enqueued_at` in the DDL, which is better anyway), and
   `ORDER BY` requires the idiom in the projection — a caveat `claim_next`
   carries in a comment I had read that same session.

## Regression — zero, asserted

`pnpm workflow` 18/18 · `pnpm sse` 8/8 · `pnpm mail` 15/15 · kernel suites
green: `surql_monopoly` (mail.rs stayed **outside** the allowlist),
`tenancy_monopoly` 4/4, `keyguard_canary` 4/4 (**ADR-013's three-way proof
intact** — meta v5 touched `identity_ddl`), `boot_discipline` 4/4,
`identity_hardening` 2/2, `worker_queue` 4/4, `rest_surface`, `acceptance_e2e`,
`workflow_engine`, `metadata_sync`, `permission_proof`, `hook_document_fidelity`,
`money_reconciliation`, `observability_e2e`, `meta_cache_invalidation`,
`accounting_seed_e2e`, `aggregates_ladder`, `row_write_permission`,
`demo_app_e2e`, `app_lifecycle`, `decimal_rollups`, `session_cache_per_tenant`.

## Dev-store mutations (stated, per standing discipline)

- `skeleton` meta migrated **v4 → v5** (`--accept-meta-migrations`, as the
  kernel already runs): adds `notification`, `_frust_mail`, `app_user.email`.
- `app_user:manager.email = 'approver@frust.local'` — set via the DB, because
  identity is write-closed (finding 4).
- `notification:invoice_submitted` created and **left in place** (it is the
  live demonstration).
- Several `sales_invoice` rows from the e2e runs; captured mail in
  `frust-e2e/mail-capture/` (gitignored scratch).
- The kernel now runs with `FRUST_MAIL=file FRUST_MAIL_DIR=D:/Dev/rust/frust-e2e/mail-capture`.

## Related
[[WO-043 Email Batteries]] · [[2026-07-30 WO-043 mail transport decision]] ·
[[ADR-004 Topcoat for Desk v0]] · [[ADR-010 Rollup Ladder]] ·
[[ADR-009 Docstatus Lattice]] · [[ADR-007 Decimal Money]] ·
[[ADR-008 Fail-Closed Boot]] · [[2026-07-29 WO-042 frust ui re-skin]]
