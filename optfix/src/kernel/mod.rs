//! Binary fixed-point core of `exp` and `ln`, shared by both representations.
//!
//! The core works on 64-bit digits. On SBF a `u64` multiply or divide is one instruction, while
//! a `u128` multiply is a `__multi3` call (about 50 CU) and a `u128` divide a `__udivti3` call
//! (about 580 CU); a 64 × 64 → 128 product from 32-bit halves costs about 26 CU. So the
//! polynomials run on `u64` fractions, and no step divides.
//!
//! Formats are written `Qa.b`: `b` fractional bits. `exp` reduces `x = (64k + j) ln2/64 + r`
//! with `|r| <= ln2/128` and returns `2^k × 2^(j/64) × e^r`; `ln` writes `x = 2^e × m` with
//! `m` in `[1, 2)`, picks the nearest of 129 bucket centres `c_j = 1 + j/128` and returns
//! `e ln2 − ln(1/c_j) + ln(1 + z)` with `|z| <= 2^-8`. Constants come from
//! `tools/gen_tables.py`.

mod tables;

use crate::PricingError;
pub(crate) use tables::{
    DEC_TO_Q70_Q53, EXP_INV_STEP_DEC, EXP_INV_STEP_Q25, EXP_STEP_HI_DEC, EXP_STEP_HI_Q64,
    EXP_STEP_LO_DEC_Q110, EXP_STEP_LO_Q114, LN_1E18_Q120,
};
use tables::{
    EXP_COEF, EXP_TABLE, EXP_TABLE_BITS, LN2_Q120, LN_COEF, LN_RINV, LN_TABLE, LN_TABLE_BITS,
};
pub(crate) use tables::{
    EXP_COEF_DEC, EXP_STEP_LO_DEC27, EXP_TABLE_DEC, LN2_DEC, LN2_LO_DEC27, LN_COEF_DEC,
    LN_RINV_DEC, LN_TABLE_DEC,
};
pub(crate) use tables::{EXP_TABLE_BITS as EXP_BITS, LN_TABLE_BITS as LN_BITS};

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
/// below 2 units of `2^-63` (measured bound in the crate tests).
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

/// `(hi × 2^64 + lo) / d` for a normalized divisor (`d >= 2^63`) and `hi < d`, so the
/// quotient fits a `u64`. Knuth's algorithm D on 32-bit digits, where every digit operation is
/// a native 64-bit instruction; each quotient digit is estimated from the top divisor digit and
/// corrected at most twice.
pub(crate) fn div_128_64(hi: u64, lo: u64, d: u64) -> Result<u64, PricingError> {
    if d < 1 << 63 || hi >= d {
        return Err(PricingError::Overflow);
    }
    let (d1, d0) = (shr64(d, 32)?, d & LOW_32);
    let (l1, l0) = (shr64(lo, 32)?, lo & LOW_32);
    let q1 = div_digit(hi, l1, d1, d0)?;
    // Remainder after the first digit: (hi : l1) − q1 × d is below d < 2^64, so it is exact
    // modulo 2^64 and the wrapping operations below compute it exactly.
    let u = shl64(hi & LOW_32, 32)? | l1;
    let rem = u.wrapping_sub(q1.wrapping_mul(d));
    let q0 = div_digit(rem, l0, d1, d0)?;
    Ok(shl64(q1, 32)? | q0)
}

/// One 32-bit quotient digit of `(u × 2^32 + next) / d` for `d = d1 × 2^32 + d0`, `u < d`.
/// With a two-digit divisor the refinement test covers the whole divisor, so the corrected
/// estimate is exact and needs no add-back.
fn div_digit(u: u64, next: u64, d1: u64, d0: u64) -> Result<u64, PricingError> {
    let mut q = u.checked_div(d1).ok_or(PricingError::DomainError)?;
    let mut r = u.checked_sub(mul64(q, d1)?).ok_or(PricingError::Overflow)?;
    for _ in 0..2 {
        // q is at most 2 too large (Knuth's theorem B): fix it against the full divisor.
        let too_big = q > LOW_32 || (r <= LOW_32 && mul64(q, d0)? > (shl64(r, 32)? | next));
        if !too_big {
            break;
        }
        q = q.checked_sub(1).ok_or(PricingError::Overflow)?;
        r = r.checked_add(d1).ok_or(PricingError::Overflow)?;
    }
    Ok(q)
}

/// `ln(m / 2^63 × 2^e) × 2^120` through `ln(c_j) + 2 atanh((m − c_j) / (m + c_j))`, the
/// one-division form of `docs/PLAN.md` 4.3, on the same buckets as [`ln_core`].
pub(crate) fn ln_core_atanh(e: i64, m: u64) -> Result<i128, PricingError> {
    use tables::{ATANH_COEF, LN_C_TABLE};
    let frac = m.checked_sub(1 << 63).ok_or(PricingError::DomainError)?;
    let j = round_shr_u64(frac, 63u32.saturating_sub(LN_TABLE_BITS));
    // c_j = 1 + j/128 in Q1.63, up to 2 (j = 128) as a u128.
    let c = u128::from(j)
        .checked_shl(63u32.saturating_sub(LN_TABLE_BITS))
        .and_then(|v| v.checked_add(1 << 63))
        .ok_or(PricingError::Overflow)?;
    let j = usize::try_from(j).map_err(|_| PricingError::Overflow)?;
    let lnc = *LN_C_TABLE.get(j).ok_or(PricingError::Overflow)?;
    // s = (m − c) / (m + c) in Q.72: |s| <= 2^-9. The denominator (m + c) / 2 is in
    // [2^63, 2^64), so it fits and is normalized; s = |m − c| × 2^71 / ((m + c) / 2).
    let m128 = u128::from(m);
    let neg = m128 < c;
    let num = if neg {
        c.checked_sub(m128)
    } else {
        m128.checked_sub(c)
    }
    .and_then(|v| u64::try_from(v).ok())
    .ok_or(PricingError::Overflow)?;
    let den = u64::try_from(m128.checked_add(c).ok_or(PricingError::Overflow)? >> 1)
        .map_err(|_| PricingError::Overflow)?;
    // num × 2^71 as (hi, lo) = (num × 2^7, 0): num < 2^56, so hi < 2^63 <= den.
    let s72 = div_128_64(shl64(num, 7)?, 0, den)?;
    // atanh(s) = s + s^3 (1/3 + s^2 (1/5 + s^2 / 7)), s in Q.72.
    let s2 = mul_hi(s72, s72)?; // Q.80
    let (c3, rest) = ATANH_COEF.split_first().ok_or(PricingError::Overflow)?;
    let (c5, c7) = match rest {
        [a, b] => (*a, *b),
        _ => return Err(PricingError::Overflow),
    };
    // Q.80 × Q0.64 / 2^64 = Q.80; back to Q0.64 by >> 16. The error is scaled by s^5.
    let inner = c5
        .checked_add(round_shr_u64(mul_hi_approx(s2, c7)?, 16))
        .ok_or(PricingError::Overflow)?;
    let poly = c3
        .checked_add(round_shr_u64(mul_hi_approx(s2, inner)?, 16))
        .ok_or(PricingError::Overflow)?;
    // s^3 P: (s × s^2) in Q.88, then × P (Q0.64) in Q.88; to Q.72 by >> 16.
    let s3 = mul_hi(s72, s2)?;
    let s3p = round_shr_u64(mul_hi(s3, poly)?, 16);
    let at = s72.checked_add(s3p).ok_or(PricingError::Overflow)?; // atanh |s|, Q.72
                                                                  // 2 atanh in Q.120: × 2 × 2^48.
    let at120 = i128::from(at) << 49;
    let at120 = if neg {
        at120.checked_neg()
    } else {
        Some(at120)
    }
    .ok_or(PricingError::Overflow)?;
    let e_ln2 = i128::try_from(mul_u128(e.unsigned_abs(), LN2_Q120)?)
        .map_err(|_| PricingError::Overflow)?;
    let e_ln2 = if e < 0 {
        e_ln2.checked_neg()
    } else {
        Some(e_ln2)
    }
    .ok_or(PricingError::Overflow)?;
    let table = i128::try_from(round_shr_u128(lnc, 8)).map_err(|_| PricingError::Overflow)?;
    e_ln2
        .checked_add(table)
        .and_then(|v| v.checked_add(at120))
        .ok_or(PricingError::Overflow)
}
