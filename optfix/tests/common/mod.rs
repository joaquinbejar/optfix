//! Shared test reference: arbitrary-precision fixed-point values on `num-bigint` with 256
//! fractional bits (series for `ln 2`, `π`, `e^r`, `atanh` and the normal CDF), sharing no code
//! or constants with the crate's kernels, plus the error measures and a seeded sampler.
//! `tools/reference.txt`, from Python's correctly rounded `decimal` module
//! (`tools/gen_reference.py`), checks this reference in `exp_ln_props.rs` and `norm_props.rs`.

#![allow(dead_code)]

use std::sync::OnceLock;

use num_bigint::BigInt;
use proptest::test_runner::{Config, RngSeed};

pub const P: u32 = 256;

/// Cases per property. `OPTFIX_PROPTEST_CASES` overrides the default of one million.
pub fn config() -> Config {
    let cases = std::env::var("OPTFIX_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000_000);
    Config {
        cases,
        rng_seed: RngSeed::Fixed(0x0F7F_1C5E_ED00_0003),
        failure_persistence: None,
        ..Config::default()
    }
}

// ---------------------------------------------------------------------------------------
// Reference: values are BigInt v meaning v / 2^P.
// ---------------------------------------------------------------------------------------

pub fn one() -> BigInt {
    BigInt::from(1) << P
}

/// `a × b` in Q.P, truncated.
pub fn mul(a: &BigInt, b: &BigInt) -> BigInt {
    (a * b) >> P
}

/// `a / b` in Q.P, truncated.
pub fn div(a: &BigInt, b: &BigInt) -> BigInt {
    (a << P) / b
}

/// `2 atanh(s) = 2 (s + s^3/3 + s^5/5 + ...)` for `|s| <= 1/3`.
pub fn two_atanh(s: &BigInt) -> BigInt {
    let s2 = mul(s, s);
    let mut term = s.clone();
    let mut sum = BigInt::from(0);
    let mut k = 1u32;
    while term != BigInt::from(0) {
        sum += &term / k;
        term = mul(&term, &s2);
        k += 2;
    }
    sum * 2
}

/// `floor(a / b)` for `b > 0` (`BigInt` division truncates toward zero).
pub fn floor_div(a: &BigInt, b: &BigInt) -> BigInt {
    let q = a / b;
    if a < &BigInt::from(0) && &q * b != *a {
        q - 1
    } else {
        q
    }
}

pub fn ln2() -> &'static BigInt {
    static LN2: OnceLock<BigInt> = OnceLock::new();
    // ln 2 = 2 atanh(1/3).
    LN2.get_or_init(|| two_atanh(&(one() / 3)))
}

/// `e^x` for `x` in Q.P.
pub fn exp_ref(x: &BigInt) -> BigInt {
    let l = ln2();
    // k = round(x / ln 2), r = x − k ln 2, |r| <= ln2 / 2.
    let k = floor_div(&(x + l / 2), l);
    let r = x - &k * l;
    let mut term = one();
    let mut sum = BigInt::from(0);
    let mut i = 1u32;
    while term != BigInt::from(0) {
        sum += &term;
        term = mul(&term, &r) / i;
        i += 1;
    }
    let k = i64::try_from(&k).unwrap();
    if k >= 0 {
        sum << k
    } else {
        sum >> (-k)
    }
}

/// `ln x` for `x > 0` in Q.P.
pub fn ln_ref(x: &BigInt) -> BigInt {
    assert!(x > &BigInt::from(0));
    // x = 2^e m with m in [1, 2), then ln m = 2 atanh((m − 1)/(m + 1)), |s| <= 1/3.
    let e = i64::try_from(x.bits()).unwrap() - 1 - i64::from(P);
    let m = if e >= 0 { x >> e } else { x << (-e) };
    let s = div(&(&m - one()), &(&m + one()));
    two_atanh(&s) + ln2() * e
}

/// Raw value of 1.0.
pub const ONE: i128 = 1 << 64;

/// `raw` as a Q.P reference value.
pub fn to_ref(raw: i128) -> BigInt {
    BigInt::from(raw) << (P - 64)
}

/// `num / 2^P` as an f64.
pub fn ratio(num: &BigInt) -> f64 {
    let shift = i64::try_from(num.bits()).unwrap() - 60;
    if shift <= 0 {
        return i64::try_from(num).unwrap() as f64 / 2f64.powi(P as i32);
    }
    let top = i64::try_from(&(num >> shift)).unwrap() as f64;
    top * 2f64.powi(shift as i32 - P as i32)
}

/// Signed ulps (`2^-64`) between `raw` and the reference value `y`: `raw − y`.
pub fn ulps(raw: i128, y: &BigInt) -> f64 {
    ratio(&((to_ref(raw) - y) << 64))
}

/// `exp` error in ulps of `max(e^x, 1)`.
pub fn exp_err(raw: i128, y: &BigInt) -> f64 {
    let e = ulps(raw, y).abs();
    if y > &one() {
        e / ratio(y)
    } else {
        e
    }
}

/// `π` by Machin's formula: `16 atan(1/5) − 4 atan(1/239)`.
pub fn pi() -> &'static BigInt {
    static PI: OnceLock<BigInt> = OnceLock::new();
    PI.get_or_init(|| {
        let atan_inv = |n: u32| {
            let x = one() / n;
            let x2 = mul(&x, &x);
            let (mut term, mut sum, mut k) = (x, BigInt::from(0), 1u32);
            while term != BigInt::from(0) {
                if k % 4 == 1 {
                    sum += &term / k;
                } else {
                    sum -= &term / k;
                }
                term = mul(&term, &x2);
                k += 2;
            }
            sum
        };
        atan_inv(5) * 16 - atan_inv(239) * 4
    })
}

/// `1 / √(2π)` in Q.P.
pub fn inv_sqrt_2pi() -> &'static BigInt {
    static V: OnceLock<BigInt> = OnceLock::new();
    V.get_or_init(|| {
        let two_pi: BigInt = pi() * BigInt::from(2);
        let root: BigInt = (two_pi << P).sqrt(); // √(2π) × 2^P
        (one() << P) / root
    })
}

/// `n(x) = e^(−x²/2) / √(2π)` for `x` in Q.P.
pub fn norm_pdf_ref(x: &BigInt) -> BigInt {
    let half_sq: BigInt = mul(x, x) / 2;
    mul(&exp_ref(&-half_sq), inv_sqrt_2pi())
}

/// `N(x)` for `x` in Q.P, `|x| < 12`: `N(−a) = 1/2 − n(a) S(a)` with the positive series
/// `S(a) = Σ a^(2k+1) / (2k+1)!!` (Marsaglia), and `N(a) = 1 − N(−a)`. At `a = 12` the
/// subtraction cancels about 105 of the 256 bits.
pub fn norm_cdf_ref(x: &BigInt) -> BigInt {
    let a = BigInt::from(x.magnitude().clone());
    assert!(a < BigInt::from(12) << P, "norm_cdf_ref needs |x| < 12");
    let a2 = mul(&a, &a);
    let (mut term, mut sum, mut k) = (a.clone(), BigInt::from(0), 1u32);
    while term != BigInt::from(0) {
        sum += &term;
        k += 2;
        term = mul(&term, &a2) / k;
    }
    let lower = (one() >> 1) - mul(&norm_pdf_ref(&a), &sum);
    if x < &BigInt::from(0) {
        lower
    } else {
        one() - lower
    }
}

/// splitmix64, for a sample that does not depend on proptest's internals.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn wide(&mut self) -> u128 {
        (u128::from(self.next()) << 64) | u128::from(self.next())
    }
    /// Uniform in `[lo, hi)`.
    pub fn range(&mut self, lo: i128, hi: i128) -> i128 {
        lo + (self.wide() % (hi - lo) as u128) as i128
    }
}
