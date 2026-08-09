# Dependency verdicts

- 2026-08-09 — PR #1 `surrealdb/surrealdb` 3.2.3 → 3.2.4: deferred; deploy-only would skew the `[framework]` pin in `release/compatibility.toml`, so this needs an empirical-first, deliberate pinned-upgrade decision.
- 2026-08-09 — PR #2 `base64` 0.23.0 → 0.23.1: applied; patch update in `frust-kernel`.
- 2026-08-09 — PR #3 `wit-bindgen` 0.57.1 → 0.60.0: deferred; batch with the WASM toolchain artifact-regeneration pass on the Linux/Docker canonical builder because checksums would drift here.
- 2026-08-09 — PR #4 `lettre` 0.11.22 → 0.11.23: applied; patch update in `frust-kernel`.
- 2026-08-09 — PR #5 `playwright` 1.62.0 → 1.62.1: applied; patch update in the `frust-e2e` pnpm lockfile.
- 2026-08-09 — PR #6 `@bytecodealliance/preview2-shim` 0.19.x → 0.20.1: deferred; batch with the WASM toolchain artifact-regeneration pass on the Linux/Docker canonical builder because checksums would drift here.
- 2026-08-09 — PR #7 `@bytecodealliance/jco-transpile` 0.5.2 → 0.6.x: deferred; batch with the canonical artifact-regeneration pass and make a deliberate compatibility-ledger decision for `[build].jco_transpile = "0.5.2"`.
