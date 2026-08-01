# Frust App SDK

`frust-app-sdk` is the offline app-author contract and `frust-app` is its CLI.
Both consume the kernel's public `Manifest` type and validator; there is no
second manifest schema to drift.

## Commands

```powershell
cargo run -p frust-app-sdk --bin frust-app -- new ledger --label "Ledger"
cargo run -p frust-app-sdk --bin frust-app -- check ledger
cargo run -p frust-app-sdk --bin frust-app -- pack ledger
```

`new` creates a standalone Rust `wasm32-wasip2` hook project. Its `wit/plugin.wit`
is copied from `frust-kernel/wit/plugin.wit`, the canonical source compiled into
the matching SDK release. `check` verifies that exact normalized source hash,
the kernel manifest rules, DocType ownership, component presence, and Wasmtime
component decoding without a database.

## Bundle format 1

A `.frust` file is an uncompressed deterministic tar archive containing:

- `bundle.json`: bundle format, app version, manifest version, and WIT digest.
- `manifest.json`: canonical JSON serialization of the checked manifest.
- `wit/plugin.wit`: the pinned host contract.
- `components/*.wasm`: only components declared by the manifest.
- `checksums.sha256`: SHA-256 for every preceding entry.

Entries are lexically ordered with fixed mode, uid, gid, and timestamp. The CLI
also prints the digest of the complete archive. It refuses to overwrite an
existing project or bundle.

The current kernel install route accepts manifest JSON and resolves components
from `FRUST_ARTIFACTS`; it does not yet ingest `.frust` archives directly. The
archive is the portable, verified handoff format for a deployment or future
registry installer, not a second runtime installation path.
