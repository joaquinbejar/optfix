//! Benchmark-only copy of the milestone 1 kernel (two `u128` limbs), for the milestone 2
//! layout comparison. Deleted once the comparison is recorded.
//!
//! Unsigned 256-bit intermediates for `Fixed` products, quotients and square roots.
//!
//! Only the operations `Fixed` needs: a full `u128 × u128` product, division of a 256-bit
//! value by a `u128` whose quotient fits in a `u128`, and an integer square root. The limbs
//! are two `u128`; milestone 2 benchmarks this layout against four `u64` limbs on SBF.

use crate::PricingError;

/// Mask of the low 64 bits of a `u128`.
const LOW_64: u128 = u64::MAX as u128;

/// Upper bound on Newton iterations in [`U256::isqrt`]. Starting from a power of two above the
/// root, the integer Newton step converges in well under 16 iterations for every input below
/// `2^254`; the cap only turns a broken invariant into an error instead of a loop.
const ISQRT_MAX_ITERATIONS: u32 = 128;

/// Unsigned 256-bit integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub(crate) struct U256 {
    /// High 128 bits. Declared first so the derived `Ord` is numeric order.
    pub(crate) hi: u128,
    /// Low 128 bits.
    pub(crate) lo: u128,
}

/// Splits a `u128` into its high and low 64-bit halves.
#[inline]
fn halves(x: u128) -> (u64, u64) {
    let hi = x.checked_shr(64).unwrap_or(0);
    // Truncation intended: keeps the low 64 bits.
    (hi as u64, (x & LOW_64) as u64)
}

/// Joins two 64-bit halves into a `u128`.
#[inline]
fn join(hi: u64, lo: u64) -> u128 {
    // `hi < 2^64`, so shifting by 64 loses no bits.
    u128::from(hi).checked_shl(64).unwrap_or(0) | u128::from(lo)
}

/// Full product of two 64-bit limbs. `(2^64 − 1)^2 < 2^128`, so it never fails.
#[inline]
fn mul_64(a: u64, b: u64) -> Result<u128, PricingError> {
    u128::from(a)
        .checked_mul(u128::from(b))
        .ok_or(PricingError::Overflow)
}

impl U256 {
    /// Zero.
    pub(crate) const ZERO: Self = Self { hi: 0, lo: 0 };

    /// Widens a `u128`.
    #[inline]
    pub(crate) const fn from_u128(x: u128) -> Self {
        Self { hi: 0, lo: x }
    }

    /// Exact product `a × b`. It always fits in 256 bits; the `Result` only carries the checked
    /// limb arithmetic.
    pub(crate) fn mul_wide(a: u128, b: u128) -> Result<Self, PricingError> {
        let (a1, a0) = halves(a);
        let (b1, b0) = halves(b);
        let (p00_hi, p00_lo) = halves(mul_64(a0, b0)?);
        let (p01_hi, p01_lo) = halves(mul_64(a0, b1)?);
        let (p10_hi, p10_lo) = halves(mul_64(a1, b0)?);
        let p11 = mul_64(a1, b1)?;
        // Column 64: at most 3 × (2^64 − 1), so it fits in a u128.
        let mid = u128::from(p00_hi)
            .checked_add(u128::from(p01_lo))
            .and_then(|m| m.checked_add(u128::from(p10_lo)))
            .ok_or(PricingError::Overflow)?;
        let (mid_hi, mid_lo) = halves(mid);
        let hi = p11
            .checked_add(u128::from(p01_hi))
            .and_then(|h| h.checked_add(u128::from(p10_hi)))
            .and_then(|h| h.checked_add(u128::from(mid_hi)))
            .ok_or(PricingError::Overflow)?;
        Ok(Self {
            hi,
            lo: join(mid_lo, p00_lo),
        })
    }

    /// `self − o`; `Overflow` if `o > self`.
    pub(crate) fn checked_sub(self, o: Self) -> Result<Self, PricingError> {
        let (lo, borrow) = self.lo.overflowing_sub(o.lo);
        let hi = self
            .hi
            .checked_sub(o.hi)
            .and_then(|h| h.checked_sub(u128::from(borrow)))
            .ok_or(PricingError::Overflow)?;
        Ok(Self { hi, lo })
    }

    /// Number of leading zero bits, in `0..=256`.
    #[inline]
    pub(crate) fn leading_zeros(self) -> u32 {
        if self.hi == 0 {
            // `lo.leading_zeros() <= 128`, so the sum is at most 256.
            128u32.saturating_add(self.lo.leading_zeros())
        } else {
            self.hi.leading_zeros()
        }
    }

    /// Quotient and remainder of `self / d`.
    ///
    /// `DomainError` if `d == 0`; `Overflow` if the quotient does not fit in a `u128`
    /// (`self.hi >= d`). Knuth's algorithm D with 64-bit digits: the divisor is normalized so
    /// its top bit is set, then two quotient digits are estimated and corrected.
    pub(crate) fn div_rem(self, d: u128) -> Result<(u128, u128), PricingError> {
        if d == 0 {
            return Err(PricingError::DomainError);
        }
        if self.hi >= d {
            return Err(PricingError::Overflow);
        }
        let s = d.leading_zeros();
        // `s` counts leading zeros of `d`, so the shift drops no bits.
        let dn = d.checked_shl(s).ok_or(PricingError::Overflow)?;
        let (n_hi, n_lo) = if s == 0 {
            (self.hi, self.lo)
        } else {
            let back = 128u32.checked_sub(s).ok_or(PricingError::Overflow)?;
            // `self.hi < d < 2^(128 − s)`, so `self.hi << s` drops no bits.
            let hi = self.hi.checked_shl(s).ok_or(PricingError::Overflow)?
                | self.lo.checked_shr(back).ok_or(PricingError::Overflow)?;
            let lo = self.lo.checked_shl(s).ok_or(PricingError::Overflow)?;
            (hi, lo)
        };
        let (n1, n0) = halves(n_lo);
        let (q1, r1) = div_step(n_hi, n1, dn)?;
        let (q0, r0) = div_step(r1, n0, dn)?;
        let rem = r0.checked_shr(s).ok_or(PricingError::Overflow)?;
        Ok((join(q1, q0), rem))
    }

    /// `floor(sqrt(self))`; `Overflow` if `self >= 2^254`.
    ///
    /// Integer Newton iteration from a power of two at or above the root; the sequence
    /// decreases strictly until it reaches `floor(sqrt(self))`.
    pub(crate) fn isqrt(self) -> Result<u128, PricingError> {
        if self == Self::ZERO {
            return Ok(0);
        }
        let bits = 256u32
            .checked_sub(self.leading_zeros())
            .ok_or(PricingError::Overflow)?;
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

/// One step of algorithm D: divides the 192-bit value `r × 2^64 + digit` by the normalized
/// `d` (top bit set), given `r < d`. Returns the 64-bit quotient digit and the remainder.
fn div_step(r: u128, digit: u64, d: u128) -> Result<(u64, u128), PricingError> {
    let (r1, r0) = halves(r);
    let value = U256 {
        hi: u128::from(r1),
        lo: join(r0, digit),
    };
    let (d1, _) = halves(d);
    // `d` is normalized, so `d1 >= 2^63` and the estimate exceeds the true digit by at most
    // 2 (Knuth, TAOCP vol. 2, 4.3.1, Theorem B).
    let mut q = r
        .checked_div(u128::from(d1))
        .ok_or(PricingError::Overflow)?
        .min(LOW_64);
    let mut prod = U256::mul_wide(q, d)?;
    for _ in 0..2 {
        if prod > value {
            q = q.checked_sub(1).ok_or(PricingError::Overflow)?;
            prod = prod.checked_sub(U256::from_u128(d))?;
        }
    }
    let rem = value.checked_sub(prod)?;
    if rem.hi != 0 || rem.lo >= d {
        // Excluded by Theorem B; reported rather than asserted.
        return Err(PricingError::Overflow);
    }
    let q = u64::try_from(q).map_err(|_| PricingError::Overflow)?;
    Ok((q, rem.lo))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::integer_division
)]
mod tests {
    use super::U256;
    use crate::PricingError;

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
        assert_eq!(p.hi, u128::MAX - 1);
        assert_eq!(p.lo, 1);
    }

    #[test]
    fn test_div_rem_inverts_mul_wide() {
        let cases = [
            (u128::MAX, u128::MAX, 0u128),
            (u128::MAX, u128::MAX - 1, u128::MAX - 2),
            (1u128 << 100, (1u128 << 127) + 12_345, 999),
            (10u128.pow(18), 3, 1),
        ];
        for (a, b, r) in cases {
            let n = U256::mul_wide(a, b).unwrap();
            let n = U256 {
                hi: n.hi + u128::from(n.lo.checked_add(r).is_none()),
                lo: n.lo.wrapping_add(r),
            };
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
        let n = U256 { hi: 5, lo: 0 };
        assert_eq!(n.div_rem(5), Err(PricingError::Overflow));
        assert!(n.div_rem(6).is_ok());
    }

    #[test]
    fn test_isqrt_perfect_squares_and_neighbours() {
        for s in [0u128, 1, 2, 3, 1 << 40, 10u128.pow(18), (1u128 << 100) + 7] {
            let sq = U256::mul_wide(s, s).unwrap();
            assert_eq!(sq.isqrt().unwrap(), s);
            if s > 0 {
                let below = sq.checked_sub(U256::from_u128(1)).unwrap();
                assert_eq!(below.isqrt().unwrap(), s - 1);
            }
        }
    }

    #[test]
    fn test_isqrt_too_large_is_overflow() {
        let n = U256 {
            hi: u128::MAX,
            lo: 0,
        };
        assert_eq!(n.isqrt(), Err(PricingError::Overflow));
    }
}
