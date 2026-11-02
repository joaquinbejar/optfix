//! The published contracts of the pricing families and of the implied-volatility round trip, as
//! checks shared by the property tests and the fuzz targets (`docs/PLAN-improvements.md`,
//! milestone 16). This file depends on `optfix` and `std` only, so `fuzz/` includes it by
//! `#[path]` and both run one definition.
//!
//! Bounds that need `Dr = e^(−rT)` or `Dq = e^(−qT)` evaluate them exactly enough: on
//! `num-bigint` with 192 fractional bits (relative error below `2^-110` over the domain, where
//! `|rT|, |qT| <= 15`), comparing in units of `2^-128` with a margin of [`MARGIN_RAW`] raw units.

use num_bigint::BigInt;
use optfix::{
    price, Fixed, MarketInputs, OptionKind, Priced, PricingError, Volatility, BINARY_ERR_ABS,
    BINARY_ERR_REL, IV_ERR_PRICE_FACTOR, IV_ERR_REL, IV_VEGA_MIN_FACTOR, PRICE_ERR_ABS,
    PRICE_ERR_REL,
};
use optfix::{EuropeanOption, Price};

const ONE: f64 = 18_446_744_073_709_551_616.0;

/// Raw units (`2^-64`) allowed for the reference's own rounding on top of an envelope.
pub const MARGIN_RAW: i64 = 2;

/// `x` as an `f64`.
pub fn f(x: Fixed) -> f64 {
    x.raw() as f64 / ONE
}

/// A raw `Fixed` in units of `2^-128`.
fn w(raw: i128) -> BigInt {
    BigInt::from(raw) << 64
}

/// `a × b` for two values in units of `2^-128`, truncated.
fn wmul(a: &BigInt, b: &BigInt) -> BigInt {
    (a * b) >> 128
}

/// `e^x` for `x` in units of `2^-128` (`|x| <= 16`), in units of `2^-128`: Taylor series of
/// `x / 2^12` at 192 fractional bits, squared 12 times.
pub fn exp_w(x: &BigInt) -> BigInt {
    let x192: BigInt = x.clone() << 64;
    let y: BigInt = x192 >> 12;
    let one: BigInt = BigInt::from(1) << 192;
    let (mut sum, mut term) = (one.clone(), one);
    for n in 1u32..40 {
        term = ((term * &y) >> 192) / n;
        sum += &term;
    }
    for _ in 0..12 {
        sum = (&sum * &sum) >> 192;
    }
    sum >> 64
}

/// `(Dr, Dq)` in units of `2^-128`, from the exact products `rT` and `qT`.
pub fn discounts(opt: &EuropeanOption, m: &MarketInputs) -> (BigInt, BigInt) {
    let t = BigInt::from(opt.expiry.get().raw());
    let rt = BigInt::from(m.rate.get().raw()) * &t;
    let qt = BigInt::from(m.carry.get().raw()) * &t;
    (exp_w(&-rt), exp_w(&-qt))
}

/// `v` (units of `2^-128`) as an `f64`, for messages.
fn wf(v: &BigInt) -> f64 {
    let raw = i128::try_from(v >> 64).unwrap_or(i128::MAX);
    raw as f64 / ONE
}

/// `S/K` in `[0.05, 20]`, exactly as documented (the pricing functions' `DomainError` check).
pub fn moneyness_ok(s: Fixed, k: Fixed) -> bool {
    // 20 K >= S and 20 S >= K, in integers.
    let (s, k) = (s.raw(), k.raw());
    k.checked_mul(20).is_none_or(|k20| k20 >= s) && s.checked_mul(20).is_none_or(|s20| s20 >= k)
}

/// `rel × notional + abs` in units of `2^-128`, plus [`MARGIN_RAW`].
fn env_w(rel: Fixed, notional: Fixed, abs: Fixed) -> BigInt {
    BigInt::from(rel.raw()) * BigInt::from(notional.raw()) + w(abs.raw()) + w(MARGIN_RAW.into())
}

/// Checks `lo − e <= v <= hi + e`, all in units of `2^-128`.
fn within(what: &str, v: Fixed, lo: &BigInt, hi: &BigInt, e: &BigInt) -> Result<(), String> {
    let vw = w(v.raw());
    if vw < lo - e || vw > hi + e {
        return Err(format!(
            "{what} {:e} outside [{:e}, {:e}] ± {:e}",
            f(v),
            wf(lo),
            wf(hi),
            wf(e)
        ));
    }
    Ok(())
}

/// A vanilla premium within `[max(0, S Dq − K Dr) − E, S Dq + E]` for a call and
/// `[max(0, K Dr − S Dq) − E, K Dr + E]` for a put, `E = PRICE_ERR_REL × max(S, K) +
/// PRICE_ERR_ABS`.
pub fn vanilla_bounds(opt: &EuropeanOption, m: &MarketInputs, p: Price) -> Result<(), String> {
    let (dr, dq) = discounts(opt, m);
    let (s, k) = (m.spot.get(), opt.strike.get());
    let sdq = wmul(&w(s.raw()), &dq);
    let kdr = wmul(&w(k.raw()), &dr);
    let zero = BigInt::from(0);
    let (lo, hi) = match opt.kind {
        OptionKind::Call => ((&sdq - &kdr).max(zero), sdq),
        OptionKind::Put => ((&kdr - &sdq).max(zero), kdr),
    };
    within(
        "vanilla",
        p.get(),
        &lo,
        &hi,
        &env_w(PRICE_ERR_REL, s.max(k), PRICE_ERR_ABS),
    )
}

/// Whether a binary is strictly in the money at expiry.
pub fn strictly_itm(opt: &EuropeanOption, m: &MarketInputs) -> bool {
    match opt.kind {
        OptionKind::Call => m.spot.get() > opt.strike.get(),
        OptionKind::Put => m.spot.get() < opt.strike.get(),
    }
}

/// A cash-or-nothing premium: at `T = 0` exactly `payout` strictly in the money and zero
/// otherwise; for `T > 0` within `[0, payout Dr + Eb]`, `Eb = BINARY_ERR_REL × payout +
/// BINARY_ERR_ABS` (the payout undiscounted, independent of `S` and `K`).
pub fn cash_bounds(
    opt: &EuropeanOption,
    m: &MarketInputs,
    payout: Price,
    p: Price,
) -> Result<(), String> {
    if opt.expiry.get() == Fixed::ZERO {
        let want = if strictly_itm(opt, m) {
            payout
        } else {
            Price::MIN
        };
        return if p == want {
            Ok(())
        } else {
            Err(format!("cash at T = 0: {p:?}, want {want:?}"))
        };
    }
    let (dr, _) = discounts(opt, m);
    let hi = wmul(&w(payout.get().raw()), &dr);
    let e = env_w(BINARY_ERR_REL, payout.get(), BINARY_ERR_ABS);
    within("cash", p.get(), &BigInt::from(0), &hi, &e)
}

/// For `T > 0`: cash call + cash put within `2 Eb` (plus two raw roundings) of `payout Dr`.
pub fn cash_parity(
    opt: &EuropeanOption,
    m: &MarketInputs,
    payout: Price,
    call: Price,
    put: Price,
) -> Result<(), String> {
    let (dr, _) = discounts(opt, m);
    let want = wmul(&w(payout.get().raw()), &dr);
    let sum = call
        .get()
        .checked_add(put.get())
        .map_err(|e| format!("call + put: {e:?}"))?;
    let e = env_w(BINARY_ERR_REL, payout.get(), BINARY_ERR_ABS) * 2 + w(2);
    within("cash call + put", sum, &want, &want, &e)
}

/// An asset-or-nothing premium: at `T = 0` exactly `S` strictly in the money and zero
/// otherwise; for `T > 0` within `[0, S Dq + Ea]`, `Ea = BINARY_ERR_REL × S + BINARY_ERR_ABS`.
pub fn asset_bounds(opt: &EuropeanOption, m: &MarketInputs, p: Price) -> Result<(), String> {
    if opt.expiry.get() == Fixed::ZERO {
        let want = if strictly_itm(opt, m) {
            m.spot.get()
        } else {
            Fixed::ZERO
        };
        return if p.get() == want {
            Ok(())
        } else {
            Err(format!("asset at T = 0: {p:?}, want {want:?}"))
        };
    }
    let (_, dq) = discounts(opt, m);
    let hi = wmul(&w(m.spot.get().raw()), &dq);
    let e = env_w(BINARY_ERR_REL, m.spot.get(), BINARY_ERR_ABS);
    within("asset", p.get(), &BigInt::from(0), &hi, &e)
}

/// For `T > 0` and a call: asset call − `K` × (unit-payout cash call, `mul_down`) against the
/// vanilla call, within the asset envelope, `K` times the cash envelope at unit payout, the
/// price envelope and four raw roundings, computed exactly in raw units (the bound of
/// `binary_props.rs`, `test_identities`).
pub fn binary_identity(
    opt: &EuropeanOption,
    m: &MarketInputs,
    asset_call: Price,
    unit_cash_call: Price,
    vanilla_call: Price,
) -> Result<(), String> {
    let k = opt.strike.get();
    let k_cu = k
        .mul_down(unit_cash_call.get())
        .map_err(|e| format!("K × cash: {e:?}"))?;
    let lhs = asset_call
        .get()
        .raw()
        .checked_sub(k_cu.raw())
        .ok_or("asset − K cash overflows")?;
    let err = lhs
        .checked_sub(vanilla_call.get().raw())
        .ok_or("identity difference overflows")?
        .unsigned_abs();
    let (s_raw, k_raw) = (m.spot.get().raw().unsigned_abs(), k.raw().unsigned_abs());
    let rel = BINARY_ERR_REL.raw().unsigned_abs();
    let abs = BINARY_ERR_ABS.raw().unsigned_abs();
    // ceil(x × y / 2^64) for x < 2^95 and y < 2^33: the product fits a u128.
    let mul_up = |x: u128, y: u128| (x * y).div_ceil(1 << 64);
    let price_env = mul_up(s_raw.max(k_raw), PRICE_ERR_REL.raw().unsigned_abs())
        + PRICE_ERR_ABS.raw().unsigned_abs();
    let bound = mul_up(s_raw, rel) + abs + mul_up(k_raw, rel + abs) + price_env + 4;
    if err > bound {
        return Err(format!("asset − K cash − vanilla = {err} raw > {bound}"));
    }
    Ok(())
}

/// `IV_ERR_PRICE_FACTOR × E / vega + IV_ERR_REL × σ`.
pub fn envelope(e: Fixed, vega: Fixed, sigma: Fixed) -> f64 {
    f(IV_ERR_PRICE_FACTOR) * f(e) / f(vega) + f(IV_ERR_REL) * f(sigma)
}

/// Vega against the conditioning threshold `thr = IV_VEGA_MIN_FACTOR × E`: `Some(true)` at
/// `thr (1 + 2^-8)` or above (`implied_vol` must answer; it widens `thr` by far less, `δ`),
/// `Some(false)` below `thr (1 − lower)` (it must not), `None` in between. `lower` is 0 where
/// `vega` is the exact vega at the exact root of the target (the mpmath fixtures), and covers
/// the distance to that root where it is the vega at the `σ` that priced the target.
pub fn conditioned(vega: Fixed, e: Fixed, lower: f64) -> Option<bool> {
    // Exact in integers: thr = E × 2^28 (IV_VEGA_MIN_FACTOR is 2^28).
    let thr_raw = e.raw() << 28;
    assert_eq!(IV_VEGA_MIN_FACTOR.raw(), 1 << 92);
    let v = vega.raw();
    if v as f64 >= thr_raw as f64 * (1.0 + 1.0 / 256.0) {
        Some(true)
    } else if lower == 0.0 && v < thr_raw || (v as f64) < thr_raw as f64 * (1.0 - lower) {
        Some(false)
    } else {
        None
    }
}

/// The grid's lower band: the vega at `σ` against the vega at the root of `price(σ)`, which is
/// within `E / vega` of `σ`; `d ln vega / d ln σ = d1 d2` is below 50 here, so the two differ by
/// under `50 × 2^-28 / 0.005`, about `2^-14.7`, at the threshold.
pub const GRID_LOWER: f64 = 1.0 / 4096.0;

/// What a round trip returned, once checked against the contract.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IvOutcome {
    /// `T = 0`: `DomainError`.
    Expired,
    /// The out-of-the-money value rounded to zero: the target is a bound, `ArbitrageBound`.
    NoTimeValue,
    /// `Ok`, with its `σ` error over the envelope, its price round trip error over that
    /// envelope, and its relative `σ` error.
    Answered {
        sigma_ratio: f64,
        price_ratio: f64,
        rel: f64,
    },
    /// `IllConditioned`; `band` when vega was within the uncertainty band of the threshold.
    Ill { band: bool },
    /// `DomainError` because the root is beyond a `Volatility` range end by more than its
    /// envelope.
    Domain,
}

/// Checks `got = implied_vol(opt, m without σ, priced.price)`, where `priced =
/// price_and_greeks(opt, m)` for a valid `σ = m.vol`, against the round trip contract
/// (`docs/PLAN-improvements.md`, milestone 16; `iv_round_trip.rs`).
pub fn iv_round_trip(
    opt: &EuropeanOption,
    m: &MarketInputs,
    priced: &Priced,
    got: Result<Volatility, PricingError>,
) -> Result<IvOutcome, String> {
    if opt.expiry.get() == Fixed::ZERO {
        return if got == Err(PricingError::DomainError) {
            Ok(IvOutcome::Expired)
        } else {
            Err(format!("T = 0 returned {got:?}"))
        };
    }
    let e = optfix::price_error_bound(opt, m).map_err(|e| format!("envelope: {e:?}"))?;
    let vega = priced.greeks.vega;
    let cond = conditioned(vega, e, GRID_LOWER);
    let sigma = m.vol.get();
    match got {
        Err(PricingError::ArbitrageBound) => {
            // The out-of-the-money value rounded to zero: the target is a bound.
            if cond == Some(true) {
                return Err("ArbitrageBound at a well-conditioned point".into());
            }
            Ok(IvOutcome::NoTimeValue)
        }
        Ok(iv) => {
            if cond == Some(false) {
                return Err(format!("a value below the threshold: {iv:?}"));
            }
            let err = f(iv.get().checked_sub(sigma).map_err(|e| format!("{e:?}"))?).abs();
            let sigma_ratio = err / envelope(e, vega, sigma);
            if sigma_ratio > 1.0 {
                return Err(format!(
                    "round trip at {sigma_ratio:.3} of its envelope: {iv:?}"
                ));
            }
            // price → implied vol → price
            let back = price(opt, &MarketInputs { vol: iv, ..*m })
                .map_err(|e| format!("price at the root: {e:?}"))?;
            let perr = f(back
                .get()
                .checked_sub(priced.price.get())
                .map_err(|e| format!("{e:?}"))?)
            .abs();
            let pallowed =
                (f(IV_ERR_PRICE_FACTOR) + 1.0) * f(e) + f(IV_ERR_REL) * f(sigma) * f(vega);
            let price_ratio = perr / pallowed;
            if price_ratio > 1.0 {
                return Err(format!(
                    "price round trip at {price_ratio:.3} of its envelope"
                ));
            }
            Ok(IvOutcome::Answered {
                sigma_ratio,
                price_ratio,
                rel: err / f(sigma),
            })
        }
        Err(PricingError::IllConditioned) => {
            if cond == Some(true) {
                return Err("IllConditioned above the threshold".into());
            }
            Ok(IvOutcome::Ill {
                band: cond.is_none(),
            })
        }
        Err(PricingError::DomainError) => {
            // Only a root beyond a range end by more than its envelope; S/K and T were valid.
            let env = envelope(e, vega, sigma);
            let near_end = f(sigma) - env < f(Volatility::MIN.get())
                || f(sigma) + env > f(Volatility::MAX.get());
            if near_end && moneyness_ok(m.spot.get(), opt.strike.get()) {
                Ok(IvOutcome::Domain)
            } else {
                Err(format!(
                    "DomainError at σ = {:e}, envelope {env:e}",
                    f(sigma)
                ))
            }
        }
        Err(e) => Err(format!("{e:?} (conditioning {cond:?})")),
    }
}
