//! The two candidate representations of milestone 3 (`docs/PLAN.md` 4.1), behind one trait.
//!
//! Both run `exp` and `ln` on the binary core in [`crate::kernel`]; they differ only in how a
//! raw value enters and leaves it. [`Binary`] is Q64.64 (`raw / 2^64`), so conversions are
//! shifts. [`Decimal`] is `raw / 10^18`: `exp` reduces its argument in decimal and converts the
//! small remainder with one multiply, `ln` folds the scale into the constant `ln(10^18)`, and
//! each result leaves with one multiply by `10^18`. Hidden: it exists only so the bench program
//! can measure both until the decision deletes one.

use crate::kernel::{
    exp_core, ln_core, ln_core_atanh, lo64, mul_wide, normalize, round_shr_i128, round_shr_i64,
    round_shr_u128, round_shr_u64, DEC_TO_Q70_Q53, EXP_INV_STEP_DEC, EXP_INV_STEP_Q25,
    EXP_STEP_HI_DEC, EXP_STEP_HI_Q64, EXP_STEP_LO_DEC_Q110, EXP_STEP_LO_Q114, LN_1E18_Q120,
};
use crate::kernel::{
    EXP_BITS, EXP_COEF_DEC, EXP_STEP_LO_DEC27, EXP_TABLE_DEC, LN2_DEC, LN2_LO_DEC27, LN_BITS,
    LN_COEF_DEC, LN_RINV_DEC, LN_TABLE_DEC,
};
use crate::{Fixed, PricingError};

/// A fixed-point representation with its own `exp` and `ln`.
pub trait Repr {
    /// Short name for tables.
    const NAME: &'static str;
    /// The raw value of `1.0`.
    const ONE: i128;
    /// `e^x`, rounded to nearest.
    ///
    /// # Errors
    ///
    /// `Overflow` if the result does not fit. Results below half an ulp return zero.
    fn exp(x: i128) -> Result<i128, PricingError>;
    /// `ln x`, rounded to nearest.
    ///
    /// # Errors
    ///
    /// `DomainError` if `x <= 0`.
    fn ln(x: i128) -> Result<i128, PricingError>;
}

/// Binary Q64.64: value `raw / 2^64`.
#[derive(Debug, Clone, Copy)]
pub struct Binary;

/// Decimal: value `raw / 10^18`.
#[derive(Debug, Clone, Copy)]
pub struct Decimal;

/// Decimal `10^18` computed natively: the same reduction, tables and polynomials as
/// [`Decimal`], but every step is a decimal [`Fixed`] multiply (a 256-bit product divided by
/// `10^18`, rounded down). The baseline that shows what the binary core saves.
#[derive(Debug, Clone, Copy)]
pub struct DecimalNative;

/// Q64.64 with `ln` in the atanh form of `docs/PLAN.md` 4.3 (one division), for the
/// comparison of the two `ln` forms on the binary representation. `exp` is [`Binary`]'s.
#[derive(Debug, Clone, Copy)]
pub struct BinaryAtanh;

impl Repr for BinaryAtanh {
    const NAME: &'static str = "Q64.64, ln atanh form";
    const ONE: i128 = 1 << 64;

    fn exp(x: i128) -> Result<i128, PricingError> {
        Binary::exp(x)
    }

    fn ln(x: i128) -> Result<i128, PricingError> {
        let raw = u128::try_from(x)
            .ok()
            .filter(|&r| r != 0)
            .ok_or(PricingError::DomainError)?;
        let (p, m) = normalize(raw)?;
        let y = ln_core_atanh(p.checked_sub(1).ok_or(PricingError::Overflow)?, m)?;
        round_shr_i128(y, 56)
    }
}

/// `10^18`.
const E18: u64 = 1_000_000_000_000_000_000;

/// `exp` arguments at or above `47` overflow both representations; at or below `−47` the
/// result is below half an ulp of both. As raw Q64.64 values:
const EXP_LIMIT_BIN: i128 = 47 << 64;
/// The same limits as raw decimal values.
const EXP_LIMIT_DEC: i128 = 47 * Decimal::ONE;

impl Repr for Binary {
    const NAME: &'static str = "Q64.64";
    const ONE: i128 = 1 << 64;

    fn exp(x: i128) -> Result<i128, PricingError> {
        if x >= EXP_LIMIT_BIN {
            return Err(PricingError::Overflow);
        }
        if x <= -EXP_LIMIT_BIN {
            return Ok(0);
        }
        // n = round(x × 64 / ln 2), estimated in 64-bit arithmetic: |x| < 2^70, so x / 2^39 <
        // 2^31 and the product with the Q.25 constant is below 2^63.
        let xs = i64::try_from(x >> 39).map_err(|_| PricingError::Overflow)?;
        let n = xs
            .checked_mul(EXP_INV_STEP_Q25)
            .and_then(|p| p.checked_add(1 << 49))
            .map(|p| p >> 50)
            .ok_or(PricingError::Overflow)?;
        // r = x − n (HI + LO) with the step split Cody-Waite style: x − n HI is exact and,
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
        if s >= 0 {
            let s = u32::try_from(s).map_err(|_| PricingError::Overflow)?;
            // v < 2^64, so a shift by at most 64 loses no bits; the conversion checks the top.
            u128::from(v)
                .checked_shl(s)
                .filter(|_| s <= 64)
                .and_then(|r| i128::try_from(r).ok())
                .ok_or(PricingError::Overflow)
        } else {
            let s = u32::try_from(s.unsigned_abs()).map_err(|_| PricingError::Overflow)?;
            Ok(i128::from(round_shr_u64(v, s)))
        }
    }

    fn ln(x: i128) -> Result<i128, PricingError> {
        let raw = u128::try_from(x)
            .ok()
            .filter(|&r| r != 0)
            .ok_or(PricingError::DomainError)?;
        let (p, m) = normalize(raw)?;
        // x = raw / 2^64 = (m / 2^63) × 2^(p − 1).
        let y = ln_core(p.checked_sub(1).ok_or(PricingError::Overflow)?, m)?;
        round_shr_i128(y, 56)
    }
}

impl Repr for Decimal {
    const NAME: &'static str = "decimal 10^18";
    const ONE: i128 = E18 as i128;

    fn exp(x: i128) -> Result<i128, PricingError> {
        if x >= EXP_LIMIT_DEC {
            return Err(PricingError::Overflow);
        }
        if x <= -EXP_LIMIT_DEC {
            return Ok(0);
        }
        // n = round(x × 64 / (ln 2 × 10^18)): |x| < 2^66, so x / 2^30 < 2^36 and the product
        // with the constant (below 2^27) fits an i64.
        let xs = i64::try_from(x >> 30).map_err(|_| PricingError::Overflow)?;
        let n = xs
            .checked_mul(EXP_INV_STEP_DEC)
            .and_then(|p| p.checked_add(1 << 49))
            .map(|p| p >> 50)
            .ok_or(PricingError::Overflow)?;
        // r = x − n (HI + LO) as in `Binary::exp`, with HI in units of 10^-18: x − n HI is
        // exact and below 2^53. It converts to Q.70 with one multiply by 2^70 / 10^18.
        let nhi = signed(mul_wide(n.unsigned_abs(), EXP_STEP_HI_DEC)?, n < 0)?;
        let d = x
            .checked_sub(nhi)
            .and_then(|d| i64::try_from(d).ok())
            .ok_or(PricingError::Overflow)?;
        let dq = signed(
            round_shr_u128(mul_wide(d.unsigned_abs(), DEC_TO_Q70_Q53)?, 53),
            d < 0,
        )
        .and_then(|v| i64::try_from(v).map_err(|_| PricingError::Overflow))?;
        let lo = n
            .checked_mul(EXP_STEP_LO_DEC_Q110)
            .ok_or(PricingError::Overflow)?;
        let r70 = dq
            .checked_sub(round_shr_i64(lo, 40)?)
            .ok_or(PricingError::Overflow)?;
        let (v, k) = exp_core(n, r70)?;
        // Result raw = v / 2^63 × 2^k × 10^18.
        let prod = mul_wide(v, E18)?;
        let s = 63i64.checked_sub(k).ok_or(PricingError::Overflow)?;
        let raw = if s >= 0 {
            round_shr_u128(prod, u32::try_from(s).map_err(|_| PricingError::Overflow)?)
        } else {
            let s = u32::try_from(s.unsigned_abs()).map_err(|_| PricingError::Overflow)?;
            prod.checked_shl(s)
                .filter(|r| r.checked_shr(s) == Some(prod))
                .ok_or(PricingError::Overflow)?
        };
        i128::try_from(raw).map_err(|_| PricingError::Overflow)
    }

    fn ln(x: i128) -> Result<i128, PricingError> {
        let raw = u128::try_from(x)
            .ok()
            .filter(|&r| r != 0)
            .ok_or(PricingError::DomainError)?;
        let (p, m) = normalize(raw)?;
        // ln(x) = ln(raw) − ln(10^18), with raw = (m / 2^63) × 2^(p + 63).
        let y = ln_core(p.checked_add(63).ok_or(PricingError::Overflow)?, m)?
            .checked_sub_unsigned(LN_1E18_Q120)
            .ok_or(PricingError::Overflow)?;
        // To decimal: |y| = I + F / 2^120, result = I × 10^18 + round(F × 10^18 / 2^120).
        let a = y.unsigned_abs();
        let int = u64::try_from(a >> 120).map_err(|_| PricingError::Overflow)?;
        // The top 64 fractional bits; the 56 dropped bits are below 2^-64, 0.06 ulp of 10^-18.
        let frac = lo64((a & ((1u128 << 120) - 1)) >> 56);
        let frac = round_shr_u128(mul_wide(frac, E18)?, 64);
        let mag = mul_wide(int, E18)?
            .checked_add(frac)
            .ok_or(PricingError::Overflow)?;
        signed(mag, y < 0)
    }
}

impl Repr for DecimalNative {
    const NAME: &'static str = "decimal 10^18, native";
    const ONE: i128 = E18 as i128;

    fn exp(x: i128) -> Result<i128, PricingError> {
        if x >= EXP_LIMIT_DEC {
            return Err(PricingError::Overflow);
        }
        if x <= -EXP_LIMIT_DEC {
            return Ok(0);
        }
        // Same reduction index and exact high part as `Decimal::exp`.
        let xs = i64::try_from(x >> 30).map_err(|_| PricingError::Overflow)?;
        let n = xs
            .checked_mul(EXP_INV_STEP_DEC)
            .and_then(|p| p.checked_add(1 << 49))
            .map(|p| p >> 50)
            .ok_or(PricingError::Overflow)?;
        let nhi = signed(mul_wide(n.unsigned_abs(), EXP_STEP_HI_DEC)?, n < 0)?;
        let d = x.checked_sub(nhi).ok_or(PricingError::Overflow)?;
        // r = d − n LO, with n LO (below 2^42 units of 10^-27) rounded to 10^-18.
        let lo = n
            .checked_mul(EXP_STEP_LO_DEC27)
            .and_then(|v| v.checked_add(500_000_000))
            .and_then(|v| v.checked_div_euclid(1_000_000_000))
            .ok_or(PricingError::Overflow)?;
        let r = Fixed::from_raw(
            d.checked_sub(i128::from(lo))
                .ok_or(PricingError::Overflow)?,
        );
        let k = n.checked_shr(EXP_BITS).ok_or(PricingError::Overflow)?;
        let j = usize::try_from(n & ((1 << EXP_BITS) - 1)).map_err(|_| PricingError::Overflow)?;
        // p = c2 + r (c3 + ...), q = r + r^2 p, v = T_j (1 + q).
        let (last, rest) = EXP_COEF_DEC.split_last().ok_or(PricingError::Overflow)?;
        let mut p = Fixed::from_raw(*last);
        for &c in rest.iter().rev() {
            p = Fixed::from_raw(c).checked_add(r.mul_down(p)?)?;
        }
        let q = r.checked_add(r.mul_down(r)?.mul_down(p)?)?;
        let t = Fixed::from_raw(*EXP_TABLE_DEC.get(j).ok_or(PricingError::Overflow)?);
        let v = t.checked_add(t.mul_down(q)?)?.raw();
        // × 2^k.
        if k >= 0 {
            let s = u32::try_from(k).map_err(|_| PricingError::Overflow)?;
            v.checked_shl(s)
                .filter(|r| r.checked_shr(s) == Some(v))
                .ok_or(PricingError::Overflow)
        } else {
            let s = u32::try_from(k.unsigned_abs()).map_err(|_| PricingError::Overflow)?;
            if s >= 127 {
                return Ok(0);
            }
            round_shr_i128(v, s)
        }
    }

    fn ln(x: i128) -> Result<i128, PricingError> {
        let raw = u128::try_from(x)
            .ok()
            .filter(|&r| r != 0)
            .ok_or(PricingError::DomainError)?;
        // m = x / 2^k in [1, 2): 10^18 is in [2^59, 2^60), so start from k = bits − 60 and fix
        // up by one.
        let bits = 128u32.saturating_sub(raw.leading_zeros());
        let mut k = i64::from(bits).saturating_sub(60);
        let mut m = if k >= 0 {
            round_shr_u128(raw, u32::try_from(k).map_err(|_| PricingError::Overflow)?)
        } else {
            raw.checked_shl(u32::try_from(k.unsigned_abs()).map_err(|_| PricingError::Overflow)?)
                .ok_or(PricingError::Overflow)?
        };
        let one = u128::from(E18);
        if m < one {
            m = m.checked_mul(2).ok_or(PricingError::Overflow)?;
            k = k.saturating_sub(1);
        } else if m >= 2 * one {
            m = round_shr_u128(m, 1);
            k = k.saturating_add(1);
        }
        let m = i128::try_from(m).map_err(|_| PricingError::Overflow)?;
        // Bucket j = round((m − 1) × 128); 10^18 / 128 is an integer.
        let width = i128::from(E18 >> LN_BITS);
        let j = m
            .checked_sub(Self::ONE)
            .and_then(|f| f.checked_add(width >> 1))
            .and_then(|f| f.checked_div(width))
            .and_then(|j| usize::try_from(j).ok())
            .ok_or(PricingError::Overflow)?;
        let rinv = Fixed::from_raw(*LN_RINV_DEC.get(j).ok_or(PricingError::Overflow)?);
        let z = Fixed::from_raw(m).mul_down(rinv)?.checked_sub(Fixed::ONE)?;
        // q = c2 − z (c3 − ...), ln(1 + z) = z − z^2 q.
        let (last, rest) = LN_COEF_DEC.split_last().ok_or(PricingError::Overflow)?;
        let mut q = Fixed::from_raw(*last);
        for &c in rest.iter().rev() {
            q = Fixed::from_raw(c).checked_sub(z.mul_down(q)?)?;
        }
        let l = z.checked_sub(z.mul_down(z)?.mul_down(q)?)?;
        // k ln 2 with the constant split in two, as in the exp reduction: k × HI is exact and
        // k × LO (below 2^37 units of 10^-27) is rounded to 10^-18.
        let k_lo = k
            .checked_mul(LN2_LO_DEC27)
            .and_then(|v| v.checked_add(500_000_000))
            .and_then(|v| v.checked_div_euclid(1_000_000_000))
            .ok_or(PricingError::Overflow)?;
        let k_ln2 = i128::from(k)
            .checked_mul(LN2_DEC)
            .and_then(|v| v.checked_add(i128::from(k_lo)))
            .ok_or(PricingError::Overflow)?;
        let lnt = *LN_TABLE_DEC.get(j).ok_or(PricingError::Overflow)?;
        k_ln2
            .checked_add(lnt)
            .and_then(|s| s.checked_add(l.raw()))
            .ok_or(PricingError::Overflow)
    }
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
