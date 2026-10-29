//! Benchmark-only kernel variants for the milestone 2 comparison. Compiled only with the
//! `bench-internals` feature, hidden from the docs, and deleted once the comparison is recorded
//! in `docs/PLAN.md`.
//!
//! The variants separate three effects on SBF:
//!
//! - limb layout: four `u64` limbs (production) against two `u128` limbs (milestone 1);
//! - checked arithmetic: the `u128` kernel with `wrapping_*` limb operations;
//! - divisor specialization: `mul_down` divides by the constant `10^18`; the `_opaque` variants
//!   hide it behind `core::hint::black_box`, as the shared production `quotient` effectively
//!   does, and the `_const` variants let LLVM fold it.
//!
//! Every variant returns exactly what the production function returns (tested here on the host,
//! and on SBF by the bench).

use core::hint::black_box;

use crate::bench_u128::U256 as U256x2;
use crate::u256::U256;
use crate::{Fixed, PricingError, Rounding};

const SCALE: i128 = 1_000_000_000_000_000_000;
const SCALE_U: u128 = SCALE.unsigned_abs();

/// Production sign and rounding logic around a `(x, y, z) -> (x × y / z, x × y % z)` kernel;
/// generic, so each variant gets its own specialized copy.
fn quotient_with(
    a: i128,
    b: i128,
    d: i128,
    r: Rounding,
    kernel: impl FnOnce(u128, u128, u128) -> Result<(u128, u128), PricingError>,
) -> Result<Fixed, PricingError> {
    if d == 0 {
        return Err(PricingError::DomainError);
    }
    let negative = ((a < 0) ^ (b < 0)) ^ (d < 0);
    let (q, rem) = kernel(a.unsigned_abs(), b.unsigned_abs(), d.unsigned_abs())?;
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
    raw.map(Fixed::from_raw).ok_or(PricingError::Overflow)
}

/// Production `sqrt_down` logic around a `raw -> (floor(sqrt(raw × SCALE)), exact)` kernel.
fn sqrt_down_with(
    x: Fixed,
    kernel: impl FnOnce(u128) -> Result<(u128, bool), PricingError>,
) -> Result<Fixed, PricingError> {
    let raw = u128::try_from(x.raw()).map_err(|_| PricingError::DomainError)?;
    let (root, _) = kernel(raw)?;
    i128::try_from(root)
        .map(Fixed::from_raw)
        .map_err(|_| PricingError::Overflow)
}

fn kernel_u64(x: u128, y: u128, z: u128) -> Result<(u128, u128), PricingError> {
    U256::mul_wide(x, y)?.div_rem(z)
}

fn kernel_u128(x: u128, y: u128, z: u128) -> Result<(u128, u128), PricingError> {
    U256x2::mul_wide(x, y)?.div_rem(z)
}

fn kernel_u128_wrapping(x: u128, y: u128, z: u128) -> Result<(u128, u128), PricingError> {
    wrapping::div_rem(wrapping::mul_wide(x, y), z)
}

macro_rules! mul_variant {
    ($(#[$doc:meta])* $name:ident, $kernel:path, $divisor:expr) => {
        $(#[$doc])*
        ///
        /// # Errors
        ///
        /// As [`Fixed::mul_down`].
        #[must_use = "the result carries a possible error"]
        #[inline(never)]
        pub fn $name(a: Fixed, b: Fixed) -> Result<Fixed, PricingError> {
            quotient_with(a.raw(), b.raw(), $divisor, Rounding::Down, $kernel)
        }
    };
}

mul_variant!(
    /// `mul_down`, four `u64` limbs, opaque divisor.
    mul_down_u64_opaque, kernel_u64, black_box(SCALE)
);
mul_variant!(
    /// `mul_down`, two `u128` limbs, opaque divisor.
    mul_down_u128_opaque, kernel_u128, black_box(SCALE)
);
mul_variant!(
    /// `mul_down`, two `u128` limbs with wrapping arithmetic, opaque divisor.
    mul_down_u128_wrapping_opaque, kernel_u128_wrapping, black_box(SCALE)
);
mul_variant!(
    /// `mul_down`, four `u64` limbs, constant divisor.
    mul_down_u64_const, kernel_u64, SCALE
);
mul_variant!(
    /// `mul_down`, two `u128` limbs, constant divisor.
    mul_down_u128_const, kernel_u128, SCALE
);

/// `div_down` with two `u128` limbs.
///
/// # Errors
///
/// As [`Fixed::div_down`].
#[must_use = "the result carries a possible error"]
#[inline(never)]
pub fn div_down_u128(a: Fixed, b: Fixed) -> Result<Fixed, PricingError> {
    quotient_with(a.raw(), SCALE, b.raw(), Rounding::Down, kernel_u128)
}

/// `sqrt_down` with two `u128` limbs.
///
/// # Errors
///
/// As [`Fixed::sqrt_down`].
#[must_use = "the result carries a possible error"]
#[inline(never)]
pub fn sqrt_down_u128(a: Fixed) -> Result<Fixed, PricingError> {
    sqrt_down_with(a, |raw| {
        let n = U256x2::mul_wide(raw, SCALE_U)?;
        let root = n.isqrt()?;
        Ok((root, U256x2::mul_wide(root, root)? == n))
    })
}

/// The milestone 1 two-`u128`-limb algorithm with wrapping limb arithmetic. Every wrapped
/// operation is one the checked version proves cannot overflow, so results are identical.
mod wrapping {
    use crate::PricingError;

    const LOW_64: u128 = u64::MAX as u128;

    #[inline]
    fn lo64(x: u128) -> u64 {
        // Truncation intended: keeps the low 64 bits.
        (x & LOW_64) as u64
    }

    #[inline]
    fn hi64(x: u128) -> u64 {
        x.wrapping_shr(64) as u64
    }

    #[inline]
    fn join(hi: u64, lo: u64) -> u128 {
        u128::from(hi).wrapping_shl(64) | u128::from(lo)
    }

    #[inline]
    fn mul64(a: u64, b: u64) -> u128 {
        u128::from(a).wrapping_mul(u128::from(b))
    }

    /// Exact `a × b` as (high, low) 128-bit halves.
    pub(super) fn mul_wide(a: u128, b: u128) -> (u128, u128) {
        let (a1, a0) = (hi64(a), lo64(a));
        let (b1, b0) = (hi64(b), lo64(b));
        let p00 = mul64(a0, b0);
        let p01 = mul64(a0, b1);
        let p10 = mul64(a1, b0);
        let p11 = mul64(a1, b1);
        let mid = u128::from(hi64(p00))
            .wrapping_add(u128::from(lo64(p01)))
            .wrapping_add(u128::from(lo64(p10)));
        let hi = p11
            .wrapping_add(u128::from(hi64(p01)))
            .wrapping_add(u128::from(hi64(p10)))
            .wrapping_add(u128::from(hi64(mid)));
        (hi, join(lo64(mid), lo64(p00)))
    }

    /// `(n / d, n % d)` for `n = (hi, lo)`; same contract as the checked `div_rem`.
    pub(super) fn div_rem(n: (u128, u128), d: u128) -> Result<(u128, u128), PricingError> {
        let (hi, lo) = n;
        if d == 0 {
            return Err(PricingError::DomainError);
        }
        if hi >= d {
            return Err(PricingError::Overflow);
        }
        let s = d.leading_zeros();
        let dn = d.wrapping_shl(s);
        let (n_hi, n_lo) = if s == 0 {
            (hi, lo)
        } else {
            (
                hi.wrapping_shl(s) | lo.wrapping_shr(128u32.wrapping_sub(s)),
                lo.wrapping_shl(s),
            )
        };
        let (q1, r1) = step(n_hi, hi64(n_lo), dn);
        let (q0, r0) = step(r1, lo64(n_lo), dn);
        Ok((join(q1, q0), r0.wrapping_shr(s)))
    }

    /// One digit of the checked `div_step`, unchecked.
    fn step(r: u128, digit: u64, d: u128) -> (u64, u128) {
        let value = (u128::from(hi64(r)), join(lo64(r), digit));
        let d1 = u128::from(hi64(d));
        // `d` is normalized, so `d1 >= 2^63` and never zero.
        let mut q = r.checked_div(d1).unwrap_or(0).min(LOW_64);
        let mut prod = mul_wide(q, d);
        for _ in 0..2 {
            if prod > value {
                q = q.wrapping_sub(1);
                prod = sub(prod, (0, d));
            }
        }
        (lo64(q), sub(value, prod).1)
    }

    fn sub(a: (u128, u128), b: (u128, u128)) -> (u128, u128) {
        let (lo, borrow) = a.1.overflowing_sub(b.1);
        (a.0.wrapping_sub(b.0).wrapping_sub(u128::from(borrow)), lo)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::arithmetic_side_effects)]
mod tests {
    use super::*;

    /// splitmix64: deterministic inputs without a dependency.
    fn next(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn sample(state: &mut u64) -> Fixed {
        let x = (u128::from(next(state)) << 64) | u128::from(next(state));
        let shift = (next(state) % 128) as u32;
        Fixed::from_raw((x >> shift) as i128)
    }

    #[test]
    fn test_variants_match_production() {
        let mut state = 7u64;
        for _ in 0..200_000 {
            let (a, b) = (sample(&mut state), sample(&mut state));
            let mul = a.mul_down(b);
            assert_eq!(mul_down_u64_opaque(a, b), mul);
            assert_eq!(mul_down_u128_opaque(a, b), mul);
            assert_eq!(mul_down_u128_wrapping_opaque(a, b), mul);
            assert_eq!(mul_down_u64_const(a, b), mul);
            assert_eq!(mul_down_u128_const(a, b), mul);
            assert_eq!(div_down_u128(a, b), a.div_down(b));
            assert_eq!(sqrt_down_u128(a), a.sqrt_down());
        }
    }
}
