//! The pricing families against their own bounds and envelopes (`docs/PLAN-improvements.md`,
//! milestone 16, target 4): vanilla `price`, `price_and_greeks`, Black-76 and strips;
//! cash-or-nothing and asset-or-nothing at `T = 0` and above, with their parity and the
//! asset − `K` cash = vanilla identity; and `price_strip_svi` (`docs/PLAN-phase2.md`,
//! milestone 24) against its contract, `svi_vol` then `price` per leg, on the same legs with a
//! slice from the remaining bytes. Half the inputs are any in-range raw quantities (the
//! whole domain, `DomainError` exactly outside `S/K` in `[0.05, 20]`), half the weighted
//! sampler of `optfix_fuzz::bsm_point`.
#![no_main]

use libfuzzer_sys::fuzz_target;
use optfix::{
    price, price_and_greeks, price_and_greeks_black76, price_and_greeks_strip,
    price_asset_or_nothing, price_black76, price_cash_or_nothing, price_strip, price_strip_svi,
    svi_vol, EuropeanOption, Fixed, FuturesInputs, MarketInputs, OptionKind, Price, PricingError,
    Spot, Strike, StripLeg, SviMarket, SviParams, Years,
};
use optfix_fuzz::contract::{
    asset_bounds, binary_identity, cash_bounds, cash_parity, moneyness_ok, vanilla_bounds,
};
use optfix_fuzz::{bsm_point, inputs, Bytes};

fn or_fail<T>(r: Result<T, String>, what: &str, opt: &EuropeanOption, m: &MarketInputs) -> T {
    r.unwrap_or_else(|why| panic!("{what}: {why} at {opt:?} {m:?}"))
}

fn flip(kind: OptionKind) -> OptionKind {
    match kind {
        OptionKind::Call => OptionKind::Put,
        OptionKind::Put => OptionKind::Call,
    }
}

/// `ONE` in raw units.
const ONE: i128 = 1 << 64;

/// A slice from five raw values. With `scaled`, `a` in `[0, 4 T)` and `b` in `[0, min(T, 1) / 2)`,
/// so most legs' volatilities fall in `[0.005, 5]`; otherwise every parameter anywhere in its
/// range (`try_new` may still refuse a negative minimum).
fn svi_slice(raw: [i128; 5], scaled: bool, t: Fixed) -> Option<SviParams> {
    let [a, b, rho, m, sigma] = raw;
    let sigma_min = 18_446_744_073_709;
    let rho = Fixed::from_raw(rho % ONE);
    let m = Fixed::from_raw(m % (10 * ONE + 1));
    let sigma = Fixed::from_raw(sigma_min + sigma.rem_euclid(10 * ONE - sigma_min + 1));
    let (a, b) = if scaled {
        let short = t.min(Fixed::ONE);
        (
            t.mul_down(Fixed::from_raw(a.rem_euclid(4 * ONE))).ok()?,
            short
                .mul_down(Fixed::from_raw(b.rem_euclid(ONE / 2)))
                .ok()?,
        )
    } else {
        (
            Fixed::from_raw(a % (1000 * ONE + 1)),
            Fixed::from_raw(b.rem_euclid(10 * ONE + 1)),
        )
    };
    SviParams::try_new(a, b, rho, m, sigma).ok()
}

/// `price_strip_svi` against `svi_vol` then `price` for each leg, with the partial-output
/// contract.
fn check_svi_strip(slice: &SviParams, t: Years, m: &SviMarket, legs: &[StripLeg]) {
    let sentinel = Price::try_new(Fixed::from_raw(7)).expect("7 is a price");
    let mut out = [sentinel; 3];
    let r = price_strip_svi(t, m, slice, legs, &mut out);
    if t.get() == Fixed::ZERO {
        assert_eq!(r, Err(PricingError::DomainError), "T = 0");
        assert!(out.iter().all(|p| *p == sentinel), "written at T = 0");
        return;
    }
    let mut failed = false;
    for (i, leg) in legs.iter().enumerate() {
        if failed {
            assert_eq!(out[i], sentinel, "SVI leg {i} written after an error");
            continue;
        }
        let one = svi_vol(slice, t, m, leg.strike).and_then(|vol| {
            let o = EuropeanOption {
                kind: leg.kind,
                strike: leg.strike,
                expiry: t,
            };
            let mi = MarketInputs {
                spot: m.spot,
                vol,
                rate: m.rate,
                carry: m.carry,
            };
            price(&o, &mi)
        });
        match one {
            Ok(v) => assert_eq!(out[i], v, "SVI strip leg {i} at {slice:?} {t:?} {m:?}"),
            Err(e) => {
                assert_eq!(
                    r,
                    Err(e),
                    "SVI strip error at leg {i} at {slice:?} {t:?} {m:?}"
                );
                failed = true;
            }
        }
    }
    if !failed {
        assert_eq!(r, Ok(()));
    }
}

fuzz_target!(|data: &[u8]| {
    let mut b = Bytes(data);
    let mode = b.u8();
    let (opt, m) = if mode & 1 == 0 {
        bsm_point(&mut b)
    } else {
        let kind = if mode & 2 == 0 {
            OptionKind::Call
        } else {
            OptionKind::Put
        };
        let raw = [b.i128(), b.i128(), b.i128(), b.i128(), b.i128(), b.i128()];
        match inputs(kind, raw) {
            Some(p) => p,
            None => return,
        }
    };
    let payout = match b.u8() % 4 {
        0 => Fixed::ZERO,
        1 => Fixed::from_raw(1),
        2 => Spot::MAX.get(),
        _ => Fixed::from_raw(i128::from(b.u64()) << (b.u8() % 40)).min(Spot::MAX.get()),
    };
    let payout = Price::try_new(payout).expect("a non-negative payout");
    let second_strike = b.i128();

    let valid = moneyness_ok(m.spot.get(), opt.strike.get());
    let p = price(&opt, &m);
    if !valid {
        assert_eq!(p, Err(PricingError::DomainError), "{opt:?} {m:?}");
        assert_eq!(
            price_cash_or_nothing(&opt, &m, payout),
            Err(PricingError::DomainError)
        );
        assert_eq!(
            price_asset_or_nothing(&opt, &m),
            Err(PricingError::DomainError)
        );
        return;
    }
    let p = p.unwrap_or_else(|e| panic!("price: {e:?} at {opt:?} {m:?}"));
    or_fail(vanilla_bounds(&opt, &m, p), "price", &opt, &m);
    let pg = price_and_greeks(&opt, &m).unwrap_or_else(|e| panic!("price_and_greeks: {e:?}"));
    assert_eq!(pg.price, p, "price_and_greeks' price is price's");

    // Black-76 with F = S and r: bit-identical to BSM with q = r, except rho (0.3), which
    // holds F fixed: exactly −(T ⊗ price).
    let fut = FuturesInputs {
        forward: m.spot,
        vol: m.vol,
        rate: m.rate,
    };
    let bsm_r = MarketInputs { carry: m.rate, ..m };
    assert_eq!(price_black76(&opt, &fut), price(&opt, &bsm_r));
    match (
        price_and_greeks_black76(&opt, &fut),
        price_and_greeks(&opt, &bsm_r),
    ) {
        (Ok(b76), Ok(mut same)) => {
            let rho = opt
                .expiry
                .get()
                .mul_down(b76.price.get())
                .and_then(Fixed::checked_neg);
            assert_eq!(Ok(b76.greeks.rho), rho, "Black-76 rho");
            same.greeks.rho = b76.greeks.rho;
            assert_eq!(b76, same);
        }
        (b76, same) => assert_eq!(b76, same),
    }

    // A strip of this leg, its flip and a second strike (any raw value: it may be out of the
    // domain, then the earlier legs are written and the later untouched).
    let legs = [
        StripLeg {
            kind: opt.kind,
            strike: opt.strike,
        },
        StripLeg {
            kind: flip(opt.kind),
            strike: opt.strike,
        },
        StripLeg {
            kind: opt.kind,
            strike: Strike::try_new(Fixed::from_raw(second_strike)).unwrap_or(opt.strike),
        },
    ];
    let sentinel = Price::try_new(Fixed::from_raw(7)).expect("7 is a price");
    let mut out = [sentinel; 3];
    // `Priced` is `#[non_exhaustive]` (0.3): a sentinel from a computed value.
    let mut sentinel_g = pg;
    sentinel_g.price = sentinel;
    let mut out_g = [sentinel_g; 3];
    let r = price_strip(opt.expiry, &m, &legs, &mut out);
    let rg = price_and_greeks_strip(opt.expiry, &m, &legs, &mut out_g);
    let mut failed = false;
    for (i, leg) in legs.iter().enumerate() {
        let o = EuropeanOption {
            kind: leg.kind,
            strike: leg.strike,
            expiry: opt.expiry,
        };
        let (one, one_g) = (price(&o, &m), price_and_greeks(&o, &m));
        if failed {
            assert_eq!(out[i], sentinel, "leg {i} written after an error");
            assert_eq!(out_g[i].price, sentinel, "leg {i} written after an error");
            continue;
        }
        match one {
            Ok(v) => {
                assert_eq!(out[i], v, "strip leg {i}");
                assert_eq!(Ok(out_g[i]), one_g, "greeks strip leg {i}");
            }
            Err(e) => {
                assert_eq!(r, Err(e), "strip error at leg {i}");
                assert_eq!(rg, Err(e), "greeks strip error at leg {i}");
                failed = true;
            }
        }
    }
    if !failed {
        assert_eq!(r, Ok(()));
        assert_eq!(rg, Ok(()));
    }

    // Binaries.
    let cash = price_cash_or_nothing(&opt, &m, payout)
        .unwrap_or_else(|e| panic!("cash: {e:?} at {opt:?} {m:?} {payout:?}"));
    or_fail(cash_bounds(&opt, &m, payout, cash), "cash", &opt, &m);
    let asset = price_asset_or_nothing(&opt, &m).unwrap_or_else(|e| panic!("asset: {e:?}"));
    or_fail(asset_bounds(&opt, &m, asset), "asset", &opt, &m);
    let other = EuropeanOption {
        kind: flip(opt.kind),
        ..opt
    };
    let cash_other = price_cash_or_nothing(&other, &m, payout).expect("same domain");
    or_fail(
        cash_bounds(&other, &m, payout, cash_other),
        "cash (flipped)",
        &opt,
        &m,
    );
    if opt.expiry.get() > Fixed::ZERO {
        let (call, put) = match opt.kind {
            OptionKind::Call => (cash, cash_other),
            OptionKind::Put => (cash_other, cash),
        };
        or_fail(
            cash_parity(&opt, &m, payout, call, put),
            "cash parity",
            &opt,
            &m,
        );
        let c = EuropeanOption {
            kind: OptionKind::Call,
            ..opt
        };
        let unit = Price::try_new(Fixed::ONE).expect("1 is a price");
        let a = price_asset_or_nothing(&c, &m).expect("same domain");
        let cu = price_cash_or_nothing(&c, &m, unit).expect("same domain");
        let v = price(&c, &m).expect("same domain");
        or_fail(
            binary_identity(&c, &m, a, cu, v),
            "binary identity",
            &opt,
            &m,
        );
    }
    // A payout above Spot::MAX is a DomainError.
    let too_big = Price::try_new(Fixed::from_raw(Spot::MAX.get().raw() + 1)).expect("a price");
    assert_eq!(
        price_cash_or_nothing(&opt, &m, too_big),
        Err(PricingError::DomainError)
    );

    // The same legs from an SVI slice (read last, so the earlier bytes keep their meaning).
    let scaled = b.u8() & 1 == 0;
    let raw = [b.i128(), b.i128(), b.i128(), b.i128(), b.i128()];
    if let Some(slice) = svi_slice(raw, scaled, opt.expiry.get()) {
        let sm = SviMarket {
            spot: m.spot,
            rate: m.rate,
            carry: m.carry,
        };
        check_svi_strip(&slice, opt.expiry, &sm, &legs);
    }
});
