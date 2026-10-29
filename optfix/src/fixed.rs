//! The signed fixed-point number and its directed-rounding arithmetic.

use bytemuck::{Pod, Zeroable};

use crate::u256::U256;
use crate::PricingError;

/// Number of decimal digits after the point. Provisional: milestone 3 chooses between this
/// decimal representation and binary Q64.64.
const SCALE_DECIMALS: u8 = 18;

/// `10^SCALE_DECIMALS`: the raw value of `1.0`.
const SCALE: i128 = 1_000_000_000_000_000_000;

/// [`SCALE`] as an unsigned divisor.
const SCALE_U: u128 = SCALE.unsigned_abs();

/// Direction of a rounding.
///
/// "Down" means toward negative infinity and "up" toward positive infinity, never toward zero,
/// so the sign of a value does not flip the meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rounding {
    /// Toward negative infinity (floor).
    Down,
    /// Toward positive infinity (ceiling).
    Up,
}

/// Signed fixed-point number with value `raw / 10^18`.
///
/// Resolution (one ulp) is `10^-18`; the range is about `±1.7 × 10^20`. `Fixed` does not
/// implement `Add`, `Mul` and the other operators: an operator can neither take a rounding
/// direction nor return an error. Every operation is checked and returns
/// [`PricingError::Overflow`] instead of wrapping.
///
/// Alignment differs between targets (`align_of::<i128>()` is 16 on 64-bit hosts and 8 on
/// SBF), so on-chain accounts store a [`FixedBytes`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Pod, Zeroable)]
#[repr(transparent)]
pub struct Fixed(i128);

impl Fixed {
    /// `0`.
    pub const ZERO: Self = Self(0);
    /// `1`.
    pub const ONE: Self = Self(SCALE);
    /// The largest value, `(2^127 − 1) / 10^18`.
    pub const MAX: Self = Self(i128::MAX);
    /// The smallest value, `−2^127 / 10^18`.
    pub const MIN: Self = Self(i128::MIN);

    /// Wraps a raw value `raw / 10^18`. Every `i128` is a valid `Fixed`, so this cannot fail;
    /// it exists for zero-copy storage.
    #[must_use]
    #[inline]
    pub const fn from_raw(raw: i128) -> Self {
        Self(raw)
    }

    /// The raw value: `self × 10^18`.
    #[must_use]
    #[inline]
    pub const fn raw(self) -> i128 {
        self.0
    }

    /// The integer `n`, exactly.
    #[must_use]
    #[inline]
    pub const fn from_int(n: i64) -> Self {
        // |n| <= 2^63 and SCALE < 2^60, so |n × SCALE| < 2^123: the product always fits.
        #[allow(clippy::arithmetic_side_effects)]
        let raw = n as i128 * SCALE;
        Self(raw)
    }

    /// `num / den`, rounded in direction `r`.
    ///
    /// # Errors
    ///
    /// `DomainError` if `den == 0`; `Overflow` if the result does not fit.
    #[must_use = "the result carries a possible error"]
    pub fn from_ratio(num: i128, den: i128, r: Rounding) -> Result<Self, PricingError> {
        // num / den = (num × SCALE / den) / SCALE.
        quotient(num, SCALE, den, r)
    }

    /// `self × 10^decimals` as an integer, rounded in direction `r`: converts to an amount in
    /// a token's smallest unit (for example `decimals = 6` for USDC).
    ///
    /// # Errors
    ///
    /// `Overflow` if the result, or the factor `10^(decimals − 18)` when `decimals > 18`, does
    /// not fit in an `i128`.
    #[must_use = "the result carries a possible error"]
    pub fn to_scaled(self, decimals: u8, r: Rounding) -> Result<i128, PricingError> {
        match SCALE_DECIMALS.checked_sub(decimals) {
            Some(shift) => {
                let d = pow10(shift)?;
                let q = self.0.checked_div_euclid(d).ok_or(PricingError::Overflow)?;
                let rem = self.0.checked_rem_euclid(d).ok_or(PricingError::Overflow)?;
                // `d > 0`, so `div_euclid` is the floor.
                match r {
                    Rounding::Down => Ok(q),
                    Rounding::Up if rem != 0 => q.checked_add(1).ok_or(PricingError::Overflow),
                    Rounding::Up => Ok(q),
                }
            }
            None => {
                let shift = decimals
                    .checked_sub(SCALE_DECIMALS)
                    .ok_or(PricingError::Overflow)?;
                // Exact: no rounding needed.
                self.0
                    .checked_mul(pow10(shift)?)
                    .ok_or(PricingError::Overflow)
            }
        }
    }

    /// `self + o`, exact.
    ///
    /// # Errors
    ///
    /// `Overflow` if the sum does not fit.
    #[must_use = "the result carries a possible error"]
    #[inline]
    pub fn checked_add(self, o: Self) -> Result<Self, PricingError> {
        self.0
            .checked_add(o.0)
            .map(Self)
            .ok_or(PricingError::Overflow)
    }

    /// `self − o`, exact.
    ///
    /// # Errors
    ///
    /// `Overflow` if the difference does not fit.
    #[must_use = "the result carries a possible error"]
    #[inline]
    pub fn checked_sub(self, o: Self) -> Result<Self, PricingError> {
        self.0
            .checked_sub(o.0)
            .map(Self)
            .ok_or(PricingError::Overflow)
    }

    /// `−self`, exact.
    ///
    /// # Errors
    ///
    /// `Overflow` for [`Fixed::MIN`], which has no negation.
    #[must_use = "the result carries a possible error"]
    #[inline]
    pub fn checked_neg(self) -> Result<Self, PricingError> {
        self.0.checked_neg().map(Self).ok_or(PricingError::Overflow)
    }

    /// `self × o`, rounded down (toward negative infinity). Uses a 256-bit intermediate.
    ///
    /// # Errors
    ///
    /// `Overflow` if the result does not fit.
    #[must_use = "the result carries a possible error"]
    pub fn mul_down(self, o: Self) -> Result<Self, PricingError> {
        // a × b = (a.raw × b.raw / SCALE) / SCALE.
        quotient(self.0, o.0, SCALE, Rounding::Down)
    }

    /// `self × o`, rounded up (toward positive infinity). Uses a 256-bit intermediate.
    ///
    /// # Errors
    ///
    /// `Overflow` if the result does not fit.
    #[must_use = "the result carries a possible error"]
    pub fn mul_up(self, o: Self) -> Result<Self, PricingError> {
        quotient(self.0, o.0, SCALE, Rounding::Up)
    }

    /// `self / o`, rounded down (toward negative infinity).
    ///
    /// # Errors
    ///
    /// `DomainError` if `o == 0`; `Overflow` if the result does not fit.
    #[must_use = "the result carries a possible error"]
    pub fn div_down(self, o: Self) -> Result<Self, PricingError> {
        // a / b = (a.raw × SCALE / b.raw) / SCALE.
        quotient(self.0, SCALE, o.0, Rounding::Down)
    }

    /// `self / o`, rounded up (toward positive infinity).
    ///
    /// # Errors
    ///
    /// `DomainError` if `o == 0`; `Overflow` if the result does not fit.
    #[must_use = "the result carries a possible error"]
    pub fn div_up(self, o: Self) -> Result<Self, PricingError> {
        quotient(self.0, SCALE, o.0, Rounding::Up)
    }

    /// `sqrt(self)`, rounded down.
    ///
    /// # Errors
    ///
    /// `DomainError` if `self < 0`.
    #[must_use = "the result carries a possible error"]
    pub fn sqrt_down(self) -> Result<Self, PricingError> {
        self.sqrt(Rounding::Down)
    }

    /// `sqrt(self)`, rounded up.
    ///
    /// # Errors
    ///
    /// `DomainError` if `self < 0`.
    #[must_use = "the result carries a possible error"]
    pub fn sqrt_up(self) -> Result<Self, PricingError> {
        self.sqrt(Rounding::Up)
    }

    /// `sqrt(self)` rounded in direction `r`: the integer square root of `raw × SCALE`.
    fn sqrt(self, r: Rounding) -> Result<Self, PricingError> {
        let raw = u128::try_from(self.0).map_err(|_| PricingError::DomainError)?;
        let n = U256::mul_wide(raw, SCALE_U)?;
        // n < 2^127 × 2^60, so the root is below 2^94 and the square below 2^188.
        let root = n.isqrt()?;
        let exact = U256::mul_wide(root, root)? == n;
        let root = match r {
            Rounding::Up if !exact => root.checked_add(1).ok_or(PricingError::Overflow)?,
            _ => root,
        };
        i128::try_from(root)
            .map(Self)
            .map_err(|_| PricingError::Overflow)
    }

    /// The little-endian storage form.
    #[must_use]
    #[inline]
    pub const fn to_bytes(self) -> FixedBytes {
        FixedBytes(self.0.to_le_bytes())
    }
}

/// A [`Fixed`] stored as 16 little-endian bytes, with alignment 1.
///
/// Use it in `#[repr(C)]` account structs: `Fixed` wraps an `i128`, which is 16-aligned on
/// 64-bit hosts but 8-aligned on SBF, while account data is only guaranteed 8-aligned. The
/// byte form has the same layout everywhere. Every 16-byte pattern is a valid `Fixed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Pod, Zeroable)]
#[repr(transparent)]
pub struct FixedBytes(pub [u8; 16]);

impl FixedBytes {
    /// The stored value.
    #[must_use]
    #[inline]
    pub const fn get(self) -> Fixed {
        Fixed(i128::from_le_bytes(self.0))
    }
}

impl From<Fixed> for FixedBytes {
    #[inline]
    fn from(x: Fixed) -> Self {
        x.to_bytes()
    }
}

impl From<FixedBytes> for Fixed {
    #[inline]
    fn from(b: FixedBytes) -> Self {
        b.get()
    }
}

/// `10^k` as an `i128`; `Overflow` above `10^38`.
fn pow10(k: u8) -> Result<i128, PricingError> {
    10i128
        .checked_pow(u32::from(k))
        .ok_or(PricingError::Overflow)
}

/// `a × b / d` as a raw `Fixed`, rounded in direction `r`, with a 256-bit intermediate.
///
/// `DomainError` if `d == 0`; `Overflow` if the result does not fit in an `i128`.
fn quotient(a: i128, b: i128, d: i128, r: Rounding) -> Result<Fixed, PricingError> {
    if d == 0 {
        return Err(PricingError::DomainError);
    }
    let negative = ((a < 0) ^ (b < 0)) ^ (d < 0);
    let n = U256::mul_wide(a.unsigned_abs(), b.unsigned_abs())?;
    let (q, rem) = n.div_rem(d.unsigned_abs())?;
    // The exact value is ±(q + rem / |d|) with 0 <= rem / |d| < 1. Rounding moves the
    // magnitude one unit away from zero when the value is inexact and the direction points
    // away from zero: up for a positive value, down for a negative one.
    let away = rem != 0
        && matches!(
            (negative, r),
            (false, Rounding::Up) | (true, Rounding::Down)
        );
    let magnitude = if away {
        q.checked_add(1).ok_or(PricingError::Overflow)?
    } else {
        q
    };
    let raw = if negative {
        0i128.checked_sub_unsigned(magnitude)
    } else {
        i128::try_from(magnitude).ok()
    };
    raw.map(Fixed).ok_or(PricingError::Overflow)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::disallowed_macros
)]
mod tests {
    use super::{Fixed, FixedBytes, Rounding, SCALE};
    use crate::PricingError;

    fn fx(raw: i128) -> Fixed {
        Fixed::from_raw(raw)
    }

    #[test]
    fn test_from_int_extremes_are_exact() {
        assert_eq!(Fixed::from_int(0), Fixed::ZERO);
        assert_eq!(Fixed::from_int(1), Fixed::ONE);
        assert_eq!(
            Fixed::from_int(i64::MAX).raw(),
            i128::from(i64::MAX) * SCALE
        );
        assert_eq!(
            Fixed::from_int(i64::MIN).raw(),
            i128::from(i64::MIN) * SCALE
        );
    }

    #[test]
    fn test_mul_rounds_toward_infinities_not_zero() {
        let third = fx(333_333_333_333_333_333); // just below 1/3
        let three = Fixed::from_int(3);
        assert_eq!(third.mul_down(three).unwrap(), fx(999_999_999_999_999_999));
        // 1 ulp × 0.5: exact 0.5 ulp
        let half = fx(SCALE / 2);
        assert_eq!(fx(1).mul_down(half).unwrap(), fx(0));
        assert_eq!(fx(1).mul_up(half).unwrap(), fx(1));
        assert_eq!(fx(-1).mul_down(half).unwrap(), fx(-1));
        assert_eq!(fx(-1).mul_up(half).unwrap(), fx(0));
    }

    #[test]
    fn test_mul_min_times_one_is_exact() {
        assert_eq!(Fixed::MIN.mul_down(Fixed::ONE).unwrap(), Fixed::MIN);
        assert_eq!(Fixed::MAX.mul_up(Fixed::ONE).unwrap(), Fixed::MAX);
    }

    #[test]
    fn test_mul_overflow_is_error() {
        assert_eq!(
            Fixed::MAX.mul_down(Fixed::from_int(2)),
            Err(PricingError::Overflow)
        );
        assert_eq!(Fixed::MIN.mul_down(fx(-SCALE)), Err(PricingError::Overflow));
    }

    #[test]
    fn test_div_by_zero_is_domain_error() {
        assert_eq!(
            Fixed::ONE.div_down(Fixed::ZERO),
            Err(PricingError::DomainError)
        );
        assert_eq!(
            Fixed::ONE.div_up(Fixed::ZERO),
            Err(PricingError::DomainError)
        );
        assert_eq!(
            Fixed::from_ratio(1, 0, Rounding::Down),
            Err(PricingError::DomainError)
        );
    }

    #[test]
    fn test_div_one_third_brackets_exact_value() {
        let three = Fixed::from_int(3);
        assert_eq!(
            Fixed::ONE.div_down(three).unwrap(),
            fx(333_333_333_333_333_333)
        );
        assert_eq!(
            Fixed::ONE.div_up(three).unwrap(),
            fx(333_333_333_333_333_334)
        );
        assert_eq!(
            Fixed::ONE.checked_neg().unwrap().div_down(three).unwrap(),
            fx(-333_333_333_333_333_334)
        );
    }

    #[test]
    fn test_div_min_by_minus_one_is_overflow() {
        assert_eq!(Fixed::MIN.div_down(fx(-SCALE)), Err(PricingError::Overflow));
    }

    #[test]
    fn test_from_ratio_negative_denominator_flips_sign() {
        assert_eq!(
            Fixed::from_ratio(1, -4, Rounding::Down).unwrap(),
            fx(-SCALE / 4)
        );
    }

    #[test]
    fn test_sqrt_of_two_brackets_exact_value() {
        let two = Fixed::from_int(2);
        assert_eq!(two.sqrt_down().unwrap(), fx(1_414_213_562_373_095_048));
        assert_eq!(two.sqrt_up().unwrap(), fx(1_414_213_562_373_095_049));
        assert_eq!(Fixed::from_int(4).sqrt_up().unwrap(), Fixed::from_int(2));
    }

    #[test]
    fn test_sqrt_negative_is_domain_error() {
        assert_eq!(fx(-1).sqrt_down(), Err(PricingError::DomainError));
    }

    #[test]
    fn test_sqrt_max_succeeds() {
        assert!(Fixed::MAX.sqrt_up().is_ok());
    }

    #[test]
    fn test_checked_neg_min_is_overflow() {
        assert_eq!(Fixed::MIN.checked_neg(), Err(PricingError::Overflow));
    }

    #[test]
    fn test_add_sub_overflow_is_error() {
        assert_eq!(Fixed::MAX.checked_add(fx(1)), Err(PricingError::Overflow));
        assert_eq!(Fixed::MIN.checked_sub(fx(1)), Err(PricingError::Overflow));
    }

    #[test]
    fn test_to_scaled_rounds_toward_infinities() {
        let x = fx(1_234_567_890_123_456_789); // 1.234567890123456789
        assert_eq!(x.to_scaled(6, Rounding::Down).unwrap(), 1_234_567);
        assert_eq!(x.to_scaled(6, Rounding::Up).unwrap(), 1_234_568);
        let neg = x.checked_neg().unwrap();
        assert_eq!(neg.to_scaled(6, Rounding::Down).unwrap(), -1_234_568);
        assert_eq!(neg.to_scaled(6, Rounding::Up).unwrap(), -1_234_567);
        assert_eq!(x.to_scaled(18, Rounding::Up).unwrap(), x.raw());
        assert_eq!(
            Fixed::ONE.to_scaled(20, Rounding::Down).unwrap(),
            10i128.pow(20)
        );
    }

    #[test]
    fn test_to_scaled_large_decimals_overflow() {
        assert_eq!(
            Fixed::ONE.to_scaled(57, Rounding::Down),
            Err(PricingError::Overflow)
        );
        assert_eq!(Fixed::ZERO.to_scaled(56, Rounding::Down), Ok(0));
        assert_eq!(
            Fixed::ZERO.to_scaled(u8::MAX, Rounding::Down),
            Err(PricingError::Overflow)
        );
    }

    #[test]
    fn test_fixed_bytes_round_trip() {
        for x in [Fixed::MIN, Fixed::MAX, Fixed::ZERO, fx(-7)] {
            assert_eq!(FixedBytes::from(x).get(), x);
            assert_eq!(Fixed::from(x.to_bytes()), x);
        }
        assert_eq!(core::mem::align_of::<FixedBytes>(), 1);
        assert_eq!(core::mem::size_of::<FixedBytes>(), 16);
    }
}
