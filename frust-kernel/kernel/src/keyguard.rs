//! The restored-signing-key guard — the fail-closed boot lineage, extended
//! from meta-version to **key integrity**.
//!
//! ## The hole this closes
//!
//! `surreal export` redacts the record access's JWT signing key to the literal
//! string `[REDACTED]`. `surreal import` then installs that string **as the
//! actual key**. The restored instance boots, logs users in, and serves the
//! application — while accepting tokens forged by anyone who knows the
//! constant, which is anyone who has ever seen a SurrealDB export file.
//!
//! Nothing looks broken. That is what makes it dangerous: it fires at the
//! moment an operator is least auditing and most exposing — restoring under
//! incident pressure.
//!
//! ## Why a self-forge test and not introspection
//!
//! Probed on v3.2.0: `INFO FOR DB` reports `KEY '[REDACTED]'` for a **healthy**
//! access too. Introspection therefore cannot tell a real key from the
//! placeholder — a comparison-based guard would fire on every healthy store or
//! on none. So the guard **tests the vulnerability itself**: mint a token
//! signed with the published constant and ask the database whether it accepts
//! it. A guard that exercises the hole beats one that checks a proxy for it.
//!
//! The forged identity is deliberately a **nonexistent** record: probing showed
//! acceptance depends only on signature validity, so the guard needs no real
//! user and works against an empty database.
//!
//! ## No "serve anyway" escape
//!
//! Unlike `--accept-meta-migrations`, where proceeding is a legitimate operator
//! choice, proceeding here *is* serving-compromised. The only forward path is
//! re-issuing `DEFINE ACCESS` with a fresh key. A flag saying "I know the key
//! is public, boot anyway" would be a footgun wearing a safety label.

use base64::Engine as _;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha512;

use crate::contract::BrokerError;
use crate::db::Db;

/// The string SurrealDB substitutes for secrets in `export` output.
///
/// **Pinned deliberately.** The guard's correctness rests on this constant; a
/// SurrealDB version that changes it would silently disable the guard and
/// quietly reopen the hole. `keyguard_canary` fails the build if the redaction
/// behaviour moves.
pub const REDACTED_KEY: &str = "[REDACTED]";

/// A record id that must not exist — the guard cares only whether the
/// SIGNATURE validates, never who the token claims to be.
const CANARY_ID: &str = "app_user:__frust_keyguard_canary__";

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Mint a token signed with `key`, claiming the canary identity.
pub fn forge_token(ns: &str, db: &str, access: &str, key: &str) -> String {
    let header = b64(br#"{"alg":"HS512","typ":"JWT"}"#);
    // no wall-clock dependency beyond validity: a token that is already
    // expired would be rejected for the WRONG reason and hide the finding
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let claims = serde_json::json!({
        "ns": ns, "db": db, "ac": access, "ID": CANARY_ID,
        "iat": now, "nbf": now, "exp": now + 300,
    });
    let payload = b64(claims.to_string().as_bytes());
    let signing_input = format!("{header}.{payload}");
    let mut mac = <Hmac<Sha512>>::new_from_slice(key.as_bytes()).expect("hmac accepts any key len");
    mac.update(signing_input.as_bytes());
    format!("{signing_input}.{}", b64(&mac.finalize().into_bytes()))
}

/// Does this store accept a token signed with the given key?
///
/// `Ok(true)` means the signature validated — for `REDACTED_KEY`, that is the
/// vulnerability. Transport failures are `Err`: a guard that cannot reach the
/// database must not report "safe".
pub fn store_accepts_key(db: &Db, key: &str) -> Result<bool, BrokerError> {
    // the JWT's `db` claim is the DATABASE, not the tenant id. They
    // coincide under database-per-tenant and would silently diverge under a
    // namespace topology — a forged probe that named the wrong database would
    // be refused for the wrong reason and report the store "safe".
    let token = forge_token(
        db.target().namespace().as_str(),
        db.target().database().as_str(),
        db.access(),
        key,
    );
    // `_quiet` — this call is EXPECTED to fail on a healthy
    // store, so its refusal must not log at error level and make every clean
    // boot look broken. Log level only: the match below is unchanged, so a
    // transport failure still propagates as `Err` and still refuses the boot.
    match db.sql_with_auth_quiet(&format!("Bearer {token}"), "RETURN 1;") {
        Ok(_) => Ok(true),
        // an auth refusal is the healthy answer, not an error to propagate
        Err(BrokerError::PermissionDenied { .. }) => Ok(false),
        Err(BrokerError::Db { detail }) if detail.contains("401") || detail.to_lowercase().contains("unauthor") => {
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

/// Boot gate: refuse to serve a store whose signing key is the export
/// placeholder.
///
/// Fail-closed in both directions — a store that accepts the published key is
/// refused, and a guard that cannot complete its own check is also refused,
/// because "I could not verify" is not "verified safe".
pub fn assert_access_key_is_not_redacted(db: &Db) -> Result<(), BrokerError> {
    if store_accepts_key(db, REDACTED_KEY)? {
        return Err(BrokerError::PermissionDenied {
            detail: format!(
                "FRUST:E_RESTORED_ACCESS_KEY: this database accepts tokens signed with SurrealDB's \
                 export placeholder key ('{REDACTED_KEY}'), so ANY party can forge a session for ANY \
                 user. This is what a `surreal import` of an exported dump leaves behind — the key \
                 is redacted on export and restored literally.\n\
                 REMEDY: re-issue the record access (yours is named '{access}') with a fresh secret \
                 before serving —\n\x20 {ddl}\n\
                 Use OVERWRITE: the IF-NOT-EXISTS form is a no-op against an existing access and \
                 fixes nothing. Existing sessions do not survive a restore in any case. There is \
                 deliberately no flag to serve anyway.",
                access = db.access(),
                ddl = crate::meta::restored_key_remediation_ddl(),
            ),
        });
    }
    Ok(())
}
