//! `Fixed::from_decimal_str` on arbitrary bytes never panics and, on valid text, brackets the
//! exact value; `Display` of any raw value parses back to it in both directions
//! (`docs/PLAN-improvements.md`, milestone 16 target 6, added by milestone 17).
#![no_main]

use libfuzzer_sys::fuzz_target;
use num_bigint::BigInt;
use optfix::{Fixed, PricingError, Rounding};
use optfix_fuzz::{fits_i128, floor_ceil, Bytes, ONE};

/// `(value × 10^n, n)` of text of the documented form, or `None`.
fn exact(s: &str) -> Option<(BigInt, u32)> {
    let (neg, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) if !f.is_empty() => (i, f),
        Some(_) => return None,
        None => (body, ""),
    };
    let digits = |t: &str| t.bytes().all(|c| c.is_ascii_digit());
    if int.is_empty() || !digits(int) || !digits(frac) {
        return None;
    }
    let v: BigInt = format!("{int}{frac}").parse().ok()?;
    Some((if neg { -v } else { v }, u32::try_from(frac.len()).ok()?))
}

fuzz_target!(|data: &[u8]| {
    let mut b = Bytes(data);
    let raw = b.i128();
    let x = Fixed::from_raw(raw);
    let shown = x.to_string();
    assert_eq!(
        Fixed::from_decimal_str(&shown, Rounding::Down),
        Ok(x),
        "{shown}"
    );
    assert_eq!(
        Fixed::from_decimal_str(&shown, Rounding::Up),
        Ok(x),
        "{shown}"
    );

    let Ok(text) = std::str::from_utf8(b.0) else {
        return;
    };
    let (down, up) = (
        Fixed::from_decimal_str(text, Rounding::Down),
        Fixed::from_decimal_str(text, Rounding::Up),
    );
    match exact(text) {
        None => {
            assert_eq!(down, Err(PricingError::DomainError), "{text:?}");
            assert_eq!(up, Err(PricingError::DomainError), "{text:?}");
        }
        Some((v, n)) => {
            let (lo, hi) = floor_ceil(&(v * BigInt::from(ONE)), &BigInt::from(10).pow(n));
            for (got, want) in [(down, fits_i128(&lo)), (up, fits_i128(&hi))] {
                match (got, want) {
                    (Ok(g), Some(w)) => assert_eq!(g.raw(), w, "{text:?}"),
                    (Err(PricingError::Overflow), None) => {}
                    (got, want) => panic!("{text:?}: {got:?}, want {want:?}"),
                }
            }
        }
    }
});
