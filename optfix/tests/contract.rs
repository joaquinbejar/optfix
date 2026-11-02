//! The shared contracts of `common::contract` (`docs/PLAN-improvements.md`, milestone 16) on
//! stable: its exponential against the 256-bit reference of `common`, and its bounds accepting
//! optfix's prices over the milestone 5 grid, so the fuzz targets' checks are themselves
//! tested by `make test`. `OPTFIX_PROPTEST_CASES` overrides the default of 20,000 points.

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

use common::contract::{
    asset_bounds, binary_identity, cash_bounds, cash_parity, exp_w, vanilla_bounds,
};
use common::{bsm_inputs, bsm_sample, exp_ref, Rng, P};
use num_bigint::BigInt;
use optfix::{
    price, price_asset_or_nothing, price_cash_or_nothing, EuropeanOption, Fixed, OptionKind, Price,
};

fn cases() -> usize {
    std::env::var("OPTFIX_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000)
}

/// `exp_w` (units of `2^-128`) within `2^-100` relative of `exp_ref` (`2^-P`) over `[−16, 16]`.
#[test]
fn test_exp_w_matches_the_reference() {
    let mut g = Rng(21);
    for i in 0..2_000 {
        let x = if i == 0 {
            BigInt::from(0)
        } else {
            // A uniform value in [−16, 16] at 128 fractional bits.
            let u = (BigInt::from(g.next()) << 64 | BigInt::from(g.next())) << 4;
            u - (BigInt::from(16) << 128)
        };
        let got = exp_w(&x);
        let want = exp_ref(&(x.clone() << (P - 128))) >> (P - 128);
        let err = (&got - &want).magnitude().clone();
        let bound = want.magnitude() >> 100u32;
        assert!(err <= bound + 1u32, "exp_w({x}) = {got}, reference {want}");
    }
}

/// optfix's vanilla and binary prices satisfy every contract the `pricing` fuzz target checks.
#[test]
fn test_contracts_accept_the_grid() {
    let mut g = Rng(22);
    for _ in 0..cases() {
        let p = bsm_sample(&mut g);
        let (o, m) = bsm_inputs(p);
        let v = price(&o, &m).unwrap();
        vanilla_bounds(&o, &m, v).unwrap_or_else(|why| panic!("{why} at {p:?}"));
        let payout = Price::try_new(Fixed::from_raw(p.0)).unwrap();
        let c = price_cash_or_nothing(&o, &m, payout).unwrap();
        cash_bounds(&o, &m, payout, c).unwrap_or_else(|why| panic!("{why} at {p:?}"));
        let a = price_asset_or_nothing(&o, &m).unwrap();
        asset_bounds(&o, &m, a).unwrap_or_else(|why| panic!("{why} at {p:?}"));
        if p.2 > 0 {
            let call = EuropeanOption {
                kind: OptionKind::Call,
                ..o
            };
            let put = EuropeanOption {
                kind: OptionKind::Put,
                ..o
            };
            let cc = price_cash_or_nothing(&call, &m, payout).unwrap();
            let cp = price_cash_or_nothing(&put, &m, payout).unwrap();
            cash_parity(&o, &m, payout, cc, cp).unwrap_or_else(|why| panic!("{why} at {p:?}"));
            let unit = Price::try_new(Fixed::ONE).unwrap();
            let a = price_asset_or_nothing(&call, &m).unwrap();
            let cu = price_cash_or_nothing(&call, &m, unit).unwrap();
            let vc = price(&call, &m).unwrap();
            binary_identity(&call, &m, a, cu, vc).unwrap_or_else(|why| panic!("{why} at {p:?}"));
        }
    }
}

/// The bounds reject a premium one envelope outside them, so they are not vacuous.
#[test]
fn test_contracts_reject_a_shifted_price() {
    let mut g = Rng(23);
    let mut rejected = 0;
    for _ in 0..1_000 {
        let p = bsm_sample(&mut g);
        let (o, m) = bsm_inputs(p);
        let v = price(&o, &m).unwrap();
        // Twice the envelope above the upper bound S Dq or K Dr, which is at most S × e^3.
        let hi = m.spot.get().max(o.strike.get()).raw() * 21;
        let shifted = Price::try_new(Fixed::from_raw(v.get().raw().max(hi))).unwrap();
        if vanilla_bounds(&o, &m, shifted).is_err() {
            rejected += 1;
        }
    }
    assert_eq!(rejected, 1_000);
}
