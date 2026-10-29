//! Property tests for `exp` and `ln`: every result is within the published bound.
//!
//! The reference is an arbitrary-precision fixed-point implementation written here on
//! `num-bigint` with 256 fractional bits (series for `ln 2`, `e^r` and `atanh`), so it shares
//! no code or constants with the crate's kernel. `tools/reference.txt`, produced by Python's
//! correctly rounded `decimal` module (`tools/gen_reference.py`), checks the reference itself.
//!
//! Errors are measured in ulps (`2^-64`): for `exp`, ulps of `max(e^x, 1)` (a relative bound
//! for large results, an absolute one below 1); for `ln`, absolute ulps.

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
use optfix::{exp, ln, Fixed, PricingError, EXP_MAX_ERROR_ULPS, LN_MAX_ERROR_ULPS};
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

/// Raw value of 1.0.
const ONE: i128 = 1 << 64;

/// `raw` as a Q.P reference value.
fn to_ref(raw: i128) -> BigInt {
    BigInt::from(raw) << (P - 64)
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

/// Signed ulps (`2^-64`) between `raw` and the reference value `y`: `raw − y`.
fn ulps(raw: i128, y: &BigInt) -> f64 {
    ratio(&((to_ref(raw) - y) << 64))
}

/// `exp` error in ulps of `max(e^x, 1)`.
fn exp_err(raw: i128, y: &BigInt) -> f64 {
    let e = ulps(raw, y).abs();
    if y > &one() {
        e / ratio(y)
    } else {
        e
    }
}

fn check_exp(x: i128) -> Result<(), TestCaseError> {
    let bound = f64::from(EXP_MAX_ERROR_ULPS);
    // Beyond ±50 the result is far outside the range: no reference needed.
    if x.unsigned_abs() > (50 * ONE).unsigned_abs() {
        let want = if x > 0 {
            Err(PricingError::Overflow)
        } else {
            Ok(Fixed::ZERO)
        };
        prop_assert_eq!(exp(Fixed::from_raw(x)), want, "exp({})", x);
        return Ok(());
    }
    let y = exp_ref(&to_ref(x));
    match exp(Fixed::from_raw(x)) {
        Ok(v) => {
            let e = exp_err(v.raw(), &y);
            prop_assert!(e <= bound, "exp({x}) = {}: error {e} ulp", v.raw());
        }
        Err(PricingError::Overflow) => {
            // Overflow only when the true result is within the bound of the top.
            let margin = &y >> 60;
            prop_assert!(
                &y + margin > to_ref(i128::MAX),
                "exp({x}) overflowed below the maximum"
            );
        }
        Err(e) => prop_assert!(false, "exp({x}): unexpected {e:?}"),
    }
    Ok(())
}

fn check_ln(x: i128) -> Result<(), TestCaseError> {
    let bound = f64::from(LN_MAX_ERROR_ULPS);
    match ln(Fixed::from_raw(x)) {
        Ok(v) => {
            prop_assert!(x > 0);
            let e = ulps(v.raw(), &ln_ref(&to_ref(x))).abs();
            prop_assert!(e <= bound, "ln({x}) = {}: error {e} ulp", v.raw());
        }
        Err(PricingError::DomainError) => prop_assert!(x <= 0),
        Err(e) => prop_assert!(false, "ln({x}): unexpected {e:?}"),
    }
    Ok(())
}

/// Arguments of `exp`: uniform over `[−48, 48]`, small magnitudes, near zero and any raw
/// value.
fn exp_arg() -> impl Strategy<Value = i128> {
    prop_oneof![
        4 => -48 * ONE..48 * ONE,
        2 => -ONE..ONE,
        1 => -1_000_000i128..1_000_000,
        1 => any::<i128>(),
    ]
}

/// Arguments of `ln`: log-uniform positive magnitudes over the whole raw range, values near
/// one, and any raw value (non-positive ones must be `DomainError`).
fn ln_arg() -> impl Strategy<Value = i128> {
    prop_oneof![
        4 => (0u32..127, any::<u128>()).prop_map(|(s, m)| ((m >> 1) >> s) as i128),
        2 => ONE - ONE / 16..ONE + ONE / 16,
        1 => any::<i128>(),
    ]
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn prop_exp_within_bound(x in exp_arg()) {
        check_exp(x)?;
    }

    #[test]
    fn prop_ln_within_bound(x in ln_arg()) {
        check_ln(x)?;
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
            "bin" => to_ref(x),
            "dec" => (BigInt::from(x) << P) / BigInt::from(10i128.pow(18)),
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

/// One sampled domain: label and raw-argument generator.
struct Domain {
    label: &'static str,
    gen: fn(&mut Rng) -> i128,
}

fn exp_domains() -> Vec<Domain> {
    vec![
        Domain {
            label: "x in [−1, 1]",
            gen: |g| g.range(-ONE, ONE),
        },
        Domain {
            label: "x in [−18, 18]",
            gen: |g| g.range(-18 * ONE, 18 * ONE),
        },
        Domain {
            label: "x in [−47, 47]",
            gen: |g| g.range(-47 * ONE, 47 * ONE),
        },
    ]
}

fn ln_domains() -> Vec<Domain> {
    vec![
        Domain {
            label: "x in [0.9, 1.1]",
            gen: |g| g.range(ONE * 9 / 10, ONE * 11 / 10),
        },
        Domain {
            label: "x in [0.05, 20]",
            gen: |g| g.range(ONE / 20, 20 * ONE),
        },
        Domain {
            label: "x in [10^-6, 10^9], log-uniform",
            gen: |g| {
                // 2^k × [1, 2) with k uniform over the binary exponents of [10^-6, 10^9].
                let lo = ONE / 1_000_000;
                let hi = ONE * 1_000_000_000;
                let (kl, kh) = (
                    127 - lo.leading_zeros() as i32,
                    127 - hi.leading_zeros() as i32,
                );
                let k = kl + (g.next() % (kh - kl + 1) as u64) as i32;
                let base = 1i128 << k;
                (base + g.range(0, base)).clamp(lo, hi)
            },
        },
        Domain {
            label: "any positive raw value",
            gen: |g| ((g.wide() >> 1) >> (g.next() % 127)).max(1) as i128,
        },
    ]
}

/// Max error (ulps) and the argument where it occurs.
fn max_error(d: &Domain, is_exp: bool, n: usize, seed: u64) -> (f64, i128) {
    let mut g = Rng(seed);
    let mut worst = (0.0, 0);
    for _ in 0..n {
        let x = (d.gen)(&mut g);
        let e = if is_exp {
            match exp(Fixed::from_raw(x)) {
                Ok(v) => exp_err(v.raw(), &exp_ref(&to_ref(x))),
                Err(_) => continue,
            }
        } else {
            ulps(ln(Fixed::from_raw(x)).unwrap().raw(), &ln_ref(&to_ref(x))).abs()
        };
        if e > worst.0 {
            worst = (e, x);
        }
    }
    worst
}

/// Milestone 3 record, measured at commit `ca7fedd` with 1,000,000 samples per domain
/// (`make precision` there reproduces it); the decimal candidates no longer exist.
const COMPARISON: &str = "
## Representation comparison (milestone 3)

Maximum error over every domain above, measured at commit `ca7fedd` (`make precision` there \
reproduces the full table), for each candidate of `docs/PLAN.md` 4.1. Decimal ulps are \
`10^-18`. Q64.64 won on compute units (`docs/benchmarks.md`) and is the only one kept.

| Function | Representation | Max error | Unit | Absolute |
| --- | --- | ---: | --- | ---: |
| `exp` | Q64.64 | 1.974 | ulp `2^-64` of `max(e^x, 1)` | `1.1 × 10^-19` |
| `ln` | Q64.64 | 1.507 | ulp `2^-64` | `8.2 × 10^-20` |
| `ln` | Q64.64, atanh form | 1.502 | ulp `2^-64` | `8.1 × 10^-20` |
| `exp` | decimal `10^18`, binary core | 0.594 | ulp `10^-18` of `max(e^x, 1)` | `5.9 × 10^-19` |
| `ln` | decimal `10^18`, binary core | 0.597 | ulp `10^-18` | `6.0 × 10^-19` |
| `exp` | decimal `10^18`, native | 3.127 | ulp `10^-18` of `max(e^x, 1)` | `3.1 × 10^-18` |
| `ln` | decimal `10^18`, native | 3.032 | ulp `10^-18` | `3.0 × 10^-18` |
";

#[test]
fn test_precision_report() {
    let n: usize = std::env::var("OPTFIX_PRECISION_SAMPLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000);
    let mut s = String::new();
    s.push_str("# Precision\n\n");
    s.push_str(&format!(
        "Generated by `make precision` (`optfix/tests/exp_ln_props.rs`); do not edit by hand. \
         Maximum error over {n} deterministic samples per row (splitmix64), against a \
         256-bit reference (`num-bigint` series, itself checked against Python's correctly \
         rounded `decimal` in `tools/reference.txt`). One ulp is `2^-64`, about \
         `5.4 × 10^-20`. `exp` errors are in ulps of `max(e^x, 1)`, a relative bound for \
         results above 1 and an absolute one below; `ln` errors are absolute. The bounds are \
         `EXP_MAX_ERROR_ULPS` and `LN_MAX_ERROR_ULPS`, which the property tests (`make test`) \
         assert over one million random cases each.\n\n"
    ));
    s.push_str("| Function | Domain | Max error (ulp) | Bound (ulp) | At x |\n");
    s.push_str("| --- | --- | ---: | ---: | ---: |\n");
    for (name, is_exp, bound, domains) in [
        ("exp", true, EXP_MAX_ERROR_ULPS, exp_domains()),
        ("ln", false, LN_MAX_ERROR_ULPS, ln_domains()),
    ] {
        for (i, d) in domains.iter().enumerate() {
            let seed = 0x0F7F_1C5E_0000_0000 ^ (i as u64) << 8 ^ u64::from(is_exp);
            let (e, x) = max_error(d, is_exp, n, seed);
            let xv = ratio(&to_ref(x));
            s.push_str(&format!(
                "| `{name}` | {} | {e:.3} | {bound} | {xv:.6e} |\n",
                d.label
            ));
        }
    }
    s.push_str(COMPARISON);
    s.push_str("\nRun `make precision` to regenerate this file.\n");
    println!("{s}");
    if let Ok(path) = std::env::var("OPTFIX_PRECISION_OUT") {
        std::fs::write(path, s).unwrap();
    }
}
