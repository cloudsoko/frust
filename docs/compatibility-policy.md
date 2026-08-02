# Compatibility policy

`release/compatibility.toml` is the machine-readable compatibility record for
each Frust release. Release preflight rejects drift between it, the Cargo
workspace version, the Rust toolchain pin, and the WASM artifact lock.

## Current matrix

| Surface | Supported value | Rule |
| --- | --- | --- |
| Frust framework | `0.1.0`, experimental | No long-term support promise yet |
| Rust build toolchain | `1.96.0` | Exact pin for reproducible builds |
| SurrealDB | `3.2.3` | Exact tested runtime version |
| App manifest | `manifest_version = 1` | Unknown versions are refused |
| REST surface | major `1` | Additive policy in `frust-kernel/docs/evolution-policy.md` |
| WASI target | `wasm32-wasip2` | Exact component build target |
| WIT package | `frust:plugin` | Current worlds are listed in the manifest |
| pnpm / JCO transpiler | `11.1.2` / `@bytecodealliance/jco-transpile 0.5.2` | Exact artifact-build inputs |
| Artifact builders | Digest-pinned Linux containers | Prevents host-specific WASM output |

Only combinations listed in a release's compatibility file are supported. A
dependency version being semver-compatible is not evidence that Frust supports
it; support begins after the full live suite and relevant upgrade/restore gates
pass on that combination.

## Change rules

- A breaking app-manifest shape increments `manifest_version`; old versions are
  either migrated explicitly or refused before partial installation.
- Existing WIT interfaces do not grow. Capabilities are introduced through a
  new interface/world so already-built components remain loadable.
- The documented REST surface follows its own additive-within-major policy.
- SurrealDB version changes require schema convergence, permission, auth-mode,
  tenant isolation, backup/restore, and performance evidence before the
  compatibility file changes.
- Rust, pnpm, JCO, WIT, or guest-source changes require rebuilding every locked
  runtime artifact and reviewing the resulting checksums.

## Support status

The project has not yet selected release support windows, deprecation periods,
or an LTS branch policy. Until those owner decisions are recorded, only the
latest published experimental release is eligible for fixes, and no response
time or maintenance duration is promised.
