//! Milestone 13 (`docs/PLAN.md` section 8): binary prices against `tests/data/binary_fixture.csv`,
//! exact cash-or-nothing and asset-or-nothing prices computed at 100 digits by mpmath
//! (`tools/binary_fixture/gen_binary_fixture.py`) on the plan's boundary sets: `d2` or `d1` in
//! `{−9, −5, −1, −1/4, 0, 1/4, 1, 5, 9}` at 26 expiries from `10^-12` s to 30 years, plus
//! at-the-money strikes, both sides of the `d1` switch and `T = 0`. Every row must be within
//! `BINARY_ERR_REL × scale + BINARY_ERR_ABS` (`scale = payout` or `S`).

#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::cast_precision_loss,
    clippy::disallowed_macros,
    missing_docs
)]

use optfix::{
    price_asset_or_nothing, price_cash_or_nothing, EuropeanOption, Fixed, MarketInputs, OptionKind,
    Price, Rate, Spot, Strike, Volatility, Years, BINARY_ERR_ABS, BINARY_ERR_REL,
};

const FIXTURE: &str = include_str!("data/binary_fixture.csv");

/// `REL × scale + ABS` in raw units, rounded up (`REL` is `2^-48`, so `scale × REL` is
/// `scale >> 48` up to one unit).
fn bound(scale: i128) -> i128 {
    let rel_shift = 64 - (BINARY_ERR_REL.raw().trailing_zeros() as i128);
    assert_eq!(
        BINARY_ERR_REL.raw().count_ones(),
        1,
        "REL is a power of two"
    );
    (scale >> rel_shift) + 1 + BINARY_ERR_ABS.raw()
}

#[test]
fn test_binary_prices_match_the_mpmath_fixture() {
    let (mut rows, mut worst_cash, mut worst_asset) = (0usize, 0f64, 0f64);
    for line in FIXTURE.lines() {
        let v: Vec<i128> = line.split(',').map(|x| x.parse().unwrap()).collect();
        let fx = |r: i128| Fixed::from_raw(r);
        let o = EuropeanOption {
            kind: if v[0] == 0 {
                OptionKind::Call
            } else {
                OptionKind::Put
            },
            strike: Strike::try_new(fx(v[2])).unwrap(),
            expiry: Years::try_new(fx(v[3])).unwrap(),
        };
        let m = MarketInputs {
            spot: Spot::try_new(fx(v[1])).unwrap(),
            vol: Volatility::try_new(fx(v[4])).unwrap(),
            rate: Rate::try_new(fx(v[5])).unwrap(),
            carry: Rate::try_new(fx(v[6])).unwrap(),
        };
        let pay = Price::try_new(fx(v[7])).unwrap();
        let cash = price_cash_or_nothing(&o, &m, pay).unwrap().get().raw();
        let asset = price_asset_or_nothing(&o, &m).unwrap().get().raw();
        let (ec, ea) = ((cash - v[8]).abs(), (asset - v[9]).abs());
        let (bc, ba) = (bound(v[7]), bound(v[1]));
        assert!(
            ec <= bc,
            "cash-or-nothing {line}: ours {cash}, error {ec} > {bc}"
        );
        assert!(
            ea <= ba,
            "asset-or-nothing {line}: ours {asset}, error {ea} > {ba}"
        );
        worst_cash = worst_cash.max(ec as f64 / bc as f64);
        worst_asset = worst_asset.max(ea as f64 / ba as f64);
        rows += 1;
    }
    println!("{rows} rows; worst cash {worst_cash:.3e}, asset {worst_asset:.3e} of the bound");
    assert!(rows >= 30_000, "{rows} rows");
}
