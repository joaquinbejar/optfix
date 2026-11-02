//! `wide::mul_shr` for every shift and both roundings against the exact `a × b / 2^shift`
//! (`docs/PLAN-improvements.md`, milestone 16, target 3).
#![no_main]

use libfuzzer_sys::fuzz_target;
use num_bigint::BigInt;
use optfix::{wide::mul_shr, PricingError, Rounding};
use optfix_fuzz::{floor_ceil, Bytes};

fuzz_target!(|data: &[u8]| {
    let mut b = Bytes(data);
    let (x, y) = (b.u128(), b.u128());
    // Weight the shifts that matter (0 to 256) without excluding the rest.
    let raw_shift = b.u32();
    let shift = if raw_shift & 1 == 0 { (raw_shift >> 1) % 300 } else { raw_shift >> 1 };
    let product = BigInt::from(x) * BigInt::from(y);
    let (lo, hi) = if shift >= 512 {
        // 2^shift exceeds the product: 0, or 1 rounding a nonzero product up.
        let nonzero = product != BigInt::from(0);
        (BigInt::from(0), BigInt::from(u8::from(nonzero)))
    } else {
        floor_ceil(&product, &(BigInt::from(1) << shift))
    };
    for (r, want) in [(Rounding::Down, &lo), (Rounding::Up, &hi)] {
        let got = mul_shr(x, y, shift, r);
        match (got, u128::try_from(want).ok()) {
            (Ok(v), Some(w)) => assert_eq!(v, w, "mul_shr({x}, {y}, {shift}, {r:?})"),
            (Err(PricingError::Overflow), None) => {}
            (got, want) => panic!("mul_shr({x}, {y}, {shift}, {r:?}) = {got:?}, want {want:?}"),
        }
    }
});
