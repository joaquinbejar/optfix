//! Property tests for `Fixed`: every directed operation brackets the exact value, the two
//! directions differ by at most one ulp, overflow is reported exactly when the rounded value
//! does not fit, and nothing panics.
//!
//! The reference is a small arbitrary-precision integer written here with `u32` limbs, so it
//! shares no code with the crate's `U256`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::panic_in_result_fn,
    clippy::disallowed_macros,
    missing_docs
)]

use std::cmp::Ordering;

use optfix::{
    Fixed, FixedBytes, Price, PricingError, Rate, Rounding, Spot, Strike, Volatility, Years,
    SECONDS_PER_YEAR,
};
use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};

/// Raw value of 1.0: `Fixed` is Q64.64.
const SCALE: i128 = 1 << 64;

/// Cases per property. `OPTFIX_PROPTEST_CASES` overrides the default of one million.
fn config() -> Config {
    let cases = std::env::var("OPTFIX_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000_000);
    Config {
        cases,
        rng_seed: RngSeed::Fixed(0x0F7F_1C5E_ED00_0001),
        failure_persistence: None,
        ..Config::default()
    }
}

// ---------------------------------------------------------------------------------------
// Reference arithmetic: signed integers of any size, little-endian u32 limbs.
// ---------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Big {
    neg: bool,
    mag: Vec<u32>,
}

fn trim(mut v: Vec<u32>) -> Vec<u32> {
    while v.last() == Some(&0) {
        v.pop();
    }
    v
}

fn mag_from_u128(mut x: u128) -> Vec<u32> {
    let mut v = Vec::new();
    while x != 0 {
        v.push(x as u32);
        x >>= 32;
    }
    v
}

fn mag_cmp(a: &[u32], b: &[u32]) -> Ordering {
    a.len()
        .cmp(&b.len())
        .then_with(|| a.iter().rev().cmp(b.iter().rev()))
}

fn mag_add(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len().max(b.len()) + 1);
    let mut carry = 0u64;
    for i in 0..a.len().max(b.len()) {
        let s = u64::from(*a.get(i).unwrap_or(&0)) + u64::from(*b.get(i).unwrap_or(&0)) + carry;
        out.push(s as u32);
        carry = s >> 32;
    }
    out.push(carry as u32);
    trim(out)
}

/// `a − b` for `a >= b`.
fn mag_sub(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len());
    let mut borrow = 0i64;
    for (i, &x) in a.iter().enumerate() {
        let mut d = i64::from(x) - i64::from(*b.get(i).unwrap_or(&0)) - borrow;
        borrow = 0;
        if d < 0 {
            d += 1 << 32;
            borrow = 1;
        }
        out.push(d as u32);
    }
    assert_eq!(borrow, 0, "mag_sub underflow");
    trim(out)
}

fn mag_mul(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = vec![0u32; a.len() + b.len() + 1];
    for (i, &x) in a.iter().enumerate() {
        let mut carry = 0u64;
        for (j, &y) in b.iter().enumerate() {
            let t = u64::from(out[i + j]) + u64::from(x) * u64::from(y) + carry;
            out[i + j] = t as u32;
            carry = t >> 32;
        }
        let mut k = i + b.len();
        while carry != 0 {
            let t = u64::from(out[k]) + carry;
            out[k] = t as u32;
            carry = t >> 32;
            k += 1;
        }
    }
    trim(out)
}

impl Big {
    fn new(neg: bool, mag: Vec<u32>) -> Self {
        let mag = trim(mag);
        let neg = neg && !mag.is_empty();
        Self { neg, mag }
    }

    fn int(x: i128) -> Self {
        Self::new(x < 0, mag_from_u128(x.unsigned_abs()))
    }

    fn mul(&self, o: &Self) -> Self {
        Self::new(self.neg != o.neg, mag_mul(&self.mag, &o.mag))
    }

    fn add(&self, o: &Self) -> Self {
        if self.neg == o.neg {
            return Self::new(self.neg, mag_add(&self.mag, &o.mag));
        }
        match mag_cmp(&self.mag, &o.mag) {
            Ordering::Less => Self::new(o.neg, mag_sub(&o.mag, &self.mag)),
            _ => Self::new(self.neg, mag_sub(&self.mag, &o.mag)),
        }
    }

    fn add_int(&self, x: i128) -> Self {
        self.add(&Self::int(x))
    }

    fn neg(&self) -> Self {
        Self::new(!self.neg, self.mag.clone())
    }

    fn pow10(k: u32) -> Self {
        (0..k).fold(Self::int(1), |acc, _| acc.mul(&Self::int(10)))
    }
}

impl PartialOrd for Big {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Big {
    fn cmp(&self, o: &Self) -> Ordering {
        match (self.neg, o.neg) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => mag_cmp(&self.mag, &o.mag),
            (true, true) => mag_cmp(&o.mag, &self.mag),
        }
    }
}

/// Checks `got` against the exact value `n / d` (with `d > 0`) rounded in direction `r`:
/// a returned value must bracket the exact quotient, and `Overflow` must mean the rounded
/// value lies outside the `i128` range.
fn check_rounded(
    n: &Big,
    d: &Big,
    r: Rounding,
    got: Result<i128, PricingError>,
) -> Result<(), TestCaseError> {
    prop_assert!(*d > Big::int(0));
    let max = Big::int(i128::MAX);
    let min = Big::int(i128::MIN);
    match got {
        Ok(v) => {
            let lo = Big::int(v).mul(d);
            match r {
                Rounding::Down => {
                    prop_assert!(lo <= *n, "down {v} above exact");
                    prop_assert!(*n < lo.add(d), "down {v} more than 1 ulp below exact");
                }
                Rounding::Up => {
                    prop_assert!(*n <= lo, "up {v} below exact");
                    prop_assert!(lo.add(&d.neg()) < *n, "up {v} more than 1 ulp above exact");
                }
            }
        }
        Err(PricingError::Overflow) => {
            let fits = match r {
                // floor(n / d) in [MIN, MAX]  <=>  MIN·d <= n < (MAX + 1)·d
                Rounding::Down => min.mul(d) <= *n && *n < max.add_int(1).mul(d),
                // ceil(n / d) in [MIN, MAX]  <=>  (MIN − 1)·d < n <= MAX·d
                Rounding::Up => min.add_int(-1).mul(d) < *n && *n <= max.mul(d),
            };
            prop_assert!(!fits, "Overflow reported for a representable result");
        }
        Err(e) => prop_assert!(false, "unexpected error {e:?}"),
    }
    Ok(())
}

/// Down and up differ by one ulp exactly when the result is inexact.
fn check_gap(
    n: &Big,
    d: &Big,
    down: Result<i128, PricingError>,
    up: Result<i128, PricingError>,
) -> Result<(), TestCaseError> {
    if let (Ok(lo), Ok(hi)) = (down, up) {
        let exact = Big::int(lo).mul(d) == *n;
        prop_assert_eq!(hi - lo, if exact { 0 } else { 1 });
    }
    Ok(())
}

fn raw(r: Result<Fixed, PricingError>) -> Result<i128, PricingError> {
    r.map(Fixed::raw)
}

// ---------------------------------------------------------------------------------------
// Input strategies: log-uniform magnitudes plus the extremes.
// ---------------------------------------------------------------------------------------

const SPECIAL: &[i128] = &[
    0,
    1,
    -1,
    SCALE,
    -SCALE,
    SCALE - 1,
    SCALE + 1,
    i128::MAX,
    i128::MIN,
    i128::MAX - 1,
    i128::MIN + 1,
    i64::MAX as i128,
    i64::MIN as i128,
    // floor(sqrt(i128::MAX × 2^64)): squaring it with mul_* is the overflow edge.
    56_022_770_974_786_139_918_731_938_227,
    56_022_770_974_786_139_918_731_938_228,
];

fn log_uniform() -> impl Strategy<Value = i128> {
    (any::<u128>(), 0u32..128, any::<bool>()).prop_map(|(x, shift, neg)| {
        let mag = (x >> shift).min(1u128 << 127);
        if neg {
            0i128.checked_sub_unsigned(mag).unwrap()
        } else {
            i128::try_from(mag).unwrap_or(i128::MAX)
        }
    })
}

fn raw_value() -> impl Strategy<Value = i128> {
    prop_oneof![
        6 => log_uniform(),
        2 => any::<i128>(),
        1 => proptest::sample::select(SPECIAL.to_vec()),
    ]
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn prop_mul_brackets_exact_product(a in raw_value(), b in raw_value()) {
        let (x, y) = (Fixed::from_raw(a), Fixed::from_raw(b));
        let n = Big::int(a).mul(&Big::int(b));
        let d = Big::int(SCALE);
        let down = raw(x.mul_down(y));
        let up = raw(x.mul_up(y));
        check_rounded(&n, &d, Rounding::Down, down)?;
        check_rounded(&n, &d, Rounding::Up, up)?;
        check_gap(&n, &d, down, up)?;
    }

    #[test]
    fn prop_div_brackets_exact_quotient(a in raw_value(), b in raw_value()) {
        let (x, y) = (Fixed::from_raw(a), Fixed::from_raw(b));
        let down = raw(x.div_down(y));
        let up = raw(x.div_up(y));
        if b == 0 {
            prop_assert_eq!(down, Err(PricingError::DomainError));
            prop_assert_eq!(up, Err(PricingError::DomainError));
        } else {
            // a / b with a positive denominator: (±a·SCALE) / |b|.
            let n = Big::int(a).mul(&Big::int(SCALE));
            let n = if b < 0 { n.neg() } else { n };
            let d = Big::new(false, mag_from_u128(b.unsigned_abs()));
            check_rounded(&n, &d, Rounding::Down, down)?;
            check_rounded(&n, &d, Rounding::Up, up)?;
            check_gap(&n, &d, down, up)?;
        }
    }

    #[test]
    fn prop_from_ratio_brackets_exact_quotient(num in raw_value(), den in raw_value()) {
        let down = raw(Fixed::from_ratio(num, den, Rounding::Down));
        let up = raw(Fixed::from_ratio(num, den, Rounding::Up));
        if den == 0 {
            prop_assert_eq!(down, Err(PricingError::DomainError));
        } else {
            let n = Big::int(num).mul(&Big::int(SCALE));
            let n = if den < 0 { n.neg() } else { n };
            let d = Big::new(false, mag_from_u128(den.unsigned_abs()));
            check_rounded(&n, &d, Rounding::Down, down)?;
            check_rounded(&n, &d, Rounding::Up, up)?;
            check_gap(&n, &d, down, up)?;
        }
    }

    #[test]
    fn prop_sqrt_brackets_exact_root(a in raw_value()) {
        let x = Fixed::from_raw(a);
        let down = raw(x.sqrt_down());
        let up = raw(x.sqrt_up());
        if a < 0 {
            prop_assert_eq!(down, Err(PricingError::DomainError));
            prop_assert_eq!(up, Err(PricingError::DomainError));
        } else {
            // sqrt(a / SCALE) × SCALE = sqrt(a × SCALE).
            let m = Big::int(a).mul(&Big::int(SCALE));
            let s = down.expect("sqrt_down of a non-negative value");
            let t = up.expect("sqrt_up of a non-negative value");
            let sq = |v: i128| Big::int(v).mul(&Big::int(v));
            prop_assert!(sq(s) <= m && m < sq(s + 1), "sqrt_down {s} not the floor");
            prop_assert!((t == 0 || sq(t - 1) < m) && m <= sq(t), "sqrt_up {t} not the ceiling");
            prop_assert_eq!(t - s, if sq(s) == m { 0 } else { 1 });
        }
    }

    #[test]
    fn prop_to_scaled_brackets_exact_value(a in raw_value(), decimals in 0u8..=80) {
        let x = Fixed::from_raw(a);
        let down = x.to_scaled(decimals, Rounding::Down);
        let up = x.to_scaled(decimals, Rounding::Up);
        // The excess factor 10^(decimals − 38) itself must fit in an i128.
        if decimals > 38 + 38 {
            prop_assert_eq!(down, Err(PricingError::Overflow));
        } else {
            let n = Big::int(a).mul(&Big::pow10(u32::from(decimals)));
            let d = Big::int(SCALE);
            check_rounded(&n, &d, Rounding::Down, down)?;
            check_rounded(&n, &d, Rounding::Up, up)?;
            check_gap(&n, &d, down, up)?;
        }
    }

    #[test]
    fn prop_add_sub_neg_are_exact_or_overflow(a in raw_value(), b in raw_value()) {
        let (x, y) = (Fixed::from_raw(a), Fixed::from_raw(b));
        prop_assert_eq!(raw(x.checked_add(y)), a.checked_add(b).ok_or(PricingError::Overflow));
        prop_assert_eq!(raw(x.checked_sub(y)), a.checked_sub(b).ok_or(PricingError::Overflow));
        prop_assert_eq!(raw(x.checked_neg()), a.checked_neg().ok_or(PricingError::Overflow));
    }

    #[test]
    fn prop_from_seconds_brackets_exact_years(seconds in prop_oneof![
        4 => 0i64..=30 * SECONDS_PER_YEAR,
        1 => any::<i64>(),
        1 => proptest::sample::select(vec![0, 1, 30 * SECONDS_PER_YEAR, 30 * SECONDS_PER_YEAR + 1, -1, i64::MAX, i64::MIN]),
    ]) {
        for r in [Rounding::Down, Rounding::Up] {
            let got = Years::from_seconds(seconds, r);
            let max_seconds = 30 * SECONDS_PER_YEAR;
            if !(0..=max_seconds).contains(&seconds) {
                prop_assert_eq!(got, Err(PricingError::DomainError));
                continue;
            }
            let n = Big::int(i128::from(seconds)).mul(&Big::int(SCALE));
            let d = Big::int(i128::from(SECONDS_PER_YEAR));
            let v = got.expect("in range").get().raw();
            check_rounded(&n, &d, r, Ok(v))?;
        }
    }

    #[test]
    fn prop_quantity_constructors_check_range(a in raw_value()) {
        let x = Fixed::from_raw(a);
        macro_rules! check {
            ($t:ident) => {{
                let inside = $t::MIN.get() <= x && x <= $t::MAX.get();
                match $t::try_new(x) {
                    Ok(q) => prop_assert!(inside && q.get() == x),
                    Err(e) => prop_assert!(!inside && e == PricingError::DomainError),
                }
            }};
        }
        check!(Spot);
        check!(Strike);
        check!(Price);
        check!(Volatility);
        check!(Rate);
        check!(Years);
    }

    #[test]
    fn prop_fixed_bytes_round_trip(a in raw_value()) {
        let x = Fixed::from_raw(a);
        let b = FixedBytes::from(x);
        prop_assert_eq!(b.0, a.to_le_bytes());
        prop_assert_eq!(Fixed::from(b), x);
    }
}
