//! Milestone 24 (`docs/PLAN-phase2.md`): `price_strip_svi` against its contract, `svi_vol` then
//! `price` per leg, bit for bit.
//!
//! - Random strips (1 to 16 legs) over the markets and slices of `common::svi`: every output
//!   equals the composition, and on the first failing leg the strip returns the composition's
//!   error with `out[..i]` written and `out[i..]` untouched. Some strikes are drawn just
//!   outside `S/K` in `[0.05, 20]`, and short expiries push volatilities outside
//!   `[0.005, 5]`, so both failure paths occur.
//! - A flat slice (`b = 0`) prices every leg as `price_strip` does at that one volatility.
//! - Named cases: a volatility above 5 partway through a strip, an overflowing `w / T` at the
//!   first leg, a length mismatch, `T = 0` and the empty strip.
//!
//! Cases: 20,000 strips (`OPTFIX_PROPTEST_CASES` overrides).

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::float_arithmetic,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::disallowed_macros,
    missing_docs
)]

mod common;

use common::svi::{market, market_sample, market_slice, params};
use common::{Rng, ONE};
use optfix::{
    price, price_strip, price_strip_svi, svi_vol, EuropeanOption, Fixed, MarketInputs, OptionKind,
    Price, PricingError, Rate, Spot, Strike, StripLeg, SviMarket, SviParams, Volatility, Years,
};

fn cases(default: usize) -> usize {
    std::env::var("OPTFIX_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// A value no leg returns here, to see which slots a strip wrote.
fn sentinel() -> Price {
    Price::try_new(Fixed::from_int(777_777_777)).unwrap()
}

/// The contract for one leg: `svi_vol`, then `price` at that volatility.
fn composed(
    slice: &SviParams,
    t: Years,
    m: &SviMarket,
    leg: &StripLeg,
) -> Result<Price, PricingError> {
    let vol = svi_vol(slice, t, m, leg.strike)?;
    price(
        &EuropeanOption {
            kind: leg.kind,
            strike: leg.strike,
            expiry: t,
        },
        &MarketInputs {
            spot: m.spot,
            vol,
            rate: m.rate,
            carry: m.carry,
        },
    )
}

/// Runs the strip and checks it against the composition, leg by leg. Returns the index and
/// error of the first failing leg, if any.
fn check_strip(
    slice: &SviParams,
    t: Years,
    m: &SviMarket,
    legs: &[StripLeg],
) -> Option<(usize, PricingError)> {
    let mut out = vec![sentinel(); legs.len()];
    let got = price_strip_svi(t, m, slice, legs, &mut out);
    let mut expected = Vec::with_capacity(legs.len());
    let mut failure = None;
    for (i, leg) in legs.iter().enumerate() {
        match composed(slice, t, m, leg) {
            Ok(p) => expected.push(p),
            Err(e) => {
                failure = Some((i, e));
                break;
            }
        }
    }
    match failure {
        None => assert_eq!(got, Ok(()), "{slice:?} {t:?} {m:?} {legs:?}"),
        Some((_, e)) => assert_eq!(got, Err(e), "{slice:?} {t:?} {m:?} {legs:?}"),
    }
    let written = expected.len();
    assert_eq!(
        &out[..written],
        &expected[..],
        "{slice:?} {t:?} {m:?} {legs:?}"
    );
    assert!(
        out[written..].iter().all(|p| *p == sentinel()),
        "a slot after the failing leg was written: {slice:?} {t:?} {m:?} {legs:?}"
    );
    failure
}

/// A strike for a leg at spot `s` (raw): `ln(K/S)` uniform in `[−3.1, 3.1]`, so about one leg
/// in thirty is outside `S/K` in `[0.05, 20]` (`|ln| > ln 20 ≈ 3.0`), inside `Strike`'s range.
fn strike_for(g: &mut Rng, s: i128) -> Strike {
    let lk = -3.1 + 6.2 * common::uniform(g);
    let k = ((s as f64 / ONE as f64) * lk.exp() * ONE as f64) as i128;
    Strike::try_new(Fixed::from_raw(
        k.clamp(Strike::MIN.get().raw(), Strike::MAX.get().raw()),
    ))
    .unwrap()
}

#[test]
fn prop_price_strip_svi_is_svi_vol_then_price() {
    let n = cases(20_000);
    let mut g = Rng(0x5F1_2400);
    let (mut clean, mut legs_priced) = (0usize, 0usize);
    let (mut domain, mut overflow) = (0usize, 0usize);
    let mut failed_after_first = 0usize;
    for _ in 0..n {
        let (s, _, t, r, q) = market_sample(&mut g);
        let p = market_slice(&mut g, t);
        let Some(slice) = params(&p) else { continue };
        let (t, m, _) = market(s, s, t, r, q);
        let len = 1 + (g.next() % 16) as usize;
        let legs: Vec<StripLeg> = (0..len)
            .map(|_| StripLeg {
                kind: if g.next() & 1 == 0 {
                    OptionKind::Call
                } else {
                    OptionKind::Put
                },
                strike: strike_for(&mut g, s),
            })
            .collect();
        match check_strip(&slice, t, &m, &legs) {
            None => {
                clean += 1;
                legs_priced += len;
            }
            Some((i, e)) => {
                legs_priced += i;
                if i > 0 {
                    failed_after_first += 1;
                }
                match e {
                    PricingError::DomainError => domain += 1,
                    PricingError::Overflow => overflow += 1,
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
    }
    println!(
        "strips: {clean} of {n} priced every leg, {legs_priced} legs priced; failures: \
         {domain} DomainError, {overflow} Overflow, {failed_after_first} after the first leg"
    );
    assert!(clean > n / 4, "too few strips priced in full");
    assert!(
        failed_after_first > n / 100,
        "too few partial outputs exercised"
    );
}

#[test]
fn prop_flat_slice_matches_price_strip() {
    let n = cases(20_000) / 4;
    let mut g = Rng(0x5F1_2410);
    let mut compared = 0usize;
    for _ in 0..n {
        let (s, _, t_raw, r, q) = market_sample(&mut g);
        // w = a everywhere: the volatility sqrt(a / T) is the same for every leg.
        let v0 = 0.01 + 1.5 * common::uniform(&mut g);
        let a = ((v0 * v0) * (t_raw as f64 / ONE as f64) * ONE as f64) as i128;
        let Ok(slice) = SviParams::try_new(
            Fixed::from_raw(a.max(1)),
            Fixed::ZERO,
            Fixed::ZERO,
            Fixed::ZERO,
            Fixed::ONE,
        ) else {
            continue;
        };
        let (t, m, _) = market(s, s, t_raw, r, q);
        let w = slice.total_variance(Fixed::ZERO).unwrap();
        let Ok(vol) = w
            .div_down(t.get())
            .and_then(|v| v.sqrt_down())
            .and_then(Volatility::try_new)
        else {
            continue;
        };
        let legs: Vec<StripLeg> = (0..1 + (g.next() % 16) as usize)
            .map(|_| StripLeg {
                kind: if g.next() & 1 == 0 {
                    OptionKind::Call
                } else {
                    OptionKind::Put
                },
                strike: strike_for(&mut g, s),
            })
            .collect();
        let mut a_out = vec![sentinel(); legs.len()];
        let mut b_out = vec![sentinel(); legs.len()];
        let mi = MarketInputs {
            spot: m.spot,
            vol,
            rate: m.rate,
            carry: m.carry,
        };
        let ra = price_strip_svi(t, &m, &slice, &legs, &mut a_out);
        let rb = price_strip(t, &mi, &legs, &mut b_out);
        assert_eq!(ra, rb, "{slice:?} {t:?} {m:?} {legs:?}");
        assert_eq!(a_out, b_out, "{slice:?} {t:?} {m:?} {legs:?}");
        compared += 1;
    }
    println!("flat slices: {compared} strips compared with price_strip");
    assert!(compared > n / 2);
}

fn fx(num: i128, den: i128) -> Fixed {
    Fixed::from_raw((num << 64) / den)
}

fn leg(kind: OptionKind, k: i64) -> StripLeg {
    StripLeg {
        kind,
        strike: Strike::try_new(Fixed::from_int(k)).unwrap(),
    }
}

fn spx_market() -> SviMarket {
    SviMarket {
        spot: Spot::try_new(Fixed::from_int(100)).unwrap(),
        rate: Rate::try_new(fx(4, 100)).unwrap(),
        carry: Rate::try_new(fx(1, 100)).unwrap(),
    }
}

#[test]
fn test_out_of_range_volatility_partway_through() {
    // A steep left wing at T = 0.01: w(0) = 0.08 (σ ≈ 2.83), but at K = 8 (S/K = 12.5, inside
    // the moneyness range) w ≈ 1.5 and σ ≈ 12 > 5.
    let slice =
        SviParams::try_new(fx(4, 100), fx(4, 10), fx(-5, 10), Fixed::ZERO, fx(1, 10)).unwrap();
    let t = Years::try_new(fx(1, 100)).unwrap();
    let m = spx_market();
    let legs = [
        leg(OptionKind::Call, 100),
        leg(OptionKind::Put, 95),
        leg(OptionKind::Put, 8),
        leg(OptionKind::Call, 105),
    ];
    assert_eq!(
        svi_vol(&slice, t, &m, legs[2].strike),
        Err(PricingError::DomainError)
    );
    assert_eq!(
        check_strip(&slice, t, &m, &legs),
        Some((2, PricingError::DomainError))
    );
}

#[test]
fn test_overflowing_variance_rate_at_first_leg() {
    // T = 2^-64 years and w >= 1: w / T does not fit Fixed.
    let slice =
        SviParams::try_new(Fixed::ONE, fx(1, 10), Fixed::ZERO, Fixed::ZERO, fx(1, 10)).unwrap();
    let t = Years::try_new(Fixed::from_raw(1)).unwrap();
    let legs = [leg(OptionKind::Call, 100), leg(OptionKind::Put, 90)];
    assert_eq!(
        check_strip(&slice, t, &spx_market(), &legs),
        Some((0, PricingError::Overflow))
    );
}

#[test]
fn test_length_mismatch_zero_expiry_and_empty_strip() {
    let slice =
        SviParams::try_new(fx(4, 100), fx(1, 10), fx(-3, 10), Fixed::ZERO, fx(2, 10)).unwrap();
    let m = spx_market();
    let t = Years::try_new(fx(1, 2)).unwrap();
    let legs = [leg(OptionKind::Call, 100), leg(OptionKind::Put, 90)];
    let mut out = [sentinel(); 3];
    assert_eq!(
        price_strip_svi(t, &m, &slice, &legs, &mut out),
        Err(PricingError::DomainError)
    );
    assert!(out.iter().all(|p| *p == sentinel()));
    let mut out = [sentinel(); 2];
    assert_eq!(
        price_strip_svi(Years::MIN, &m, &slice, &legs, &mut out),
        Err(PricingError::DomainError)
    );
    assert!(out.iter().all(|p| *p == sentinel()));
    assert_eq!(price_strip_svi(t, &m, &slice, &[], &mut []), Ok(()));
    assert_eq!(
        price_strip_svi(Years::MIN, &m, &slice, &[], &mut []),
        Err(PricingError::DomainError)
    );
    assert_eq!(check_strip(&slice, t, &m, &legs), None);
}
