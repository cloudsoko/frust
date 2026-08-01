# Frust kernel — REST surface

The complete HTTP surface of `frust serve`. Every route below is read from
`kernel/src/rest.rs`; every example in these docs is executed against a live
kernel by `frust-e2e/docs.spec.mjs`, and the route table is checked against the
source by the same harness — a route that appears here and not in the code (or
the reverse) fails the run.

Stability promises: see [evolution-policy.md](./evolution-policy.md).
New to the surface: start with [byo-quickstart.md](./byo-quickstart.md).
Known warts: [gaps.md](./gaps.md) — named, not papered over.

---

## Shape of every request and response

- **Transport**: HTTP/1.1, JSON in and out, `Content-Type: application/json`.
- **Body**: JSON object, or empty. A non-empty body that is not valid JSON is
  `400 invalid-value` with `bad json body: …`. A body that is not valid UTF-8
  is `400 invalid-value` with `request body is not valid UTF-8`.
- **Success**: `200` with a JSON object. The object's keys are documented per
  route below.
- **Failure**: a non-2xx status with `{"error": {"kind": "...", ...}}`. `kind`
  is the stable discriminant; the other fields carry detail.
- **Method is not routed on.** The kernel dispatches on the path only, so any
  method reaches any route. Use `POST` for everything that takes a body and
  `GET` for `/health` and `/metrics`. This is a wart, recorded in
  [gaps.md](./gaps.md) — do not build on it.

### Authentication

Three tiers:

| tier | how | routes |
|---|---|---|
| **none** | — | `/health`, `/metrics`, `/login` |
| **session** | `Authorization: Bearer <token>` | reads, writes, workflow, realtime, plugin routes |
| **manager** | session whose `app_user.role == "manager"` | metadata, apps, audit, revoke, reclaim, outbox |

A missing or unknown token is `401` (`{"kind":"permission-denied","detail":"E_UNAUTHENTICATED"}`).
A valid token without the manager role on a manager route is `403`
(`{"kind":"permission-denied","detail":"manager role required"}`).

**The token carries its tenant.** `/login` returns `<TenantId>.<random>`, where
the prefix is the tenant *the kernel resolved and authenticated against* — not
a string the client chose. Send it back verbatim. Presenting another tenant's
prefix looks the secret up in **that** tenant's database, where it does not
exist, so spoofing fails closed as `401` rather than crossing a boundary.

### Error kinds and their statuses

Read from `status_for()`:

| `kind` | status | meaning |
|---|---|---|
| `permission-denied` with `detail: "E_UNAUTHENTICATED"` | **401** | no/unknown token |
| `permission-denied` with `detail: "FRUST:E_AUTH_REJECTED"` | **401** | `/login` refused the credentials |
| `permission-denied`, `field-not-readable`, `identity-unresolved` | **403** | authenticated, not allowed |
| `unknown-doctype` | **404** | no such DocType or record |
| `workflow-denied` | **422** | a rule refused a transition (`code` carries `FRUST:E_WORKFLOW:*`) |
| `hook-rejected`, `hook-cycle`, `hook-depth-exceeded` | **422** | app logic refused the write |
| `tenant-throttled` | **429** | door budget spent; `retry_after_ms` says when |
| `db` containing `FRUST:E_SUB_BUDGET` | **429** | live-subscription budget spent; poll instead |
| `db`, `write-conflict-exhausted` | **500** | storage-tier failure |
| anything else (e.g. `invalid-value`) | **400** | malformed request |

**422 is the interesting one**: a rejection by an app's own rule is not a bad
request and not a server error. It carries the app's message and, since WO-050,
the app that raised it.

### Conventions a client must know

- **Money is a decimal string, never a float.** `"37.50"`, not `37.5`. The
  kernel stores decimals; JSON numbers with fraction digits are floats and are
  refused on money fields. Reads return money as a string. SurrealDB strips
  trailing zeros on write, so `"25.00"` reads back as `"25"` — pad for display,
  never round.
- **Typed values**: any field value may be written in the explicit form
  `{"kind":"decimal","v":"19.99"}` when inference is not enough. Inference
  handles strings, bools, integers, nulls, objects and arrays.
- **Realtime ticks carry no row data** — only `{action, id}`. Re-read through
  `/read/{doctype}` so row permissions are applied by the database on the
  refetch. A tick is a hint that something changed, never the change itself.
- **`docstatus`**: `0` draft, `1` submitted, `2` cancelled. The lattice permits
  `0→1`, `1→2`, `0→2` is refused, and nothing leaves `2`.

---

## Routes

### `GET /health` — no auth

Liveness for the *process*; the only route with no tenant.

```
→ GET /health
← 200 {"ok": true}
```

### `GET /metrics` — no auth

Prometheus text exposition (`text/plain; version=0.0.4`), not JSON. Handled
before routing, so it never touches a tenant or a session.

### `GET /ready` — no auth

Readiness, as opposed to `/health`'s liveness: what has actually **booted**.

```
→ GET /ready
← 200 {"ready": true,
       "tenants": [{"tenant":"acme","meta_version":8,"doctypes":10,
                    "orphan_columns":["sales_invoice.crm_followup"]}]}
```

**Read the ~25 s boot window honestly:** the kernel does not accept connections
until boot completes, so `/ready` answering at all already means it is up — the
window shows as a *refused connection*, not as `ready: false`. A health check
must budget for it or it will kill a kernel that is working (the WO-019 ops
caveat). What this endpoint adds over `/health` is the positive signal plus the
boot facts to assert against — meta version, DocType count, and any orphan
columns carried.

### `POST /login` — no auth

```
→ POST /login   {"user": "manager", "pass": "…", "tenant": "acme"}
← 200 {"token": "acme.9f3…", "user": "manager", "role": "manager", "tenant": "acme"}
```

`tenant` is optional and resolved in this order: the explicit field, then the
request's subdomain, then — **only when the process serves exactly one tenant**
— that one. A hint that is present but does not resolve is refused outright,
never falling through to "the only tenant".

An empty `user` is `400`. **Bad credentials are `401`** with
`{"kind":"permission-denied","detail":"FRUST:E_AUTH_REJECTED"}` — and a wrong
password is indistinguishable from an unknown user, deliberately, so the
endpoint is not a user-enumeration oracle.

A signin that fails for a reason that is *not* the credentials — the tenant's
database missing, the store half-provisioned — is still a `500` naming what is
wrong. That distinction is drawn from SurrealDB's response body, not its status:
it answers `404` for both a wrong password and a vanished database (WO-055).

### `POST /logout` — session

Deletes the session row and bumps the generation, so the token is dead
immediately rather than at cache expiry. `← 200 {"ok": true}`

### `POST /revoke/{user}` — manager

Kills every session for that user. `← 200 {"ok": true, "user": "clerk1", "revoked": 22}`

### `GET /meta` — session

Every DocType the caller may see. `← 200 {"doctypes": [...]}`

### `GET /meta/{doctype}` — session

One DocType's metadata: fields, types, options, client rules, workflow slot.
`← 200 {"doctype": {...}}`

### `POST /read/{doctype}` — session

```
→ POST /read/sales_invoice
  {"filter": {"path":"customer","op":"eq","value":"Northwind Traders"},
   "fields": ["customer","total"],
   "order":  {"path":"total","dir":"desc"},
   "limit":  20, "start": 0}
← 200 {"rows": [ {...}, … ]}
```

`filter` is the ADR-006 **structured filter** — a typed tree, never query text.
SurrealQL in any position is a parse error, not an escape. Row permissions are
applied by the database under the caller's own session, so a filter cannot
widen what the caller may see.

### `POST /write/{doctype}` — session

```
→ POST /write/sales_invoice   {"doc": {"customer": "…", "total": 25.00}}
← 200 {"action": "created", "record": "sales_invoice:…",
       "created": { "id": "sales_invoice:…", "docstatus": 0, … }}

→ POST /write/sales_invoice   {"record": "uc1ebw…", "doc": {"customer": "…"}}
← 200 {"action": "updated", "record": "sales_invoice:uc1ebw…", "created": { … }}
```

**Create vs update is inferred from the presence of `record`, and that is the
only discriminant.** There is no `op` field — and since WO-055 an unknown
top-level key is **refused** (`400 FRUST:E_UNKNOWN_FIELD`) rather than ignored,
naming both the offending key and the accepted ones. It used to be discarded in
silence, so a client sending `{"op":"create","record":…}` got an update and no
complaint.

`action` is `"created"` or `"updated"`; `record` is the id. **`created` carries
the row and is DEPRECATED** — it says `created` on updates too, which is why
`action` exists. It is unchanged and still populated (removing it would be a
breaking change); read `action` instead.

Updates are partial: only the fields you send, plus whatever hooks changed, are
persisted.

Hooks fire on this path; a rejection is `422 hook-rejected` carrying the app's
message and its app.

**A write that stores nothing is refused, never reported as done.** If the
database accepts the statement but persists no row — a write-closed table
(kernel-maintained rollups and registries), or a row your role may not write —
the answer is `403 permission-denied` with `E_WRITE_NO_ROWS`, naming the table.
It is never a `200` carrying a null record. (Both the create and update halves;
WO-020 fixed update, WO-057 create.)

### `POST /aggregate/{doctype}` — session

```
→ POST /aggregate/sales_invoice
  {"group_by": ["customer"], "metrics": [{"metric":"sum","path":"total"}]}
← 200 {"rows": [ {...} ]}
```

### `GET /workflow/{doctype}/{key}` — session

The transitions available *to this caller* from the record's current state —
already filtered by role, so a client renders buttons without knowing the rules.

### `POST /transition/{doctype}/{key}` — session

```
→ POST /transition/sales_invoice/1imk54…   {"action": "Approve"}
← 200 { "id": "sales_invoice:…", "workflow_state": "Approved", "docstatus": 1, … }
```

Refusals are `422 workflow-denied` with a `code`:
`FRUST:E_WORKFLOW:UNKNOWN_ACTION`, `…:WRONG_STATE`, `…:ROLE_DENIED`,
`…:UNMANAGED`. A hook may also refuse a transition (`422 hook-rejected`) — and
when it does, nothing is written: the document keeps its state and docstatus.

### `GET /audit/{doctype}/{key}` — manager

The record's changefeed history.
`← 200 {"record": "…", "total": 7, "entries": [...]}`

### `GET /lag/{rollup}` — session

Staleness of a Tier-2 worker rollup.
`← 200 {"rollup": "…", "source": "…", "cursor": {...}, "pending": 0}`

### `POST /subscribe/{doctype}` — session

```
→ POST /subscribe/sales_invoice
← 200 {"sub": "…", "budget": 20}
```

### `GET /events/{sub}` — session

```
→ GET /events/{sub}
← 200 {"alive": true, "events": [ {"action":"update","id":"sales_invoice:…"} ]}
```

### `POST /unsubscribe/{sub}` — session

```
→ POST /unsubscribe/{sub}
← 200 {"ok": true}
```

The subscription runs under the **subscriber's own** database credential, so
the push path is filtered by the same row rules as a read. Ticks are
`{action, id}` only — refetch through `/read`. Exceeding the per-table budget
is `429`, which is a capacity answer, not an error: poll instead.

### Metadata writes — manager

| route | does |
|---|---|
| `POST /doctype` | create/replace a DocType; returns `{created, applied, orphan_columns}` |
| `POST /doctype/{name}/script` | save a client script; returns `{doctype, script, bytes}` |
| `POST /doctype/{name}/reclaim` | drop an orphan column; **requires `{"acknowledge": true}`**, refusal names the column and the rows that still hold data |
| `POST /notification` | create a notification rule; returns `{created, doctype, event}` |
| `GET /mail/outbox` | delivery state as data; returns `{outbox: [...]}` |

### Apps — manager

| route | does |
|---|---|
| `GET /app` | installed apps: `{apps: [...]}` |
| `POST /app/plan` | dry-run an install; returns the plan incl. `destructive` |
| `POST /app/install` | install a bundle |
| `POST /app/update` | publish a new version; destructive changes need `{"acknowledge": true}` |
| `POST /app/{name}/disable` | make the app unavailable; its routes become `404` |
| `POST /app/{name}/enable` | restore it — restoration, not reconstruction |
| `POST /app/{name}/uninstall` | metadata detaches, **data remains** |

### `POST|GET /app/{app}/{path}` — session

Dispatch to a route an installed app declared. Same authority, same throttle,
same audit as a kernel route — an app route is not a side door. A disabled or
unknown app is `404`, deliberately indistinguishable.

### `POST /enqueue/{kind}` — manager

Enqueue a background job. `← 200 {"job": "job:…"}`

---

## Throttling

Per-tenant door budgets return `429 tenant-throttled` with `retry_after_ms`.
The Desk tier in front of the kernel sheds excess with `503` and a
`Retry-After` header. Both are deliberate answers under load, not failures:
a client should back off and retry, not treat them as errors.
