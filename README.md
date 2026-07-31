# Frust

A Rust rewrite of a Frappe-style metadata-driven ERP framework — *not a port, an
upgrade*. Three processes: the metadata kernel, SurrealDB, and a server-rendered
Desk.

```
git clone --recurse-submodules git@github.com:AmeinEskinder/frust.git
```

## Layout

| path | what it is | tracked |
|---|---|---|
| `frust-kernel/` | the metadata kernel (`frust serve`) — broker, hooks, workers, REST | source |
| `frust-desk/` | the Desk: a server-rendered, REST-only client (ADR-004 headless contract) | source |
| `frust-e2e/` | the browser evidence harness (`pnpm workflow` / `sse` / `mail` / `print`) | source |
| `frust/` | the vault: vision, ADRs, Work Orders, dated build logs — **the decision record** | source |
| `topcoat/` | **submodule** → `topcoat-vendored` — the vendored web framework trunk with our carried patches | submodule |
| `frust-desk-ui/` | **submodule** → retired WO-037 UI foundation, kept for its history | submodule |

Every claim in the code traces to a linked ADR / Work Order / build log in
`frust/`. That is deliberate: `04 Build Log/` holds dated evidence with real
numbers, and `05 Work Orders/` holds the order each piece of work was built
against, including what it refused to do.

## ⚠️ A fresh clone cannot run the kernel yet

**`wasm-spike/` is deliberately outside this repo.** The kernel loads
`script_engine.wasm` (the Tier-2 Boa script host) and `plugin_demo.wasm` from
it at runtime — the `FRUST_ARTIFACTS` default points at
`../../wasm-spike/artifacts`. Without those artifacts the kernel refuses to
boot with `hooks_unavailable`.

Supply them by either building the spike or pointing at an existing copy:

```bash
export FRUST_ARTIFACTS=/path/to/wasm-spike/artifacts
```

Also outside the repo, each for a stated reason (see `.gitignore`): the
benchmark harness, the dev SurrealDB deployment and its live data, and the 1 M-row
scale fixture.

**One deliberate asymmetry:** the Desk's browser-side engine artifact
(`frust-desk/assets/engine/script_engine.core.wasm`, 4.1 MB) *is* committed,
while the kernel's are not. Both come from the same source — the script engine is
one source with two builds, a wasip2 component for the kernel and a jco-transpiled
core for the browser — but the browser build is a **served asset** living inside
the Desk it belongs to, so omitting it would break the Desk from a clone. The
kernel's artifacts live in the excluded tree instead. Rebuild both together or
they diverge silently.

## Running it

Three processes:

```bash
# 1. the store
surreal start --user root --pass root --bind 127.0.0.1:8899 surrealkv://<data-dir>

# 2. the kernel  (:8790)
cd frust-kernel && cargo run --release --bin frust -- serve

# 3. the Desk    (:3000)
cd frust-desk && cargo run --release
```

Notable environment switches, all fail-closed on an unrecognised value:

| var | default | effect |
|---|---|---|
| `FRUST_TENANCY` | `single` | topology: `single` · `database-per-tenant` · `namespace-per-tenant` · `namespace-per-tenant-env` (ADR-003) |
| `FRUST_MAIL` | `file` | `file` captures `.eml` to `FRUST_MAIL_DIR`; `smtp` relays via `FRUST_MAIL_SMTP` |
| `FRUST_ROOT_AUTH` | `jwt` | `basic` is the documented escape hatch back to pre-WO-044 root auth |
| `FRUST_LOG` | `info` | `debug` emits per-call `db_call` spans — the trace instrument the perf work uses |

`cargo` is capped at `--jobs 2` on the dev machine: rustc OOMs on
`surrealdb-core` above that.

## Submodules

`topcoat/` is a **vendored** trunk, not a consumer pin. It carries patches we own
until they land upstream (`tokio-rs/topcoat`); the ledger of what is carried and
why lives in `frust/02 Building Blocks/Topcoat.md`. Bump policy: the merge *is*
the probe — a "non-breaking" upstream release is only non-breaking once the
build says so.

```bash
git submodule update --init --recursive   # after clone
git submodule update --remote topcoat     # take the latest vendored trunk
```

## Tests

```bash
cd frust-kernel && cargo test --jobs 2          # 53 binaries, 331 tests
cd frust-desk   && cargo test --jobs 2          # incl. the CSS-seam guard
cd frust-e2e    && pnpm workflow && pnpm sse && pnpm print && pnpm mail
```

The kernel suite needs SurrealDB on `:8899`; the browser suites need the kernel
and Desk running. Several suites are guards rather than feature tests —
`surql_monopoly`, `tenancy_monopoly`, `keyguard_canary` and the Desk's
custom-property/variant checks each exist because they caught a real defect, and
each has been shown to fail on the defect it was written for.

**Use pnpm, not npm** — npm project installs are broken machine-wide on the dev
box (an injected `allow-scripts` config).
