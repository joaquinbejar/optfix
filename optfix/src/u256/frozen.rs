//! Milestone 10 (`docs/PLAN.md` section 8): the 32-bit-digit kernel against the frozen 0.1.0
//! kernel ([`super::v01`]) and the exact `num-bigint` results, on random cases and on each
//! path of algorithm D. Test-only.

#![allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    clippy::disallowed_macros,
    clippy::cast_possible_truncation
)]

use super::{v01, ADD_BACK, ISQRT_STEPS, REFINE, U256};
use crate::PricingError;
use core::sync::atomic::Ordering;
use num_bigint::BigUint;
use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};

/// Cases per property; `OPTFIX_PROPTEST_CASES` overrides the default of one million.
fn config() -> Config {
    let cases = std::env::var("OPTFIX_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000_000);
    Config {
        cases,
        rng_seed: RngSeed::Fixed(0x0F7F_1C5E_ED00_0010),
        failure_persistence: None,
        ..Config::default()
    }
}

fn words(n: U256) -> [u64; 4] {
    [n.w0, n.w1, n.w2, n.w3]
}

fn old(n: U256) -> v01::U256 {
    v01::U256::from_words(n.w0, n.w1, n.w2, n.w3)
}

fn big(n: U256) -> BigUint {
    words(n)
        .iter()
        .rev()
        .fold(BigUint::from(0u8), |b, &x| (b << 64u32) + BigUint::from(x))
}

fn from_parts(hi: u128, lo: u128) -> U256 {
    U256 {
        w0: lo as u64,
        w1: (lo >> 64) as u64,
        w2: hi as u64,
        w3: (hi >> 64) as u64,
    }
}

fn halves(n: U256) -> (u128, u128) {
    (
        (u128::from(n.w3) << 64) | u128::from(n.w2),
        (u128::from(n.w1) << 64) | u128::from(n.w0),
    )
}

/// `q × d + r`, or `None` when it does not fit in 256 bits.
fn compose(q: u128, d: u128, r: u128) -> Option<U256> {
    let (hi, lo) = halves(U256::mul_wide(q, d).unwrap());
    let (lo, carry) = lo.overflowing_add(r);
    hi.checked_add(u128::from(carry))
        .map(|hi| from_parts(hi, lo))
}

/// The new `div_rem` equals 0.1.0's, results and errors, and the exact quotient and
/// remainder.
fn check_div(n: U256, d: u128) {
    let new = n.div_rem(d);
    assert_eq!(new, old(n).div_rem(d), "n={:?} d={d}", words(n));
    if let Ok((q, r)) = new {
        let (bn, bd) = (big(n), BigUint::from(d));
        assert_eq!(BigUint::from(q), &bn / &bd, "n={:?} d={d}", words(n));
        assert_eq!(BigUint::from(r), &bn % &bd, "n={:?} d={d}", words(n));
    }
}

/// The new `isqrt` equals 0.1.0's and the exact floor of the root.
fn check_sqrt(n: U256) {
    let new = n.isqrt();
    assert_eq!(new, old(n).isqrt(), "n={:?}", words(n));
    if let Ok(x) = new {
        assert_eq!(BigUint::from(x), big(n).sqrt(), "n={:?}", words(n));
    }
}

/// A divisor with exactly `k` significant 32-bit digits (1 to 4), its top digit drawn to cover
/// every normalization shift, 0 (top bit set) to 31 (top digit 1).
fn divisor() -> impl Strategy<Value = u128> {
    (1u32..=4, any::<u128>(), 0u32..32, any::<bool>()).prop_map(|(k, x, s, exact_top)| {
        let bits = 32 * k;
        let top_bit = 1u128 << (bits - 1 - s);
        let below = if exact_top { 0 } else { x & (top_bit - 1) };
        top_bit | below
    })
}

/// 256-bit values: mostly `q × d + r` (the quotient fits), plus random values and the two
/// sides of the overflow boundary `(w3:w2) = d − 1` and `= d`.
fn numerator(d: u128) -> impl Strategy<Value = U256> {
    prop_oneof![
        3 => (any::<u128>(), 0u32..=128, any::<u128>()).prop_map(move |(q, sh, r)| {
            let q = if sh == 128 { 0 } else { q >> sh };
            compose(q, d, r % d).unwrap_or_else(|| from_parts(d - 1, r))
        }),
        1 => (any::<u128>(), any::<u128>()).prop_map(|(hi, lo)| from_parts(hi, lo)),
        1 => any::<u128>().prop_map(move |lo| from_parts(d - 1, lo)),
        1 => any::<u128>().prop_map(move |lo| from_parts(d, lo)),
    ]
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn prop_div_rem_matches_0_1_0_and_exact(
        (n, d) in divisor().prop_flat_map(|d| (numerator(d), Just(d)))
    ) {
        check_div(n, d);
    }

    #[test]
    fn prop_isqrt_matches_0_1_0_and_exact(
        hi in any::<u128>(), lo in any::<u128>(), shift in 0u32..256
    ) {
        // A random size: the 256-bit value shifted right.
        let n = match shift {
            0 => from_parts(hi, lo),
            1..=127 => from_parts(hi >> shift, (lo >> shift) | (hi << (128 - shift))),
            _ => from_parts(0, hi >> (shift - 128)),
        };
        check_sqrt(n);
    }

    #[test]
    fn prop_isqrt_of_squares_and_neighbours(x in any::<u128>(), shift in 0u32..128) {
        let x = (x >> shift) >> 1;
        check_sqrt(U256::mul_wide(x, x).unwrap());
        if let Some(n) = compose(x, x, 1) {
            check_sqrt(n);
        }
        if x > 0 {
            // x² − 1 = (x − 1)(x + 1).
            check_sqrt(compose(x - 1, x + 1, 0).unwrap());
        }
    }
}

#[test]
fn test_every_divisor_length_and_shift() {
    let mut d_cases: Vec<u128> = vec![
        1,
        2,
        3,
        u128::from(u32::MAX),
        1 << 32,
        (1 << 32) + 1,
        u128::from(u64::MAX),
        1 << 64,
        (1 << 95) | 1,
        1 << 96,
        u128::MAX,
        u128::MAX >> 1,
        (1 << 127) | 1,
    ];
    for k in 1..=4u32 {
        let base = 32 * (k - 1);
        // Top digit 1 (shift 31) and 2^31 (shift 0), with zero and full lower digits.
        for top in [1u128 << base, 1u128 << (base + 31)] {
            d_cases.push(top);
            d_cases.push(top | ((1u128 << base) - 1));
        }
    }
    for &d in &d_cases {
        for q in [
            0,
            1,
            2,
            u128::from(u32::MAX),
            u128::from(u64::MAX),
            u128::MAX,
        ] {
            for r in [0, 1, d / 2, d - 1] {
                if let Some(n) = compose(q, d, r) {
                    check_div(n, d);
                }
            }
        }
        // The overflow boundary: (w3:w2) = d − 1 fits, d does not.
        check_div(from_parts(d - 1, u128::MAX), d);
        check_div(from_parts(d, 0), d);
    }
    assert_eq!(from_parts(0, 5).div_rem(0), Err(PricingError::DomainError));
}

/// The refinement (D3) and add-back (D6) branches are rare for random digits (D6 about 2 in
/// 2^32 steps), so the cases are constructed. The counters prove both branches run.
#[test]
fn test_refinement_and_add_back_run() {
    let before = (
        REFINE.load(Ordering::Relaxed),
        ADD_BACK.load(Ordering::Relaxed),
    );
    // Add-back cases from Hacker's Delight's `divmnu` tests (base 2^16 there), scaled to base
    // 2^32: (dividend digits, divisor digits), least significant first.
    let cases: [(&[u64], &[u64]); 3] = [
        (&[0, 0, 0x8000_0000, 0x7fff_ffff], &[1, 0, 0x8000_0000]),
        (
            &[0, 0xffff_fffe, 0, 0x8000_0000],
            &[0xffff_ffff, 0x8000_0000],
        ),
        (&[3, 0, 0x8000_0000], &[1, 0x2000_0000]),
    ];
    for (n, d) in cases {
        let mut w = [0u128; 2];
        for (i, &x) in n.iter().enumerate() {
            w[i / 4] |= u128::from(x) << (32 * (i % 4));
        }
        let dv = d
            .iter()
            .enumerate()
            .fold(0u128, |a, (i, &x)| a | (u128::from(x) << (32 * i)));
        check_div(from_parts(w[1], w[0]), dv);
    }
    // Random dividends just below a multiple of the divisor plus a random search: the
    // estimate's error concentrates there.
    let mut x = 0x0123_4567_89ab_cdef_u64;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for _ in 0..200_000 {
        let d = (u128::from(next() | (1 << 63)) << 32) | u128::from(next() >> 32);
        let q = (u128::from(next()) << 32) | u128::from(next() >> 32);
        let r = d - 1 - u128::from(next() % 3);
        if let Some(n) = compose(q, d, r) {
            check_div(n, d);
        }
    }
    let after = (
        REFINE.load(Ordering::Relaxed),
        ADD_BACK.load(Ordering::Relaxed),
    );
    assert!(after.0 > before.0, "D3 refinement never ran");
    assert!(after.1 > before.1, "D6 add-back never ran");
}

#[test]
fn test_isqrt_edges() {
    check_sqrt(U256::default());
    for e in 0..254u32 {
        let (hi, lo) = if e >= 128 {
            (1u128 << (e - 128), 0)
        } else {
            (0, 1u128 << e)
        };
        check_sqrt(from_parts(hi, lo));
        let (m, borrow) = lo.overflowing_sub(1);
        check_sqrt(from_parts(hi - u128::from(borrow), m));
        check_sqrt(from_parts(hi, lo | 1));
    }
    check_sqrt(from_parts(u128::MAX >> 2, u128::MAX));
    assert_eq!(from_parts(1 << 126, 0).isqrt(), Err(PricingError::Overflow));
    assert_eq!(
        from_parts(u128::MAX, 0).isqrt(),
        Err(PricingError::Overflow)
    );
}

/// The seed is within `2^-30` of the root, so `isqrt` takes at most four divisions (three
/// decreasing steps and the one that confirms). Counted per call (per-thread counter) on
/// random sizes.
#[test]
fn test_isqrt_takes_at_most_four_divisions() {
    let steps = || ISQRT_STEPS.with(std::cell::Cell::get);
    let mut x = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut worst = 0;
    for i in 0..100_000u32 {
        let hi = (u128::from(next()) << 64 | u128::from(next())) >> (2 + i % 126);
        let lo = u128::from(next()) << 64 | u128::from(next());
        let n = from_parts(hi, lo);
        let before = steps();
        n.isqrt().unwrap();
        worst = worst.max(steps() - before);
    }
    assert!(worst <= 4, "{worst} divisions");
}
