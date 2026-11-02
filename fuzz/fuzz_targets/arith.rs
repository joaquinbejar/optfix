//! `mul_*`, `div_*`, `sqrt_*` and `from_ratio` against the exact value: the floor rounding
//! down and the ceiling up, `Overflow` exactly when it does not fit and `DomainError` exactly
//! on a zero divisor or a negative square root (`docs/PLAN-improvements.md`, milestone 16,
//! target 2). The 256-bit division kernel runs under `div_*` and `from_ratio`.
#![no_main]

use libfuzzer_sys::fuzz_target;
use num_bigint::BigInt;
use optfix::{Fixed, PricingError, Rounding};
use optfix_fuzz::{fits_i128, floor_ceil, Bytes, ONE};

fn check(what: &str, a: i128, b: i128, got: Result<Fixed, PricingError>, want: Option<&BigInt>) {
    let want = match want {
        None => {
            assert_eq!(got, Err(PricingError::DomainError), "{what}({a}, {b})");
            return;
        }
        Some(w) => fits_i128(w),
    };
    match (got, want) {
        (Ok(x), Some(w)) => assert_eq!(x.raw(), w, "{what}({a}, {b})"),
        (Err(PricingError::Overflow), None) => {}
        (got, want) => panic!("{what}({a}, {b}) = {got:?}, want {want:?}"),
    }
}

fuzz_target!(|data: &[u8]| {
    let mut bytes = Bytes(data);
    let op = bytes.u8();
    let (a, b) = (bytes.i128(), bytes.i128());
    let (x, y) = (Fixed::from_raw(a), Fixed::from_raw(b));
    let one = BigInt::from(ONE);
    match op % 4 {
        0 => {
            let (lo, hi) = floor_ceil(&(BigInt::from(a) * BigInt::from(b)), &one);
            check("mul_down", a, b, x.mul_down(y), Some(&lo));
            check("mul_up", a, b, x.mul_up(y), Some(&hi));
        }
        1 => {
            let q = (b != 0).then(|| {
                let (n, d) = (BigInt::from(a) * &one, BigInt::from(b));
                // floor_ceil wants d > 0: move the sign to the numerator.
                if b < 0 {
                    floor_ceil(&-n, &-d)
                } else {
                    floor_ceil(&n, &d)
                }
            });
            check("div_down", a, b, x.div_down(y), q.as_ref().map(|q| &q.0));
            check("div_up", a, b, x.div_up(y), q.as_ref().map(|q| &q.1));
            check(
                "from_ratio_down",
                a,
                b,
                Fixed::from_ratio(a, b, Rounding::Down),
                q.as_ref().map(|q| &q.0),
            );
            check(
                "from_ratio_up",
                a,
                b,
                Fixed::from_ratio(a, b, Rounding::Up),
                q.as_ref().map(|q| &q.1),
            );
        }
        2 => {
            let (down, up) = (x.sqrt_down(), x.sqrt_up());
            if a < 0 {
                assert_eq!(down, Err(PricingError::DomainError));
                assert_eq!(up, Err(PricingError::DomainError));
                return;
            }
            let m = BigInt::from(a) * &one;
            let sq = |v: i128| BigInt::from(v) * BigInt::from(v);
            let s = down.expect("sqrt_down of a non-negative value").raw();
            let t = up.expect("sqrt_up of a non-negative value").raw();
            assert!(
                sq(s) <= m && m < sq(s + 1),
                "sqrt_down({a}) = {s}, not the floor"
            );
            assert!(
                (t == 0 || sq(t - 1) < m) && m <= sq(t),
                "sqrt_up({a}) = {t}, not the ceiling"
            );
        }
        _ => {
            let sum = BigInt::from(a) + BigInt::from(b);
            let diff = BigInt::from(a) - BigInt::from(b);
            check("checked_add", a, b, x.checked_add(y), Some(&sum));
            check("checked_sub", a, b, x.checked_sub(y), Some(&diff));
            check(
                "checked_neg",
                a,
                0,
                x.checked_neg(),
                Some(&-BigInt::from(a)),
            );
        }
    }
});
