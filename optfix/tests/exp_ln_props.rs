//! Property tests for `exp` and `ln` in both milestone 3 representations.
//!
//! The reference is an arbitrary-precision fixed-point implementation written here on
//! `num-bigint` with 256 fractional bits (series for `ln 2`, `e^r` and `atanh`), so it shares
//! no code or constants with the crate's kernel. `tools/reference.txt`, produced by Python's
//! correctly rounded `decimal` module (`tools/gen_reference.py`), checks the reference itself.
//!
//! Errors are measured in ulps of the representation: for `exp`, ulps of `max(e^x, 1)` (a
//! relative bound for large results, an absolute one below 1); for `ln`, absolute ulps.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::float_arithmetic,
    clippy::cast_precision_loss,
    clippy::panic_in_result_fn,
    clippy::disallowed_macros,
    missing_docs
)]

use std::sync::OnceLock;

use num_bigint::BigInt;
use optfix::repr::{Binary, BinaryAtanh, Decimal, DecimalNative, Repr};
use optfix::PricingError;
use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};

/// Fractional bits of the reference.
const P: u32 = 256;

/// Cases per property. `OPTFIX_PROPTEST_CASES` overrides the default of one million.
fn config() -> Config {
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

fn one() -> BigInt {
    BigInt::from(1) << P
}

/// `a × b` in Q.P, truncated.
fn mul(a: &BigInt, b: &BigInt) -> BigInt {
    (a * b) >> P
}

/// `a / b` in Q.P, truncated.
fn div(a: &BigInt, b: &BigInt) -> BigInt {
    (a << P) / b
}

/// `2 atanh(s) = 2 (s + s^3/3 + s^5/5 + ...)` for `|s| <= 1/3`.
fn two_atanh(s: &BigInt) -> BigInt {
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
fn floor_div(a: &BigInt, b: &BigInt) -> BigInt {
    let q = a / b;
    if a < &BigInt::from(0) && &q * b != *a {
        q - 1
    } else {
        q
    }
}

fn ln2() -> &'static BigInt {
    static LN2: OnceLock<BigInt> = OnceLock::new();
    // ln 2 = 2 atanh(1/3).
    LN2.get_or_init(|| two_atanh(&(one() / 3)))
}

/// `e^x` for `x` in Q.P.
fn exp_ref(x: &BigInt) -> BigInt {
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
fn ln_ref(x: &BigInt) -> BigInt {
    assert!(x > &BigInt::from(0));
    // x = 2^e m with m in [1, 2), then ln m = 2 atanh((m − 1)/(m + 1)), |s| <= 1/3.
    let e = i64::try_from(x.bits()).unwrap() - 1 - i64::from(P);
    let m = if e >= 0 { x >> e } else { x << (-e) };
    let s = div(&(&m - one()), &(&m + one()));
    two_atanh(&s) + ln2() * e
}

trait Unit: Repr {
    /// `raw` as a Q.P reference value.
    fn to_ref(raw: i128) -> BigInt;
    /// Ulps between `raw` and the reference value `y` (`y` in Q.P), signed: `raw − y`.
    fn ulps(raw: i128, y: &BigInt) -> f64;
}

/// `num / 2^P` as an f64.
fn ratio(num: &BigInt) -> f64 {
    let shift = i64::try_from(num.bits()).unwrap() - 60;
    if shift <= 0 {
        return i64::try_from(num).unwrap() as f64 / 2f64.powi(P as i32);
    }
    let top = i64::try_from(&(num >> shift)).unwrap() as f64;
    top * 2f64.powi(shift as i32 - P as i32)
}

impl Unit for Binary {
    fn to_ref(raw: i128) -> BigInt {
        BigInt::from(raw) << (P - 64)
    }
    fn ulps(raw: i128, y: &BigInt) -> f64 {
        // (raw × 2^(P−64) − y) / 2^(P−64), in units of 2^-64.
        ratio(&((Self::to_ref(raw) - y) << 64))
    }
}

impl Unit for Decimal {
    fn to_ref(raw: i128) -> BigInt {
        (BigInt::from(raw) << P) / BigInt::from(Self::ONE)
    }
    fn ulps(raw: i128, y: &BigInt) -> f64 {
        // raw − y × 10^18, in units of 10^-18.
        ratio(&((BigInt::from(raw) << P) - y * BigInt::from(Self::ONE)))
    }
}

impl Unit for BinaryAtanh {
    fn to_ref(raw: i128) -> BigInt {
        Binary::to_ref(raw)
    }
    fn ulps(raw: i128, y: &BigInt) -> f64 {
        Binary::ulps(raw, y)
    }
}

impl Unit for DecimalNative {
    fn to_ref(raw: i128) -> BigInt {
        Decimal::to_ref(raw)
    }
    fn ulps(raw: i128, y: &BigInt) -> f64 {
        Decimal::ulps(raw, y)
    }
}

/// `exp` error in ulps of `max(e^x, 1)`.
fn exp_err<R: Unit>(raw: i128, y: &BigInt) -> f64 {
    let e = R::ulps(raw, y).abs();
    if y > &one() {
        e / ratio(y)
    } else {
        e
    }
}

/// Largest representable value as a reference value.
fn max_ref<R: Unit>() -> BigInt {
    R::to_ref(i128::MAX)
}

// ---------------------------------------------------------------------------------------
// Bounds asserted by the properties (ulps of the representation).
// ---------------------------------------------------------------------------------------

const EXP_BOUND_BINARY: f64 = 3.0;
const LN_BOUND_BINARY: f64 = 2.0;
const EXP_BOUND_DECIMAL: f64 = 1.0;
const LN_BOUND_DECIMAL: f64 = 1.0;
const EXP_BOUND_NATIVE: f64 = 4.0;
const LN_BOUND_NATIVE: f64 = 4.0;

fn check_exp<R: Unit>(x: i128, bound: f64) -> Result<(), TestCaseError> {
    // Beyond ±50 the result is far outside both ranges: no reference needed.
    let far = 50 * R::ONE;
    if x.unsigned_abs() > far.unsigned_abs() {
        let want = if x > 0 {
            Err(PricingError::Overflow)
        } else {
            Ok(0)
        };
        prop_assert_eq!(R::exp(x), want, "{} exp({})", R::NAME, x);
        return Ok(());
    }
    let y = exp_ref(&R::to_ref(x));
    match R::exp(x) {
        Ok(raw) => {
            let e = exp_err::<R>(raw, &y);
            prop_assert!(e <= bound, "{} exp({x}) = {raw}: error {e} ulp", R::NAME);
        }
        Err(PricingError::Overflow) => {
            // Overflow only when the true result is within the bound of the top.
            let margin = &y >> 60;
            prop_assert!(
                &y + margin > max_ref::<R>(),
                "{} exp({x}) overflowed below the maximum",
                R::NAME
            );
        }
        Err(e) => prop_assert!(false, "{} exp({x}): unexpected {e:?}", R::NAME),
    }
    Ok(())
}

fn check_ln<R: Unit>(x: i128, bound: f64) -> Result<(), TestCaseError> {
    match R::ln(x) {
        Ok(raw) => {
            prop_assert!(x > 0);
            let e = R::ulps(raw, &ln_ref(&R::to_ref(x))).abs();
            prop_assert!(e <= bound, "{} ln({x}) = {raw}: error {e} ulp", R::NAME);
        }
        Err(PricingError::DomainError) => prop_assert!(x <= 0),
        Err(e) => prop_assert!(false, "{} ln({x}): unexpected {e:?}", R::NAME),
    }
    Ok(())
}

/// Arguments of `exp`: uniform over the interesting range `[−48, 48]` (raw), plus small
/// magnitudes, near zero and any raw value.
fn exp_arg<R: Repr>() -> impl Strategy<Value = i128> {
    let one = R::ONE;
    prop_oneof![
        4 => -48 * one..48 * one,
        2 => -one..one,
        1 => -1_000_000i128..1_000_000,
        1 => any::<i128>(),
    ]
}

/// Arguments of `ln`: log-uniform positive magnitudes over the whole raw range, values near
/// one, and any raw value (non-positive ones must be `DomainError`).
fn ln_arg<R: Repr>() -> impl Strategy<Value = i128> {
    let one = R::ONE;
    prop_oneof![
        4 => (0u32..127, any::<u128>()).prop_map(|(s, m)| ((m >> 1) >> s) as i128),
        2 => one - one / 16..one + one / 16,
        1 => any::<i128>(),
    ]
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn prop_exp_binary_within_bound(x in exp_arg::<Binary>()) {
        check_exp::<Binary>(x, EXP_BOUND_BINARY)?;
    }

    #[test]
    fn prop_exp_decimal_within_bound(x in exp_arg::<Decimal>()) {
        check_exp::<Decimal>(x, EXP_BOUND_DECIMAL)?;
    }

    #[test]
    fn prop_exp_decimal_native_within_bound(x in exp_arg::<DecimalNative>()) {
        check_exp::<DecimalNative>(x, EXP_BOUND_NATIVE)?;
    }

    #[test]
    fn prop_ln_decimal_native_within_bound(x in ln_arg::<DecimalNative>()) {
        check_ln::<DecimalNative>(x, LN_BOUND_NATIVE)?;
    }

    #[test]
    fn prop_ln_binary_atanh_within_bound(x in ln_arg::<BinaryAtanh>()) {
        check_ln::<BinaryAtanh>(x, LN_BOUND_BINARY)?;
    }

    #[test]
    fn prop_ln_binary_within_bound(x in ln_arg::<Binary>()) {
        check_ln::<Binary>(x, LN_BOUND_BINARY)?;
    }

    #[test]
    fn prop_ln_decimal_within_bound(x in ln_arg::<Decimal>()) {
        check_ln::<Decimal>(x, LN_BOUND_DECIMAL)?;
    }
}

#[test]
fn test_exact_points() {
    for (one, exp, ln) in [
        (
            Binary::ONE,
            Binary::exp as fn(i128) -> _,
            Binary::ln as fn(i128) -> _,
        ),
        (Decimal::ONE, Decimal::exp, Decimal::ln),
        (DecimalNative::ONE, DecimalNative::exp, DecimalNative::ln),
    ] {
        assert_eq!(exp(0), Ok(one));
        assert_eq!(ln(one), Ok(0));
        assert_eq!(ln(0), Err(PricingError::DomainError));
        assert_eq!(ln(-1), Err(PricingError::DomainError));
        assert_eq!(ln(i128::MIN), Err(PricingError::DomainError));
        assert_eq!(exp(i128::MAX), Err(PricingError::Overflow));
        assert_eq!(exp(i128::MIN), Ok(0));
        assert!(ln(i128::MAX).is_ok());
        assert!(ln(1).is_ok());
    }
}

/// The reference agrees with Python's correctly rounded `decimal` on every committed point.
#[test]
fn test_reference_matches_python_decimal() {
    let table = include_str!("../../tools/reference.txt");
    let mut n = 0;
    for line in table
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (func, repr, x, want) = (f[0], f[1], f[2].parse::<i128>().unwrap(), f[3]);
        let want: BigInt = want.parse().unwrap(); // y × 2^160, rounded
        let xr = match repr {
            "bin" => Binary::to_ref(x),
            "dec" => Decimal::to_ref(x),
            _ => panic!("repr {repr}"),
        };
        let got = match func {
            "exp" => exp_ref(&xr),
            "ln" => ln_ref(&xr),
            _ => panic!("func {func}"),
        };
        let got = (got + (BigInt::from(1) << (P - 161))) >> (P - 160);
        let diff = (&got - &want).magnitude().clone();
        let tol = std::cmp::max(
            want.magnitude().clone(),
            BigInt::from(1u8).magnitude() << 160u32,
        ) >> 120u32;
        assert!(diff <= tol, "{line}: reference {got}");
        n += 1;
    }
    assert!(n >= 400, "only {n} reference points");
}

// ---------------------------------------------------------------------------------------
// Precision report: maximum error over a deterministic sample per function and
// representation. `make precision` writes it to `docs/precision.md`.
// ---------------------------------------------------------------------------------------

/// splitmix64, for a sample that does not depend on proptest's internals.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn wide(&mut self) -> u128 {
        (u128::from(self.next()) << 64) | u128::from(self.next())
    }
    /// Uniform in `[lo, hi)`.
    fn range(&mut self, lo: i128, hi: i128) -> i128 {
        lo + (self.wide() % (hi - lo) as u128) as i128
    }
}

/// One sampled domain of a function: label, raw-argument generator.
struct Domain<R: Repr> {
    label: &'static str,
    gen: fn(&mut Rng) -> i128,
    _r: std::marker::PhantomData<R>,
}

fn domain<R: Repr>(label: &'static str, gen: fn(&mut Rng) -> i128) -> Domain<R> {
    Domain {
        label,
        gen,
        _r: std::marker::PhantomData,
    }
}

fn exp_domains<R: Repr>() -> Vec<Domain<R>> {
    vec![
        domain("x in [−1, 1]", |g| g.range(-R::ONE, R::ONE)),
        domain("x in [−18, 18]", |g| g.range(-18 * R::ONE, 18 * R::ONE)),
        domain("x in [−47, 47]", |g| g.range(-47 * R::ONE, 47 * R::ONE)),
    ]
}

fn ln_domains<R: Repr>() -> Vec<Domain<R>> {
    vec![
        domain("x in [0.9, 1.1]", |g| {
            g.range(R::ONE * 9 / 10, R::ONE * 11 / 10)
        }),
        domain("x in [0.05, 20]", |g| g.range(R::ONE / 20, 20 * R::ONE)),
        domain("x in [10^-6, 10^9], log-uniform", |g| {
            // 2^k × [1, 2) with k uniform over the binary exponents of [10^-6, 10^9].
            let lo = R::ONE / 1_000_000;
            let hi = R::ONE * 1_000_000_000;
            let (kl, kh) = (
                127 - lo.leading_zeros() as i32,
                127 - hi.leading_zeros() as i32,
            );
            let k = kl + (g.next() % (kh - kl + 1) as u64) as i32;
            let base = 1i128 << k;
            (base + g.range(0, base)).clamp(lo, hi)
        }),
        domain("any positive raw value", |g| {
            ((g.wide() >> 1) >> (g.next() % 127)).max(1) as i128
        }),
    ]
}

/// Max error (ulps) and the argument where it occurs.
fn max_error<R: Unit>(d: &Domain<R>, exp: bool, n: usize, seed: u64) -> (f64, i128) {
    let mut g = Rng(seed);
    let mut worst = (0.0, 0);
    for _ in 0..n {
        let x = (d.gen)(&mut g);
        let e = if exp {
            match R::exp(x) {
                Ok(raw) => exp_err::<R>(raw, &exp_ref(&R::to_ref(x))),
                Err(_) => continue,
            }
        } else {
            R::ulps(R::ln(x).unwrap(), &ln_ref(&R::to_ref(x))).abs()
        };
        if e > worst.0 {
            worst = (e, x);
        }
    }
    worst
}

fn report_rows<R: Unit>(out: &mut String, n: usize, ulp: &str) {
    use std::fmt::Write as _;
    for (name, exp, domains) in [
        ("exp", true, exp_domains::<R>()),
        ("ln", false, ln_domains::<R>()),
    ] {
        for (i, d) in domains.iter().enumerate() {
            let (e, x) = max_error::<R>(
                d,
                exp,
                n,
                0x0F7F_1C5E_0000_0000 ^ (i as u64) << 8 ^ u64::from(exp),
            );
            let xv = ratio(&R::to_ref(x));
            writeln!(
                out,
                "| `{name}` | {} | {} | {e:.3} | {ulp} | {xv:.6e} |",
                R::NAME,
                d.label
            )
            .unwrap();
        }
    }
}

fn report_ln_rows<R: Unit>(out: &mut String, n: usize, ulp: &str) {
    use std::fmt::Write as _;
    for (i, d) in ln_domains::<R>().iter().enumerate() {
        let (e, x) = max_error::<R>(d, false, n, 0x0F7F_1C5E_0000_0000 ^ (i as u64) << 8);
        let xv = ratio(&R::to_ref(x));
        writeln!(
            out,
            "| `ln` | {} | {} | {e:.3} | {ulp} | {xv:.6e} |",
            R::NAME,
            d.label
        )
        .unwrap();
    }
}

#[test]
fn test_precision_report() {
    let n: usize = std::env::var("OPTFIX_PRECISION_SAMPLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000);
    let mut s = String::new();
    s.push_str("| Function | Representation | Domain | Max error | Unit | At x |\n");
    s.push_str("| --- | --- | --- | ---: | --- | ---: |\n");
    report_rows::<Binary>(&mut s, n, "ulp `2^-64`");
    report_ln_rows::<BinaryAtanh>(&mut s, n, "ulp `2^-64`");
    report_rows::<Decimal>(&mut s, n, "ulp `10^-18`");
    report_rows::<DecimalNative>(&mut s, n, "ulp `10^-18`");
    println!("{s}");
    if let Ok(path) = std::env::var("OPTFIX_PRECISION_OUT") {
        std::fs::write(path, s).unwrap();
    }
}
