//! Milestone 17 (`docs/PLAN-improvements.md`): `Fixed`'s decimal text against `num-bigint`.
//! Printing is exact (`parse(format(x)) == x` in both directions, and the printed digits
//! times `2^64` equal the raw value); parsing a random decimal brackets its exact value, the
//! two directions one raw unit apart unless exact. `OPTFIX_PROPTEST_CASES` overrides the
//! default of 100,000 cases.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::float_arithmetic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::disallowed_macros,
    missing_docs
)]

mod common;

use num_bigint::BigInt;
use optfix::{Fixed, PricingError, Rounding};
use proptest::prelude::*;

/// `(value × 10^n, n)` of a decimal string, exactly.
fn exact(s: &str) -> (BigInt, u32) {
    let (neg, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    let digits: BigInt = format!("{int}{frac}").parse().unwrap();
    (if neg { -digits } else { digits }, frac.len() as u32)
}

fn raw_any() -> impl Strategy<Value = i128> {
    prop_oneof![
        3 => any::<i128>(),
        3 => (any::<i64>(), 0u32..64).prop_map(|(v, s)| i128::from(v) << s),
        1 => proptest::sample::select(vec![0, 1, -1, i128::MAX, i128::MIN, 1 << 64, -(1 << 64)]),
    ]
}

/// A decimal string: sign, up to 20 integer digits, and up to 90 fractional digits.
fn decimal() -> impl Strategy<Value = String> {
    (
        prop_oneof![Just(""), Just("-"), Just("+")],
        "[0-9]{1,20}",
        proptest::option::of("[0-9]{1,90}"),
    )
        .prop_map(|(sign, int, frac)| match frac {
            Some(f) => format!("{sign}{int}.{f}"),
            None => format!("{sign}{int}"),
        })
}

proptest! {
    #![proptest_config(common::config_with(100_000))]

    #[test]
    fn prop_display_is_exact_and_parses_back(raw in raw_any()) {
        let x = Fixed::from_raw(raw);
        let s = x.to_string();
        prop_assert_eq!(Fixed::from_decimal_str(&s, Rounding::Down), Ok(x));
        prop_assert_eq!(Fixed::from_decimal_str(&s, Rounding::Up), Ok(x));
        // digits / 10^n == raw / 2^64 exactly.
        let (v, n) = exact(&s);
        prop_assert_eq!(v << 64, BigInt::from(raw) * BigInt::from(10).pow(n), "{}", s);
        prop_assert!(!s.ends_with('0') || !s.contains('.'), "trailing zero in {}", s);
    }

    #[test]
    fn prop_parse_brackets_the_exact_value(s in decimal()) {
        let (v, n) = exact(&s);
        let scaled: BigInt = v << 64; // value × 10^n × 2^64
        let p = BigInt::from(10).pow(n);
        let down = Fixed::from_decimal_str(&s, Rounding::Down);
        let up = Fixed::from_decimal_str(&s, Rounding::Up);
        // floor and ceiling of scaled / p.
        let q: BigInt = &scaled / &p;
        let r: BigInt = &scaled - &q * &p;
        let zero = BigInt::from(0);
        let floor = if r < zero { &q - 1 } else { q.clone() };
        let ceil = if r > zero { &q + 1 } else { q };
        for (got, want) in [(down, floor), (up, ceil)] {
            match (got, i128::try_from(&want)) {
                (Ok(x), Ok(w)) => prop_assert_eq!(x.raw(), w, "{}", s),
                (Err(PricingError::Overflow), Err(_)) => {}
                (got, want) => prop_assert!(false, "{}: {:?} against {:?}", s, got, want),
            }
        }
    }
}
