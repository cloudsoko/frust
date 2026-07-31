---
tags: [frust, adr, tenancy, surrealdb]
status: AMENDED 2026-07-28 — runtime-selected TenancyStrategy (supersedes the back-filled single-strategy decision)
decided: 2026-07-23 · amended: 2026-07-28
---

# ADR-003: Tenancy — Runtime-Selected `TenancyStrategy`

> [!success] AMENDMENT (2026-07-28, Boss ruling) — tenancy topology is a **runtime-selected strategy**, not a fixed choice.
> *"Tenancy is not a feature. Tenancy topology is a strategy. Tenant resolution is one operation performed by that strategy."* The original back-filled decision (one platform namespace + database-per-tenant, below) becomes **one strategy among several**, not the model.
>
> **Decision:** Frust tenancy is a runtime-selected `TenancyStrategy`. **All** namespace/database resolution, tenant topology operations, access-definition placement, and backup planning pass through this strategy boundary. One binary; the topology is chosen at startup from validated config (`FRUST_TENANCY=single | database-per-tenant | namespace-per-tenant | namespace-per-tenant-env`). Cargo features may *optionally* strip unused strategies for specialized distributions, but **compile-time selection is not the primary mechanism** — runtime config is (a distributor ships one build; the operator picks the shape without recompiling). This cashes the "pluggable strategy" promise the back-fill stated but never built, and folds the ns-vs-db question into *two strategies a deployment chooses between* rather than an either/or.
>
> **The shape:**
> ```rust
> pub trait TenancyStrategy: Send + Sync {
>     fn resolve(&self, request: &TenantRequest) -> Result<ResolvedTenant, TenancyError>;
>     fn access_placement(&self) -> AccessPlacement;
>     fn backup_plan(&self, tenant: &TenantId) -> Result<BackupPlan, TenancyError>;
>     fn provisioning_plan(&self, tenant: &TenantId) -> Result<ProvisioningPlan, TenancyError>;
> }
> ```
> `ResolvedTenant { tenant_id, namespace, database, environment }` is a **typed value with a crate-private constructor** — only the tenancy module can mint one; fields readable. It is **the only value accepted by tenant-scoped persistence APIs.** (Split the trait into `TenantResolver`/`TenantProvisioner`/`TenantBackupPolicy`/`TenantAccessPolicy` only when the operational methods grow substantial — one trait suffices for now.)
>
> **Four structural invariants (the security boundary — mechanically enforced, not disciplinary):**
> 1. **No direct ns/db selection.** No request-serving code selects/constructs/modifies a namespace/database target except through the configured strategy. `db.rs` exposes `scoped_db(&ResolvedTenant) -> Db`; **no bare `db()` fallback exists** (it would recreate the bypass). Remove every `ns: "frust"`, direct `cfg.db`, `use_ns`/`use_db` from unapproved paths.
> 2. **No shared-context mutation.** A resolved tenant yields a request-scoped, concurrency-safe DB context — *Request A's resolved target cannot affect Request B's*. Proven by a **concurrent isolation test**, not only a unit test (the WO-039 provenance rule at concurrency).
> 3. **Resolution consumes TRUSTED identity.** Flow: `credential → authenticated principal → authorized tenant membership → canonical tenant-registry record → validated ResolvedTenant`. The strategy **never turns an arbitrary header/hostname/token-slug directly into a namespace/database name.** A kernel-*minted* token's tenant prefix is trusted (kernel asserted it post-login-validation) and is a **canonical `TenantId` the strategy resolves via the registry** — never a raw client string substituted into a db name. (This tightens the earlier tenant-prefixed-token ruling: the prefix is the entry point; the registry-validated resolution is the mechanism.)
> 4. **Enforce the monopoly structurally:** private target constructors, typed `NamespaceName`/`DatabaseName` (not `String`), scans for ns/db-selection APIs outside the boundary + for hardcoded ns/db constants, all repo ops through a scoped handle, cross-tenant concurrency tests, and an implementation matrix across every strategy. The guard fails when a developer introduces a new direct db-selection path.
>
> **Ratified trait-shape deviations (2026-07-29, WO-040 Chunk A — both tighten the boundary vs the sketch):**
> - `resolve(TenantId) -> TenantPlacement`, **not** `resolve(&TenantRequest) -> Result<ResolvedTenant>`. The credential→principal→membership→canonical-`TenantId` flow (invariant 3) lives in the tenancy-module core; the pluggable strategy receives only an already-validated `TenantId` and maps it to a placement. **A strategy author never touches untrusted input**, so validation cannot be skipped by a third-party strategy. `ResolvedTenant`'s constructor is **module-private** (tighter than crate-private) — only the module mints it, from `(validated TenantId, strategy's TenantPlacement)`.
> - `backup_plan`/`provisioning_plan(&ResolvedTenant) -> Plan` — **no `Result`.** Computing a plan from a validated tenant is pure; failure lives in *execution*, separately. A fallible planner would invite handling an error that can't occur.
>
> **Security finding surfaced BY the seam (Chunk A):** `keyguard.rs` forged its ADR-013 probe token with the tenant-id as the `db` JWT claim — correct by coincidence under db-per-tenant, but under a **namespace topology it names the wrong database**, the probe is refused for the wrong reason, and **a vulnerable store reports SAFE** — the exact fail-open class WO-033/ADR-013 exists to prevent. Fixed in the seam. Found by *doing the removal*, not reasoning about it — the argument for structural refactors surfacing latent fail-open bugs.
>
> **Sequencing (Boss ruling):** *update this ADR first (done), then WO-040 Chunk A as the structural seam; do NOT begin namespace-per-tenant operational work until database-per-tenant is migrated through the seam and the bypass guards are green.* → [[WO-040 Multi-Tenant Routing]]. **Chunk A DONE 2026-07-29** ([[2026-07-29 WO-040 chunk A tenancy seam]]) — seam built, `ns`/`db` deleted from the connection config (no field = no bypass), guard shown-to-fail on planted bypasses (caught 3 real defects first run), isolation provenance-proven under concurrency, all M3 numbers hold. **Chunk B active.**

---
## Original decision (2026-07-23, back-filled) — now one strategy of several

# ADR-003: Tenancy — Platform Namespace + Database-per-Tenant

**Context:** [[Frappe Pain Points#8. Multi-tenancy|P-8.x]] (site-per-directory tenancy) and the open SRS multi-tenancy gap. SurrealDB's Namespace → Database hierarchy is a native tenancy primitive ([[SurrealDB#11. Namespace/Database hierarchy → tenancy model|§11]]).

**Decision** *(back-filled from what `framework-core`/ORM already implement)*:
- One **platform namespace**; one **database per tenant** within it.
- Tenancy strategy is **pluggable** — database-per-tenant is the default implementation, not a hard-wired assumption.
- Cross-tenant operations (migrations, fleet maintenance) run as **fan-out** over the tenant database list.

> [!warning] Honest caveats
> - The intermediate strategy definitions live in `framework-core`, which is **not on this machine** — verify the details against source next time it's local.
> - ~~P-8.2 (resource starvation) is explicitly NOT solved~~ → **BOUNDED (WO-013, 2026-07-25):** door throttling + queue fairness give a noisy-neighbor bound of **1.4×** (vs 1.8× unshaped, vs *no bound at all* in Frappe). The residual ~0.4× is shared-DB contention — admitted traffic competing inside one surreal process. **Tighter than 1.4× requires the DB-compute isolation trade (per-tenant surreal processes or equivalent) — an amendment to this ADR, now justifiable with a number instead of a fear.** [[2026-07-25 WO-013 tenant fairness]]

**Consequences:** closes the SRS multi-tenancy gap; per-tenant changefeed and live-query costs scale with tenant count — measure at fleet scale before GA.

**Related:** [[Frust Hub]] · [[SurrealDB]] · [[SRS#Open Requirement Gaps]]
