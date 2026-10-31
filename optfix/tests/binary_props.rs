//! Milestone 13 (`docs/PLAN.md` section 8): cash-or-nothing and asset-or-nothing prices
//! against the 256-bit reference of `common` (`N(d1)`, `N(d2)`, the discount factors, all
//! independent of the crate's kernels), within `BINARY_ERR_REL × scale + BINARY_ERR_ABS`
//! (`scale = payout` or `S`, undiscounted like the price envelope's `max(S, K)`), over the supported domain with extra weight on tiny `T`,
//! the money, the forward money (`d2 ≈ 0`) and the `d1` switch; the `T = 0` rule; and the
//! identities of the plan. `OPTFIX_PROPTEST_CASES` overrides the default of 100,000 points.

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

use common::{bsm_inputs, bsm_ref, bsm_sample, mul, one, ratio, to_ref, uniform, Rng, P};
use num_bigint::BigInt;
use optfix::{
    price, price_asset_or_nothing, price_cash_or_nothing, EuropeanOption, Fixed, MarketInputs,
    OptionKind, Price, PricingError, Spot, Strike, BINARY_ERR_ABS, BINARY_ERR_REL,
};

fn cases() -> usize {
    std::env::var("OPTFIX_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100_000)
}

/// A point of the domain: `bsm_sample`, then a strike moved to a special place one time in
/// three (at the money, the forward money `d2 ≈ 0`, or the `d1` switch `|S − K| × 256 = K`).
fn sample(g: &mut Rng) -> (i128, i128, i128, i128, i128, i128, bool) {
    let mut p = bsm_sample(g);
    let s = p.0;
    let k = match g.next() % 9 {
        0 => s + g.range(-64, 65),
        1 => s / 255 * 256 + g.range(-4, 5),
        2 => s / 257 * 256 + g.range(-4, 5),
        _ => p.1,
    };
    let lo = Strike::MIN.get().raw().max(s / 20 + 1);
    let hi = Strike::MAX.get().raw().min(s * 20);
    p.1 = k.clamp(lo, hi);
    p
}

fn payout(g: &mut Rng) -> i128 {
    let v = 10f64.powf(-6.0 + 15.0 * uniform(g));
    ((v * (1u128 << 64) as f64) as i128).clamp(Spot::MIN.get().raw(), Spot::MAX.get().raw())
}

/// `x` in Q.P as a raw Q64.64 value's reference.
fn q(raw: i128) -> BigInt {
    to_ref(raw)
}

struct Worst {
    /// Largest `(|err| − ABS) / scale`, the measured relative part.
    rel: f64,
    /// Largest `|err| / (REL × scale + ABS)`: at most 1 passes.
    ratio: f64,
    at: String,
}

impl Worst {
    fn new() -> Self {
        Self {
            rel: 0.0,
            ratio: 0.0,
            at: String::new(),
        }
    }

    fn add(&mut self, ours: i128, exact: &BigInt, scale: &BigInt, what: &str) {
        let err = (q(ours) - exact).magnitude().clone();
        let err = BigInt::from(err);
        let abs = q(BINARY_ERR_ABS.raw());
        let bound = mul(&q(BINARY_ERR_REL.raw()), scale) + &abs;
        let r = ratio(&((&err << P) / bound.max(BigInt::from(1))));
        let rel = if scale > &BigInt::from(0) {
            ratio(&(((&err - &abs).max(BigInt::from(0)) << P) / scale))
        } else {
            0.0
        };
        if rel > self.rel {
            self.rel = rel;
        }
        if r > self.ratio {
            self.ratio = r;
            self.at = what.to_string();
        }
    }
}

#[test]
fn test_binary_prices_against_the_reference() {
    let mut g = Rng(0x0F7F_1C5E_ED00_0013);
    let (mut cash, mut asset) = (Worst::new(), Worst::new());
    for _ in 0..cases() {
        let p = sample(&mut g);
        let (o, m) = bsm_inputs(p);
        let pay = payout(&mut g);
        let rf = bsm_ref(p.6, &q(p.0), &q(p.1), &q(p.2), &q(p.3), &q(p.4), &q(p.5));
        let zero = BigInt::from(0);
        let (cash_exact, asset_exact) = if p.2 == 0 {
            // T = 0: strictly in the money pays.
            let itm = if p.6 { p.0 > p.1 } else { p.0 < p.1 };
            if itm {
                (q(pay), q(p.0))
            } else {
                (zero.clone(), zero)
            }
        } else if p.6 {
            (mul(&mul(&q(pay), &rf.dr), &rf.nd2), mul(&rf.s_dq, &rf.nd1))
        } else {
            (
                mul(&mul(&q(pay), &rf.dr), &(one() - &rf.nd2)),
                mul(&rf.s_dq, &(one() - &rf.nd1)),
            )
        };
        let what = format!("{p:?} payout {pay}");
        let c =
            price_cash_or_nothing(&o, &m, Price::try_new(Fixed::from_raw(pay)).unwrap()).unwrap();
        cash.add(c.get().raw(), &cash_exact, &q(pay), &what);
        let a = price_asset_or_nothing(&o, &m).unwrap();
        asset.add(a.get().raw(), &asset_exact, &q(p.0), &what);
    }
    println!(
        "cash-or-nothing:  measured relative part {:.3e}, worst {:.3e} of the bound at {}",
        cash.rel, cash.ratio, cash.at
    );
    println!(
        "asset-or-nothing: measured relative part {:.3e}, worst {:.3e} of the bound at {}",
        asset.rel, asset.ratio, asset.at
    );
    assert!(cash.ratio <= 1.0, "cash-or-nothing outside its bound");
    assert!(asset.ratio <= 1.0, "asset-or-nothing outside its bound");
}

fn opt(kind: OptionKind, k: i128, t: i128) -> EuropeanOption {
    let (mut o, _) = bsm_inputs((k, k, t, 1 << 60, 0, 0, true));
    o.kind = kind;
    o
}

#[test]
fn test_expiry_zero_pays_only_strictly_in_the_money() {
    let mut g = Rng(13);
    for _ in 0..10_000 {
        let mut p = sample(&mut g);
        p.2 = 0;
        if g.next().is_multiple_of(4) {
            p.1 = p.0;
        }
        let (o, m) = bsm_inputs(p);
        let pay = Price::try_new(Fixed::from_raw(payout(&mut g))).unwrap();
        let itm = if p.6 { p.0 > p.1 } else { p.0 < p.1 };
        let c = price_cash_or_nothing(&o, &m, pay).unwrap();
        let a = price_asset_or_nothing(&o, &m).unwrap();
        if itm {
            assert_eq!(c, pay);
            assert_eq!(a.get(), m.spot.get());
        } else {
            assert_eq!(c, Price::MIN);
            assert_eq!(a, Price::MIN);
        }
        // At the money both sides are worth nothing, so call + put is 0, not payout × Dr.
        if p.0 == p.1 {
            let mut put = o;
            put.kind = match o.kind {
                OptionKind::Call => OptionKind::Put,
                OptionKind::Put => OptionKind::Call,
            };
            assert_eq!(price_cash_or_nothing(&put, &m, pay).unwrap(), Price::MIN);
        }
    }
}

/// For `T > 0`: cash call + cash put = `payout × Dr` and asset call − K × cash call (unit
/// payout) = the vanilla call, each within the sum of the bounds involved.
#[test]
fn test_identities() {
    let mut g = Rng(14);
    let abs = q(BINARY_ERR_ABS.raw());
    let rel = q(BINARY_ERR_REL.raw());
    for _ in 0..cases() / 10 {
        let mut p = sample(&mut g);
        if p.2 == 0 {
            p.2 = 1 << 50;
        }
        let (o, m) = bsm_inputs(p);
        let rf = bsm_ref(true, &q(p.0), &q(p.1), &q(p.2), &q(p.3), &q(p.4), &q(p.5));
        let pay_raw = payout(&mut g);
        let pay = Price::try_new(Fixed::from_raw(pay_raw)).unwrap();
        let (mut call, mut put) = (o, o);
        call.kind = OptionKind::Call;
        put.kind = OptionKind::Put;
        let c = price_cash_or_nothing(&call, &m, pay).unwrap().get().raw();
        let pt = price_cash_or_nothing(&put, &m, pay).unwrap().get().raw();
        let err = BigInt::from((q(c + pt) - mul(&q(pay_raw), &rf.dr)).magnitude().clone());
        let bound = (mul(&rel, &q(pay_raw)) + &abs) * 2 + 2;
        assert!(err <= bound, "cash call + put at {p:?}");

        let unit = Price::try_new(Fixed::ONE).unwrap();
        let a = price_asset_or_nothing(&call, &m).unwrap().get().raw();
        let cu = price_cash_or_nothing(&call, &m, unit).unwrap().get();
        let k_cu = o.strike.get().mul_down(cu).unwrap().raw();
        let vanilla = price(&call, &m).unwrap().get().raw();
        let err = BigInt::from((q(a - k_cu) - q(vanilla)).magnitude().clone());
        // asset bound + K × (cash bound at unit payout) + the price envelope + 2 roundings.
        let k = q(p.1);
        let price_env = mul(&q(optfix::PRICE_ERR_REL.raw()), &q(p.0).max(k.clone()))
            + q(optfix::PRICE_ERR_ABS.raw());
        let bound = mul(&rel, &q(p.0)) + &abs + mul(&k, &(rel.clone() + &abs)) + price_env + 4;
        assert!(err <= bound, "asset call − K cash call vs vanilla at {p:?}");
    }
}

#[test]
fn test_domain_errors() {
    let (o, m) = bsm_inputs(bsm_sample(&mut Rng(15)));
    let too_big = Price::try_new(Spot::MAX.get().checked_add(Fixed::from_raw(1)).unwrap()).unwrap();
    assert_eq!(
        price_cash_or_nothing(&o, &m, too_big),
        Err(PricingError::DomainError)
    );
    let far = opt(OptionKind::Call, Strike::MAX.get().raw(), 1 << 64);
    let m2 = MarketInputs {
        spot: Spot::MIN,
        ..m
    };
    assert_eq!(
        price_asset_or_nothing(&far, &m2),
        Err(PricingError::DomainError)
    );
    assert_eq!(
        price_cash_or_nothing(&far, &m2, Price::try_new(Fixed::ONE).unwrap()),
        Err(PricingError::DomainError)
    );
}

/// OptionStratLib 0.23's `binary_black_scholes` (payout 1, carry `q >= 0` as its dividend yield),
/// within the oracle's price tolerance `10^-9 ×` the scale. Its normal CDF is f64 grade, so
/// this is a cross-check of the formulas and conventions, not of the bounds (those come from
/// the 256-bit reference above and the mpmath fixture).
#[test]
fn test_binary_prices_match_the_oracle() {
    use common::oracle::{d2f, oracle_sample, ORACLE_PRICE_REL};
    use optfix::{Rate, Volatility, Years};
    use optionstratlib::model::types::BinaryType;
    use optionstratlib::model::{ExpirationDate, Positive};
    use optionstratlib::pricing::binary::binary_black_scholes;
    use optionstratlib::{OptionStyle, OptionType, Options, Side};
    let mut g = Rng(0x0F7F_1C5E_B1A2_0000);
    let mut worst = (0.0f64, String::new());
    for i in 0..20_000 {
        let p = oracle_sample(&mut g, if i % 2 == 0 { 1 } else { 0 });
        let (s, k, t, sigma, r, qd, call) = p;
        let o = EuropeanOption {
            kind: if call {
                OptionKind::Call
            } else {
                OptionKind::Put
            },
            strike: Strike::try_new(k.fixed()).unwrap(),
            expiry: Years::try_new(t.fixed()).unwrap(),
        };
        let m = MarketInputs {
            spot: Spot::try_new(s.fixed()).unwrap(),
            vol: Volatility::try_new(sigma.fixed()).unwrap(),
            rate: Rate::try_new(r.fixed()).unwrap(),
            carry: Rate::try_new(qd.fixed()).unwrap(),
        };
        let pos = |d: rust_decimal::Decimal| Positive::new_decimal(d).unwrap();
        for (binary_type, ours, scale) in [
            (
                BinaryType::CashOrNothing,
                price_cash_or_nothing(&o, &m, Price::try_new(Fixed::ONE).unwrap()).unwrap(),
                1.0,
            ),
            (
                BinaryType::AssetOrNothing,
                price_asset_or_nothing(&o, &m).unwrap(),
                s.f64(),
            ),
        ] {
            let option = Options::new(
                OptionType::Binary { binary_type },
                Side::Long,
                "X".to_string(),
                pos(k.decimal()),
                ExpirationDate::Days(pos(t.decimal() * rust_decimal::Decimal::from(365))),
                pos(sigma.decimal()),
                pos(rust_decimal::Decimal::ONE),
                pos(s.decimal()),
                r.decimal(),
                if call {
                    OptionStyle::Call
                } else {
                    OptionStyle::Put
                },
                pos(qd.decimal()),
                None,
            );
            let oracle = d2f(binary_black_scholes(&option).unwrap());
            let ours = ratio(&to_ref(ours.get().raw()));
            let rel = (ours - oracle).abs() / (ORACLE_PRICE_REL * scale);
            if rel > worst.0 {
                worst = (
                    rel,
                    format!("{binary_type:?} {p:?} ours {ours} oracle {oracle}"),
                );
            }
        }
    }
    println!(
        "binary oracle: worst {:.3e} of tolerance at {}",
        worst.0, worst.1
    );
    assert!(
        worst.0 <= 1.0,
        "outside the oracle tolerance at {}",
        worst.1
    );
}
