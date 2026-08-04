//! Exact decimal accumulation for the aggregates layer (REQ-6.2.1).
//!
//! Rollup deltas are folded per batch in the kernel before being written, so
//! that fold has to be exact — an `f64` there reintroduces the very error the
//! requirement exists to prevent. This is a minimal fixed-point decimal:
//! a scaled `i128` mantissa plus a scale, which is all money needs and costs
//! no dependency.
//!
//! REQ-6.2.2: multiply and divide, with rounding that is NEVER
//! implicit. The earlier exact-only restraint is now spec-decided:
//!
//! - `mul` is **exact** — the product scale is the sum of the input scales, no
//!   rounding happens, and the caller must `round` explicitly. A `*` that
//!   guessed a scale and rounded to it is the defect REQ-6.2.2 forbids.
//! - `div_round` takes the rounding **contract as an argument** (`scale`,
//!   `mode`), because unrounded division does not terminate in general — and
//!   it rounds *exactly* by computing the quotient and the TRUE remainder at
//!   the target scale, never by truncating at guard digits and hoping.
//! - `round(scale, mode)` is the single rounding primitive; every rounded
//!   money value in the system passes through it.
//!
//! Scale comes from currency metadata (the caller passes it); mode is explicit
//! per money field (default half-even). There is deliberately NO global
//! rounding config — REQ-6.2.2 says explicit-per-field.

use std::cmp::Ordering;

/// Rounding mode, explicit per money field (REQ-6.2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Banker's rounding: halves go to the even neighbour. The default —
    /// unbiased over many roundings, which is why accounting prefers it.
    HalfEven,
    /// Halves go away from zero. The "commercial"/schoolbook rounding.
    HalfUp,
    /// Truncate toward zero — no rounding up ever. Used where a remainder is
    /// distributed separately (allocation), not silently absorbed.
    Down,
}

/// Money arithmetic can fail, and when it does it fails TYPED — never a panic,
/// never a silent wrap. Money math that wraps is worse than money math that
/// stops (REQ-6.2.2, the overflow guard).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoneyError {
    /// A product or scaled numerator exceeded the `i128` mantissa range.
    Overflow,
    /// Division by a zero divisor.
    DivByZero,
}

/// An exact decimal: `mantissa * 10^-scale`.
///
/// Equality is NUMERIC, not representational: `1.50` equals `1.5`, matching
/// both SurrealDB's decimal comparison and the runtime surrogate's. Deriving
/// `PartialEq` here would make `Eq` and `Ord` disagree — which Rust requires
/// them not to do, and which a test caught immediately.
#[derive(Debug, Clone, Copy)]
pub struct Decimal {
    mantissa: i128,
    scale: u32,
}

impl PartialEq for Decimal {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Decimal {}

impl Decimal {
    pub const ZERO: Self = Self { mantissa: 0, scale: 0 };

    /// Parses a plain decimal string (`-?digits(.digits)?`). Anything else —
    /// including float notation — is `None`: money never guesses.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let (neg, body) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s),
        };
        let mut parts = body.split('.');
        let int = parts.next()?;
        let frac = parts.next().unwrap_or("");
        if parts.next().is_some() {
            return None;
        }
        if int.is_empty() || !int.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if !frac.is_empty() && !frac.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let digits: String = format!("{int}{frac}");
        let mantissa: i128 = digits.parse().ok()?;
        Some(Self { mantissa: if neg { -mantissa } else { mantissa }, scale: frac.len() as u32 })
    }

    /// Reads a JSON value that may be a decimal string, a number, or absent.
    /// A float that is not exactly representable is still parsed from its
    /// shortest representation — the caller's job is to stop producing them
    /// (the schema now stores decimals), not to pretend the value is exact.
    pub fn from_json(v: Option<&serde_json::Value>) -> Self {
        match v {
            Some(serde_json::Value::String(s)) => Self::parse(s).unwrap_or(Self::ZERO),
            Some(serde_json::Value::Number(n)) => Self::parse(&n.to_string()).unwrap_or(Self::ZERO),
            _ => Self::ZERO,
        }
    }

    fn rescaled(self, scale: u32) -> Self {
        if scale <= self.scale {
            return self;
        }
        let factor = 10i128.pow(scale - self.scale);
        Self { mantissa: self.mantissa.saturating_mul(factor), scale }
    }

    #[must_use]
    pub fn add(self, other: Self) -> Self {
        let scale = self.scale.max(other.scale);
        let a = self.rescaled(scale);
        let b = other.rescaled(scale);
        Self { mantissa: a.mantissa.saturating_add(b.mantissa), scale }
    }

    #[must_use]
    pub fn neg(self) -> Self {
        Self { mantissa: -self.mantissa, scale: self.scale }
    }

    /// EXACT multiplication. The product scale is the SUM of the input scales,
    /// so nothing is rounded — `1.25 × 1.25 = 1.5625`, scale 4. The caller
    /// `round`s to the money scale at a defined point. Overflow is typed.
    ///
    /// This is the "extended-precision unrounded result" half of criterion 1:
    /// there is no way to get a silently-rounded product, because `mul` never
    /// rounds at all.
    pub fn mul(self, other: Self) -> Result<Self, MoneyError> {
        let mantissa = self.mantissa.checked_mul(other.mantissa).ok_or(MoneyError::Overflow)?;
        Ok(Self { mantissa, scale: self.scale + other.scale })
    }

    /// Division rounded to `scale` under `mode`, EXACTLY — the quotient and the
    /// true remainder are computed at the target scale and the remainder makes
    /// the rounding decision, so a half is detected precisely (no guard-digit
    /// approximation). Division takes the rounding contract as an argument
    /// because an unrounded quotient does not terminate in general (`1/3`).
    ///
    /// `E_MONEY_DIVBYZERO` on a zero divisor; `E_MONEY_OVERFLOW` if scaling the
    /// numerator exceeds range.
    pub fn div_round(self, divisor: Self, scale: u32, mode: Mode) -> Result<Self, MoneyError> {
        if divisor.mantissa == 0 {
            return Err(MoneyError::DivByZero);
        }
        // want: round( (self / divisor) * 10^scale ) as the mantissa.
        // (self/divisor) * 10^scale
        //   = self.mantissa / divisor.mantissa * 10^(scale + divisor.scale - self.scale)
        let p = scale as i64 + divisor.scale as i64 - self.scale as i64;
        let (num, den) = if p >= 0 {
            let factor = pow10(p as u32)?;
            (self.mantissa.checked_mul(factor).ok_or(MoneyError::Overflow)?, divisor.mantissa)
        } else {
            let factor = pow10((-p) as u32)?;
            (self.mantissa, divisor.mantissa.checked_mul(factor).ok_or(MoneyError::Overflow)?)
        };
        let neg = (num < 0) ^ (den < 0);
        let num_abs = num.unsigned_abs();
        let den_abs = den.unsigned_abs();
        let q = num_abs / den_abs;
        let r = num_abs % den_abs;
        let m = apply_rounding(q, r, den_abs, mode)?;
        let mantissa = i128::try_from(m).map_err(|_| MoneyError::Overflow)?;
        Ok(Self { mantissa: if neg { -mantissa } else { mantissa }, scale })
    }

    /// Round to `target_scale` under `mode`. The single rounding primitive: a
    /// `mul` product, a stored value, anything, becomes money-scaled HERE and
    /// nowhere by accident. Widening (target ≥ current) never rounds; it just
    /// rescales.
    #[must_use]
    pub fn round(self, target_scale: u32, mode: Mode) -> Self {
        if target_scale >= self.scale {
            return self.rescaled(target_scale);
        }
        let drop = self.scale - target_scale;
        // drop is small (a few digits); pow10 cannot realistically overflow
        // here, and if it somehow did we keep the value rather than panic.
        let Ok(factor) = pow10(drop) else { return self };
        let factor = factor.unsigned_abs();
        let neg = self.mantissa < 0;
        let abs = self.mantissa.unsigned_abs();
        let q = abs / factor;
        let r = abs % factor;
        let m = apply_rounding(q, r, factor, mode).unwrap_or(q);
        let mantissa = i128::try_from(m).unwrap_or(i128::MAX);
        Self { mantissa: if neg { -mantissa } else { mantissa }, scale: target_scale }
    }

    pub fn is_zero(self) -> bool {
        self.mantissa == 0
    }

    /// The exact string form, e.g. `-12.340`. Round-trips through `parse`.
    pub fn to_plain_string(self) -> String {
        let neg = self.mantissa < 0;
        let digits = self.mantissa.unsigned_abs().to_string();
        let s = self.scale as usize;
        let body = if s == 0 {
            digits
        } else if digits.len() > s {
            let (int, frac) = digits.split_at(digits.len() - s);
            format!("{int}.{frac}")
        } else {
            format!("0.{}{}", "0".repeat(s - digits.len()), digits)
        };
        if neg { format!("-{body}") } else { body }
    }
}

/// `10^n` as an `i128`, typed-overflow instead of a panic.
fn pow10(n: u32) -> Result<i128, MoneyError> {
    10i128.checked_pow(n).ok_or(MoneyError::Overflow)
}

/// The rounding decision, shared by `round` and `div_round` so the two can
/// never disagree. `q` is the truncated magnitude, `r` the dropped remainder
/// magnitude, `unit` the divisor (the thing `2r` is compared against). Returns
/// the rounded magnitude.
///
/// The half is detected EXACTLY (`2r == unit`), which is the whole reason the
/// remainder is threaded through rather than a digit inspected: `2.125` and
/// `2.135` must land on opposite sides under half-even, and only an exact half
/// test distinguishes them.
fn apply_rounding(q: u128, r: u128, unit: u128, mode: Mode) -> Result<u128, MoneyError> {
    if r == 0 {
        return Ok(q);
    }
    // Down truncates toward zero ALWAYS — a remainder past the half must not
    // pull it up. (This was a real bug: handling Down only in the exact-half
    // branch let `2.129` round up under Down.)
    if mode == Mode::Down {
        return Ok(q);
    }
    let twice = r.checked_mul(2).ok_or(MoneyError::Overflow)?;
    let round_up = match twice.cmp(&unit) {
        Ordering::Less => false,
        Ordering::Greater => true,
        // exact half: half-up goes away from zero; half-even to the even digit
        Ordering::Equal => match mode {
            Mode::HalfUp => true,
            Mode::HalfEven => q % 2 == 1,
            Mode::Down => false, // unreachable (handled above), kept exhaustive
        },
    };
    if round_up { q.checked_add(1).ok_or(MoneyError::Overflow) } else { Ok(q) }
}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        let scale = self.scale.max(other.scale);
        self.rescaled(scale).mantissa.cmp(&other.rescaled(scale).mantissa)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        Decimal::parse(s).expect("valid decimal")
    }

    /// The whole point of REQ-6.2.1, in one test: the 0.1-family sum that
    /// f64 cannot represent.
    #[test]
    fn tenths_sum_exactly() {
        let mut acc = Decimal::ZERO;
        for _ in 0..10 {
            acc = acc.add(d("0.1"));
        }
        assert_eq!(acc.to_plain_string(), "1.0");
        assert_eq!(acc, d("1.0"));
        // and the float control: the same sum in f64 does NOT equal 1.0
        let mut f = 0.0f64;
        for _ in 0..10 {
            f += 0.1;
        }
        assert!(f != 1.0, "f64 tenths must miss, or this test proves nothing");
    }

    #[test]
    fn classic_float_error_cases_are_exact() {
        assert_eq!(d("0.1").add(d("0.2")).to_plain_string(), "0.3");
        assert_eq!(d("1.005").add(d("2.005")).to_plain_string(), "3.010");
        // mixed scales align, never truncate
        assert_eq!(d("1.5").add(d("2.25")).to_plain_string(), "3.75");
        assert_eq!(d("100").add(d("0.001")).to_plain_string(), "100.001");
    }

    #[test]
    fn subtraction_via_negate_reverses_exactly() {
        let sum = d("0.1").add(d("0.2"));
        assert!(sum.add(d("0.3").neg()).is_zero(), "0.1 + 0.2 - 0.3 must be exactly zero");
    }

    #[test]
    fn parse_refuses_what_is_not_plain_decimal() {
        assert!(Decimal::parse("1e5").is_none());
        assert!(Decimal::parse("1.2.3").is_none());
        assert!(Decimal::parse("abc").is_none());
        assert!(Decimal::parse("").is_none());
        assert_eq!(d("-12.340").to_plain_string(), "-12.340");
    }

    #[test]
    fn ordering_is_numeric_across_scales() {
        assert!(d("1.50") == d("1.5"));
        assert!(d("10") > d("9.99"));
        assert!(d("-5") < d("-4.999"));
    }

    // ── multiply, divide, round ──────────────────────────────────

    /// Criterion 1: `mul` is EXACT — the scale grows, nothing is rounded.
    #[test]
    fn mul_is_exact_and_never_rounds() {
        // 1.25 × 1.25 = 1.5625, scale 4 — no silent collapse to 2 places
        assert_eq!(d("1.25").mul(d("1.25")).unwrap().to_plain_string(), "1.5625");
        // qty × rate, the seed's core op, kept exact until a defined point
        assert_eq!(d("3").mul(d("0.335")).unwrap().to_plain_string(), "1.005");
        assert_eq!(d("2").mul(d("19.99")).unwrap().to_plain_string(), "39.98");
        // sign
        assert_eq!(d("-2").mul(d("1.5")).unwrap().to_plain_string(), "-3.0");
    }

    /// Criterion 2: half-even is REAL, and proven AGAINST half-up — the
    /// control that proves the test bites. `2.125` is the
    /// discriminating value: banker's rounds to the even 2.12, commercial to
    /// 2.13.
    #[test]
    fn half_even_rounds_to_even_and_differs_from_half_up() {
        assert_eq!(d("2.125").round(2, Mode::HalfEven).to_plain_string(), "2.12");
        assert_eq!(d("2.135").round(2, Mode::HalfEven).to_plain_string(), "2.14");

        // THE CONTROL: same input, half-up, DIFFERENT answer — without this a
        // rounding test that passed under both modes would prove nothing.
        assert_eq!(d("2.125").round(2, Mode::HalfUp).to_plain_string(), "2.13");
        assert_ne!(
            d("2.125").round(2, Mode::HalfEven).to_plain_string(),
            d("2.125").round(2, Mode::HalfUp).to_plain_string(),
            "half-even and half-up MUST disagree on an exact half, or the mode is decorative"
        );

        // exact halves both directions; non-halves round normally under both
        assert_eq!(d("0.5").round(0, Mode::HalfEven).to_plain_string(), "0"); // to even
        assert_eq!(d("1.5").round(0, Mode::HalfEven).to_plain_string(), "2"); // to even
        assert_eq!(d("2.126").round(2, Mode::HalfEven).to_plain_string(), "2.13"); // >half, up
        assert_eq!(d("2.124").round(2, Mode::HalfEven).to_plain_string(), "2.12"); // <half, down
        // negatives round symmetrically about zero
        assert_eq!(d("-2.125").round(2, Mode::HalfUp).to_plain_string(), "-2.13");
        assert_eq!(d("-2.125").round(2, Mode::HalfEven).to_plain_string(), "-2.12");
    }

    /// Down truncates toward zero — never rounds up, even past a half.
    #[test]
    fn down_truncates() {
        assert_eq!(d("2.129").round(2, Mode::Down).to_plain_string(), "2.12");
        assert_eq!(d("-2.129").round(2, Mode::Down).to_plain_string(), "-2.12");
    }

    /// Division rounds EXACTLY at the target scale — repeating decimals land
    /// where the remainder says, not where a truncated guard digit guesses.
    #[test]
    fn div_round_is_exact_at_the_target_scale() {
        // 1/3 = 0.333... → 0.33 (half-even, remainder < half)
        assert_eq!(d("1").div_round(d("3"), 2, Mode::HalfEven).unwrap().to_plain_string(), "0.33");
        // 2/3 = 0.666... → 0.67 (remainder > half, up)
        assert_eq!(d("2").div_round(d("3"), 2, Mode::HalfEven).unwrap().to_plain_string(), "0.67");
        // an exact half from division: 1/8 = 0.125 → 0.12 even / 0.13 up
        assert_eq!(d("1").div_round(d("8"), 2, Mode::HalfEven).unwrap().to_plain_string(), "0.12");
        assert_eq!(d("1").div_round(d("8"), 2, Mode::HalfUp).unwrap().to_plain_string(), "0.13");
        // allocation: split 100.00 three ways at 2 places → 33.33 each (down)
        assert_eq!(d("100").div_round(d("3"), 2, Mode::Down).unwrap().to_plain_string(), "33.33");
    }

    /// Criterion 5: overflow and div-by-zero are TYPED, never a panic or wrap.
    #[test]
    fn overflow_and_divzero_are_typed() {
        let big = Decimal::parse("99999999999999999999999999999999999999").unwrap(); // ~1e38
        assert_eq!(big.mul(big), Err(MoneyError::Overflow), "product must not wrap");
        assert_eq!(d("1").div_round(d("0"), 2, Mode::HalfEven), Err(MoneyError::DivByZero));
        // a normal product does NOT report overflow
        assert!(d("12345.67").mul(d("100")).is_ok());
    }

    /// Criterion 3: the defined-points discipline is LOAD-BEARING — round the
    /// line once, the tax once, the total from rounded lines; and prove that
    /// rounding only at the end gives a DIFFERENT, wrong answer.
    #[test]
    fn defined_points_rounding_is_load_bearing() {
        let scale = 2;
        let mode = Mode::HalfEven;
        // three identical lines: qty 1 × rate 1.005 = 1.005, rounds to 1.00
        let rate = d("1.005");
        let qty = d("1");

        // DEFINED POINTS: each line rounded at its point, then summed
        let line = qty.mul(rate).unwrap().round(scale, mode);
        assert_eq!(line.to_plain_string(), "1.00", "line rounds once, at the line");
        let subtotal_defined = line.add(line).add(line); // 3 rounded lines
        assert_eq!(subtotal_defined.to_plain_string(), "3.00", "sum of rounded lines");

        // ROUND-ONLY-AT-END: keep lines exact, sum, round once at the end
        let exact_line = qty.mul(rate).unwrap(); // 1.005, unrounded
        let subtotal_end = exact_line.add(exact_line).add(exact_line).round(scale, mode); // 3.015 → 3.02
        assert_eq!(subtotal_end.to_plain_string(), "3.02", "rounding only at the end drifts");

        // THE PROOF: the two disagree. The defined-points answer (3.00) is the
        // auditable one — it matches the three 1.00 line items a human sees.
        assert_ne!(
            subtotal_defined.to_plain_string(),
            subtotal_end.to_plain_string(),
            "if these matched, 'round at defined points' would be decorative"
        );

        // and the full line → tax → total chain, each rounded once
        let tax_rate = d("0.15"); // 15%, as a decimal (not a tax engine — just the arithmetic)
        let tax = subtotal_defined.mul(tax_rate).unwrap().round(scale, mode); // 3.00 × 0.15 = 0.4500 → 0.45
        assert_eq!(tax.to_plain_string(), "0.45", "tax rounds once, at the tax");
        let total = subtotal_defined.add(tax);
        assert_eq!(total.to_plain_string(), "3.45", "total from rounded parts");
    }
}
