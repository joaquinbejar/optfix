//! The signed fixed-point number and its directed-rounding arithmetic.

use bytemuck::{Pod, Zeroable};

use crate::u256::U256;
use crate::PricingError;

/// Number of fractional bits: `Fixed` is binary Q64.64, chosen by measurement in milestone 3
/// (`docs/PLAN.md`, decision log).
pub(crate) const FRAC_BITS: u32 = 64;

/// `2^64`: the raw value of `1.0`.
const SCALE: i128 = 1 << FRAC_BITS;

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

/// Signed fixed-point number with value `raw / 2^64` (binary Q64.64).
///
/// Resolution (one ulp) is `2^-64`, about `5.4 × 10^-20`; the range is about `±9.2 × 10^18`.
/// Decimal values such as `0.1` are not exact: convert them with [`Fixed::from_ratio`] and an
/// explicit rounding, and back to token units with [`Fixed::to_scaled`]. `Fixed` does not
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
    /// The largest value, `2^63 − 2^-64`.
    pub const MAX: Self = Self(i128::MAX);
    /// The smallest value, `−2^63`.
    pub const MIN: Self = Self(i128::MIN);

    /// Wraps a raw value `raw / 2^64`. Every `i128` is a valid `Fixed`, so this cannot fail;
    /// it exists for zero-copy storage.
    #[must_use]
    #[inline]
    pub const fn from_raw(raw: i128) -> Self {
        Self(raw)
    }

    /// The raw value: `self × 2^64`.
    #[must_use]
    #[inline]
    pub const fn raw(self) -> i128 {
        self.0
    }

    /// The integer `n`, exactly.
    #[must_use]
    #[inline]
    pub const fn from_int(n: i64) -> Self {
        // n × 2^64 is in [−2^127, 2^127 − 2^64]: the shift never loses a bit.
        Self((n as i128) << FRAC_BITS)
    }

    /// `num / den`, rounded in direction `r`.
    ///
    /// # Errors
    ///
    /// `DomainError` if `den == 0`; `Overflow` if the result does not fit.
    #[must_use = "the result carries a possible error"]
    pub fn from_ratio(num: i128, den: i128, r: Rounding) -> Result<Self, PricingError> {
        // num / den = (num × 2^64 / den) / 2^64.
        scaled_quotient(num, den, r)
    }

    /// `self × 10^decimals` as an integer, rounded in direction `r`: converts to an amount in
    /// a token's smallest unit (for example `decimals = 6` for USDC).
    ///
    /// # Errors
    ///
    /// `Overflow` if the result, or the factor `10^(decimals − 38)` when `decimals > 38`, does
    /// not fit in an `i128`.
    #[must_use = "the result carries a possible error"]
    pub fn to_scaled(self, decimals: u8, r: Rounding) -> Result<i128, PricingError> {
        // raw × 10^decimals / 2^64. 10^38 is the largest power of ten in an i128; above it
        // the excess factor goes on the raw value first, and when that overflows so does the
        // result (it is then at least 2^127 × 10^38 / 2^64).
        let (a, b) = match decimals.checked_sub(38) {
            Some(excess) if excess > 0 => {
                let head = self
                    .0
                    .checked_mul(pow10(excess)?)
                    .ok_or(PricingError::Overflow)?;
                (head, pow10(38)?)
            }
            _ => (self.0, pow10(decimals)?),
        };
        scaled_product(a, b, r).map(Self::raw)
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
        // a × b = (a.raw × b.raw / 2^64) / 2^64: a shift, no division.
        scaled_product(self.0, o.0, Rounding::Down)
    }

    /// `self × o`, rounded up (toward positive infinity). Uses a 256-bit intermediate.
    ///
    /// # Errors
    ///
    /// `Overflow` if the result does not fit.
    #[must_use = "the result carries a possible error"]
    pub fn mul_up(self, o: Self) -> Result<Self, PricingError> {
        scaled_product(self.0, o.0, Rounding::Up)
    }

    /// `self / o`, rounded down (toward negative infinity).
    ///
    /// # Errors
    ///
    /// `DomainError` if `o == 0`; `Overflow` if the result does not fit.
    #[must_use = "the result carries a possible error"]
    pub fn div_down(self, o: Self) -> Result<Self, PricingError> {
        // a / b = (a.raw × 2^64 / b.raw) / 2^64.
        scaled_quotient(self.0, o.0, Rounding::Down)
    }

    /// `self / o`, rounded up (toward positive infinity).
    ///
    /// # Errors
    ///
    /// `DomainError` if `o == 0`; `Overflow` if the result does not fit.
    #[must_use = "the result carries a possible error"]
    pub fn div_up(self, o: Self) -> Result<Self, PricingError> {
        scaled_quotient(self.0, o.0, Rounding::Up)
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

    /// `sqrt(self)` rounded in direction `r`: the integer square root of `raw × 2^64`.
    fn sqrt(self, r: Rounding) -> Result<Self, PricingError> {
        let raw = u128::try_from(self.0).map_err(|_| PricingError::DomainError)?;
        let n = U256::shl_64(raw);
        // n < 2^127 × 2^64, so the root is below 2^96 and the square below 2^192.
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

/// `a × b / 2^64` as a raw `Fixed`, rounded in direction `r`, with a 256-bit intermediate.
///
/// `Overflow` if the result does not fit in an `i128`.
fn scaled_product(a: i128, b: i128, r: Rounding) -> Result<Fixed, PricingError> {
    let negative = (a < 0) ^ (b < 0);
    let (q, rem) = U256::mul_wide(a.unsigned_abs(), b.unsigned_abs())?.split_64()?;
    finish(negative, q, rem != 0, r)
}

/// `a × 2^64 / d` as a raw `Fixed`, rounded in direction `r`, with a 256-bit intermediate.
///
/// `DomainError` if `d == 0`; `Overflow` if the result does not fit in an `i128`.
fn scaled_quotient(a: i128, d: i128, r: Rounding) -> Result<Fixed, PricingError> {
    if d == 0 {
        return Err(PricingError::DomainError);
    }
    let negative = (a < 0) ^ (d < 0);
    let (q, rem) = U256::shl_64(a.unsigned_abs()).div_rem(d.unsigned_abs())?;
    finish(negative, q, rem != 0, r)
}

/// The signed result `±(q + f)` for an exact magnitude `q + f`, `0 <= f < 1`, `f > 0` iff
/// `inexact`, rounded in direction `r`.
fn finish(negative: bool, q: u128, inexact: bool, r: Rounding) -> Result<Fixed, PricingError> {
    // Rounding moves the magnitude one unit away from zero when the value is inexact and the
    // direction points away from zero: up for a positive value, down for a negative one.
    let away = inexact
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
        assert_eq!(Fixed::from_int(-1).raw(), -SCALE);
        assert_eq!(
            Fixed::from_int(i64::MAX).raw(),
            i128::from(i64::MAX) * SCALE
        );
        assert_eq!(Fixed::from_int(i64::MIN), Fixed::MIN);
    }

    #[test]
    fn test_mul_rounds_toward_infinities_not_zero() {
        // 1/3 is not representable: floor and ceiling of 2^64 / 3.
        let third_down = fx(SCALE / 3);
        let three = Fixed::from_int(3);
        assert_eq!(third_down.mul_down(three).unwrap(), fx(SCALE - 1));
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
        // 2^64 = 3 × 6148914691236517205 + 1.
        assert_eq!(
            Fixed::ONE.div_down(three).unwrap(),
            fx(6_148_914_691_236_517_205)
        );
        assert_eq!(
            Fixed::ONE.div_up(three).unwrap(),
            fx(6_148_914_691_236_517_206)
        );
        assert_eq!(
            Fixed::ONE.checked_neg().unwrap().div_down(three).unwrap(),
            fx(-6_148_914_691_236_517_206)
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
    fn test_from_ratio_one_tenth_brackets_exact_value() {
        // 2^64 / 10 = 1844674407370955161.6
        assert_eq!(
            Fixed::from_ratio(1, 10, Rounding::Down).unwrap(),
            fx(1_844_674_407_370_955_161)
        );
        assert_eq!(
            Fixed::from_ratio(1, 10, Rounding::Up).unwrap(),
            fx(1_844_674_407_370_955_162)
        );
    }

    #[test]
    fn test_sqrt_of_two_brackets_exact_value() {
        let two = Fixed::from_int(2);
        // sqrt(2) × 2^64 = 26087635650665564424.699...
        assert_eq!(two.sqrt_down().unwrap(), fx(26_087_635_650_665_564_424));
        assert_eq!(two.sqrt_up().unwrap(), fx(26_087_635_650_665_564_425));
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
        // 1.25 + 2^-64: just above 1.25, so 6 decimals round to 1.250000 or 1.250001.
        let x = fx(SCALE + SCALE / 4 + 1);
        assert_eq!(x.to_scaled(6, Rounding::Down).unwrap(), 1_250_000);
        assert_eq!(x.to_scaled(6, Rounding::Up).unwrap(), 1_250_001);
        let neg = x.checked_neg().unwrap();
        assert_eq!(neg.to_scaled(6, Rounding::Down).unwrap(), -1_250_001);
        assert_eq!(neg.to_scaled(6, Rounding::Up).unwrap(), -1_250_000);
        // Exact: 1.25 × 10^2.
        assert_eq!(
            fx(SCALE + SCALE / 4).to_scaled(2, Rounding::Up).unwrap(),
            125
        );
        assert_eq!(
            Fixed::ONE.to_scaled(20, Rounding::Down).unwrap(),
            10i128.pow(20)
        );
        // One ulp at 40 decimals: 10^40 / 2^64 = 542101086242752217003.7...
        assert_eq!(
            fx(1).to_scaled(40, Rounding::Down).unwrap(),
            542_101_086_242_752_217_003
        );
        assert_eq!(
            fx(1).to_scaled(40, Rounding::Up).unwrap(),
            542_101_086_242_752_217_004
        );
    }

    #[test]
    fn test_to_scaled_large_decimals_overflow() {
        assert_eq!(
            Fixed::ONE.to_scaled(39, Rounding::Down),
            Err(PricingError::Overflow)
        );
        assert_eq!(Fixed::ZERO.to_scaled(76, Rounding::Down), Ok(0));
        assert_eq!(
            Fixed::ZERO.to_scaled(77, Rounding::Down),
            Err(PricingError::Overflow)
        );
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
