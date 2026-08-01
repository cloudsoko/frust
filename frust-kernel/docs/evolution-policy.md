# Surface evolution policy

**Normative.** This is the promise the documented REST surface makes to a
client that is not the Desk. It exists because ADR-016 ratified BYO-frontend as
*first-class supported*, and priced it: "supported" is only true once the
surface is documented and its stability promised. This is that promise.

## 1. What is promised

**Only what is documented.** A route, field, or error `kind` that appears in
[rest-api.md](./rest-api.md) is covered by everything below. Anything else —
an undocumented route, an extra key in a response, a `detail` string's wording
— **carries no promise** and may change without notice. If you depend on it,
you have taken a private dependency on an implementation detail.

Specifically **not** promised:
- the human-readable `detail` text of an error (the `kind` and any `code` are);
- the order of keys, or extra keys beyond those documented;
- routes reachable by an HTTP method other than the documented one;
- anything observable only through `/metrics`, logs, or trace fields.

## 2. Additive-only within a major

Within a major version the surface grows **additively**:

- new routes may be added;
- new **optional** request fields may be added;
- new keys may be added to a response object;
- new `kind` values may be added for genuinely new failure modes.

And it does not shrink or shift:

- no route is removed or repathed;
- no request field becomes required, and no existing field changes meaning or
  type;
- no documented response key is removed, renamed, or changes type;
- no documented `kind` changes its HTTP status;
- no route changes its auth tier **except to become less restrictive**.

**Clients must tolerate additive change.** Ignore response keys you do not
know; do not treat an unrecognised `kind` as a crash — map it by status class.
A client that breaks when a key is added is not covered by this policy.

## 3. Breaking changes are versioned majors

A change that violates §2 requires a new major, announced with a deprecation
notice before the old one stops being served. The deprecation notice states the
replacement and the date the old surface stops being served; both surfaces run
concurrently for the announced window.

## 4. The unit of additivity is the thing that is *served*

This is the same rule ADR-006 edge-1 carries for the WASM capability surface,
and it was **measured** there in WO-053 rather than assumed: growing an existing
interface broke every component; only a new world beside the old was additive.

The REST analogue: **add a route, do not widen a route's contract.** A new
shape wants a new path, not a new required field on an old one. When in doubt,
the question is not "is this a small change?" but "can a client written against
yesterday's document still be correct?" — if no, it is a major.

## 5. How this is enforced

Not by intention. `frust-e2e/docs.spec.mjs`:

- executes **every** example in these docs against a live `frust serve` and
  asserts the documented response shape;
- cross-checks the documented route table against the routes extracted from
  `kernel/src/rest.rs`, and **fails if either side has a route the other does
  not**.

So a route added without documentation fails the run, and a documented route
that no longer exists fails the run. An example you cannot re-run is an
anecdote; that applies to a promise as much as to a measurement.

## 6. What this policy does not cover

- **The Desk's own HTML surface.** It is a client, not an API.
- **The database.** Direct SurrealDB access is not a supported door — the
  kernel's permission compiler is the only path with row rules applied. Reading
  or writing tables out of band is outside this policy and always has been.
- **Bundle/manifest schema.** Governed by the app-lifecycle rules
  (`manifest_version`), not by this document.
