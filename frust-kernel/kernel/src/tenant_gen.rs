//! **Per-tenant cache generations** — WO-040 Chunk B, removing the coupling
//! Chunk A left named.
//!
//! WO-026 bought the kernel's throughput by caching the two things every
//! request needed: the token→principal lookup and the DocType metadata. Both
//! are invalidated by *generation* — bump a counter, and every cached entry
//! stops matching. That design is deliberately coarse: a forgotten bump costs
//! a cache miss, where a forgotten per-entry removal would leave a revoked
//! token live. Correctness over hit-rate, in an auth path.
//!
//! **The counters were process-global.** With one process serving N tenants,
//! coarse stopped meaning "drop this tenant's cache" and started meaning
//! "drop everyone's" — so one tenant's logout, revoke, or schema sync
//! cold-started every other tenant's caches. Never wrong, but a
//! noisy-neighbour vector aimed squarely at the caches that took throughput
//! from 15 to 124 req/s. WO-039 found it; this fixes it.
//!
//! **Why `Arc<AtomicU64>` handles rather than a lookup per read.** The
//! generation is read on *every* request. A `Mutex<HashMap>` lookup on that
//! path would trade a cross-tenant coupling for a cross-thread one — a worse
//! deal at 16 workers. Instead a tenant's handles are resolved **once**, when
//! its `Broker` is built, and the hot path stays a single atomic load. Only
//! invalidation — logout, revoke, schema sync — touches the registry, and
//! those are rare by construction.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// One tenant's generations, held by its `Broker`.
#[derive(Clone, Debug)]
pub struct TenantGenerations {
    /// Bumped by logout and admin revoke (WO-026 / WO-033).
    pub session: Arc<AtomicU64>,
    /// Bumped by every kernel-mediated write to DocType metadata.
    pub meta: Arc<AtomicU64>,
}

type Registry = Mutex<HashMap<(&'static str, String), Arc<AtomicU64>>>;

/// Entries are never evicted, deliberately. A generation must outlive every
/// cache entry that recorded it — dropping one and re-creating it at zero
/// would make a stale entry match again, which in the session cache means
/// serving a revoked principal. The cost is two counters per tenant the
/// process has ever seen, and the roster is bounded by config.
fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

fn handle(kind: &'static str, tenant: &str) -> Arc<AtomicU64> {
    let mut reg = registry().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    Arc::clone(
        reg.entry((kind, tenant.to_string())).or_insert_with(|| Arc::new(AtomicU64::new(0))),
    )
}

/// Resolve a tenant's generation handles. Called once per `Broker`.
pub fn for_tenant(tenant: &str) -> TenantGenerations {
    TenantGenerations { session: handle("session", tenant), meta: handle("meta", tenant) }
}

/// Drop **this tenant's** cached sessions. A revoked token must not survive
/// in cache for even one request — and no other tenant's cache is touched.
pub fn invalidate_sessions(tenant: &str) {
    handle("session", tenant).fetch_add(1, Ordering::AcqRel);
}

/// Drop **this tenant's** cached DocType metadata. Call from any path that
/// changes DocType metadata; forgetting it costs a stale read, which is why
/// the bump lives at every kernel write site rather than being inferred.
pub fn invalidate_meta(tenant: &str) {
    handle("meta", tenant).fetch_add(1, Ordering::AcqRel);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point, in three lines: bumping one tenant leaves the other
    /// alone, and the handle a `Broker` took earlier still observes its own
    /// tenant's bumps (the handles are shared, not snapshots).
    #[test]
    fn a_bump_is_confined_to_its_own_tenant() {
        let a = for_tenant("gen_test_a");
        let b = for_tenant("gen_test_b");
        let (a0, b0) = (a.session.load(Ordering::Acquire), b.session.load(Ordering::Acquire));

        invalidate_sessions("gen_test_a");

        assert_eq!(a.session.load(Ordering::Acquire), a0 + 1, "A's own bump was not observed");
        assert_eq!(b.session.load(Ordering::Acquire), b0, "A's logout moved B's generation");
        // and the two kinds do not alias each other
        assert_eq!(a.meta.load(Ordering::Acquire), 0, "a session bump moved the meta generation");
    }
}
