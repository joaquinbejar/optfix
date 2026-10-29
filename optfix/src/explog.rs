//! `exp` and `ln` on [`Fixed`], rounded to nearest with a published bound.
//!
//! Both run on the 64-bit-digit core in [`crate::kernel`]: Q64.64 enters and leaves it with
//! shifts only. Errors are measured against a 256-bit reference in `docs/precision.md` and
//! asserted by the property tests over one million cases each.

use crate::fixed::FRAC_BITS;
use crate::kernel::{
    exp_core, ln_core, mul_wide, normalize, round_shr_i128, round_shr_i64, round_shr_u64,
    EXP_INV_STEP_Q25, EXP_STEP_HI_Q64, EXP_STEP_LO_Q114,
};
use crate::{Fixed, PricingError};

/// Maximum error of [`exp`], in ulps (`2^-64`) of `max(e^x, 1)`: a relative bound of
/// `3 × 2^-64` (about `1.6 × 10^-19`) for results above 1 and an absolute one below.
pub const EXP_MAX_ERROR_ULPS: u32 = 3;

/// Maximum absolute error of [`ln`], in ulps (`2^-64`, about `5.4 × 10^-20`).
pub const LN_MAX_ERROR_ULPS: u32 = 2;

/// `exp` arguments at or above 47 overflow (the largest result is `e^43.668…`); at or below
/// −47 the result is below half an ulp. As raw values:
const EXP_ARG_LIMIT: i128 = 47 << FRAC_BITS;

/// `e^x`, rounded to nearest within [`EXP_MAX_ERROR_ULPS`].
///
/// Results below half an ulp (`x` below about −45.05) are [`Fixed::ZERO`], not an error.
///
/// # Errors
///
/// `Overflow` if `e^x` exceeds [`Fixed::MAX`] (`x` above about 43.668).
#[must_use = "the result carries a possible error"]
pub fn exp(x: Fixed) -> Result<Fixed, PricingError> {
    let x = x.raw();
    if x >= EXP_ARG_LIMIT {
        return Err(PricingError::Overflow);
    }
    if x <= -EXP_ARG_LIMIT {
        return Ok(Fixed::ZERO);
    }
    // n = round(x × 64 / ln 2), estimated in 64-bit arithmetic: |x| < 2^70, so x / 2^39 <
    // 2^31 and the product with the Q.25 constant is below 2^63.
    let xs = i64::try_from(x >> 39).map_err(|_| PricingError::Overflow)?;
    let n = xs
        .checked_mul(EXP_INV_STEP_Q25)
        .and_then(|p| p.checked_add(1 << 49))
        .map(|p| p >> 50)
        .ok_or(PricingError::Overflow)?;
    // r = x − n (HI + LO), the step ln2/64 split Cody-Waite style: x − n HI is exact and,
    // being about r, below 2^57 in units of 2^-64; n LO is a small correction.
    let nhi = signed(mul_wide(n.unsigned_abs(), EXP_STEP_HI_Q64)?, n < 0)?;
    let d = x
        .checked_sub(nhi)
        .and_then(|d| i64::try_from(d).ok())
        .ok_or(PricingError::Overflow)?;
    let lo = n
        .checked_mul(EXP_STEP_LO_Q114)
        .ok_or(PricingError::Overflow)?;
    // In Q.70: d × 2^6 − lo / 2^44.
    let r70 = d
        .checked_mul(64)
        .zip(round_shr_i64(lo, 44).ok())
        .and_then(|(a, b)| a.checked_sub(b))
        .ok_or(PricingError::Overflow)?;
    let (v, k) = exp_core(n, r70)?;
    // Result raw = v / 2^63 × 2^k × 2^64 = v × 2^(k + 1).
    let s = k.checked_add(1).ok_or(PricingError::Overflow)?;
    let raw = if s >= 0 {
        let s = u32::try_from(s).map_err(|_| PricingError::Overflow)?;
        // v < 2^64, so a shift by at most 64 loses no bits; the conversion checks the top.
        u128::from(v)
            .checked_shl(s)
            .filter(|_| s <= 64)
            .and_then(|r| i128::try_from(r).ok())
            .ok_or(PricingError::Overflow)?
    } else {
        let s = u32::try_from(s.unsigned_abs()).map_err(|_| PricingError::Overflow)?;
        i128::from(round_shr_u64(v, s))
    };
    Ok(Fixed::from_raw(raw))
}

/// `ln x`, rounded to nearest within [`LN_MAX_ERROR_ULPS`].
///
/// # Errors
///
/// `DomainError` if `x <= 0`.
#[must_use = "the result carries a possible error"]
pub fn ln(x: Fixed) -> Result<Fixed, PricingError> {
    let raw = u128::try_from(x.raw())
        .ok()
        .filter(|&r| r != 0)
        .ok_or(PricingError::DomainError)?;
    let (p, m) = normalize(raw)?;
    // x = raw / 2^64 = (m / 2^63) × 2^(p − 1).
    let y = ln_core(p.checked_sub(1).ok_or(PricingError::Overflow)?, m)?;
    round_shr_i128(y, 56).map(Fixed::from_raw)
}

/// `±mag` as an `i128`.
fn signed(mag: u128, negative: bool) -> Result<i128, PricingError> {
    let v = i128::try_from(mag).map_err(|_| PricingError::Overflow)?;
    if negative {
        v.checked_neg().ok_or(PricingError::Overflow)
    } else {
        Ok(v)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::disallowed_macros)]
mod tests {
    use super::{exp, ln};
    use crate::{Fixed, PricingError};

    #[test]
    fn test_exact_points() {
        assert_eq!(exp(Fixed::ZERO), Ok(Fixed::ONE));
        assert_eq!(ln(Fixed::ONE), Ok(Fixed::ZERO));
        assert_eq!(
            ln(Fixed::from_int(2)).unwrap().raw(),
            12_786_308_645_202_655_660
        );
        assert_eq!(ln(Fixed::from_int(1 << 40)).unwrap(), {
            // 40 ln 2, exactly rounded.
            Fixed::from_raw(511_452_345_808_106_226_392)
        });
    }

    #[test]
    fn test_domain_and_range_edges() {
        assert_eq!(ln(Fixed::ZERO), Err(PricingError::DomainError));
        assert_eq!(ln(Fixed::from_raw(-1)), Err(PricingError::DomainError));
        assert_eq!(ln(Fixed::MIN), Err(PricingError::DomainError));
        assert!(ln(Fixed::MAX).is_ok());
        assert!(ln(Fixed::from_raw(1)).is_ok());
        assert_eq!(exp(Fixed::MAX), Err(PricingError::Overflow));
        assert_eq!(exp(Fixed::from_int(44)), Err(PricingError::Overflow));
        assert!(exp(Fixed::from_int(43)).is_ok());
        assert_eq!(exp(Fixed::MIN), Ok(Fixed::ZERO));
        assert_eq!(exp(Fixed::from_int(-46)), Ok(Fixed::ZERO));
        assert!(exp(Fixed::from_int(-44)).unwrap() > Fixed::ZERO);
    }
}
