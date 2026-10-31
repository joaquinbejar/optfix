//! Unsigned 256-bit intermediates for `Fixed` products, quotients and square roots.
//!
//! Only the operations `Fixed` needs: a full `u128 × u128` product, division of a 256-bit
//! value by a `u128` whose quotient fits in a `u128`, and an integer square root. The value is
//! four `u64` limbs: milestone 2 measured this layout against two `u128` limbs on SBF and kept
//! it (`docs/PLAN.md`, decision log). Division runs on 32-bit digits held in `u64` (milestone
//! 10), so no step lowers to a `u128` divide; results are bit-identical to 0.1.0's kernel,
//! which the tests keep as their reference (`u256/v01.rs`, `u256/frozen.rs`). Every limb
//! operation is checked.

use crate::PricingError;

#[cfg(test)]
mod frozen;
#[cfg(test)]
mod v01;

/// Mask of the low 64 bits of a `u128`.
const LOW_64: u128 = u64::MAX as u128;

/// Upper bound on Newton iterations in [`U256::isqrt`]. From its seed (within `2^-30` above
/// the root) the integer Newton step takes at most four divisions (tested); the cap only turns
/// a broken invariant into an error instead of a loop.
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
    /// Knuth's algorithm D (TAOCP vol. 2, 4.3.1) on 32-bit digits held in `u64`, so every
    /// estimate, product and remainder is a native 64-bit operation: on SBF a `u64` divide is
    /// one instruction, a `u128` divide a `__udivti3` call of about 580 CU (milestone 3). A
    /// one-digit divisor takes short division; otherwise the divisor is normalized (top bit of
    /// its top digit set) and each of the four quotient digits is estimated from the top two
    /// remainder digits, refined with the second divisor digit (step D3) and corrected by at
    /// most one add-back (step D6).
    pub(crate) fn div_rem(self, d: u128) -> Result<(u128, u128), PricingError> {
        if d == 0 {
            return Err(PricingError::DomainError);
        }
        // The quotient fits in 128 bits iff self < d × 2^128, that is (w3:w2) < d.
        if join(self.w3, self.w2) >= d {
            return Err(PricingError::Overflow);
        }
        let n = digit_count(d);
        if n == 1 {
            return self.div_rem_digit(lo64(d));
        }
        let top = digit(&split32(d), n.checked_sub(1).ok_or(PricingError::Overflow)?)?;
        // `top` is a nonzero 32-bit digit in a `u64`: 32 to 63 leading zeros, so `s <= 31`
        // and the shifted divisor still has `n` digits.
        let s = top.leading_zeros().saturating_sub(32);
        let vn = split32(d.checked_shl(s).ok_or(PricingError::Overflow)?);
        let mut u = self.shifted_digits(s)?;
        // One instance per divisor length, so the digit loops unroll and the index checks
        // fold away.
        let q = match n {
            2 => quotient::<2>(&mut u, &vn)?,
            3 => quotient::<3>(&mut u, &vn)?,
            _ => quotient::<4>(&mut u, &vn)?,
        };
        // The remainder is in the low `n` digits; the digits above them are zero.
        let low = u.get(..4).ok_or(PricingError::Overflow)?;
        let r = join32(low)? >> s;
        Ok((join32(&q)?, r))
    }

    /// Division by a one-digit divisor `v < 2^32`: four native 64-bit divisions.
    fn div_rem_digit(self, v: u64) -> Result<(u128, u128), PricingError> {
        // (w3:w2) < v < 2^32, so the running remainder starts below 2^32.
        let mut r = self.w2;
        let mut q = [0u64; 4];
        let low = [self.w1 >> 32, lo32(self.w1), self.w0 >> 32, lo32(self.w0)];
        for (k, &next) in low.iter().enumerate() {
            // r < v <= 2^32 − 1, so r × 2^32 + next < 2^64.
            let num = (r << 32) | next;
            let qk = num.checked_div(v).ok_or(PricingError::DomainError)?;
            r = num.checked_rem(v).ok_or(PricingError::DomainError)?;
            let j = 3usize.checked_sub(k).ok_or(PricingError::Overflow)?;
            *q.get_mut(j).ok_or(PricingError::Overflow)? = qk;
        }
        Ok((join32(&q)?, u128::from(r)))
    }

    /// The nine 32-bit digits of `self × 2^s`, `s < 32`, least significant first.
    fn shifted_digits(self, s: u32) -> Result<[u64; 9], PricingError> {
        let words = [self.w0, self.w1, self.w2, self.w3];
        let mut out = [0u64; 9];
        let mut carry = 0u64;
        for (i, &w) in words.iter().enumerate() {
            // `w << s` keeps the low 64 bits; the bits shifted out carry into the next word.
            let shifted = (w << s) | carry;
            carry = if s == 0 {
                0
            } else {
                w >> 64u32.saturating_sub(s)
            };
            let k = i.checked_mul(2).ok_or(PricingError::Overflow)?;
            *out.get_mut(k).ok_or(PricingError::Overflow)? = lo32(shifted);
            *out.get_mut(k.checked_add(1).ok_or(PricingError::Overflow)?)
                .ok_or(PricingError::Overflow)? = shifted >> 32;
        }
        *out.get_mut(8).ok_or(PricingError::Overflow)? = carry;
        Ok(out)
    }

    /// The top 64 bits of `self` and the even shift `2e` with `self >> 2e` equal to them,
    /// for a nonzero `self`.
    fn top_64(self) -> Result<(u64, u32), PricingError> {
        let bits = 256u32
            .checked_sub(self.leading_zeros())
            .ok_or(PricingError::Overflow)?;
        // An even shift keeps `sqrt(self >> 2e) × 2^e` a bound on `sqrt(self)`; `bits − 64`
        // rounded up to even leaves 63 or 64 significant bits.
        let shift = bits.saturating_sub(64).div_ceil(2).saturating_mul(2);
        let (t, _) = self.shr(shift)?;
        Ok((u64::try_from(t).map_err(|_| PricingError::Overflow)?, shift))
    }

    /// `floor(sqrt(self))`; `Overflow` if `self >= 2^254`.
    ///
    /// Integer Newton iteration from a seed at or above the root: the root of the top 64
    /// bits (native `u64` Newton) plus one, scaled back. Write `self = T × 2^(2e) + low` with
    /// `T` the top 64 bits; then `sqrt(self) < sqrt(T + 1) × 2^e <= (isqrt(T) + 1) × 2^e`, the
    /// seed, whose relative excess is below `2^-30`. From any seed at or above the root the
    /// integer Newton sequence decreases strictly until it reaches `floor(sqrt(self))`, where
    /// the next step stops decreasing; quadratic convergence takes this seed there in two or
    /// three steps, and one more confirms it.
    pub(crate) fn isqrt(self) -> Result<u128, PricingError> {
        if self == Self::default() {
            return Ok(0);
        }
        if self.leading_zeros() < 2 {
            return Err(PricingError::Overflow);
        }
        let (t, shift) = self.top_64()?;
        let seed = isqrt_u64(t).checked_add(1).ok_or(PricingError::Overflow)?;
        let mut x = u128::from(seed)
            .checked_shl(shift.checked_div(2).ok_or(PricingError::Overflow)?)
            .ok_or(PricingError::Overflow)?;
        for _ in 0..ISQRT_MAX_ITERATIONS {
            note_isqrt_step();
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

/// Largest 32-bit digit, held in a `u64`.
const DIGIT: u64 = 0xffff_ffff;

/// The low 32 bits of `x`. A truncating cast, not `x & DIGIT`: sBPF immediates are
/// sign-extended, so the mask would compile to two shifts.
#[inline(always)]
fn lo32(x: u64) -> u64 {
    // Truncation intended: keeps the low 32 bits.
    u64::from(x as u32)
}

/// The four 32-bit digits of `x`, least significant first.
fn split32(x: u128) -> [u64; 4] {
    let (hi, lo) = (hi64(x), lo64(x));
    [lo32(lo), lo >> 32, lo32(hi), hi >> 32]
}

/// The `u128` with the 32-bit digits `d[0..4]`, least significant first.
fn join32(d: &[u64]) -> Result<u128, PricingError> {
    let at = |i: usize| digit(d, i);
    Ok(join((at(3)? << 32) | at(2)?, (at(1)? << 32) | at(0)?))
}

/// Number of significant 32-bit digits of a nonzero `d`, in `1..=4`.
fn digit_count(d: u128) -> usize {
    match 128u32.saturating_sub(d.leading_zeros()) {
        0..=32 => 1,
        33..=64 => 2,
        65..=96 => 3,
        _ => 4,
    }
}

/// Product of two 32-bit digits. The masks are no-ops for digits, but they let the compiler
/// see that the product fits in 64 bits: an overflow-checked `u64` multiply of unbounded
/// operands lowers to a `__multi3` call on SBF (about 50 CU), which made each quotient digit
/// cost about 390 CU in the first version of this kernel.
#[inline(always)]
fn dmul(a: u64, b: u64) -> Result<u64, PricingError> {
    if a > DIGIT || b > DIGIT {
        return Err(PricingError::Overflow);
    }
    lo32(a).checked_mul(lo32(b)).ok_or(PricingError::Overflow)
}

/// `a[i]`, or `Overflow` for an index out of range (a broken invariant).
#[inline(always)]
fn digit(a: &[u64], i: usize) -> Result<u64, PricingError> {
    a.get(i).copied().ok_or(PricingError::Overflow)
}

/// `a[i] = v`, or `Overflow` for an index out of range.
#[inline(always)]
fn set(a: &mut [u64], i: usize, v: u64) -> Result<(), PricingError> {
    *a.get_mut(i).ok_or(PricingError::Overflow)? = v;
    Ok(())
}

/// The four quotient digits of algorithm D for an `N`-digit normalized divisor.
#[inline(always)]
fn quotient<const N: usize>(u: &mut [u64; 9], vn: &[u64; 4]) -> Result<[u64; 4], PricingError> {
    // Most significant digit first: each step consumes the remainder of the one above.
    let q3 = step::<N>(u, vn, 3)?;
    let q2 = step::<N>(u, vn, 2)?;
    let q1 = step::<N>(u, vn, 1)?;
    let q0 = step::<N>(u, vn, 0)?;
    Ok([q0, q1, q2, q3])
}

/// One quotient digit of algorithm D: replaces `u[j..=j + n]` by its remainder modulo the
/// normalized `n`-digit divisor `vn` and returns the digit. Requires `u[j..=j + n] < vn × b`
/// (`b = 2^32`), which the overflow check and the previous steps establish.
#[inline(always)]
fn step<const N: usize>(u: &mut [u64; 9], vn: &[u64; 4], j: usize) -> Result<u64, PricingError> {
    let n = N;
    let idx = |k: usize| j.checked_add(k).ok_or(PricingError::Overflow);
    let n1 = n.checked_sub(1).ok_or(PricingError::Overflow)?;
    let n2 = n.checked_sub(2).ok_or(PricingError::Overflow)?;
    let (v1, v2) = (digit(vn, n1)?, digit(vn, n2)?);
    let jn = idx(n)?;
    // D3: estimate from the top two remainder digits; u[j + n] <= v1 < 2^32.
    let num = (digit(u, jn)? << 32) | digit(u, idx(n1)?)?;
    let mut qhat = num.checked_div(v1).ok_or(PricingError::Overflow)?;
    let mut rhat = num.checked_rem(v1).ok_or(PricingError::Overflow)?;
    let u2 = digit(u, idx(n2)?)?;
    for _ in 0..2 {
        // Short-circuit: the product is formed only when qhat and rhat are digits, so it
        // stays below 2^64.
        let too_big = qhat > DIGIT || dmul(qhat, v2)? > ((rhat << 32) | u2);
        if !too_big {
            break;
        }
        note(&REFINE);
        qhat = qhat.checked_sub(1).ok_or(PricingError::Overflow)?;
        rhat = rhat.checked_add(v1).ok_or(PricingError::Overflow)?;
        if rhat > DIGIT {
            break;
        }
    }
    // A zero digit leaves the remainder unchanged (common for the top digits of a quotient
    // below 2^96).
    if qhat == 0 {
        return Ok(0);
    }
    // D4: u[j..=j + n] −= qhat × vn. Each product plus carry is below 2^64; a negative
    // difference of digits wraps to a value with the top bit set.
    let (mut carry, mut borrow) = (0u64, 0u64);
    for i in 0..n {
        let p = dmul(qhat, digit(vn, i)?)?
            .checked_add(carry)
            .ok_or(PricingError::Overflow)?;
        carry = p >> 32;
        let k = idx(i)?;
        let t = digit(u, k)?.wrapping_sub(lo32(p)).wrapping_sub(borrow);
        set(u, k, lo32(t))?;
        borrow = t >> 63;
    }
    let t = digit(u, jn)?.wrapping_sub(carry).wrapping_sub(borrow);
    set(u, jn, lo32(t))?;
    if t >> 63 == 0 {
        return Ok(qhat);
    }
    // D6: qhat was one too large (probability about 2/b): add the divisor back.
    note(&ADD_BACK);
    qhat = qhat.checked_sub(1).ok_or(PricingError::Overflow)?;
    let mut c = 0u64;
    for i in 0..n {
        let k = idx(i)?;
        let s = digit(u, k)?
            .checked_add(digit(vn, i)?)
            .and_then(|s| s.checked_add(c))
            .ok_or(PricingError::Overflow)?;
        set(u, k, lo32(s))?;
        c = s >> 32;
    }
    // The carry out of the top digit cancels the borrow of D4.
    let top = lo32(digit(u, jn)?.wrapping_add(c));
    set(u, jn, top)?;
    Ok(qhat)
}

/// `floor(sqrt(t))` for a `u64`, by integer Newton from a power of two at or above the root.
/// The loop cap is never reached; were it, `x` would still be at or above the root, so the
/// seed built from it stays an upper bound.
fn isqrt_u64(t: u64) -> u64 {
    if t == 0 {
        return 0;
    }
    let bits = 64u32.saturating_sub(t.leading_zeros());
    // 2^ceil(bits/2) >= sqrt(t); at most 2^32, which fits.
    let mut x = 1u64 << bits.div_ceil(2);
    // From 2^32 at most, the sequence reaches the root in well under 64 steps.
    for _ in 0..64 {
        // floor((x + t/x) / 2) without forming x + t/x, which can exceed 2^64.
        let q = t.checked_div(x).unwrap_or(0);
        let y = (x >> 1).saturating_add(q >> 1).saturating_add(x & q & 1);
        if y >= x {
            return x;
        }
        x = y;
    }
    x
}

/// Path counters for the tests: algorithm D's refinement (D3) and add-back (D6) branches.
#[cfg(test)]
static REFINE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(test)]
static ADD_BACK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
// Newton steps (divisions) taken by [`U256::isqrt`] on this thread, for the tests; per thread,
// so tests running in parallel do not disturb one another's count.
#[cfg(test)]
std::thread_local! {
    pub(crate) static ISQRT_STEPS: core::cell::Cell<u64> = const { core::cell::Cell::new(0) };
}

#[cfg(test)]
fn note_isqrt_step() {
    ISQRT_STEPS.with(|c| c.set(c.get().saturating_add(1)));
}

#[cfg(not(test))]
#[inline(always)]
fn note_isqrt_step() {}

#[cfg(test)]
fn note(c: &core::sync::atomic::AtomicU64) {
    c.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

#[cfg(not(test))]
#[inline(always)]
fn note(_: &()) {}
#[cfg(not(test))]
const REFINE: () = ();
#[cfg(not(test))]
const ADD_BACK: () = ();

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::disallowed_macros
)]
mod tests {
    use super::U256;
    use crate::PricingError;

    impl U256 {
        /// Widens a `u128`.
        fn from_u128(x: u128) -> Self {
            Self::from_parts(0, x)
        }

        /// `hi × 2^128 + lo`.
        fn from_parts(hi: u128, lo: u128) -> Self {
            Self {
                w0: lo as u64,
                w1: (lo >> 64) as u64,
                w2: hi as u64,
                w3: (hi >> 64) as u64,
            }
        }
    }

    /// `n + r` for a small `r`, through (hi, lo) halves.
    fn plus(n: U256, r: u128) -> U256 {
        let lo = (u128::from(n.w1) << 64) | u128::from(n.w0);
        let hi = (u128::from(n.w3) << 64) | u128::from(n.w2);
        let (lo, carry) = lo.overflowing_add(r);
        U256::from_parts(hi + u128::from(carry), lo)
    }

    #[test]
    fn test_mul_wide_small_values_match_u128() {
        for (a, b) in [
            (0u128, 0u128),
            (1, 1),
            (3, 7),
            (u64::MAX as u128, u64::MAX as u128),
        ] {
            assert_eq!(U256::mul_wide(a, b).unwrap(), U256::from_u128(a * b));
        }
    }

    #[test]
    fn test_mul_wide_max_times_max_is_exact() {
        // (2^128 − 1)^2 = 2^256 − 2^129 + 1
        let p = U256::mul_wide(u128::MAX, u128::MAX).unwrap();
        assert_eq!(p, U256::from_parts(u128::MAX - 1, 1));
    }

    #[test]
    fn test_div_rem_inverts_mul_wide() {
        let cases = [
            (u128::MAX, u128::MAX, 0u128),
            (u128::MAX, u128::MAX - 1, u128::MAX - 2),
            (1u128 << 100, (1u128 << 127) + 12_345, 999),
            (10u128.pow(18), 3, 1),
            (u128::MAX, 1u128 << 64, (1u128 << 64) - 1),
            (12_345, (1u128 << 64) + 1, 1u128 << 64),
        ];
        for (a, b, r) in cases {
            let n = plus(U256::mul_wide(a, b).unwrap(), r);
            assert_eq!(n.div_rem(b).unwrap(), (a, r), "a={a} b={b} r={r}");
        }
    }

    #[test]
    fn test_div_rem_small_values_match_u128() {
        for (n, d) in [
            (0u128, 1u128),
            (17, 5),
            (u128::MAX, 1),
            (u128::MAX, 10u128.pow(18)),
            (u128::MAX, u128::MAX),
            (u128::MAX - 1, u128::MAX),
        ] {
            assert_eq!(U256::from_u128(n).div_rem(d).unwrap(), (n / d, n % d));
        }
    }

    #[test]
    fn test_div_rem_zero_divisor_is_domain_error() {
        assert_eq!(
            U256::from_u128(1).div_rem(0),
            Err(PricingError::DomainError)
        );
    }

    #[test]
    fn test_div_rem_quotient_too_large_is_overflow() {
        // One-digit divisor: self = 5 × 2^128.
        let n = U256::from_parts(5, 0);
        assert_eq!(n.div_rem(5), Err(PricingError::Overflow));
        assert!(n.div_rem(6).is_ok());
        // Two-digit divisor.
        let d = (1u128 << 64) + 3;
        assert_eq!(
            U256::from_parts(d, 0).div_rem(d),
            Err(PricingError::Overflow)
        );
        assert!(U256::from_parts(d - 1, u128::MAX).div_rem(d).is_ok());
    }

    #[test]
    fn test_isqrt_perfect_squares_and_neighbours() {
        for s in [0u128, 1, 2, 3, 1 << 40, 10u128.pow(18), (1u128 << 100) + 7] {
            let sq = U256::mul_wide(s, s).unwrap();
            assert_eq!(sq.isqrt().unwrap(), s);
            if s >= 2 {
                let below = U256::mul_wide(s, s - 2).unwrap();
                let below = plus(below, s - 1); // s^2 − 1 = s(s − 2) + (s − 1)
                assert_eq!(below.isqrt().unwrap(), s - 1);
            }
        }
    }

    #[test]
    fn test_isqrt_too_large_is_overflow() {
        assert_eq!(
            U256::from_parts(u128::MAX, 0).isqrt(),
            Err(PricingError::Overflow)
        );
    }
}
