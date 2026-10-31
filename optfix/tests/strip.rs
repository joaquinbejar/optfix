//! Milestone 11 (`docs/PLAN.md` section 8): `price_strip` and `price_and_greeks_strip` equal
//! the scalar functions bit for bit, results and errors, under the partial-output contract;
//! and a strip leg equals the frozen 0.1.0 output (`tests/data/frozen_0_1.csv`) directly, not
//! only through the rewritten scalar path.
//!
//! Random strips: 100,000 by default (`OPTFIX_PROPTEST_CASES` overrides it), 1 to 16 legs of
//! mixed kinds and unsorted strikes over the supported domain, with repeated legs, legs across
//! the near-the-money switch of `d1`, and legs outside `S/K` in `[0.05, 20]`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::float_arithmetic,
    clippy::cast_possible_truncation,
    clippy::disallowed_macros,
    missing_docs
)]

mod common;

use common::{bsm_inputs, bsm_sample, uniform, Rng};
use optfix::{
    price, price_and_greeks, price_and_greeks_strip, price_strip, EuropeanOption, Fixed, Greeks,
    MarketInputs, OptionKind, Price, Priced, PricingError, Rate, Spot, Strike, StripLeg,
    Volatility, Years,
};

const FIXTURE: &str = include_str!("data/frozen_0_1.csv");

fn cases() -> usize {
    std::env::var("OPTFIX_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100_000)
}

/// A recognisable value for slots a strip must leave unchanged.
fn sentinel_price() -> Price {
    Price::try_new(Fixed::from_raw(0x5EED)).unwrap()
}

fn sentinel_priced() -> Priced {
    let x = Fixed::from_raw(0x5EED);
    Priced {
        price: sentinel_price(),
        greeks: Greeks {
            delta: x,
            gamma: x,
            vega: x,
            theta: x,
        },
    }
}

fn option(expiry: Years, leg: &StripLeg) -> EuropeanOption {
    EuropeanOption {
        kind: leg.kind,
        strike: leg.strike,
        expiry,
    }
}

/// Checks both strip functions on `legs` against the scalar functions and the contract.
fn check(expiry: Years, m: &MarketInputs, legs: &[StripLeg]) {
    // The first failing leg, as the scalar functions see it.
    let scalar: Vec<Result<Priced, PricingError>> = legs
        .iter()
        .map(|l| price_and_greeks(&option(expiry, l), m))
        .collect();
    let scalar_price: Vec<Result<Price, PricingError>> =
        legs.iter().map(|l| price(&option(expiry, l), m)).collect();
    let first_err = scalar.iter().position(Result::is_err);
    assert_eq!(
        first_err,
        scalar_price.iter().position(Result::is_err),
        "price and price_and_greeks fail on different legs"
    );

    let mut out = vec![sentinel_price(); legs.len()];
    let got = price_strip(expiry, m, legs, &mut out);
    let mut outg = vec![sentinel_priced(); legs.len()];
    let gotg = price_and_greeks_strip(expiry, m, legs, &mut outg);
    match first_err {
        None => {
            assert_eq!(got, Ok(()));
            assert_eq!(gotg, Ok(()));
        }
        Some(i) => {
            assert_eq!(got, Err(scalar_price[i].unwrap_err()), "leg {i}");
            assert_eq!(gotg, Err(scalar[i].unwrap_err()), "leg {i}");
        }
    }
    let done = first_err.unwrap_or(legs.len());
    for i in 0..legs.len() {
        if i < done {
            assert_eq!(
                Ok(out[i]),
                scalar_price[i],
                "price leg {i} of {}",
                legs.len()
            );
            assert_eq!(Ok(outg[i]), scalar[i], "greeks leg {i} of {}", legs.len());
        } else {
            assert_eq!(out[i], sentinel_price(), "leg {i} written after the error");
            assert_eq!(
                outg[i],
                sentinel_priced(),
                "leg {i} written after the error"
            );
        }
    }
}

/// A strike for spot `s` (raw): in range, near the money across the `d1` switch
/// (`|S − K| × 256` against `K`), or with `S/K` outside `[0.05, 20]`.
fn strike(g: &mut Rng, s: i128) -> Strike {
    let lo = Strike::MIN.get().raw();
    let hi = Strike::MAX.get().raw();
    let k = match g.next() % 10 {
        // |S − K| × 256 = K: K = S × 256/255 or S × 256/257, a few ulps either side.
        0 => {
            let base = if g.next() & 1 == 0 {
                s / 255 * 256
            } else {
                s / 257 * 256
            };
            base + g.range(-4, 5)
        }
        1 => s + g.range(-1000, 1001),
        // S/K outside [0.05, 20] (only when the strike range allows it).
        2 if g.next().is_multiple_of(4) => {
            if g.next() & 1 == 0 {
                s.saturating_mul(21)
            } else {
                s / 21
            }
        }
        _ => {
            let m = 10f64.powf((0.05f64).log10() + (400f64).log10() * uniform(g));
            ((s as f64) / m) as i128
        }
    };
    Strike::try_new(Fixed::from_raw(k.clamp(lo, hi))).unwrap()
}

fn legs(g: &mut Rng, s: i128) -> Vec<StripLeg> {
    let n = 1 + (g.next() % 16) as usize;
    let mut v: Vec<StripLeg> = Vec::with_capacity(n);
    for _ in 0..n {
        let leg = if !v.is_empty() && g.next().is_multiple_of(8) {
            // A repeated leg, possibly with the other kind.
            let mut l = v[(g.next() % v.len() as u64) as usize];
            if g.next() & 1 == 0 {
                l.kind = match l.kind {
                    OptionKind::Call => OptionKind::Put,
                    OptionKind::Put => OptionKind::Call,
                };
            }
            l
        } else {
            StripLeg {
                kind: if g.next() & 1 == 0 {
                    OptionKind::Call
                } else {
                    OptionKind::Put
                },
                strike: strike(g, s),
            }
        };
        v.push(leg);
    }
    v
}

#[test]
fn test_random_strips_match_the_scalar_functions() {
    let mut g = Rng(0x0F7F_1C5E_ED00_0011);
    let (mut failed, mut legs_total) = (0usize, 0usize);
    for _ in 0..cases() {
        let p = bsm_sample(&mut g);
        let (o, m) = bsm_inputs(p);
        let l = legs(&mut g, p.0);
        legs_total += l.len();
        if l.iter().any(|x| price(&option(o.expiry, x), &m).is_err()) {
            failed += 1;
        }
        check(o.expiry, &m, &l);
    }
    // The sample exercises the error contract too.
    assert!(failed > cases() / 100, "{failed} strips with a failing leg");
    assert!(legs_total > cases() * 4);
}

#[test]
fn test_empty_strip_and_length_mismatch() {
    let (o, m) = bsm_inputs(bsm_sample(&mut Rng(1)));
    assert_eq!(price_strip(o.expiry, &m, &[], &mut []), Ok(()));
    assert_eq!(price_and_greeks_strip(o.expiry, &m, &[], &mut []), Ok(()));
    let leg = StripLeg {
        kind: o.kind,
        strike: o.strike,
    };
    let mut out = vec![sentinel_price(); 2];
    assert_eq!(
        price_strip(o.expiry, &m, &[leg], &mut out),
        Err(PricingError::DomainError)
    );
    assert_eq!(
        out,
        vec![sentinel_price(); 2],
        "written before the length check"
    );
    let mut outg: Vec<Priced> = Vec::new();
    assert_eq!(
        price_and_greeks_strip(o.expiry, &m, &[leg, leg], &mut outg),
        Err(PricingError::DomainError)
    );
}

#[test]
fn test_expiry_zero_and_tiny_expiry() {
    let mut g = Rng(2);
    for t in [0, 1, 2, 4096] {
        for _ in 0..2000 {
            let mut p = bsm_sample(&mut g);
            p.2 = t;
            let (o, m) = bsm_inputs(p);
            let l = legs(&mut g, p.0);
            check(o.expiry, &m, &l);
        }
    }
}

#[test]
fn test_failing_leg_first_middle_last() {
    let mut g = Rng(3);
    let mut seen = [0usize; 3];
    while seen.iter().any(|&c| c < 50) {
        let p = bsm_sample(&mut g);
        let (o, m) = bsm_inputs(p);
        let s = p.0;
        // Only spots where S/K = 21 has a strike in range.
        let Ok(bad) = Strike::try_new(Fixed::from_raw(s.saturating_mul(21))) else {
            continue;
        };
        let bad = StripLeg {
            kind: OptionKind::Call,
            strike: bad,
        };
        let mut l: Vec<StripLeg> = (0..5)
            .map(|_| StripLeg {
                kind: OptionKind::Put,
                strike: strike(&mut g, s),
            })
            .filter(|x| price(&option(o.expiry, x), &m).is_ok())
            .collect();
        if l.len() < 3 {
            continue;
        }
        let at = (seen.iter().enumerate().min_by_key(|x| x.1).unwrap()).0;
        let i = [0, l.len() / 2, l.len()][at];
        l.insert(i, bad);
        check(o.expiry, &m, &l);
        seen[at] += 1;
    }
}

/// Each `price` / `price_and_greeks` row of the frozen 0.1.0 fixture, priced as one leg of a
/// strip with random other legs around it, must give the fixture's output directly.
#[test]
fn test_strip_legs_reproduce_the_frozen_0_1_0_outputs() {
    let mut g = Rng(4);
    let mut rows = 0;
    for line in FIXTURE.lines() {
        let greeks = line.starts_with("price_and_greeks,");
        if !greeks && !line.starts_with("price,") {
            continue;
        }
        let (inputs, expected) = line.split_once(",=,").unwrap();
        let a: Vec<i128> = inputs
            .split(',')
            .skip(1)
            .map(|x| x.parse().unwrap())
            .collect();
        let fx = |v: i128| Fixed::from_raw(v);
        let expiry = Years::try_new(fx(a[3])).unwrap();
        let m = MarketInputs {
            spot: Spot::try_new(fx(a[1])).unwrap(),
            vol: Volatility::try_new(fx(a[4])).unwrap(),
            rate: Rate::try_new(fx(a[5])).unwrap(),
            carry: Rate::try_new(fx(a[6])).unwrap(),
        };
        let leg = StripLeg {
            kind: if a[0] == 0 {
                OptionKind::Call
            } else {
                OptionKind::Put
            },
            strike: Strike::try_new(fx(a[2])).unwrap(),
        };
        // Other legs before it that price successfully, so the fixture leg is reached.
        let mut l: Vec<StripLeg> = legs(&mut g, a[1])
            .into_iter()
            .filter(|x| price(&option(expiry, x), &m).is_ok())
            .collect();
        let at = (g.next() % (l.len() as u64 + 1)) as usize;
        l.insert(at, leg);
        let got = if greeks {
            let mut out = vec![sentinel_priced(); l.len()];
            match price_and_greeks_strip(expiry, &m, &l, &mut out) {
                Ok(()) => {
                    let p = out[at];
                    [
                        p.price.get(),
                        p.greeks.delta,
                        p.greeks.gamma,
                        p.greeks.vega,
                        p.greeks.theta,
                    ]
                    .iter()
                    .map(|v| v.raw().to_string())
                    .collect::<Vec<_>>()
                    .join(",")
                }
                Err(e) => format!("E{}", e as u8),
            }
        } else {
            let mut out = vec![sentinel_price(); l.len()];
            match price_strip(expiry, &m, &l, &mut out) {
                Ok(()) => out[at].get().raw().to_string(),
                Err(e) => format!("E{}", e as u8),
            }
        };
        assert_eq!(got, expected, "{line}");
        rows += 1;
    }
    assert_eq!(rows, 4000);
}
