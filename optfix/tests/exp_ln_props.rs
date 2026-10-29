//! Property tests for `exp` and `ln`: every result is within the published bound.
//!
//! The reference is the 256-bit `num-bigint` implementation in `common/`, which shares no code
//! or constants with the crate's kernel. `tools/reference.txt`, produced by Python's correctly
//! rounded `decimal` module (`tools/gen_reference.py`), checks the reference itself.
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

mod common;

use common::{config, exp_err, exp_ref, ln_ref, to_ref, ulps, ONE};
use optfix::{exp, ln, Fixed, PricingError, EXP_MAX_ERROR_ULPS, LN_MAX_ERROR_ULPS};
use proptest::prelude::*;

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
