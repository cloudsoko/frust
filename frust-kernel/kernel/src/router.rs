//! **Per-request tenant routing**: one process, N tenants.
//!
//! `frust serve` resolves each registered tenant at boot; this module is the
//! piece that makes an incoming request find its own tenant.
//!
//! ## The mechanism
//!
//! 1. **`/login` is told which tenant** — a `tenant` field in the payload, or
//!    the request's subdomain. It must be: `/login` has no token yet and has
//!    to authenticate against a *specific* tenant's user table.
//! 2. **The kernel mints `<tenant>.<random>`** after authenticating there.
//!    The prefix is the **canonical `TenantId`** the registry produced — it is
//!    kernel-asserted after validation, never the client's string echoed back.
//! 3. **Every authenticated request splits its token** before any database
//!    call, and the session row lives in the tenant's own database.
//!
//! ## Why a forged prefix fails safely
//!
//! Presenting `tenant_b.<my-own-secret>` routes the lookup **into B's
//! database**, where that token does not exist → 401. There is no shared
//! session table to find it in — this isolation is a property the design
//! refuses to trade away. An unregistered prefix does not resolve at all.
//!
//! ## Why resolution is not re-run per request
//!
//! [`TenantRouter::build`] resolves **every registered tenant through the
//! strategy** once, at boot, and stores the result. Membership of this map is
//! therefore identical to membership of the registry: a prefix that finds a
//! context here has been through `TenancyStrategy::resolve`, by construction.
//! Re-resolving per request would add work to the hot path to re-derive a
//! deterministic answer — and would not make the answer any more trusted.

use std::collections::HashMap;
use std::sync::Arc;

use crate::broker::Broker;
use crate::contract::BrokerError;
use crate::sync::MetadataSync;
use crate::tenancy::{ResolvedTenant, Tenancy};

/// Everything the request path needs to serve **one** tenant.
pub struct TenantContext {
    pub broker: Arc<Broker>,
    /// Enables `POST /doctype` for this tenant. `None` = 404.
    pub sync: Option<Arc<MetadataSync>>,
}

impl TenantContext {
    /// The tenant this context serves — read off its broker, so a context
    /// cannot be filed under a tenant it does not actually serve.
    pub fn tenant_id(&self) -> &str {
        self.broker.db.tenant_id()
    }
}

pub struct TenantRouter {
    contexts: HashMap<String, TenantContext>,
}

impl TenantRouter {
    /// One context per **registered** tenant, each built from the target the
    /// strategy resolved.
    pub fn build<F>(tenancy: &Tenancy, mut make: F) -> Result<Self, BrokerError>
    where
        F: FnMut(&ResolvedTenant) -> Result<TenantContext, BrokerError>,
    {
        let mut contexts = HashMap::new();
        for target in tenancy.resolve_all()? {
            let ctx = make(&target)?;
            // the context must serve the tenant it is filed under; anything
            // else would be a routing bug that no later assertion could see
            let id = target.tenant_id().as_str();
            if ctx.tenant_id() != id {
                return Err(BrokerError::InvalidValue {
                    detail: format!(
                        "router: context for {id:?} is scoped to {:?}",
                        ctx.tenant_id()
                    ),
                });
            }
            contexts.insert(id.to_string(), ctx);
        }
        Ok(Self { contexts })
    }

    /// A single-tenant router — the shape every existing caller and test has.
    pub fn single(ctx: TenantContext) -> Self {
        Self { contexts: HashMap::from([(ctx.tenant_id().to_string(), ctx)]) }
    }

    pub fn len(&self) -> usize {
        self.contexts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.contexts.is_empty()
    }

    pub fn get(&self, tenant: &str) -> Option<&TenantContext> {
        self.contexts.get(tenant)
    }

    /// The only context, when there is exactly one.
    ///
    /// Used for the login hint **only where it cannot be wrong**: with one
    /// tenant registered there is nothing to guess between. With two or more
    /// this returns `None` and login refuses rather than picking.
    pub fn sole(&self) -> Option<&TenantContext> {
        let mut it = self.contexts.values();
        match (it.next(), it.next()) {
            (Some(ctx), None) => Some(ctx),
            _ => None,
        }
    }

    /// Mint a token for `tenant`. The prefix is the canonical tenant id the
    /// caller already resolved — this function never sees client input.
    pub fn mint_token(tenant: &str, secret: &str) -> String {
        format!("{tenant}.{secret}")
    }

    /// **Split a bearer token and find its tenant.**
    ///
    /// Both halves are validated before anything reaches a query: the prefix
    /// must name a registered tenant (so it is a name the registry minted),
    /// and the secret must be plain alphanumeric. A token that fails either
    /// check resolves to nothing, which the caller turns into one
    /// indistinguishable 401 — routing is not a tenant oracle.
    pub fn route_token<'a>(&'a self, token: &str) -> Option<(&'a TenantContext, String)> {
        let (prefix, secret) = token.split_once('.')?;
        if secret.is_empty() || !secret.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        let ctx = self.contexts.get(prefix)?;
        Some((ctx, secret.to_string()))
    }
}

/// The subdomain of a `Host` header, when there is one.
///
/// Deliberately conservative — it returns `None` for anything that is not
/// plainly `<label>.<domain>.<tld>`: a bare host, an IP, or a `localhost:port`
/// has no tenant in it, and inventing one from a port number or an octet
/// would be a routing decision made out of noise.
pub fn subdomain_of(host: &str) -> Option<&str> {
    let host = host.split(':').next()?.trim();
    if host.is_empty() || host.parse::<std::net::IpAddr>().is_ok() {
        return None;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 3 || labels[0].is_empty() {
        return None;
    }
    Some(labels[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_subdomain_is_only_read_where_one_really_exists() {
        assert_eq!(subdomain_of("acme.frust.app"), Some("acme"));
        assert_eq!(subdomain_of("acme.frust.app:8790"), Some("acme"));
        // no tenant hides in any of these
        for none in ["127.0.0.1:8790", "localhost", "localhost:3000", "frust.app", "", "::1"] {
            assert_eq!(subdomain_of(none), None, "invented a tenant from {none:?}");
        }
    }

    #[test]
    fn a_token_is_split_only_when_both_halves_are_well_formed() {
        // no router needed: these all fail before the map lookup
        let empty = TenantRouter { contexts: HashMap::new() };
        for bad in [
            "no-dot-at-all",
            "acme.",              // empty secret
            "acme.has-a-dash",    // secret is not alphanumeric
            "acme.a b",
            "acme.a'b",           // the shape that would matter in a query
            ".secret",            // empty prefix cannot be a registered tenant
        ] {
            assert!(empty.route_token(bad).is_none(), "accepted {bad:?}");
        }
    }
}
