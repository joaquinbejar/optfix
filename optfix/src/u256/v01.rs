//! The 0.1.0 division and square-root kernel (`git show v0.1.0:optfix/src/u256.rs`), kept
//! verbatim in the tests as the differential reference of milestone 10 (`docs/PLAN.md`
//! section 8): the new kernel must return exactly what this one returns, results and errors.
//! Test-only; never compiled into the library.

#![allow(dead_code)]

use crate::PricingError;

/// Mask of the low 64 bits of a `u128`.
const LOW_64: u128 = u64::MAX as u128;

/// Upper bound on Newton iterations in [`U256::isqrt`]. Starting from a power of two above the
/// root, the integer Newton step converges in well under 16 iterations for every input below
/// `2^254`; the cap only turns a broken invariant into an error instead of a loop.
const ISQRT_MAX_ITERATIONS: u32 = 128;

/// Unsigned 256-bit integer as four 64-bit limbs, `w0` least significant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct U256 {
    w0: u64,
    w1: u64,
    w2: u64,
    w3: u64,
}

/// The low 64 bits of a `u128`.
#[inline]
fn lo64(x: u128) -> u64 {
    // Truncation intended: keeps the low 64 bits.
    (x & LOW_64) as u64
}

/// The high 64 bits of a `u128`.
#[inline]
fn hi64(x: u128) -> u64 {
    x.checked_shr(64).map_or(0, |h| h as u64)
}

/// Joins two 64-bit halves into a `u128`.
#[inline]
fn join(hi: u64, lo: u64) -> u128 {
    // `hi < 2^64`, so shifting by 64 loses no bits.
    u128::from(hi).checked_shl(64).unwrap_or(0) | u128::from(lo)
}

/// Full product of two 64-bit limbs, from 32-bit halves in the kernel's carry form (about
/// 26 CU on SBF against about 50 for the `__multi3` call a `u128` multiply lowers to; see
/// `crate::kernel::mul_full`). `(2^64 − 1)^2 < 2^128`, so it never fails.
#[inline]
fn mul64(a: u64, b: u64) -> Result<u128, PricingError> {
    crate::kernel::mul_wide(a, b)
}

/// Sum of 64-bit terms in a `u128`; with at most a few terms it never fails.
#[inline]
fn add(terms: &[u64]) -> Result<u128, PricingError> {
    terms.iter().try_fold(0u128, |acc, &t| {
        acc.checked_add(u128::from(t)).ok_or(PricingError::Overflow)
    })
}

impl U256 {
    /// Exact product `a × b`, schoolbook on 64-bit digits. It always fits in 256 bits; the
    /// `Result` only carries the checked limb arithmetic.
    pub(crate) fn mul_wide(a: u128, b: u128) -> Result<Self, PricingError> {
        let (a1, a0) = (hi64(a), lo64(a));
        let (b1, b0) = (hi64(b), lo64(b));
        let p00 = mul64(a0, b0)?;
        let p01 = mul64(a0, b1)?;
        let p10 = mul64(a1, b0)?;
        let p11 = mul64(a1, b1)?;
        let t1 = add(&[hi64(p00), lo64(p01), lo64(p10)])?;
        let t2 = add(&[hi64(t1), hi64(p01), hi64(p10), lo64(p11)])?;
        let t3 = add(&[hi64(t2), hi64(p11)])?;
        Ok(Self {
            w0: lo64(p00),
            w1: lo64(t1),
            w2: lo64(t2),
            w3: u64::try_from(t3).map_err(|_| PricingError::Overflow)?,
        })
    }

    /// Exact `x × 2^64`.
    pub(crate) fn shl_64(x: u128) -> Self {
        Self {
            w0: 0,
            w1: lo64(x),
            w2: hi64(x),
            w3: 0,
        }
    }

    /// Quotient and remainder of `self / 2^64`: a shift, no division.
    ///
    /// `Overflow` if the quotient does not fit in a `u128`.
    pub(crate) fn split_64(self) -> Result<(u128, u64), PricingError> {
        if self.w3 != 0 {
            return Err(PricingError::Overflow);
        }
        Ok((join(self.w2, self.w1), self.w0))
    }

    /// `floor(self / 2^s)` and whether any dropped bit was set, for any `s`.
    ///
    /// `Overflow` if the quotient does not fit in a `u128`.
    pub(crate) fn shr(self, s: u32) -> Result<(u128, bool), PricingError> {
        let words = [self.w0, self.w1, self.w2, self.w3];
        if s >= 256 {
            return Ok((0, words.iter().any(|&w| w != 0)));
        }
        // s < 256: a word offset in 0..4 and a bit offset in 0..64.
        let offset = usize::try_from(s >> 6).map_err(|_| PricingError::Overflow)?;
        let bit = s & 63;
        let word = |i: usize| -> u64 {
            i.checked_add(offset)
                .and_then(|j| words.get(j))
                .copied()
                .unwrap_or(0)
        };
        // Quotient digit i takes bits from words i + offset and i + offset + 1.
        let digit = |i: usize| -> u64 {
            let low = word(i) >> bit;
            let high = if bit == 0 {
                0
            } else {
                word(i.saturating_add(1)) << 64u32.saturating_sub(bit)
            };
            low | high
        };
        if digit(2) != 0 || digit(3) != 0 {
            return Err(PricingError::Overflow);
        }
        let below = words.iter().take(offset).any(|&w| w != 0);
        let mask = if bit == 0 {
            0
        } else {
            u64::MAX >> 64u32.saturating_sub(bit)
        };
        let inexact = below || word(0) & mask != 0;
        Ok((join(digit(1), digit(0)), inexact))
    }

    /// Number of leading zero bits, in `0..=256`.
    fn leading_zeros(self) -> u32 {
        // Each `leading_zeros` is at most 64, so the sums stay at or below 256.
        if self.w3 != 0 {
            self.w3.leading_zeros()
        } else if self.w2 != 0 {
            64u32.saturating_add(self.w2.leading_zeros())
        } else if self.w1 != 0 {
            128u32.saturating_add(self.w1.leading_zeros())
        } else {
            192u32.saturating_add(self.w0.leading_zeros())
        }
    }

    /// Quotient and remainder of `self / d`.
    ///
    /// `DomainError` if `d == 0`; `Overflow` if the quotient does not fit in a `u128`.
    /// Knuth's algorithm D on 64-bit digits: short division when `d` fits in one digit,
    /// otherwise the two-digit divisor is normalized and each quotient digit is estimated,
    /// refined with the second divisor digit and corrected.
    pub(crate) fn div_rem(self, d: u128) -> Result<(u128, u128), PricingError> {
        if d == 0 {
            return Err(PricingError::DomainError);
        }
        if hi64(d) == 0 {
            self.div_rem_short(lo64(d))
        } else {
            self.div_rem_long(d)
        }
    }

    /// Division by a one-digit divisor.
    fn div_rem_short(self, v: u64) -> Result<(u128, u128), PricingError> {
        // The quotient fits in 128 bits iff self < v × 2^128, that is w3 == 0 and w2 < v.
        if self.w3 != 0 || self.w2 >= v {
            return Err(PricingError::Overflow);
        }
        let v = u128::from(v);
        let (q1, r) = short_step(self.w2, self.w1, v)?;
        let (q0, r) = short_step(r, self.w0, v)?;
        Ok((join(q1, q0), u128::from(r)))
    }

    /// Division by a two-digit divisor (`d >= 2^64`).
    fn div_rem_long(self, d: u128) -> Result<(u128, u128), PricingError> {
        // The quotient fits in 128 bits iff self < d × 2^128, that is (w3:w2) < d.
        if join(self.w3, self.w2) >= d {
            return Err(PricingError::Overflow);
        }
        let s = hi64(d).leading_zeros();
        // `s` counts leading zeros of `d`, so the shift drops no bits.
        let dn = d.checked_shl(s).ok_or(PricingError::Overflow)?;
        let shift = |hi: u64, lo: u64| -> Result<u64, PricingError> {
            if s == 0 {
                return Ok(hi);
            }
            let back = 64u32.checked_sub(s).ok_or(PricingError::Overflow)?;
            Ok(hi.checked_shl(s).ok_or(PricingError::Overflow)?
                | lo.checked_shr(back).ok_or(PricingError::Overflow)?)
        };
        // (w3:w2) < d, so the digit shifted out of w3 is zero and (u3:u2) < dn.
        let u3 = shift(self.w3, self.w2)?;
        let u2 = shift(self.w2, self.w1)?;
        let u1 = shift(self.w1, self.w0)?;
        let u0 = self.w0.checked_shl(s).ok_or(PricingError::Overflow)?;
        let (q1, r) = long_step(join(u3, u2), u1, dn)?;
        let (q0, r) = long_step(r, u0, dn)?;
        let rem = r.checked_shr(s).ok_or(PricingError::Overflow)?;
        Ok((join(q1, q0), rem))
    }

    /// `floor(sqrt(self))`; `Overflow` if `self >= 2^254`.
    ///
    /// Integer Newton iteration from a power of two at or above the root; the sequence
    /// decreases strictly until it reaches `floor(sqrt(self))`.
    pub(crate) fn isqrt(self) -> Result<u128, PricingError> {
        let bits = 256u32
            .checked_sub(self.leading_zeros())
            .ok_or(PricingError::Overflow)?;
        if bits == 0 {
            return Ok(0);
        }
        let half = bits.div_ceil(2);
        if half > 127 {
            return Err(PricingError::Overflow);
        }
        // `self < 2^bits <= 2^(2 × half)`, so `x >= sqrt(self)`.
        let mut x = 1u128.checked_shl(half).ok_or(PricingError::Overflow)?;
        for _ in 0..ISQRT_MAX_ITERATIONS {
            let (q, _) = self.div_rem(x)?;
            let y = x
                .checked_add(q)
                .and_then(|v| v.checked_shr(1))
                .ok_or(PricingError::Overflow)?;
            if y >= x {
                return Ok(x);
            }
            x = y;
        }
        Err(PricingError::Overflow)
    }
}

/// `(r × 2^64 + digit) / v` with `r < v < 2^64`, so the quotient digit fits in 64 bits.
fn short_step(r: u64, digit: u64, v: u128) -> Result<(u64, u64), PricingError> {
    let num = join(r, digit);
    let q = num.checked_div(v).ok_or(PricingError::Overflow)?;
    let rem = q
        .checked_mul(v)
        .and_then(|p| num.checked_sub(p))
        .ok_or(PricingError::Overflow)?;
    Ok((
        u64::try_from(q).map_err(|_| PricingError::Overflow)?,
        u64::try_from(rem).map_err(|_| PricingError::Overflow)?,
    ))
}

/// One digit of algorithm D: `(top × 2^64 + digit) / dn` with `top < dn` and `dn` normalized
/// (top bit set). The estimate from the top divisor digit is refined with the second one
/// (Knuth, TAOCP vol. 2, 4.3.1, step D3), then corrected by at most two add-backs.
fn long_step(top: u128, digit: u64, dn: u128) -> Result<(u64, u128), PricingError> {
    let (vn1, vn0) = (u128::from(hi64(dn)), u128::from(lo64(dn)));
    let mut qhat = top.checked_div(vn1).ok_or(PricingError::Overflow)?;
    let mut rhat = qhat
        .checked_mul(vn1)
        .and_then(|p| top.checked_sub(p))
        .ok_or(PricingError::Overflow)?;
    for _ in 0..2 {
        // Short-circuit: the product is only formed when qhat < 2^64 and rhat < 2^64.
        let too_big = qhat > LOW_64
            || qhat.checked_mul(vn0).ok_or(PricingError::Overflow)? > join(lo64(rhat), digit);
        if !too_big {
            break;
        }
        qhat = qhat.checked_sub(1).ok_or(PricingError::Overflow)?;
        rhat = rhat.checked_add(vn1).ok_or(PricingError::Overflow)?;
        if rhat > LOW_64 {
            break;
        }
    }
    let mut q = qhat.min(LOW_64);
    // Three-digit values as (top digit, low 128 bits).
    let value = (hi64(top), join(lo64(top), digit));
    let mut prod = mul_3(q, vn1, vn0)?;
    for _ in 0..2 {
        if prod > value {
            q = q.checked_sub(1).ok_or(PricingError::Overflow)?;
            prod = sub_3(prod, (0, dn))?;
        }
    }
    let (h, rem) = sub_3(value, prod)?;
    if h != 0 || rem >= dn {
        // Excluded by the correction bound; reported rather than asserted.
        return Err(PricingError::Overflow);
    }
    Ok((u64::try_from(q).map_err(|_| PricingError::Overflow)?, rem))
}

/// `q × (vn1 × 2^64 + vn0)` for `q < 2^64`, as (top digit, low 128 bits).
fn mul_3(q: u128, vn1: u128, vn0: u128) -> Result<(u64, u128), PricingError> {
    let p0 = q.checked_mul(vn0).ok_or(PricingError::Overflow)?;
    let p1 = q.checked_mul(vn1).ok_or(PricingError::Overflow)?;
    let t = add(&[hi64(p0), lo64(p1)])?;
    let top = add(&[hi64(t), hi64(p1)])?;
    Ok((
        u64::try_from(top).map_err(|_| PricingError::Overflow)?,
        join(lo64(t), lo64(p0)),
    ))
}

/// `a − b` on (top digit, low 128 bits); `Overflow` if `b > a`.
fn sub_3(a: (u64, u128), b: (u64, u128)) -> Result<(u64, u128), PricingError> {
    let (lo, borrow) = a.1.overflowing_sub(b.1);
    let hi =
        a.0.checked_sub(b.0)
            .and_then(|h| h.checked_sub(u64::from(borrow)))
            .ok_or(PricingError::Overflow)?;
    Ok((hi, lo))
}

impl U256 {
    /// The value with these limbs, `w0` least significant.
    pub(crate) fn from_words(w0: u64, w1: u64, w2: u64, w3: u64) -> Self {
        Self { w0, w1, w2, w3 }
    }
}
