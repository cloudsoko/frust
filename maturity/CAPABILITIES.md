# Frust Capability Maturity

This file is generated from `maturity/capabilities.json`. Do not edit it directly.
Implementation and operational proof are tracked separately; code presence alone does not
make a capability production-ready.

## Summary

- Total capabilities: 16
- `planned`: 1
- `experimental`: 6
- `pilot`: 9
- `production-ready`: 0

## Status Rules

- `planned`: intended work; it cannot claim passing verification or operational proof.
- `experimental`: code may exist, but its contract or operating model is still unstable.
- `pilot`: implemented and test-backed for bounded use, without production proof.
- `production-ready`: implemented, passing automated tests, and backed by a runbook and production proof.

## Capability Matrix

| ID | Surface | Capability | Status | Implementation | Operational proof | Owner |
| --- | --- | --- | --- | --- | --- | --- |
| apps.lifecycle | Applications | Application install, upgrade, disable, and remove | pilot | implemented | automated-only | kernel-apps |
| database.migrations | Migrations | Schema migrations and fleet rollout | experimental | partial | automated-only | orm |
| desk.ui | Desk | Metadata-driven Desk | pilot | implemented | automated-only | desk |
| developer.app-sdk | SDK and CLI | Application SDK, scaffold, validation, and packaging | experimental | partial | none | developer-experience |
| engineering.release | Release engineering | Versioned and attestable releases | planned | partial | none | release-engineering |
| engineering.test-tooling | Test tooling | Hermetic test lanes and diagnostics | experimental | partial | automated-only | developer-infrastructure |
| http.rest | REST | Typed REST contract and method routing | pilot | implemented | automated-only | kernel-http |
| jobs.worker | Jobs | Durable background jobs and worker fairness | experimental | implemented | automated-only | kernel-worker |
| kernel.hooks | Hooks | Sandboxed document hooks | pilot | implemented | automated-only | kernel-runtime |
| kernel.verbs | Kernel | Permission-enforced document verbs | pilot | implemented | automated-only | kernel |
| mail.delivery | Mail | Transactional mail delivery | experimental | implemented | automated-only | kernel-mail |
| operations.deployment | Deployment | Containerized deployment package | pilot | implemented | staging | operations |
| operations.recovery | Recovery | Verified backup and destructive restore | pilot | implemented | staging | operations |
| realtime.events | Realtime | Realtime event streaming | experimental | implemented | automated-only | kernel-realtime |
| security.authentication | Authentication | Root and session authentication | pilot | implemented | automated-only | kernel-security |
| tenancy.isolation | Tenancy | Tenant routing and isolation | pilot | implemented | automated-only | kernel-tenancy |

## Non-Production Gaps

- `apps.lifecycle` (`pilot`): Compatibility policy is not yet backed by a multi-version upgrade certification matrix. Marketplace trust, signing, and revocation policy is not established.
- `database.migrations` (`experimental`): Destructive and long-running data changes need operator-facing rehearsal guidance. Fleet canary, resume, and rollback behavior has not been proven in a production topology.
- `desk.ui` (`pilot`): Accessibility, browser support, and responsive behavior lack a published certification baseline. No production support or rollback runbook is recorded.
- `developer.app-sdk` (`experimental`): Generated application compatibility has not been certified across released kernel versions. Package signing, publishing, upgrade guidance, and long-term SDK support policy are incomplete.
- `engineering.release` (`planned`): No stable release or support lifetime is published; v0.1.0-rc.3 remains a prerelease candidate. Platform trust channels such as Windows Authenticode and Apple signing or notarization are not provided.
- `engineering.test-tooling` (`experimental`): Flake ownership, quarantine rules, and historical timing budgets are not established. Live-database suites are not fully hermetic or proven stable across supported environments.
- `http.rest` (`pilot`): Load, abuse, and proxy interoperability are not operationally certified. The public API has no published OpenAPI artifact or automated compatibility gate.
- `jobs.worker` (`experimental`): Queue saturation, poison jobs, and replay recovery are not covered by an exercised runbook. Service-level objectives and capacity limits are not published.
- `kernel.hooks` (`pilot`): Hook compatibility and resource-limit changes lack a production upgrade runbook. Untrusted third-party hooks have not been externally security reviewed.
- `kernel.verbs` (`pilot`): No production operations runbook or recorded production exercise exists. The full document verb matrix is not published as a versioned compatibility contract.
- `mail.delivery` (`experimental`): No production deliverability evidence is recorded. Provider failover, bounce handling, suppression, and reputation operations are incomplete.
- `operations.deployment` (`pilot`): Capacity limits, durable observability, TLS ingress, external secret management, and production service objectives remain environment work. Only an ephemeral single-host staging topology has been exercised; production infrastructure, high availability, and failover are not certified.
- `operations.recovery` (`pilot`): Encrypted off-host storage, retention enforcement, geographic failure recovery, and recurring production drills are not implemented end to end. The staging drill used a 6,535-byte tenant backup; production-scale recovery objectives remain unproven against representative data and failure modes.
- `realtime.events` (`experimental`): Reconnect, backpressure, and event-retention behavior is not certified at production scale. There is no operations playbook for lag, fanout overload, or dropped subscribers.
- `security.authentication` (`pilot`): Key rotation, incident response, and credential recovery are not backed by an exercised runbook. No external security audit evidence is recorded.
- `tenancy.isolation` (`pilot`): Capacity and noisy-neighbor limits remain environment-dependent. Isolation has automated coverage but no production topology certification or incident drill.

## Maintenance

Evidence paths, owners, verification commands, and complete gap records live in
`maturity/capabilities.json`.

Run `python scripts/validate_maturity.py` to validate and regenerate this file.
Run `python scripts/validate_maturity.py --check` in CI to reject invalid claims or drift.
