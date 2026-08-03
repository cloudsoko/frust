# Release policy

Frust releases are immutable, version-tagged build outputs. Frust-owned code is
distributed under `AGPL-3.0-only`; independently licensed components retain
their upstream terms.

## License contract

The root project, Frust Desk, and Frust UI carry the canonical GNU AGPL version
3 text, SPDX Cargo metadata, and source notices. Topcoat remains MIT licensed,
and SurrealDB retains its upstream license. Release archives include the Frust
license and notices plus the licenses of statically linked, separately licensed
components. The per-artifact SBOM records the complete dependency inventory.

`.github/scripts/release-preflight.py --require-license` rejects altered license
text, missing notices, SPDX metadata drift, or missing independently licensed
component records. Kernel crates remain `publish = false`; a GitHub binary or
container release does not implicitly authorize a crates.io publication.

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

Protect pull-request merges to `main` with these successful checks:

1. `Static, unit, and reproducibility`
2. `Live integration (jwt)`
3. Dependency review and RustSec checks

The protected quality context aggregates parallel governance, artifact, kernel
lint, offline-test, and Desk lanes. Artifact inputs are rebuilt only when their
sources change or the lock-bound runtime cache is empty; every executable change
still verifies checksums and refuses generated drift. Documentation-only changes
retain the required context names but skip their inapplicable work.

Offline hook tests and live integration both consume the verified runtime
artifact output explicitly; they do not rely on files left behind by another
step on a shared runner.

Pull requests run the bounded, hermetic SurrealDB smoke lane with JWT root
authentication. Every protected `main` push and manual CI run executes the
exhaustive live lane with both JWT and basic root authentication. Do not tag a
release commit until its post-merge `Live integration (jwt)` and `Live
integration (basic)` main-push contexts have both succeeded; this preserves the
two-mode release guarantee without serializing both modes on every pull request.
The exhaustive target list is deterministically divided across four workers per
authentication mode, and each named context aggregates every worker result.

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

Each archive and detached SBOM is accompanied by a SHA-256 checksum where
applicable, a keyless Sigstore signature bundle, and a GitHub OIDC build
provenance attestation. The publish job verifies every signature before it
creates the immutable GitHub release. The `release` GitHub Environment limits
deployment to version tags so publishing is distinct from building.

Consumers should verify both mechanisms:

```text
sha256sum --check frust-vX.Y.Z-linux-x86_64.zip.sha256
cosign verify-blob --bundle frust-vX.Y.Z-linux-x86_64.zip.sigstore.json \
  --certificate-identity-regexp 'https://github.com/cloudsoko/frust/.github/workflows/release.yml@refs/tags/v.*' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  frust-vX.Y.Z-linux-x86_64.zip
gh attestation verify frust-vX.Y.Z-linux-x86_64.zip --repo cloudsoko/frust
```

Sigstore establishes the workflow identity that signed a release blob or OCI
digest, and GitHub provenance establishes its build origin. Neither substitutes
for Windows Authenticode or Apple signing/notarization where those platform
trust channels are promised.

## Operator configuration still required

- Enable protected branches and require the checks above.
- Keep the `release` and `staging` Environments restricted to version tags.
- Enable private vulnerability reporting or publish a monitored security
  contact before inviting third-party adoption.
- Decide supported host platforms and acquire their code-signing identities.
- Define a support lifetime before declaring any release long-term-supported.
