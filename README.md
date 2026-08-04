# Frust

A Rust rewrite of a Frappe-style, metadata-driven ERP framework — *not a port,
an upgrade*. Everything is metadata: DocTypes, permissions, workflows, and
reports are data the kernel reads, so most changes need no recompile.

Three moving parts:

- **the metadata kernel** (`frust serve`) — a headless HTTP/JSON service:
  broker, permission compiler, hooks, background workers, and REST surface.
- **SurrealDB** — the store. Row permissions are compiled once and enforced by
  the database under each caller's own session.
- **the Desk** — a server-rendered, REST-only client. It is one consumer of the
  kernel's surface, not a privileged one; anything the Desk does, your own
  client can do.

Plugins are WebAssembly components with a capability surface, so untrusted app
code runs contained rather than trusted.

**Maturity: pilot-grade, not production-ready.** The honest, machine-checked
scorecard is [`maturity/CAPABILITIES.md`](maturity/CAPABILITIES.md) — at the
time of writing, no capability is marked `production-ready`. Read it before you
deploy anything.

```
git clone --recurse-submodules https://github.com/cloudsoko/frust.git
```

## Layout

| path | what it is | tracked |
|---|---|---|
| `frust-kernel/` | the metadata kernel (`frust serve`) — broker, hooks, workers, REST | source |
| `frust-desk/` | the Desk: a server-rendered, REST-only client (headless contract) | submodule |
| `frust-e2e/` | the browser evidence harness (`pnpm workflow` / `sse` / `mail` / `print`) | source |
| `topcoat/` | **submodule** -> the maintained Topcoat fork used by the Desk | submodule |
| `frust-ui/` | **submodule** -> an earlier UI foundation, retained for its history | submodule |

The kernel's HTTP surface is documented, executable, and the recommended place
to start after the build: [`frust-kernel/docs/`](frust-kernel/docs/) — every
example is run against a live kernel by the evidence harness, so a stale example
turns the suite red.

## Bootstrap

The repository pins Rust 1.96.0, `wasm32-wasip2`,
`@bytecodealliance/jco-transpile` 0.5.2, and pnpm 11.1.2.
Install Git, rustup, Node/Corepack, and PowerShell, then run from the repository
root:

```powershell
pwsh ./scripts/frust.ps1 bootstrap
pwsh ./scripts/frust.ps1 doctor
```

On Windows PowerShell, use `powershell` in place of `pwsh`. Bootstrap initializes
the recorded submodules, installs the pinned Rust target, builds both runtime
guests, transpiles the same script engine for the browser, verifies every
artifact checksum, and builds the kernel and Desk with locked dependencies.
`doctor` is read-only and reports exact remediation for missing tools, stale
artifacts, uninitialized submodules, and an unavailable store.

The guest sources now live in `wasm-spike/`. Generated kernel artifacts remain
ignored at `wasm-spike/artifacts/`; the Desk's browser core is committed as a
served asset. `wasm-spike/artifacts.lock.json` binds both outputs to one source
build so they cannot silently diverge. The old-world components are tracked
compatibility fixtures, not current runtime artifacts.

The benchmark harness, development database and its live data, and the 1 M-row
scale fixture remain machine-local for the reasons documented in `.gitignore`.

## Running it

Three processes are required. Use SurrealDB 3.2.3; the kernel's stored queries
and authentication behavior are tested against that version.

```bash
# 1. the store
surreal start --user root --pass root --bind 127.0.0.1:8899 surrealkv://<data-dir>

# 2. the kernel  (:8790)
cd frust-kernel && cargo run --release --bin frust -- serve

# 3. the Desk    (:3000)
cd frust-desk && cargo run --release
```

Or run the development store in Docker:

```bash
docker run --rm --name frust-surreal -p 127.0.0.1:8899:8000 \
  -v frust-surreal-data:/data surrealdb/surrealdb:v3.2.3 \
  start --user root --pass root rocksdb:/data/frust.db
```

Notable environment switches, all fail-closed on an unrecognised value:

| var | default | effect |
|---|---|---|
| `FRUST_TENANCY` | `single` | topology: `single` · `database-per-tenant` · `namespace-per-tenant` · `namespace-per-tenant-env` |
| `FRUST_MAIL` | `file` | `file` captures `.eml` to `FRUST_MAIL_DIR`; `smtp` relays via `FRUST_MAIL_SMTP` |
| `FRUST_ROOT_AUTH` | `jwt` | `basic` is the documented escape hatch back to password-based root auth |
| `FRUST_LOG` | `info` | `debug` emits per-call `db_call` spans — the trace instrument the perf work uses |

`cargo` is capped at `--jobs 2` on the dev machine: rustc OOMs on
`surrealdb-core` above that.

## Install and drive an app

An app is metadata. You install one through the running kernel with a single
call — a JSON manifest of DocTypes and workflows — and it is live with no
recompile:

```bash
KERNEL=http://127.0.0.1:8790

curl -s -X POST $KERNEL/app/install \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"manifest_version":1,"name":"expenses","version":"1.0.0",
       "doctypes":[{"name":"expense_claim","submittable":true,
         "fields":[{"fieldname":"purpose","label":"Purpose","fieldtype":"Data","required":true},
                   {"fieldname":"amount","label":"Claim Amount","fieldtype":"Currency","required":true}]}]}'
```

The full, executable walk-through — log in, discover the schema, read, write,
move a document through its workflow, and subscribe to changes, all in plain
`curl` — is [`frust-kernel/docs/byo-quickstart.md`](frust-kernel/docs/byo-quickstart.md).
The `frust-e2e` suites drive the same lifecycle through a real browser as
runnable proof.

User accounts are provisioned by the operator: identities are write-closed, so
no REST call creates them.

## Submodules

`frust-desk/` is the independently versioned Desk. `topcoat/` is a maintained
fork, not a floating consumer dependency. `frust-ui/` is an earlier UI
foundation retained for history. All three public repositories are pinned to
exact commits by the root repository; branch names in `.gitmodules` are update
channels, not build inputs.

The release preflight rejects SSH-only or unexpected submodule URLs, mismatched
pins, and (in protected CI) any pinned commit that cannot be fetched
anonymously over HTTPS.

```bash
git submodule update --init --recursive   # after clone
git submodule sync --recursive            # after a URL change
```

## Tests

```bash
cd frust-kernel && cargo test --jobs 2
cd frust-desk   && cargo test --jobs 2          # includes the CSS-seam guard
cd frust-e2e    && pnpm workflow && pnpm sse && pnpm print && pnpm mail
```

The kernel suite needs SurrealDB on `:8899`; the browser suites need the kernel
and Desk running. Several suites are guards rather than feature tests —
`surql_monopoly`, `tenancy_monopoly`, `keyguard_canary` and the Desk's
custom-property/variant checks each exist because they caught a real defect, and
each has been shown to fail on the defect it was written for.

**Use pnpm, not npm** — npm project installs are broken machine-wide on the dev
box (an injected `allow-scripts` config).

## License

Frust is licensed under the GNU Affero General Public License version 3 only
(`AGPL-3.0-only`). See `LICENSE` and `NOTICE`. Frust Desk and Frust UI carry the
same license in their independent repositories. Third-party components retain
their own licenses and are identified in `NOTICE` and each release SBOM.
