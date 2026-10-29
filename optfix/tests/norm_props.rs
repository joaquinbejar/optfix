//! Property tests for `norm_cdf` and `norm_pdf`: absolute bounds everywhere, the relative
//! bound on `[−8, −5]`, saturation beyond the cutoff, symmetry, and no panics.
//!
//! The reference is the 256-bit `num-bigint` implementation in `common/`.

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

mod common;

use common::{config, norm_cdf_ref, norm_pdf_ref, one, ratio, to_ref, ulps, ONE};
use num_bigint::BigInt;
use optfix::{
    norm_cdf, norm_pdf, Fixed, NORM_CDF_CUTOFF, NORM_CDF_MAX_ERROR_ULPS, NORM_CDF_TAIL_REL_ERROR,
    NORM_PDF_MAX_ERROR_ULPS,
};
use proptest::prelude::*;

/// `|x|` beyond which the reference is not evaluated (both functions are exactly saturated
/// or zero there, which the properties check directly).
const REF_LIMIT: i128 = 10 * ONE;

fn check_cdf(x: i128) -> Result<(), TestCaseError> {
    let got = norm_cdf(Fixed::from_raw(x)).expect("norm_cdf never fails");
    let cut = NORM_CDF_CUTOFF.raw();
    if x <= -cut {
        prop_assert_eq!(got, Fixed::ZERO, "N({}) below the cutoff", x);
        return Ok(());
    }
    if x >= cut {
        prop_assert_eq!(got, Fixed::ONE, "N({}) above the cutoff", x);
        return Ok(());
    }
    let y = norm_cdf_ref(&to_ref(x));
    let e = ulps(got.raw(), &y).abs();
    prop_assert!(
        e <= f64::from(NORM_CDF_MAX_ERROR_ULPS),
        "N({x}) = {}: error {e} ulp",
        got.raw()
    );
    // Relative bound on [−8, −5], beyond the final rounding (half an ulp).
    if (-8 * ONE..=-5 * ONE).contains(&x) {
        let excess = (e - 0.5).max(0.0);
        let allowed = ratio(&to_ref(NORM_CDF_TAIL_REL_ERROR.raw())) * ratio(&(&y << 64));
        prop_assert!(
            excess <= allowed,
            "N({x}): error beyond rounding {excess} ulp, relative bound allows {allowed}"
        );
    }
    Ok(())
}

fn check_pdf(x: i128) -> Result<(), TestCaseError> {
    let got = norm_pdf(Fixed::from_raw(x)).expect("norm_pdf never fails");
    prop_assert!(got >= Fixed::ZERO);
    if x.unsigned_abs() >= REF_LIMIT.unsigned_abs() {
        prop_assert_eq!(got, Fixed::ZERO, "n({}) beyond 10", x);
        return Ok(());
    }
    let e = ulps(got.raw(), &norm_pdf_ref(&to_ref(x))).abs();
    prop_assert!(
        e <= f64::from(NORM_PDF_MAX_ERROR_ULPS),
        "n({x}) = {}: error {e} ulp",
        got.raw()
    );
    Ok(())
}

/// Arguments: uniform over `[−10, 10]`, the tail `[−8, −5]`, small magnitudes, segment
/// boundaries (multiples of `1/4` and their neighbours) and any raw value.
fn arg() -> impl Strategy<Value = i128> {
    prop_oneof![
        4 => -REF_LIMIT..REF_LIMIT,
        2 => -8 * ONE..=-5 * ONE,
        1 => -ONE..ONE,
        1 => (-40i128..=40, -2i128..=2).prop_map(|(q, d)| q * (ONE / 4) + d),
        1 => any::<i128>(),
    ]
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn prop_norm_cdf_within_bounds(x in arg()) {
        check_cdf(x)?;
    }

    #[test]
    fn prop_norm_pdf_within_bound(x in arg()) {
        check_pdf(x)?;
    }

    #[test]
    fn prop_norm_cdf_is_symmetric(x in arg()) {
        let x = x.max(-i128::MAX);
        let sum = norm_cdf(Fixed::from_raw(x)).unwrap()
            .checked_add(norm_cdf(Fixed::from_raw(-x)).unwrap()).unwrap();
        prop_assert_eq!(sum, Fixed::ONE);
        prop_assert_eq!(norm_pdf(Fixed::from_raw(x)), norm_pdf(Fixed::from_raw(-x)));
    }
}

/// The cutoff is where `N(−x)` falls below half an ulp, so saturating there loses nothing.
#[test]
fn test_cutoff_is_below_half_an_ulp() {
    let tail = norm_cdf_ref(&to_ref(-NORM_CDF_CUTOFF.raw()));
    let half_ulp = one() >> 65;
    assert!(tail < half_ulp, "N(−cutoff) = {} ulp", ratio(&(tail << 64)));
}

/// `n(x)` is zero exactly where the true value is below half an ulp: check the transition.
#[test]
fn test_pdf_zero_only_below_half_an_ulp() {
    let half_ulp = one() >> 65;
    for q in 0..=40 {
        // x from 9.0 to 10.0 in steps of 1/40.
        let x = 9 * ONE + q * (ONE / 40);
        let got = norm_pdf(Fixed::from_raw(x)).unwrap();
        let y: BigInt = norm_pdf_ref(&to_ref(x));
        if got == Fixed::ZERO {
            assert!(
                y < &half_ulp + (one() >> 63),
                "n({x}) flushed to zero at {} ulp",
                ratio(&(y << 64))
            );
        }
    }
}
