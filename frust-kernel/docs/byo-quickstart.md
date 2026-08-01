# BYO frontend — quickstart

Drive a Frust kernel from any client, with nothing but HTTP and JSON. No
Topcoat, no Desk, no SDK. This is the "BYO-frontend is first-class" claim of
ADR-016, written as the thing you would actually run.

Every request below is executed against a live kernel by
`frust-e2e/docs.spec.mjs`. If it stops working, that harness goes red.

Prerequisites: a running `frust serve` (default `http://127.0.0.1:8790`) and a
user. The examples use the dev fixture's `manager` / `clerk1`.

---

## 1. Log in

```bash
KERNEL=http://127.0.0.1:8790

TOKEN=$(curl -s -X POST $KERNEL/login \
  -H 'Content-Type: application/json' \
  -d '{"user":"clerk1","pass":"pw-clerk1"}' \
  | python -c 'import sys,json; print(json.load(sys.stdin)["token"])')
```

The response is `{"token","user","role","tenant"}`. Two things to hold onto:

- **`token` goes back verbatim** as `Authorization: Bearer <token>`. It looks
  like `acme.9f3…`; the prefix is the tenant *the kernel* resolved, not a
  string you chose. Do not construct or edit it.
- **`role` is the caller's role as the database holds it**, and it is what the
  kernel enforces against — not a claim you send. Use it to decide what to
  *render*; never to decide what is *allowed*. The kernel is the enforcement.

If the process serves several tenants, add `"tenant":"acme"` (or log in on the
tenant's subdomain). With exactly one tenant registered you may omit it.

## 2. Discover the schema

Nothing here is hardcoded — the whole point of a metadata kernel is that a
client can be written once and follow the metadata.

```bash
curl -s $KERNEL/meta -H "Authorization: Bearer $TOKEN"
# {"doctypes":[ … ]}

curl -s $KERNEL/meta/sales_invoice -H "Authorization: Bearer $TOKEN"
# {"doctype":{ "fields":[{"fieldname":"customer","fieldtype":"Data"}, … ] }}
```

Render your form from `fields`. `fieldtype` tells you the input; `options`
carries a Link's target DocType; `depends_on` / `read_only_when` /
`required_when` are the declarative client rules, evaluated in your UI.

## 3. Read a list

```bash
curl -s -X POST $KERNEL/read/sales_invoice \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"fields":["customer","total"],
       "order":{"path":"total","dir":"desc"},
       "limit":20}'
# {"rows":[ … ]}
```

Filters are a **structured tree**, never query text:

```json
{"filter": {"path": "customer", "op": "eq", "value": "Northwind Traders"}}
```

You cannot write SurrealQL here; it is not an escape hatch that is blocked, it
is a shape that has no place to put one. Row permissions are applied by the
database under *your* session, so two users running the same read get different
rows — correctly, without your client filtering anything.

## 4. Write

```bash
curl -s -X POST $KERNEL/write/sales_invoice \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"doc":{"customer":"Northwind Traders","total":25.00,
              "lines":[{"item":"Sprocket","qty":"2","rate":"12.50","amount":"25.00"}]}}'
# {"created":{"id":"sales_invoice:uc1…","docstatus":0, …}}
```

To update, add `"record": "<key>"` — the presence of `record` is what makes it
an update; there is no `op` field. Updates are partial.

**Money.** Send and expect decimal *strings* on money fields. `"25.00"` reads
back as `"25"` — the store drops trailing zeros — so pad to scale for display
and never round. A value with *more* places than the field's scale is a defect
to surface, not to round away.

**An app rule may refuse your write.** That is `422`:

```json
{"error":{"kind":"hook-rejected",
          "message":"Invoice does not balance: … (rejected by the owner, app 'acct')"}}
```

Show `message` to the user. It is written by the app author for exactly that
purpose, and it names which app refused.

## 5. Move it through its workflow

Ask what this caller may do, rather than hardcoding buttons:

```bash
curl -s $KERNEL/workflow/sales_invoice/$KEY -H "Authorization: Bearer $TOKEN"
```

The response is already filtered by role and current state — render one button
per action and you have a correct UI without knowing the rules.

```bash
curl -s -X POST $KERNEL/transition/sales_invoice/$KEY \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"action":"Submit"}'
# {"workflow_state":"Submitted for Approval","docstatus":0, …}
```

Refusals are `422 workflow-denied` with a machine `code`
(`FRUST:E_WORKFLOW:ROLE_DENIED`, `…:WRONG_STATE`, …) — branch on `code`, show
`detail`.

Note that `Submit` here leaves `docstatus` at **0**. Crossing to `1` is the
manager's `Approve`, and that is the transition an app's `on_submit` hook fires
on. Workflow state and docstatus are two different things: the workflow is the
app's business states, docstatus is the kernel's immutable lattice
(`0` draft → `1` submitted → `2` cancelled, no going back).

## 6. Stay fresh

```bash
SUB=$(curl -s -X POST $KERNEL/subscribe/sales_invoice \
  -H "Authorization: Bearer $TOKEN" | python -c 'import sys,json;print(json.load(sys.stdin)["sub"])')

curl -s $KERNEL/events/$SUB -H "Authorization: Bearer $TOKEN"
# {"alive":true,"events":[{"action":"update","id":"sales_invoice:uc1…"}]}
```

**A tick carries no row data — only `{action, id}`.** Re-read through
`/read/{doctype}` when one arrives. That is deliberate: the refetch goes
through the read door, so the database applies row permissions to what you get
back. A push that carried the row would be a second, unguarded read path.

`429` on subscribe means the per-table budget is spent. That is a capacity
answer, not an error: fall back to polling.

## 7. Clean up

```bash
curl -s -X POST $KERNEL/logout -H "Authorization: Bearer $TOKEN"
```

The token is dead immediately — the session row is deleted and the cache
generation bumped, not left to expire.

---

## Things that will bite you if you skip them

1. **Do not send money as a JSON number with fraction digits.** `25.00` in JSON
   is a float; money is decimal. Integers coerce fine; anything with a fraction
   should be a string.
2. **Do not parse `detail` strings.** Branch on `kind` and, where present,
   `code`. `detail` is human prose and is explicitly unpromised.
3. **Do not treat an unknown `kind` as a crash.** New kinds may be added within
   a major; map by status class and degrade.
4. **Do not read the database directly.** The permission compiler is only on
   the kernel's path; out-of-band SQL bypasses row rules and is not a supported
   door.
5. **Back off on `429`/`503`.** Both are deliberate answers under load, with
   `retry_after_ms` / `Retry-After` telling you when to come back.

What you may rely on, and for how long: [evolution-policy.md](./evolution-policy.md).
