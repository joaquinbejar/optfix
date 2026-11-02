//! `Fixed::from_scaled` and `Fixed::to_scaled` against the exact rational, every `decimals` and
//! both roundings (`docs/PLAN-improvements.md`, milestone 16, target 1).
#![no_main]

use libfuzzer_sys::fuzz_target;
use num_bigint::BigInt;
use optfix::{Fixed, PricingError, Rounding};
use optfix_fuzz::{fits_i128, floor_ceil, Bytes, ONE};

fuzz_target!(|data: &[u8]| {
    let mut b = Bytes(data);
    let value = b.i128();
    let decimals = b.u8();
    let one = BigInt::from(ONE);
    for (r, up) in [(Rounding::Down, false), (Rounding::Up, true)] {
        // from_scaled: value / 10^decimals.
        let got = Fixed::from_scaled(value, decimals, r);
        if decimals > 38 {
            assert_eq!(
                got,
                Err(PricingError::DomainError),
                "from_scaled({value}, {decimals})"
            );
        } else {
            let (lo, hi) = floor_ceil(
                &(BigInt::from(value) * &one),
                &BigInt::from(10).pow(u32::from(decimals)),
            );
            let want = fits_i128(if up { &hi } else { &lo });
            match (got, want) {
                (Ok(x), Some(w)) => {
                    assert_eq!(x.raw(), w, "from_scaled({value}, {decimals}, {r:?})")
                }
                (Err(PricingError::Overflow), None) => {}
                (got, want) => {
                    panic!("from_scaled({value}, {decimals}, {r:?}) = {got:?}, want {want:?}")
                }
            }
        }
        // to_scaled: raw × 10^decimals / 2^64.
        let got = Fixed::from_raw(value).to_scaled(decimals, r);
        let want = if decimals > 76 {
            // The factor 10^(decimals − 38) does not fit an i128.
            None
        } else {
            let (lo, hi) = floor_ceil(
                &(BigInt::from(value) * BigInt::from(10).pow(u32::from(decimals))),
                &one,
            );
            fits_i128(if up { &hi } else { &lo })
        };
        match (got, want) {
            (Ok(x), Some(w)) => assert_eq!(x, w, "to_scaled({value}, {decimals}, {r:?})"),
            (Err(PricingError::Overflow), None) => {}
            (got, want) => panic!("to_scaled({value}, {decimals}, {r:?}) = {got:?}, want {want:?}"),
        }
    }
});
