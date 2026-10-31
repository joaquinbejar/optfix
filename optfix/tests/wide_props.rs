//! Property test for `wide::mul_shr`: the exact floor or ceiling of `a × b / 2^shift`, the
//! two directions at most one apart, and `Overflow` exactly when the rounded value does not fit
//! in a `u128`. The reference is `num-bigint`.

#![allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::panic_in_result_fn,
    clippy::disallowed_macros,
    missing_docs
)]

use num_bigint::BigUint;
use optfix::wide::{mul_shr, MUL_SHR_MAX_ERROR_ULPS};
use optfix::{PricingError, Rounding};
use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};

/// Cases per property. `OPTFIX_PROPTEST_CASES` overrides the default of one million.
fn config() -> Config {
    let cases = std::env::var("OPTFIX_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000_000);
    Config {
        cases,
        rng_seed: RngSeed::Fixed(0x0F7F_1C5E_ED00_0004),
        failure_persistence: None,
        ..Config::default()
    }
}

/// Log-uniform magnitudes over the whole `u128` range, plus the extremes.
fn wide() -> impl Strategy<Value = u128> {
    prop_oneof![
        6 => (any::<u128>(), 0u32..128).prop_map(|(x, s)| x >> s),
        1 => proptest::sample::select(vec![0, 1, 2, u128::MAX, u128::MAX - 1, 1 << 127, 1 << 64]),
    ]
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn prop_mul_shr_is_the_exact_directed_rounding(a in wide(), b in wide(), shift in 0u32..=260) {
        let n = BigUint::from(a) * BigUint::from(b);
        let floor = &n >> shift;
        let exact = (&floor << shift) == n;
        let ceil = if exact { floor.clone() } else { &floor + 1u32 };
        let max = BigUint::from(u128::MAX);
        for (r, want) in [(Rounding::Down, &floor), (Rounding::Up, &ceil)] {
            match mul_shr(a, b, shift, r) {
                Ok(v) => prop_assert_eq!(&BigUint::from(v), want),
                Err(e) => {
                    prop_assert_eq!(e, PricingError::Overflow);
                    prop_assert!(want > &max, "Overflow for a fitting result");
                }
            }
        }
        if let (Ok(d), Ok(u)) = (mul_shr(a, b, shift, Rounding::Down), mul_shr(a, b, shift, Rounding::Up)) {
            prop_assert!(u - d <= u128::from(MUL_SHR_MAX_ERROR_ULPS));
        }
    }
}

#[test]
fn scratch_ci_control_fails() {
    panic!("scratch CI control");
}
