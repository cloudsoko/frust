# Release policy

Frust releases are immutable, version-tagged build outputs. This policy covers
the repository's release mechanics; it does not grant permission to distribute
the software.

## Current release blocker: no project license

The repository has no `LICENSE` or `COPYING` file. The owner must choose the
project's license and confirm that every bundled dependency and vendored asset
may be distributed under that choice. Automation must not infer a license from
dependency licenses or source visibility.

`.github/scripts/release-preflight.py --require-license` therefore fails every
release until the owner adds the selected license. Kernel crates also remain
`publish = false`. Removing either control requires an explicit distribution
decision and review.

## Version and tag contract

- The canonical framework version is `[workspace.package].version` in
  `frust-kernel/Cargo.toml`.
- `release/compatibility.toml` must carry the same version.
- Releases use an annotated or lightweight `vMAJOR.MINOR.PATCH` tag. A SemVer
  prerelease suffix is allowed; build metadata is not used in tags.
- A release tag must point at a commit already accepted by protected `main`.
- Tags and published release assets are never replaced. Corrections receive a
  new version.

The framework remains experimental at `0.x`: minor versions may contain
breaking framework changes, but the documented REST evolution promise remains
governed by `frust-kernel/docs/evolution-policy.md`.

## Required gates

Protect `main` with these successful checks:

1. `Static, unit, and reproducibility`
2. `Live integration (jwt)`
3. `Live integration (basic)`
4. Dependency review and RustSec checks when dependency inputs change

The quality gate initializes recorded submodules, uses the pinned Rust/pnpm/JCO
versions, rebuilds the WASM/browser runtime, and refuses checksum or generated
artifact drift. The live matrix runs the bounded, hermetic SurrealDB-backed
lane in both supported root-authentication modes.

Ignored performance tests are deliberately outside pull-request CI. The weekly
workflow runs the self-contained measurement gates in release mode and retains
their logs. `test/lanes.json` is the reviewable coverage contract. The seeded
`:8898` scale harness, the test that restarts a local Windows SurrealDB process,
and the checkout-specific `tenant_restore_ops` CLI test remain quarantined for
dedicated controlled runners; they are not represented as ordinary CI coverage.

## Published evidence

Tag pushes run legal/version/artifact preflight before building. Each supported
host archive contains:

- locked kernel and Desk binaries;
- the checked runtime WASM and browser-engine files;
- the machine-readable compatibility manifest;
- an SPDX JSON software bill of materials.

Each archive is accompanied by a SHA-256 checksum and a GitHub OIDC build
provenance attestation. The `release` GitHub Environment should require a human
reviewer so publishing is distinct from building.

Consumers should verify both mechanisms:

```text
sha256sum --check frust-vX.Y.Z-linux-x86_64.zip.sha256
gh attestation verify frust-vX.Y.Z-linux-x86_64.zip --repo cloudsoko/frust
```

GitHub provenance establishes which workflow produced an artifact. It is not a
substitute for platform code signing. Windows Authenticode, Apple signing and
notarization, and an organization-controlled container signing identity remain
owner decisions before those distribution channels are promised.

## Operator configuration still required

- Select and add the project license after legal review.
- Enable protected branches and require the checks above.
- Create the protected `release` Environment and assign reviewers.
- Enable private vulnerability reporting or publish a monitored security
  contact before inviting third-party adoption.
- Decide supported host platforms and acquire their code-signing identities.
- Define a support lifetime before declaring any release long-term-supported.
