//! A CI property: **Rust-side and DB-side money arithmetic reconcile exactly**
//! for the operations the accounting seed uses.
//!
//! The Tier-1 rollup counters do arithmetic DB-side (`add` was already probed;
//! mul/div is new territory). If `decimal.rs` and SurrealDB disagreed, the
//! seed's numbers would depend on WHERE they were computed — a silent split.
//! They do not disagree, and this asserts it so a future SurrealDB upgrade
//! that changed rounding would turn the build red instead of the books.
//!
//! Reconciled: `qty × rate` (exact mul), `rate% × subtotal`, and half-even
//! rounding at defined points. **Not** reconciled and deliberately avoided:
//! raw SurrealQL `/`, which rounds implicitly at ~28 places — DB-side division
//! must round explicitly via `math::round`, never lean on `/`.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::single_tenant;
use frust_kernel::decimal::{Decimal, Mode};

fn d(s: &str) -> Decimal {
    Decimal::parse(s).expect("decimal")
}

/// Compute `expr` DB-side and return its exact decimal string. `expr` must
/// round explicitly — this helper deliberately offers no raw `/`.
fn db_decimal(db: &Db, expr: &str) -> String {
    let v = db.sql_root(&format!("RETURN <string>({expr});")).unwrap();
    // RETURN yields a scalar; some paths wrap it in a single-element array.
    v.as_str()
        .map(str::to_string)
        .or_else(|| v.as_array().and_then(|a| a.first()).and_then(|s| s.as_str()).map(str::to_string))
        .unwrap_or_else(|| panic!("no decimal from db for `{expr}`: {v}"))
}

/// `math::round(x * 10^scale) / 10^scale`, the DB-side half-even round to a
/// money scale — the ONLY division here is by an exact power of ten on an
/// already-integral value, so the raw-`/` precision caveat never bites.
fn db_round(scale: u32, inner: &str) -> String {
    let p = 10i128.pow(scale);
    format!("math::round(({inner}) * {p}dec) / {p}dec")
}

fn setup() -> Db {
    // These tests evaluate pure expressions (`RETURN` — no tables), so the db
    // only needs to EXIST. A UNIQUE name per call: `IF NOT EXISTS` was not
    // enough, because two threads issuing `DEFINE DATABASE` for the same name
    // concurrently still collide ("Transaction write conflict") — the DEFINE
    // itself is the contended write, existence check or not. Unique names
    // remove the contention rather than narrowing it.
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let name = format!("money_recon_{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed));
    let cfg = single_tenant(&name.clone()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("DEFINE DATABASE {name};")).unwrap();
    db
}

/// `qty × rate`, rounded once, agrees Rust-side and DB-side across a spread of
/// values — including the ones that sit on a rounding half.
#[test]
fn line_amount_reconciles() {
    let db = setup();
    let cases = [
        ("3", "0.335"),   // 1.005 → half → 1.00 (even) both sides
        ("2", "19.99"),   // 39.98 exact
        ("7", "1.4285"),  // 9.9995 → half → 10.00 (even)
        ("12", "0.125"),  // 1.5 exact
        ("1", "0.005"),   // 0.005 → half → 0.00 (even)
        ("1", "0.015"),   // 0.015 → half → 0.02 (even)
    ];
    for (qty, rate) in cases {
        let rs = d(qty).mul(d(rate)).unwrap().round(2, Mode::HalfEven).to_plain_string();
        let dbside = db_decimal(&db, &db_round(2, &format!("{qty}dec * {rate}dec")));
        // decimal-equal (SurrealDB drops trailing zeros: "1.00" prints "1")
        assert_eq!(
            d(&rs), d(&dbside),
            "qty×rate {qty}×{rate}: rust={rs} db={dbside} — money math must not depend on WHERE it ran"
        );
    }
}

/// `rate% × subtotal`, rounded once, agrees both sides.
#[test]
fn tax_amount_reconciles() {
    let db = setup();
    for (subtotal, rate) in [("100.00", "0.15"), ("3.00", "0.15"), ("19.99", "0.0825"), ("1.005", "0.10")] {
        let rs = d(subtotal).mul(d(rate)).unwrap().round(2, Mode::HalfEven).to_plain_string();
        let dbside = db_decimal(&db, &db_round(2, &format!("{subtotal}dec * {rate}dec")));
        assert_eq!(d(&rs), d(&dbside), "tax {rate} of {subtotal}: rust={rs} db={dbside}");
    }
}

/// The FINDING, asserted so it cannot silently change: raw `/` rounds at ~28
/// places (implicit), which is exactly why DB-side division must round
/// explicitly. This documents the divergence rather than papering over it.
#[test]
fn raw_db_division_rounds_implicitly_and_is_therefore_avoided() {
    let db = setup();
    let raw = db_decimal(&db, "1dec / 3dec");
    println!("raw db 1/3 = {raw}");
    // ~28 fractional digits — NOT a money scale. decimal.rs refuses to produce
    // this at all: div_round requires an explicit target scale.
    assert!(raw.starts_with("0.3333333333"), "raw / is high-precision: {raw}");
    assert!(raw.len() > 10, "and it is NOT rounded to a money scale: {raw}");

    // the explicit path agrees with Rust
    let rs = d("1").div_round(d("3"), 2, Mode::HalfEven).unwrap().to_plain_string();
    let dbside = db_decimal(&db, &db_round(2, "1dec / 3dec"));
    assert_eq!(d(&rs), d(&dbside), "explicit round: rust={rs} db={dbside}");
}
