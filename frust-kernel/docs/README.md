# Frust kernel — API documentation

The kernel is headless: it serves JSON over HTTP and nothing else. The Desk is
one client of this surface, not a privileged one — anything the Desk does, your
client can do.

| | |
|---|---|
| [**byo-quickstart.md**](./byo-quickstart.md) | Start here. Login → schema → read → write → workflow → realtime, in plain `curl`. |
| [**rest-api.md**](./rest-api.md) | The complete route reference: auth tiers, request/response shapes, error kinds and statuses, conventions. |
| [**evolution-policy.md**](./evolution-policy.md) | **Normative.** What is promised, what is not, and what a breaking change costs. Read before you depend on anything. |
| [**gaps.md**](./gaps.md) | Known warts, named rather than papered over — including one escalation. |

## These docs are executable

Every request shape in these pages is run against a live `frust serve` by
`frust-e2e/docs.spec.mjs`, which also cross-checks the documented route table
against the routes extracted from `kernel/src/rest.rs`. A route added without
documentation fails the run; a documented route that no longer exists fails the
run; an example whose response shape has changed fails the run.

```bash
# with a kernel running on :8790
cd frust-e2e && node docs.spec.mjs
```

That is deliberate. This project's rule for evidence is that *an example you
cannot re-run is an anecdote*, and documentation is evidence about the surface.

## The three things most likely to trip you up

1. **Money is a decimal string, never a float.** `"37.50"`.
2. **Realtime ticks carry no row data** — `{action, id}` only. Refetch through
   the read door so row permissions apply.
3. **Branch on `kind` (and `code`), never on `detail` prose.** `detail` is
   explicitly unpromised.
