//! The standard normal CDF `N(x)` and PDF `n(x)`, rounded to nearest with published bounds.
//!
//! Both start from `e^(−x²/2)` as the 64-bit mantissa and exponent of the `exp` kernel, before
//! any rounding to Q64.64, so tiny results keep their relative precision. `n(x) = e^(−x²/2) / √(2π)`. For
//! `a = |x|`, `N(−a) = e^(−a²/2) P(a)` with `P` (a scaled Mills ratio, an entire function)
//! approximated by a degree-12 polynomial on each of 37 segments of width `1/4` covering
//! `[0, 9.25)`; `N(a) = 1 − N(−a)`. At `9.25` and beyond, `N(−a)` is below half an ulp, so
//! `N` saturates to exactly `0` or `1`. Coefficients come from `tools/gen_tables.py`.

use crate::explog::exp_parts;
use crate::fixed::FRAC_BITS;
use crate::kernel::{
    hi64, lo64, mul_full, mul_hi, mul_wide, round_shr_u128, round_shr_u64, INV_SQRT_2PI_Q128,
    NORM_B0, NORM_COEF, NORM_TABLE_BITS,
};
use crate::{Fixed, PricingError};

/// Maximum absolute error of [`norm_cdf`], in ulps (`2^-64`).
pub const NORM_CDF_MAX_ERROR_ULPS: u32 = 2;

/// Maximum absolute error of [`norm_pdf`], in ulps (`2^-64`).
pub const NORM_PDF_MAX_ERROR_ULPS: u32 = 2;

/// Relative bound of [`norm_cdf`] on the tail `x` in `[−8, −5]`, where `N(x)` is between
/// about `6 × 10^-16` and `3 × 10^-7`: `|error| <= NORM_CDF_TAIL_REL_ERROR × N(x)` plus the
/// final rounding to Q64.64 (half an ulp, which alone is a relative `4.4 × 10^-5` at `x = −8`).
/// The value is `2^-56`, about `1.4 × 10^-17`.
pub const NORM_CDF_TAIL_REL_ERROR: Fixed = Fixed::from_raw(1 << 8);

/// `|x|` at and beyond which `N(x)` is exactly `0` or `1`: the end of the segment table,
/// `37 / 4 = 9.25`, where `N(−9.25) ≈ 1.1 × 10^-20` is below half an ulp.
pub const NORM_CDF_CUTOFF: Fixed = Fixed::from_raw(37 << (FRAC_BITS - NORM_TABLE_BITS));

/// `|x|` at and beyond which [`norm_pdf`] returns zero without squaring: `n(16)` is far below
/// half an ulp, and `x²` stays below `2^8`.
const PDF_LIMIT: u128 = 16 << FRAC_BITS;

/// `N(x)`, the standard normal CDF, rounded to nearest within [`NORM_CDF_MAX_ERROR_ULPS`].
///
/// Exactly [`Fixed::ZERO`] for `x <= −9.25` and [`Fixed::ONE`] for `x >= 9.25`
/// ([`NORM_CDF_CUTOFF`]); `N(x) + N(−x) = 1` exactly.
///
/// # Errors
///
/// None in practice: the `Result` carries the checked arithmetic, whose failure would be a
/// broken internal invariant (`Overflow`).
#[must_use = "the result carries a possible error"]
pub fn norm_cdf(x: Fixed) -> Result<Fixed, PricingError> {
    let a = x.raw().unsigned_abs();
    let tail = if a >= NORM_CDF_CUTOFF.raw().unsigned_abs() {
        Fixed::ZERO
    } else {
        lower_tail(a)?
    };
    if x.raw() < 0 {
        Ok(tail)
    } else {
        Fixed::ONE.checked_sub(tail)
    }
}

/// `n(x)`, the standard normal PDF, rounded to nearest within [`NORM_PDF_MAX_ERROR_ULPS`].
///
/// [`Fixed::ZERO`] once the result is below half an ulp (`|x|` above about 9.4).
///
/// # Errors
///
/// None in practice, as for [`norm_cdf`].
#[must_use = "the result carries a possible error"]
pub fn norm_pdf(x: Fixed) -> Result<Fixed, PricingError> {
    let a = x.raw().unsigned_abs();
    if a >= PDF_LIMIT {
        return Ok(Fixed::ZERO);
    }
    let Some((v, k)) = exp_parts(neg_half_square(a)?)? else {
        return Ok(Fixed::ZERO);
    };
    scale(v, INV_SQRT_2PI_Q128, k)
}

/// `N(−a)` for `0 <= a < 9.25` (raw Q64.64), rounded to nearest.
fn lower_tail(a: u128) -> Result<Fixed, PricingError> {
    let Some((v, k)) = exp_parts(neg_half_square(a)?)? else {
        return Ok(Fixed::ZERO);
    };
    // Segment i = floor(4a); t = a − (i + 1/2)/4 in Q.66, |t| <= 1/8, so |t| × 2^66 <= 2^63.
    let shift = FRAC_BITS.saturating_sub(NORM_TABLE_BITS);
    let i = usize::try_from(a >> shift).map_err(|_| PricingError::Overflow)?;
    let coef = NORM_COEF.get(i).ok_or(PricingError::Overflow)?;
    let a66 = a
        .checked_shl(NORM_TABLE_BITS)
        .ok_or(PricingError::Overflow)?;
    let centre66 = u128::try_from(i)
        .ok()
        .and_then(|i| i.checked_mul(2))
        .and_then(|j| j.checked_add(1))
        .and_then(|j| j.checked_shl(63))
        .ok_or(PricingError::Overflow)?;
    let neg = a66 < centre66;
    let at = if neg {
        centre66.checked_sub(a66)
    } else {
        a66.checked_sub(centre66)
    }
    .and_then(|d| u64::try_from(d).ok())
    .ok_or(PricingError::Overflow)?;
    // P(t) = b0 − t (b1 − t (b2 − ...)) with every partial value positive (the magnitudes
    // fall by at least a factor 8 times |t| per step). Q.66 × Q0.64 / 2^64 = Q.66. The last
    // step keeps 128 bits: b0 is stored at 2^128 and t × p1 is not rounded.
    let (last, rest) = coef.split_last().ok_or(PricingError::Overflow)?;
    let mut p = *last;
    for &b in rest.iter().rev() {
        let tp = round_shr_u64(mul_hi(at, p)?, 2);
        p = if neg {
            b.checked_add(tp)
        } else {
            b.checked_sub(tp)
        }
        .ok_or(PricingError::Overflow)?;
    }
    // t × p1: Q.66 × Q0.64 = Q.130, to Q0.128.
    let tp = mul_wide(at, p)? >> 2;
    let b0 = *NORM_B0.get(i).ok_or(PricingError::Overflow)?;
    let p = if neg {
        b0.checked_add(tp)
    } else {
        b0.checked_sub(tp)
    }
    .ok_or(PricingError::Overflow)?;
    scale(v, p, k)
}

/// `−a² / 2` as a raw Q64.64 value for `a < 16` (raw): `a = ai + af / 2^64` with `ai < 16`,
/// so `a² / 2 × 2^64 = ai² × 2^63 + ai × af + af² / 2^65`, the last term rounded to nearest
/// (error at most half a unit, a relative `2^-65` on `e^(−a²/2)`).
fn neg_half_square(a: u128) -> Result<i128, PricingError> {
    let ai = u64::try_from(a >> FRAC_BITS).map_err(|_| PricingError::Overflow)?;
    let af = lo64(a);
    let (sq_hi, _) = mul_full(af, af)?;
    let whole = u128::from(ai.checked_mul(ai).ok_or(PricingError::Overflow)?) << 63;
    let cross = mul_wide(ai, af)?;
    let s = whole
        .checked_add(cross)
        .and_then(|v| v.checked_add(u128::from(round_shr_u64(sq_hi, 1))))
        .and_then(|v| i128::try_from(v).ok())
        .ok_or(PricingError::Overflow)?;
    s.checked_neg().ok_or(PricingError::Overflow)
}

/// `v / 2^63 × f / 2^128 × 2^k` as Q64.64, rounded once, for the `exp` mantissa `v` (Q1.63),
/// a Q0.128 factor `f` and `k <= 0`.
fn scale(v: u64, f: u128, k: i64) -> Result<Fixed, PricingError> {
    // w = v × f / 2^64 in Q1.127 units; the dropped part is below one unit.
    let w = mul_wide(v, hi64(f))?
        .checked_add(u128::from(mul_hi(v, lo64(f))?))
        .ok_or(PricingError::Overflow)?;
    // raw = w × 2^k × 2^64 / 2^127 = w / 2^(63 − k).
    let s = 63i64
        .checked_sub(k)
        .and_then(|s| u32::try_from(s).ok())
        .ok_or(PricingError::Overflow)?;
    i128::try_from(round_shr_u128(w, s))
        .map(Fixed::from_raw)
        .map_err(|_| PricingError::Overflow)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::disallowed_macros)]
mod tests {
    use super::{norm_cdf, norm_pdf, NORM_CDF_CUTOFF};
    use crate::{Fixed, Rounding};

    #[test]
    fn test_saturation_beyond_cutoff() {
        let cut = NORM_CDF_CUTOFF;
        assert_eq!(cut, Fixed::from_ratio(37, 4, Rounding::Down).unwrap());
        assert_eq!(norm_cdf(cut.checked_neg().unwrap()), Ok(Fixed::ZERO));
        assert_eq!(norm_cdf(cut), Ok(Fixed::ONE));
        assert_eq!(norm_cdf(Fixed::MIN), Ok(Fixed::ZERO));
        assert_eq!(norm_cdf(Fixed::MAX), Ok(Fixed::ONE));
        assert_eq!(norm_pdf(Fixed::MIN), Ok(Fixed::ZERO));
        assert_eq!(norm_pdf(Fixed::MAX), Ok(Fixed::ZERO));
        assert_eq!(norm_pdf(Fixed::from_int(10)), Ok(Fixed::ZERO));
    }

    #[test]
    fn test_symmetry() {
        for raw in [1i128, 1 << 60, 3 << 63, 7 << 64, (37 << 62) - 1] {
            let (x, nx) = (Fixed::from_raw(raw), Fixed::from_raw(-raw));
            let sum = norm_cdf(x)
                .unwrap()
                .checked_add(norm_cdf(nx).unwrap())
                .unwrap();
            assert_eq!(sum, Fixed::ONE);
            assert_eq!(norm_pdf(x), norm_pdf(nx));
        }
    }

    #[test]
    fn test_centre_values() {
        // N(0) = 1/2, n(0) = 1/√(2π) = 0.398942280401432677939946...
        let half = Fixed::from_raw(1 << 63);
        let n0 = norm_cdf(Fixed::ZERO).unwrap().raw();
        assert!((n0 - half.raw()).abs() <= 2, "N(0) = {n0}");
        let p0 = norm_pdf(Fixed::ZERO).unwrap().raw();
        assert!((p0 - 7_359_186_146_747_302_452).abs() <= 2, "n(0) = {p0}");
    }
}
