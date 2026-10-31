//! Milestone 12 (`docs/PLAN.md` section 8): Black-76. `price_black76` and
//! `price_and_greeks_black76` equal `price` and `price_and_greeks` with `S = F` and `q = r`,
//! bit for bit, results and errors, over the supported domain (so the 0.1 envelopes apply), and
//! the price agrees with OptionStratLib 0.23's `black_76` within the oracle's price tolerance,
//! `10^-9 × max(F, K)` (PLAN question 1). `OPTFIX_PROPTEST_CASES` and `OPTFIX_ORACLE_SAMPLES`
//! override the default counts (100,000 and 20,000).

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

use common::oracle::{d2f, oracle_sample, ORACLE_PRICE_REL};
use common::{bsm_inputs, bsm_sample, ratio, to_ref, Rng};
use optfix::{
    price, price_and_greeks, price_and_greeks_black76, price_black76, EuropeanOption, Fixed,
    FuturesInputs, MarketInputs, OptionKind, Rate, Spot, Strike, Volatility, Years,
};
use optionstratlib::model::{ExpirationDate, Positive};
use optionstratlib::{OptionStyle, OptionType, Options, Side};
use rust_decimal::Decimal;

fn count(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[test]
fn test_black76_is_bsm_with_carry_equal_to_rate() {
    let mut g = Rng(0x0F7F_1C5E_ED00_0012);
    for i in 0..count("OPTFIX_PROPTEST_CASES", 100_000) {
        let mut p = bsm_sample(&mut g);
        // A few strikes with F/K outside [0.05, 20], for the error path.
        if i % 50 == 0 {
            p.1 = (p.0 / 21).max(Strike::MIN.get().raw());
        }
        let Ok(strike) = Strike::try_new(Fixed::from_raw(p.1)) else {
            continue;
        };
        let (mut o, m) = bsm_inputs((p.0, p.0, p.2, p.3, p.4, p.5, p.6));
        o.strike = strike;
        let f = FuturesInputs {
            forward: m.spot,
            vol: m.vol,
            rate: m.rate,
        };
        let bsm = MarketInputs { carry: m.rate, ..m };
        assert_eq!(f.as_market(), bsm);
        assert_eq!(price_black76(&o, &f), price(&o, &bsm), "{o:?} {f:?}");
        assert_eq!(
            price_and_greeks_black76(&o, &f),
            price_and_greeks(&o, &bsm),
            "{o:?} {f:?}"
        );
    }
}

/// `black_76` takes the futures price as `underlying_price` and no carry.
fn oracle_option(
    f: Decimal,
    k: Decimal,
    t: Decimal,
    sigma: Decimal,
    r: Decimal,
    call: bool,
) -> Options {
    let pos = |d: Decimal| Positive::new_decimal(d).unwrap();
    Options::new(
        OptionType::European,
        Side::Long,
        "F".to_string(),
        pos(k),
        ExpirationDate::Days(pos(t * Decimal::from(365))),
        pos(sigma),
        pos(Decimal::ONE),
        pos(f),
        r,
        if call {
            OptionStyle::Call
        } else {
            OptionStyle::Put
        },
        Positive::ZERO,
        None,
    )
}

#[test]
fn test_black76_price_matches_the_oracle() {
    use optionstratlib::pricing::black_76::black_76;
    let mut g = Rng(0x0F7F_1C5E_B076_0000);
    let mut worst = (0.0f64, String::new());
    for _ in 0..count("OPTFIX_ORACLE_SAMPLES", 20_000) {
        // The oracle sampler's spot plays the futures price; its carry is unused (q = r).
        let (f, k, t, sigma, r, _, call) = oracle_sample(&mut g, 0);
        let opt = EuropeanOption {
            kind: if call {
                OptionKind::Call
            } else {
                OptionKind::Put
            },
            strike: Strike::try_new(k.fixed()).unwrap(),
            expiry: Years::try_new(t.fixed()).unwrap(),
        };
        let m = FuturesInputs {
            forward: Spot::try_new(f.fixed()).unwrap(),
            vol: Volatility::try_new(sigma.fixed()).unwrap(),
            rate: Rate::try_new(r.fixed()).unwrap(),
        };
        let ours = ratio(&to_ref(price_black76(&opt, &m).unwrap().get().raw()));
        let oracle = d2f(black_76(&oracle_option(
            f.decimal(),
            k.decimal(),
            t.decimal(),
            sigma.decimal(),
            r.decimal(),
            call,
        ))
        .unwrap());
        let scale = f.f64().max(k.f64());
        let rel = (ours - oracle).abs() / (ORACLE_PRICE_REL * scale);
        if rel > worst.0 {
            worst = (
                rel,
                format!(
                    "{:?}",
                    (
                        f.f64(),
                        k.f64(),
                        t.f64(),
                        sigma.f64(),
                        r.f64(),
                        call,
                        ours,
                        oracle
                    )
                ),
            );
        }
    }
    println!(
        "black_76 price: worst {:.3e} of tolerance at {}",
        worst.0, worst.1
    );
    assert!(
        worst.0 <= 1.0,
        "outside the oracle tolerance at {}",
        worst.1
    );
}
