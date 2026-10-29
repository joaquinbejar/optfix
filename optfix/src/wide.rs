//! Wide integer arithmetic for callers that need the crate's 256-bit intermediate directly.

use crate::u256::U256;
use crate::{PricingError, Rounding};

/// Error bound of [`mul_shr`], in units of the result: the result is the exact floor
/// ([`Rounding::Down`]) or ceiling ([`Rounding::Up`]) of `a × b / 2^shift`, so its error is
/// below one unit and its sign is the one requested; the two directions differ by at most 1.
pub const MUL_SHR_MAX_ERROR_ULPS: u32 = 1;

/// `a × b / 2^shift`, rounded in direction `r`, through the full 256-bit product.
///
/// For example `shift = 64` multiplies two Q64.64 magnitudes; `shift = 0` is an exact product
/// that fails instead of wrapping. Any `shift` is accepted: at 256 and above the result is 0,
/// or 1 rounding up a nonzero product.
///
/// # Errors
///
/// `Overflow` if the rounded result does not fit in a `u128`.
#[must_use = "the result carries a possible error"]
pub fn mul_shr(a: u128, b: u128, shift: u32, r: Rounding) -> Result<u128, PricingError> {
    let (q, inexact) = U256::mul_wide(a, b)?.shr(shift)?;
    match r {
        Rounding::Up if inexact => q.checked_add(1).ok_or(PricingError::Overflow),
        _ => Ok(q),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::disallowed_macros)]
mod tests {
    use super::mul_shr;
    use crate::{PricingError, Rounding};

    #[test]
    fn test_mul_shr_edges() {
        let (d, u) = (Rounding::Down, Rounding::Up);
        assert_eq!(mul_shr(3, 5, 0, d), Ok(15));
        assert_eq!(mul_shr(3, 5, 1, d), Ok(7));
        assert_eq!(mul_shr(3, 5, 1, u), Ok(8));
        assert_eq!(mul_shr(u128::MAX, u128::MAX, 128, d), Ok(u128::MAX - 1));
        assert_eq!(mul_shr(u128::MAX, u128::MAX, 128, u), Ok(u128::MAX));
        assert_eq!(
            mul_shr(u128::MAX, u128::MAX, 127, d),
            Err(PricingError::Overflow)
        );
        assert_eq!(mul_shr(u128::MAX, 2, 0, d), Err(PricingError::Overflow));
        assert_eq!(mul_shr(1, 1, 256, d), Ok(0));
        assert_eq!(mul_shr(1, 1, 256, u), Ok(1));
        assert_eq!(mul_shr(0, 7, 300, u), Ok(0));
        assert_eq!(mul_shr(1 << 64, 1 << 64, 64, d), Ok(1 << 64));
        // u128::MAX rounding up by one more unit overflows.
        assert_eq!(
            mul_shr(u128::MAX, u128::MAX, 128, u).and_then(|_| mul_shr(
                u128::MAX,
                (1 << 127) + 1,
                127,
                u
            )),
            Err(PricingError::Overflow)
        );
    }
}
