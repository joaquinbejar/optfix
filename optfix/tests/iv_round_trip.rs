//! `implied_vol(price(σ)) ≈ σ` over the milestone 5 grid (`common::bsm_sample`, the whole
//! supported domain) on the three carry sets of PLAN 4.7 (`q > 0`, `q < 0`, `q = 0`), and the
//! documented edges: the `ArbitrageBound` limits with carry, `IllConditioned`, `T = 0`, the
//! `Volatility` range ends and roots beyond them.
//!
//! On the grid, with `E` the price envelope and `thr = IV_VEGA_MIN_FACTOR × E`:
//!
//! - `T = 0`: `DomainError`; no time value (the out-of-the-money price rounds to zero, so the
//!   target is a bound): `ArbitrageBound`;
//! - vega at `σ` above `thr`: `Ok`, within `IV_ERR_PRICE_FACTOR × E / vega + IV_ERR_REL × σ`
//!   of `σ`, and `price(implied_vol(p))` within `(IV_ERR_PRICE_FACTOR + 1) × E +
//!   IV_ERR_REL × σ × vega` of `p`;
//! - vega below `thr`: `IllConditioned`, never a value and never `NoConvergence`;
//! - within `2^-8` of `thr` (where `implied_vol` judges vega at its last evaluation, not at
//!   `σ`): either.
//!
//! The checks live in `common::iv::round_trip`, which `make precision` also runs for
//! `docs/precision.md`. `OPTFIX_IV_SAMPLES` points per carry set (default 100,000);
//! `OPTFIX_IV_OUT=<path>` writes the summary table.

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

use common::iv::{iv_inputs, round_trip_table};
use optfix::{
    exp, implied_vol, price, price_and_greeks, price_error_bound, EuropeanOption, Fixed,
    MarketInputs, OptionKind, Price, PricingError, Rate, Rounding, Spot, Strike, Volatility, Years,
    IV_VEGA_MIN_FACTOR,
};

#[test]
fn test_round_trip_over_the_grid() {
    let n: usize = std::env::var("OPTFIX_IV_SAMPLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100_000);
    let table = round_trip_table(n, 0x1f60);
    println!("{table}");
    if let Ok(path) = std::env::var("OPTFIX_IV_OUT") {
        std::fs::write(path, table).unwrap();
    }
}

fn fx(num: i128, den: i128) -> Fixed {
    Fixed::from_ratio(num, den, Rounding::Down).unwrap()
}

fn option(kind: OptionKind, k: Fixed, t: Fixed) -> EuropeanOption {
    EuropeanOption {
        kind,
        strike: Strike::try_new(k).unwrap(),
        expiry: Years::try_new(t).unwrap(),
    }
}

fn market(s: Fixed, sigma: Fixed, r: Fixed, q: Fixed) -> MarketInputs {
    MarketInputs {
        spot: Spot::try_new(s).unwrap(),
        vol: Volatility::try_new(sigma).unwrap(),
        rate: Rate::try_new(r).unwrap(),
        carry: Rate::try_new(q).unwrap(),
    }
}

/// `(S Dq, K Dr)` as `price` forms them.
fn forwards(s: Fixed, k: Fixed, t: Fixed, r: Fixed, q: Fixed) -> (Fixed, Fixed) {
    let dq = exp(q.mul_down(t).unwrap().checked_neg().unwrap()).unwrap();
    let dr = exp(r.mul_down(t).unwrap().checked_neg().unwrap()).unwrap();
    (s.mul_down(dq).unwrap(), k.mul_down(dr).unwrap())
}

/// The `ArbitrageBound` limits of PLAN 4.6 with carry, for both kinds on both forward sides:
/// `max(0, S Dq − K Dr) < C < S Dq` and `max(0, K Dr − S Dq) < P < K Dr`, strict; checked at
/// the limits, beyond the upper one, and one ulp inside each.
#[test]
fn test_arbitrage_bounds_with_carry() {
    let t = Fixed::ONE;
    for (r, q) in [
        (fx(1, 20), fx(1, 5)),
        (fx(1, 20), fx(-1, 10)),
        (fx(1, 20), Fixed::ZERO),
    ] {
        for (s, k) in [(100, 130), (130, 100)] {
            let (s, k) = (Fixed::from_int(s), Fixed::from_int(k));
            let (s_dq, k_dr) = forwards(s, k, t, r, q);
            let im = iv_inputs(&market(s, fx(1, 5), r, q));
            for (kind, lower, upper) in [
                (
                    OptionKind::Call,
                    s_dq.checked_sub(k_dr).unwrap().max(Fixed::ZERO),
                    s_dq,
                ),
                (
                    OptionKind::Put,
                    k_dr.checked_sub(s_dq).unwrap().max(Fixed::ZERO),
                    k_dr,
                ),
            ] {
                let opt = option(kind, k, t);
                let at = |v: Fixed| implied_vol(&opt, &im, Price::try_new(v).unwrap());
                let ulp = Fixed::from_raw(1);
                assert_eq!(
                    at(lower),
                    Err(PricingError::ArbitrageBound),
                    "{kind:?} lower"
                );
                assert_eq!(
                    at(upper),
                    Err(PricingError::ArbitrageBound),
                    "{kind:?} upper"
                );
                assert_eq!(
                    at(upper.checked_add(Fixed::ONE).unwrap()),
                    Err(PricingError::ArbitrageBound),
                    "{kind:?} beyond"
                );
                for inside in [
                    lower.checked_add(ulp).unwrap(),
                    upper.checked_sub(ulp).unwrap(),
                ] {
                    assert_ne!(
                        at(inside),
                        Err(PricingError::ArbitrageBound),
                        "{kind:?} inside"
                    );
                }
            }
        }
    }
}

/// A deep out-of-the-money quarter-year option (`d1` about −6.9): vega about `1.3 × 10^-9`,
/// below `2^28` times the envelope (`3.6 × 10^-13`), so the target does not determine `σ`.
#[test]
fn test_ill_conditioned_is_rejected() {
    let (s, k, t) = (Fixed::from_int(100), Fixed::from_int(200), fx(1, 4));
    let m = market(s, fx(1, 5), fx(1, 20), Fixed::ZERO);
    let opt = option(OptionKind::Call, k, t);
    let priced = price_and_greeks(&opt, &m).unwrap();
    let e = price_error_bound(&opt, &m).unwrap();
    assert!(priced.price.get() > Fixed::ZERO);
    assert!(priced.greeks.vega < IV_VEGA_MIN_FACTOR.mul_down(e).unwrap());
    for kind in [OptionKind::Call, OptionKind::Put] {
        let opt = option(kind, k, t);
        let p = price(&opt, &m).unwrap();
        assert_eq!(
            implied_vol(&opt, &iv_inputs(&m), p),
            Err(PricingError::IllConditioned),
            "{kind:?}"
        );
    }
}

#[test]
fn test_expiry_moneyness_and_range_edges() {
    let (s, k, t) = (Fixed::from_int(100), Fixed::from_int(100), Fixed::ONE);
    let m = market(s, fx(1, 5), fx(1, 20), fx(1, 50));
    let im = iv_inputs(&m);
    let call = option(OptionKind::Call, k, t);
    let p = price(&call, &m).unwrap();
    // T = 0: the price does not depend on σ.
    let expired = option(OptionKind::Call, k, Fixed::ZERO);
    assert_eq!(
        implied_vol(&expired, &im, p),
        Err(PricingError::DomainError)
    );
    // S/K outside [0.05, 20].
    let far = option(OptionKind::Call, Fixed::from_int(2001), t);
    assert_eq!(implied_vol(&far, &im, p), Err(PricingError::DomainError));
    // The range ends round-trip to themselves, at the money forward (r = q), where vega is
    // largest.
    let atm = market(s, fx(1, 5), fx(1, 20), fx(1, 20));
    let im_atm = iv_inputs(&atm);
    for sigma in [Volatility::MIN, Volatility::MAX] {
        let m = MarketInputs { vol: sigma, ..atm };
        for kind in [OptionKind::Call, OptionKind::Put] {
            let opt = option(kind, k, t);
            let iv = implied_vol(&opt, &im_atm, price(&opt, &m).unwrap()).unwrap();
            let err = (iv.get().raw() - sigma.get().raw()).abs();
            assert!(
                err <= sigma.get().raw() >> 30,
                "{kind:?} at {sigma:?}: {iv:?}"
            );
        }
    }
    // Roots beyond the ends: 80% of the price at σ = 0.005 (about σ = 0.004 at the money), and
    // a price above the one at σ = 5.
    let at = |sigma: Volatility| {
        price(&call, &MarketInputs { vol: sigma, ..atm })
            .unwrap()
            .get()
    };
    let low = Price::try_new(at(Volatility::MIN).mul_down(fx(4, 5)).unwrap()).unwrap();
    assert_eq!(
        implied_vol(&call, &im_atm, low),
        Err(PricingError::DomainError)
    );
    // Halfway from the price at σ = 5 to its bound S Dq.
    let (s_dq, _) = forwards(s, k, t, fx(1, 20), fx(1, 20));
    let top = at(Volatility::MAX);
    let high = Price::try_new(
        top.checked_add(Fixed::from_raw((s_dq.raw() - top.raw()) / 2))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        implied_vol(&call, &im_atm, high),
        Err(PricingError::DomainError)
    );
}
