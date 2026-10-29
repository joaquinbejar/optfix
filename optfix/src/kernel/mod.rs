//! Binary fixed-point core of `exp` and `ln`.
//!
//! The core works on 64-bit digits. On SBF a `u64` multiply or divide is one instruction, while
//! a `u128` multiply is a `__multi3` call (about 50 CU) and a `u128` divide a `__udivti3` call
//! (about 580 CU); a 64 × 64 → 128 product from 32-bit halves costs about 26 CU. So the
//! polynomials run on `u64` fractions, and no step divides.
//!
//! Formats are written `Qa.b`: `b` fractional bits. `exp` reduces `x = (64k + j) ln2/64 + r`
//! with `|r| <= ln2/128` and returns `2^k × 2^(j/64) × e^r`; `ln` writes `x = 2^e × m` with
//! `m` in `[1, 2)`, picks the nearest of 129 bucket centres `c_j = 1 + j/128` and returns
//! `e ln2 − ln(RINV_j) + ln(1 + z)` with `RINV_j ≈ 1/c_j` and `z = m × RINV_j − 1`,
//! `|z| <= 2^-8`. Constants come from `tools/gen_tables.py`.

mod tables;

use crate::PricingError;
use tables::{
    EXP_COEF, EXP_TABLE, EXP_TABLE_BITS, LN2_Q120, LN_COEF, LN_RINV, LN_TABLE, LN_TABLE_BITS,
};
pub(crate) use tables::{
    EXP_INV_STEP_Q25, EXP_STEP_HI_Q64, EXP_STEP_LO_Q114, INV_SQRT_2PI_Q128, NORM_B0, NORM_COEF,
    NORM_TABLE_BITS,
};

/// Index into `EXP_COEF` (which starts at `c2`) from which Horner steps use
/// [`mul_hi_approx`]: `p_4` and above.
const EXP_APPROX_FROM: usize = 2;

/// Index into `LN_COEF` (which starts at `c2`) from which Horner steps use
/// [`mul_hi_approx`]: `q_4` and above.
const LN_APPROX_FROM: usize = 2;

/// Mask of the low 32 bits.
const LOW_32: u64 = 0xffff_ffff;

/// `|r|` bound of the reduced `exp` argument in `Q.70`: `ln2/128` plus slack for the
/// estimated reduction index. A larger value means a broken invariant.
const EXP_R70_MAX: u64 = 1 << 63;

/// Full product `a × b` as `(hi, lo)`, from 32-bit halves. Each partial product is below
/// `2^64` and the sums below `2^64` by construction, so the checks never fail.
#[inline]
pub(crate) fn mul_full(a: u64, b: u64) -> Result<(u64, u64), PricingError> {
    let (a1, a0) = (shr64(a, 32)?, a & LOW_32);
    let (b1, b0) = (shr64(b, 32)?, b & LOW_32);
    let p00 = mul64(a0, b0)?;
    let p01 = mul64(a0, b1)?;
    let p10 = mul64(a1, b0)?;
    let p11 = mul64(a1, b1)?;
    // Carry form: mid = p01 + p10 may carry into bit 96, lo = p00 + mid × 2^32 into bit 64.
    let (mid, c1) = p01.overflowing_add(p10);
    let (lo, c2) = p00.overflowing_add(shl64(mid & LOW_32, 32)?);
    let hi = add64(
        add64(add64(p11, shr64(mid, 32)?)?, shl64(u64::from(c1), 32)?)?,
        u64::from(c2),
    )?;
    Ok((hi, lo))
}

/// `floor(a × b / 2^64)`.
#[inline]
pub(crate) fn mul_hi(a: u64, b: u64) -> Result<u64, PricingError> {
    mul_full(a, b).map(|(hi, _)| hi)
}

/// `floor(a / 2^32) × floor(b / 2^32)`: `a × b / 2^64` to within `(a + b) / 2^32` units, with
/// one native multiply. Used for the high-order polynomial terms, whose error is scaled down
/// by a power of the small argument.
#[inline]
pub(crate) fn mul_hi_approx(a: u64, b: u64) -> Result<u64, PricingError> {
    mul64(shr64(a, 32)?, shr64(b, 32)?)
}

/// `a × b` as a `u128`.
#[inline]
pub(crate) fn mul_wide(a: u64, b: u64) -> Result<u128, PricingError> {
    mul_full(a, b).map(|(hi, lo)| join(hi, lo))
}

/// `n × c` for a `u64` factor and a `u128` constant, or `Overflow` if it does not fit.
#[inline]
pub(crate) fn mul_u128(n: u64, c: u128) -> Result<u128, PricingError> {
    let (c1, c0) = (hi64(c), lo64(c));
    let low = mul_wide(n, c0)?;
    let high = u128::from(mul64(n, c1)?);
    high.checked_shl(64)
        .filter(|_| high <= u128::from(u64::MAX))
        .and_then(|h| h.checked_add(low))
        .ok_or(PricingError::Overflow)
}

/// `v / 2^s` rounded to nearest, ties up, for any `s`.
#[inline]
pub(crate) fn round_shr_u64(v: u64, s: u32) -> u64 {
    match s {
        0 => v,
        1..=63 => (v >> s).saturating_add((v >> s.saturating_sub(1)) & 1),
        64 => v >> 63,
        _ => 0,
    }
}

/// `v / 2^s` rounded to nearest, ties up, for any `s`.
#[inline]
pub(crate) fn round_shr_u128(v: u128, s: u32) -> u128 {
    match s {
        0 => v,
        1..=127 => (v >> s).saturating_add((v >> s.saturating_sub(1)) & 1),
        128 => v >> 127,
        _ => 0,
    }
}

/// `v / 2^s` rounded to nearest, ties up (toward positive infinity), for `s` in `1..=63`.
#[inline]
pub(crate) fn round_shr_i64(v: i64, s: u32) -> Result<i64, PricingError> {
    let bit = v
        .checked_shr(s.checked_sub(1).ok_or(PricingError::Overflow)?)
        .ok_or(PricingError::Overflow)?
        & 1;
    v.checked_shr(s)
        .and_then(|q| q.checked_add(bit))
        .ok_or(PricingError::Overflow)
}

/// `v / 2^s` rounded to nearest, ties up (toward positive infinity), for `s` in `1..=127`.
#[inline]
pub(crate) fn round_shr_i128(v: i128, s: u32) -> Result<i128, PricingError> {
    let bit = v
        .checked_shr(s.checked_sub(1).ok_or(PricingError::Overflow)?)
        .ok_or(PricingError::Overflow)?
        & 1;
    // Arithmetic shift is the floor; adding the first dropped bit rounds half up.
    v.checked_shr(s)
        .and_then(|q| q.checked_add(bit))
        .ok_or(PricingError::Overflow)
}

/// `e^x` for `x = n ln2/64 + r70 / 2^70`, as `(v, k)` with `e^x ≈ v / 2^63 × 2^k`.
///
/// `v` is in `[0.99, 2) × 2^63` when `|r| <= ln2/128` (plus the reduction slack); its error is
/// below one unit of `2^-63` (the table and the last product are rounded to 64 bits).
#[inline(always)]
pub(crate) fn exp_core(n: i64, r70: i64) -> Result<(u64, i64), PricingError> {
    let k = n
        .checked_shr(EXP_TABLE_BITS)
        .ok_or(PricingError::Overflow)?;
    let mask = (1i64 << EXP_TABLE_BITS).saturating_sub(1);
    let j = usize::try_from(n & mask).map_err(|_| PricingError::Overflow)?;
    let neg = r70 < 0;
    let ar = r70.unsigned_abs();
    if ar >= EXP_R70_MAX {
        return Err(PricingError::Overflow);
    }
    // p2 = c2 + r (c3 + r (c4 + ...)), each p in Q0.64; |r| p < 2^-7, so the sign of r only
    // moves p a little and p stays positive.
    let (last, rest) = EXP_COEF.split_last().ok_or(PricingError::Overflow)?;
    let mut p = *last;
    for (i, &c) in rest.iter().enumerate().rev() {
        // Q.70 × Q0.64 / 2^64 = Q.70; back to Q0.64. An error in p_i reaches e^r scaled by
        // r^i <= 2^(-7.5 i), so p_4 and above need only about 2^-38: one native multiply.
        let prod = if i >= EXP_APPROX_FROM {
            mul_hi_approx(ar, p)?
        } else {
            mul_hi(ar, p)?
        };
        let t = round_shr_u64(prod, 6);
        p = if neg {
            c.checked_sub(t)
        } else {
            c.checked_add(t)
        }
        .ok_or(PricingError::Overflow)?;
    }
    // e^r − 1 = r + r^2 p2, in Q.70.
    let rp = mul_hi(ar, p)?;
    let r2p = round_shr_u64(mul_hi(ar, rp)?, 6);
    let aq = if neg {
        ar.checked_sub(r2p)
    } else {
        ar.checked_add(r2p)
    }
    .ok_or(PricingError::Overflow)?;
    // v = T_j + T_j (e^r − 1): Q1.63 × Q.70 / 2^64 = Q.69, back to Q1.63.
    let t = *EXP_TABLE.get(j).ok_or(PricingError::Overflow)?;
    let tq = round_shr_u64(mul_hi(t, aq)?, 6);
    let v = if neg {
        t.checked_sub(tq)
    } else {
        t.checked_add(tq)
    }
    .ok_or(PricingError::Overflow)?;
    Ok((v, k))
}

/// `raw ≈ m × 2^p` with `m` in `[2^63, 2^64)`, rounded to nearest. `raw` must be nonzero.
pub(crate) fn normalize(raw: u128) -> Result<(i64, u64), PricingError> {
    if raw == 0 {
        return Err(PricingError::DomainError);
    }
    // 128 − leading zeros is the bit length, in 1..=128.
    let bits = 128u32.saturating_sub(raw.leading_zeros());
    if bits <= 64 {
        let shift = 64u32.saturating_sub(bits);
        let m = lo64(raw).checked_shl(shift).ok_or(PricingError::Overflow)?;
        return Ok((i64::from(shift).saturating_neg(), m));
    }
    let p = bits.saturating_sub(64);
    let m = round_shr_u128(raw, p);
    // Rounding up can carry into 2^64: then m = 2^63 × 2.
    if m > u128::from(u64::MAX) {
        return Ok((i64::from(p).saturating_add(1), 1u64 << 63));
    }
    Ok((i64::from(p), lo64(m)))
}

/// `ln(m / 2^63 × 2^e) × 2^120` for `m` in `[2^63, 2^64)`.
pub(crate) fn ln_core(e: i64, m: u64) -> Result<i128, PricingError> {
    // Nearest bucket centre c_j = 1 + j/128, j in 0..=128.
    let frac = m.checked_sub(1 << 63).ok_or(PricingError::DomainError)?;
    let j = usize::try_from(round_shr_u64(frac, 63u32.saturating_sub(LN_TABLE_BITS)))
        .map_err(|_| PricingError::Overflow)?;
    let rinv = *LN_RINV.get(j).ok_or(PricingError::Overflow)?;
    let lnt = *LN_TABLE.get(j).ok_or(PricingError::Overflow)?;
    // z = m × RINV_j − 1: Q1.63 × Q1.63 = Q2.126, |z| <= about 2^-8.
    let prod = mul_wide(m, rinv)?;
    let z = i128::try_from(prod)
        .ok()
        .and_then(|p| p.checked_sub(1i128 << 126))
        .ok_or(PricingError::Overflow)?;
    let z70 = i64::try_from(round_shr_i128(z, 56)?).map_err(|_| PricingError::Overflow)?;
    let neg = z70 < 0;
    let az = z70.unsigned_abs();
    // q2 = c2 − z (c3 − z (c4 − ...)), each q in Q0.64 and positive.
    let (last, rest) = LN_COEF.split_last().ok_or(PricingError::Overflow)?;
    let mut q = *last;
    for (i, &c) in rest.iter().enumerate().rev() {
        // As in `exp_core`: an error in q_i is scaled by z^i <= 2^(-8 i), so q_4 and above
        // take the one-multiply product.
        let prod = if i >= LN_APPROX_FROM {
            mul_hi_approx(az, q)?
        } else {
            mul_hi(az, q)?
        };
        let t = round_shr_u64(prod, 6);
        q = if neg {
            c.checked_add(t)
        } else {
            c.checked_sub(t)
        }
        .ok_or(PricingError::Overflow)?;
    }
    // ln(1 + z) = z − z^2 q2, in Q.70.
    let zq = mul_hi(az, q)?;
    let z2q =
        i64::try_from(round_shr_u64(mul_hi(az, zq)?, 6)).map_err(|_| PricingError::Overflow)?;
    let l70 = z70.checked_sub(z2q).ok_or(PricingError::Overflow)?;
    // e ln2 + (−ln RINV_j) + ln(1 + z), in Q.120.
    let e_ln2 = i128::try_from(mul_u128(e.unsigned_abs(), LN2_Q120)?)
        .map_err(|_| PricingError::Overflow)?;
    let e_ln2 = if e < 0 {
        e_ln2.checked_neg()
    } else {
        Some(e_ln2)
    }
    .ok_or(PricingError::Overflow)?;
    let table = i128::try_from(round_shr_u128(lnt, 8)).map_err(|_| PricingError::Overflow)?;
    // |l70| < 2^63, so the shift cannot overflow an i128.
    let series = i128::from(l70) << 50;
    e_ln2
        .checked_add(table)
        .and_then(|s| s.checked_add(series))
        .ok_or(PricingError::Overflow)
}

/// The low 64 bits of a `u128`.
#[inline]
pub(crate) fn lo64(x: u128) -> u64 {
    // Truncation intended: keeps the low 64 bits.
    (x & u128::from(u64::MAX)) as u64
}

/// The high 64 bits of a `u128`.
#[inline]
pub(crate) fn hi64(x: u128) -> u64 {
    // After the shift the value is below 2^64.
    (x >> 64) as u64
}

/// Joins two 64-bit halves.
#[inline]
pub(crate) fn join(hi: u64, lo: u64) -> u128 {
    (u128::from(hi) << 64) | u128::from(lo)
}

#[inline]
fn mul64(a: u64, b: u64) -> Result<u64, PricingError> {
    a.checked_mul(b).ok_or(PricingError::Overflow)
}

#[inline]
fn add64(a: u64, b: u64) -> Result<u64, PricingError> {
    a.checked_add(b).ok_or(PricingError::Overflow)
}

#[inline]
fn shr64(a: u64, s: u32) -> Result<u64, PricingError> {
    a.checked_shr(s).ok_or(PricingError::Overflow)
}

#[inline]
fn shl64(a: u64, s: u32) -> Result<u64, PricingError> {
    a.checked_shl(s).ok_or(PricingError::Overflow)
}
