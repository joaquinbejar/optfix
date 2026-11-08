//! WebAssembly bindings for [`optfix`] (`docs/PLAN-improvements.md`, milestone 17).
//!
//! Every value crosses the boundary exactly: a Q64.64 number as its raw `bigint` (`raw / 2^64`;
//! `fixedFromDecimal` and `fixedToDecimal` convert from and to exact decimal text), never as a
//! JavaScript `number`. An option kind is `"call"` or `"put"`, a rounding `"down"` (toward
//! negative infinity) or `"up"`. A failure throws an [`OptfixError`] carrying the
//! `PricingError` code (`e as u8` in Rust) and name. The functions are optfix's, so their
//! results are bit-identical to the Rust crate's, as `conformance/vectors-g*.json` checks.
//!
//! Raw SVI slices (`docs/PLAN-phase2.md`, milestone 25) are [`SviParams`] objects; an
//! enclosure is a plain object `{ lo, hi }` and a grid report `{ ok, firstFailure, min }` with
//! `{ k, bound }` nodes (`firstFailure` is `null` when every node passes).

// wasm-bindgen's generated glue for the exported structs contains `panic!` (a misused
// JavaScript handle: a freed or foreign object), outside this crate's control and emitted at
// crate level; the bindings themselves return `OptfixError` and never panic.
#![allow(clippy::disallowed_macros)]

use optfix::wide::mul_shr as wide_mul_shr;
use optfix::{
    EuropeanOption, Fixed, FuturesInputs, IvInputs, MarketInputs, NodeBound, OptionKind, Price,
    Priced as RsPriced, PricingError, Rate, Rounding, Spot, Strike, StripLeg, SviGrid, SviMarket,
    SviParams as RsSviParams, Volatility, Years,
};
use wasm_bindgen::prelude::*;

/// A failed optfix call: `code` is the `PricingError` discriminant, `name` its variant.
#[wasm_bindgen]
#[derive(Debug, Clone)]
pub struct OptfixError {
    code: u8,
    name: String,
}

#[wasm_bindgen]
impl OptfixError {
    /// The `PricingError` code: 0 `Overflow`, 1 `DomainError`, 2 `NoConvergence`,
    /// 3 `ArbitrageBound`, 4 `IllConditioned`.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn code(&self) -> u8 {
        self.code
    }

    /// The variant name, for example `"DomainError"`.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn name(&self) -> String {
        self.name.clone()
    }

    /// A readable message.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn message(&self) -> String {
        let mut m = self.name.clone();
        m.push_str(" (optfix error ");
        m.push_str(match self.code {
            0 => "0)",
            1 => "1)",
            2 => "2)",
            3 => "3)",
            4 => "4)",
            _ => "?)",
        });
        m
    }
}

impl From<PricingError> for OptfixError {
    fn from(e: PricingError) -> Self {
        let name = match e {
            PricingError::Overflow => "Overflow",
            PricingError::DomainError => "DomainError",
            PricingError::NoConvergence => "NoConvergence",
            PricingError::ArbitrageBound => "ArbitrageBound",
            PricingError::IllConditioned => "IllConditioned",
            _ => "PricingError",
        };
        Self {
            code: e as u8,
            name: name.into(),
        }
    }
}

type R<T> = Result<T, OptfixError>;

fn domain() -> OptfixError {
    PricingError::DomainError.into()
}

fn rounding(r: &str) -> R<Rounding> {
    match r {
        "down" => Ok(Rounding::Down),
        "up" => Ok(Rounding::Up),
        _ => Err(domain()),
    }
}

fn kind(k: &str) -> R<OptionKind> {
    match k {
        "call" => Ok(OptionKind::Call),
        "put" => Ok(OptionKind::Put),
        _ => Err(domain()),
    }
}

// Exact argument conversions. wasm-bindgen's own conversion of an `i128` (or `u8`, `u32`)
// parameter wraps an out-of-range JavaScript value silently (`2n ** 127n` arrives as `−2^127`,
// `2.7` as `2`), so every numeric parameter is a `JsValue` checked here: a `bigint` within the
// Rust type's range, or for the small integers a `number` that is an integer in range.
// Anything else is a `DomainError`.

fn int(v: JsValue) -> R<i128> {
    i128::try_from(v).map_err(|_| domain())
}

fn uint(v: JsValue) -> R<u128> {
    u128::try_from(v).map_err(|_| domain())
}

fn long(v: JsValue) -> R<i64> {
    i64::try_from(v).map_err(|_| domain())
}

/// A JavaScript `number` that is an integer in `[0, max]`.
fn small(v: &JsValue, max: u32) -> R<u32> {
    let n = v.as_f64().ok_or_else(domain)?;
    if !(n >= 0.0 && n <= f64::from(max)) {
        return Err(domain());
    }
    // In range, so the cast is exact iff `n` is an integer.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let i = n as u32;
    if f64::from(i) == n {
        Ok(i)
    } else {
        Err(domain())
    }
}

fn small_u8(v: JsValue) -> R<u8> {
    u8::try_from(small(&v, u32::from(u8::MAX))?).map_err(|_| domain())
}

fn small_u32(v: JsValue) -> R<u32> {
    small(&v, u32::MAX)
}

fn fx(raw: i128) -> Fixed {
    Fixed::from_raw(raw)
}

fn option(k: &str, strike: i128, expiry: i128) -> R<EuropeanOption> {
    Ok(EuropeanOption {
        kind: kind(k)?,
        strike: Strike::try_new(fx(strike))?,
        expiry: Years::try_new(fx(expiry))?,
    })
}

fn market(spot: i128, vol: i128, rate: i128, carry: i128) -> R<MarketInputs> {
    Ok(MarketInputs {
        spot: Spot::try_new(fx(spot))?,
        vol: Volatility::try_new(fx(vol))?,
        rate: Rate::try_new(fx(rate))?,
        carry: Rate::try_new(fx(carry))?,
    })
}

fn futures(forward: i128, vol: i128, rate: i128) -> R<FuturesInputs> {
    Ok(FuturesInputs {
        forward: Spot::try_new(fx(forward))?,
        vol: Volatility::try_new(fx(vol))?,
        rate: Rate::try_new(fx(rate))?,
    })
}

/// A premium and its greeks, each a raw Q64.64 `bigint` (the units of `optfix::Greeks`).
#[wasm_bindgen]
#[derive(Debug, Clone, Copy)]
pub struct Priced {
    price: i128,
    delta: i128,
    gamma: i128,
    vega: i128,
    theta: i128,
    rho: i128,
}

#[wasm_bindgen]
impl Priced {
    /// The premium.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn price(&self) -> i128 {
        self.price
    }
    /// `∂price/∂S`.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn delta(&self) -> i128 {
        self.delta
    }
    /// `∂²price/∂S²`.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn gamma(&self) -> i128 {
        self.gamma
    }
    /// `∂price/∂σ`.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn vega(&self) -> i128 {
        self.vega
    }
    /// `∂price/∂t`, per year.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn theta(&self) -> i128 {
        self.theta
    }
    /// `∂price/∂r`, per 1.00 of the rate (Black-76: holding `F` fixed).
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn rho(&self) -> i128 {
        self.rho
    }
}

impl From<RsPriced> for Priced {
    fn from(p: RsPriced) -> Self {
        Self {
            price: p.price.get().raw(),
            delta: p.greeks.delta.raw(),
            gamma: p.greeks.gamma.raw(),
            vega: p.greeks.vega.raw(),
            theta: p.greeks.theta.raw(),
            rho: p.greeks.rho.raw(),
        }
    }
}

/// Parses exact decimal text (`Fixed::from_decimal_str`), rounded in direction `rounding`.
#[wasm_bindgen(js_name = fixedFromDecimal)]
pub fn fixed_from_decimal(text: &str, rounding_: &str) -> R<i128> {
    Ok(Fixed::from_decimal_str(text, rounding(rounding_)?)?.raw())
}

/// The exact decimal text of a raw value (`Fixed`'s `Display`).
#[wasm_bindgen(js_name = fixedToDecimal)]
pub fn fixed_to_decimal(
    #[wasm_bindgen(unchecked_param_type = "bigint")] raw: JsValue,
) -> R<String> {
    let raw = int(raw)?;
    Ok(fx(raw).to_string())
}

/// `a × b`, rounded down.
#[wasm_bindgen(js_name = mulDown)]
pub fn mul_down(
    #[wasm_bindgen(unchecked_param_type = "bigint")] a: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] b: JsValue,
) -> R<i128> {
    let a = int(a)?;
    let b = int(b)?;
    Ok(fx(a).mul_down(fx(b))?.raw())
}

/// `a × b`, rounded up.
#[wasm_bindgen(js_name = mulUp)]
pub fn mul_up(
    #[wasm_bindgen(unchecked_param_type = "bigint")] a: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] b: JsValue,
) -> R<i128> {
    let a = int(a)?;
    let b = int(b)?;
    Ok(fx(a).mul_up(fx(b))?.raw())
}

/// `a / b`, rounded down.
#[wasm_bindgen(js_name = divDown)]
pub fn div_down(
    #[wasm_bindgen(unchecked_param_type = "bigint")] a: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] b: JsValue,
) -> R<i128> {
    let a = int(a)?;
    let b = int(b)?;
    Ok(fx(a).div_down(fx(b))?.raw())
}

/// `a / b`, rounded up.
#[wasm_bindgen(js_name = divUp)]
pub fn div_up(
    #[wasm_bindgen(unchecked_param_type = "bigint")] a: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] b: JsValue,
) -> R<i128> {
    let a = int(a)?;
    let b = int(b)?;
    Ok(fx(a).div_up(fx(b))?.raw())
}

/// `sqrt(a)`, rounded down.
#[wasm_bindgen(js_name = sqrtDown)]
pub fn sqrt_down(#[wasm_bindgen(unchecked_param_type = "bigint")] a: JsValue) -> R<i128> {
    let a = int(a)?;
    Ok(fx(a).sqrt_down()?.raw())
}

/// `sqrt(a)`, rounded up.
#[wasm_bindgen(js_name = sqrtUp)]
pub fn sqrt_up(#[wasm_bindgen(unchecked_param_type = "bigint")] a: JsValue) -> R<i128> {
    let a = int(a)?;
    Ok(fx(a).sqrt_up()?.raw())
}

/// `bps / 10_000` as a raw value (`Fixed::from_bps`); `bps` is a signed 64-bit `bigint`.
#[wasm_bindgen(js_name = fromBps)]
pub fn from_bps(
    #[wasm_bindgen(unchecked_param_type = "bigint")] bps: JsValue,
    rounding_: &str,
) -> R<i128> {
    let bps = long(bps)?;
    Ok(Fixed::from_bps(bps, rounding(rounding_)?)?.raw())
}

/// `num / den` as a raw value (`Fixed::from_ratio`).
#[wasm_bindgen(js_name = fromRatio)]
pub fn from_ratio(
    #[wasm_bindgen(unchecked_param_type = "bigint")] num: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] den: JsValue,
    rounding_: &str,
) -> R<i128> {
    let num = int(num)?;
    let den = int(den)?;
    Ok(Fixed::from_ratio(num, den, rounding(rounding_)?)?.raw())
}

/// `amount / 10^decimals` as a raw value (`Fixed::from_scaled`).
#[wasm_bindgen(js_name = fromScaled)]
pub fn from_scaled(
    #[wasm_bindgen(unchecked_param_type = "bigint")] amount: JsValue,
    #[wasm_bindgen(unchecked_param_type = "number")] decimals: JsValue,
    rounding_: &str,
) -> R<i128> {
    let amount = int(amount)?;
    let decimals = small_u8(decimals)?;
    Ok(Fixed::from_scaled(amount, decimals, rounding(rounding_)?)?.raw())
}

/// `value × 10^decimals` as an integer (`Fixed::to_scaled`).
#[wasm_bindgen(js_name = toScaled)]
pub fn to_scaled(
    #[wasm_bindgen(unchecked_param_type = "bigint")] raw: JsValue,
    #[wasm_bindgen(unchecked_param_type = "number")] decimals: JsValue,
    rounding_: &str,
) -> R<i128> {
    let raw = int(raw)?;
    let decimals = small_u8(decimals)?;
    Ok(fx(raw).to_scaled(decimals, rounding(rounding_)?)?)
}

/// `a × b / 2^shift` (`optfix::wide::mul_shr`).
#[wasm_bindgen(js_name = mulShr)]
pub fn mul_shr(
    #[wasm_bindgen(unchecked_param_type = "bigint")] a: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] b: JsValue,
    #[wasm_bindgen(unchecked_param_type = "number")] shift: JsValue,
    rounding_: &str,
) -> R<u128> {
    let a = uint(a)?;
    let b = uint(b)?;
    let shift = small_u32(shift)?;
    Ok(wide_mul_shr(a, b, shift, rounding(rounding_)?)?)
}

/// `seconds / 31,536,000` as raw years (`Years::from_seconds`).
#[wasm_bindgen(js_name = yearsFromSeconds)]
pub fn years_from_seconds(
    #[wasm_bindgen(unchecked_param_type = "bigint")] seconds: JsValue,
    rounding_: &str,
) -> R<i128> {
    let seconds = long(seconds)?;
    Ok(Years::from_seconds(seconds, rounding(rounding_)?)?
        .get()
        .raw())
}

/// `e^x`.
#[wasm_bindgen]
pub fn exp(#[wasm_bindgen(unchecked_param_type = "bigint")] x: JsValue) -> R<i128> {
    let x = int(x)?;
    Ok(optfix::exp(fx(x))?.raw())
}

/// `ln x`.
#[wasm_bindgen]
pub fn ln(#[wasm_bindgen(unchecked_param_type = "bigint")] x: JsValue) -> R<i128> {
    let x = int(x)?;
    Ok(optfix::ln(fx(x))?.raw())
}

/// The standard normal CDF.
#[wasm_bindgen(js_name = normCdf)]
pub fn norm_cdf(#[wasm_bindgen(unchecked_param_type = "bigint")] x: JsValue) -> R<i128> {
    let x = int(x)?;
    Ok(optfix::norm_cdf(fx(x))?.raw())
}

/// The standard normal density.
#[wasm_bindgen(js_name = normPdf)]
pub fn norm_pdf(#[wasm_bindgen(unchecked_param_type = "bigint")] x: JsValue) -> R<i128> {
    let x = int(x)?;
    Ok(optfix::norm_pdf(fx(x))?.raw())
}

/// The Black-Scholes-Merton premium (`optfix::price`).
#[wasm_bindgen]
pub fn price(
    kind_: &str,
    #[wasm_bindgen(unchecked_param_type = "bigint")] spot: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] strike: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] expiry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] vol: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] rate: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] carry: JsValue,
) -> R<i128> {
    let spot = int(spot)?;
    let strike = int(strike)?;
    let expiry = int(expiry)?;
    let vol = int(vol)?;
    let rate = int(rate)?;
    let carry = int(carry)?;
    let o = option(kind_, strike, expiry)?;
    Ok(optfix::price(&o, &market(spot, vol, rate, carry)?)?
        .get()
        .raw())
}

/// The premium and greeks (`optfix::price_and_greeks`).
#[wasm_bindgen(js_name = priceAndGreeks)]
pub fn price_and_greeks(
    kind_: &str,
    #[wasm_bindgen(unchecked_param_type = "bigint")] spot: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] strike: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] expiry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] vol: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] rate: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] carry: JsValue,
) -> R<Priced> {
    let spot = int(spot)?;
    let strike = int(strike)?;
    let expiry = int(expiry)?;
    let vol = int(vol)?;
    let rate = int(rate)?;
    let carry = int(carry)?;
    let o = option(kind_, strike, expiry)?;
    Ok(optfix::price_and_greeks(&o, &market(spot, vol, rate, carry)?)?.into())
}

/// The Black-76 premium (`optfix::price_black76`).
#[wasm_bindgen(js_name = priceBlack76)]
pub fn price_black76(
    kind_: &str,
    #[wasm_bindgen(unchecked_param_type = "bigint")] forward: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] strike: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] expiry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] vol: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] rate: JsValue,
) -> R<i128> {
    let forward = int(forward)?;
    let strike = int(strike)?;
    let expiry = int(expiry)?;
    let vol = int(vol)?;
    let rate = int(rate)?;
    let o = option(kind_, strike, expiry)?;
    Ok(optfix::price_black76(&o, &futures(forward, vol, rate)?)?
        .get()
        .raw())
}

/// The Black-76 premium and greeks (`optfix::price_and_greeks_black76`).
#[wasm_bindgen(js_name = priceAndGreeksBlack76)]
pub fn price_and_greeks_black76(
    kind_: &str,
    #[wasm_bindgen(unchecked_param_type = "bigint")] forward: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] strike: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] expiry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] vol: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] rate: JsValue,
) -> R<Priced> {
    let forward = int(forward)?;
    let strike = int(strike)?;
    let expiry = int(expiry)?;
    let vol = int(vol)?;
    let rate = int(rate)?;
    let o = option(kind_, strike, expiry)?;
    Ok(optfix::price_and_greeks_black76(&o, &futures(forward, vol, rate)?)?.into())
}

/// The cash-or-nothing premium (`optfix::price_cash_or_nothing`).
#[wasm_bindgen(js_name = priceCashOrNothing)]
#[allow(clippy::too_many_arguments)]
pub fn price_cash_or_nothing(
    kind_: &str,
    #[wasm_bindgen(unchecked_param_type = "bigint")] spot: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] strike: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] expiry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] vol: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] rate: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] carry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] payout: JsValue,
) -> R<i128> {
    let spot = int(spot)?;
    let strike = int(strike)?;
    let expiry = int(expiry)?;
    let vol = int(vol)?;
    let rate = int(rate)?;
    let carry = int(carry)?;
    let payout = int(payout)?;
    let o = option(kind_, strike, expiry)?;
    let m = market(spot, vol, rate, carry)?;
    Ok(
        optfix::price_cash_or_nothing(&o, &m, Price::try_new(fx(payout))?)?
            .get()
            .raw(),
    )
}

/// The asset-or-nothing premium (`optfix::price_asset_or_nothing`).
#[wasm_bindgen(js_name = priceAssetOrNothing)]
pub fn price_asset_or_nothing(
    kind_: &str,
    #[wasm_bindgen(unchecked_param_type = "bigint")] spot: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] strike: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] expiry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] vol: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] rate: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] carry: JsValue,
) -> R<i128> {
    let spot = int(spot)?;
    let strike = int(strike)?;
    let expiry = int(expiry)?;
    let vol = int(vol)?;
    let rate = int(rate)?;
    let carry = int(carry)?;
    let o = option(kind_, strike, expiry)?;
    Ok(
        optfix::price_asset_or_nothing(&o, &market(spot, vol, rate, carry)?)?
            .get()
            .raw(),
    )
}

/// The implied volatility of `target` (`optfix::implied_vol`).
#[wasm_bindgen(js_name = impliedVol)]
pub fn implied_vol(
    kind_: &str,
    #[wasm_bindgen(unchecked_param_type = "bigint")] spot: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] strike: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] expiry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] rate: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] carry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] target: JsValue,
) -> R<i128> {
    let spot = int(spot)?;
    let strike = int(strike)?;
    let expiry = int(expiry)?;
    let rate = int(rate)?;
    let carry = int(carry)?;
    let target = int(target)?;
    let o = option(kind_, strike, expiry)?;
    let m = IvInputs {
        spot: Spot::try_new(fx(spot))?,
        rate: Rate::try_new(fx(rate))?,
        carry: Rate::try_new(fx(carry))?,
    };
    Ok(optfix::implied_vol(&o, &m, Price::try_new(fx(target))?)?
        .get()
        .raw())
}

fn strip_legs(kinds: &[String], strikes: Vec<JsValue>) -> R<Vec<StripLeg>> {
    if kinds.len() != strikes.len() {
        return Err(domain());
    }
    kinds
        .iter()
        .zip(strikes)
        .map(|(k, s)| {
            let raw = i128::try_from(s).map_err(|_| domain())?;
            Ok(StripLeg {
                kind: kind(k)?,
                strike: Strike::try_new(fx(raw))?,
            })
        })
        .collect()
}

/// The premiums of a strip sharing `T` and the market (`optfix::price_strip`): `kinds[i]` and
/// the raw `bigint` `strikes[i]` per leg.
#[wasm_bindgen(js_name = priceStrip)]
pub fn price_strip(
    #[wasm_bindgen(unchecked_param_type = "bigint")] expiry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] spot: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] vol: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] rate: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] carry: JsValue,
    kinds: Vec<String>,
    strikes: Vec<JsValue>,
) -> R<Vec<JsValue>> {
    let expiry = int(expiry)?;
    let spot = int(spot)?;
    let vol = int(vol)?;
    let rate = int(rate)?;
    let carry = int(carry)?;
    let legs = strip_legs(&kinds, strikes)?;
    let mut out = vec![Price::MIN; legs.len()];
    optfix::price_strip(
        Years::try_new(fx(expiry))?,
        &market(spot, vol, rate, carry)?,
        &legs,
        &mut out,
    )?;
    Ok(out.iter().map(|p| JsValue::from(p.get().raw())).collect())
}

/// The premiums and greeks of a strip (`optfix::price_and_greeks_strip`).
#[wasm_bindgen(js_name = priceAndGreeksStrip)]
pub fn price_and_greeks_strip(
    #[wasm_bindgen(unchecked_param_type = "bigint")] expiry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] spot: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] vol: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] rate: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] carry: JsValue,
    kinds: Vec<String>,
    strikes: Vec<JsValue>,
) -> R<Vec<Priced>> {
    let expiry = int(expiry)?;
    let spot = int(spot)?;
    let vol = int(vol)?;
    let rate = int(rate)?;
    let carry = int(carry)?;
    let legs = strip_legs(&kinds, strikes)?;
    let mut out = vec![RsPriced::ZERO; legs.len()];
    optfix::price_and_greeks_strip(
        Years::try_new(fx(expiry))?,
        &market(spot, vol, rate, carry)?,
        &legs,
        &mut out,
    )?;
    Ok(out.into_iter().map(Priced::from).collect())
}

/// A plain JavaScript object with these fields.
fn object(fields: &[(&str, JsValue)]) -> R<JsValue> {
    let o = js_sys::Object::new();
    for (k, v) in fields {
        js_sys::Reflect::set(&o, &JsValue::from_str(k), v).map_err(|_| domain())?;
    }
    Ok(o.into())
}

/// `{ lo, hi }` of two raw values.
fn enclosure(lo: Fixed, hi: Fixed) -> R<JsValue> {
    object(&[
        ("lo", JsValue::from(lo.raw())),
        ("hi", JsValue::from(hi.raw())),
    ])
}

/// `{ k, bound }` of a grid node.
fn node(n: NodeBound) -> R<JsValue> {
    object(&[
        ("k", JsValue::from(n.k.raw())),
        ("bound", JsValue::from(n.bound.raw())),
    ])
}

/// `{ ok, firstFailure, min }` of a grid report.
fn report(ok: bool, first_failure: Option<NodeBound>, min: NodeBound) -> R<JsValue> {
    let first = match first_failure {
        Some(n) => node(n)?,
        None => JsValue::NULL,
    };
    object(&[
        ("ok", JsValue::from_bool(ok)),
        ("firstFailure", first),
        ("min", node(min)?),
    ])
}

fn svi_market(spot: i128, rate: i128, carry: i128) -> R<SviMarket> {
    Ok(SviMarket {
        spot: Spot::try_new(fx(spot))?,
        rate: Rate::try_new(fx(rate))?,
        carry: Rate::try_new(fx(carry))?,
    })
}

fn grid(k_min: JsValue, k_max: JsValue, nodes: JsValue) -> R<SviGrid> {
    Ok(SviGrid {
        k_min: fx(int(k_min)?),
        k_max: fx(int(k_max)?),
        nodes: small_u32(nodes)?,
    })
}

/// A raw SVI slice `w(k) = a + b (ρ (k − m) + sqrt((k − m)² + σ²))` (`optfix::SviParams`), in
/// total variance against log-forward-moneyness `k = ln(K / F)`.
#[wasm_bindgen]
#[derive(Debug, Clone, Copy)]
pub struct SviParams {
    inner: RsSviParams,
}

#[wasm_bindgen]
impl SviParams {
    /// `new SviParams(a, b, rho, m, sigma)`, raw `bigint`s; throws `DomainError` outside the
    /// domain of `SviParams::try_new` (`|a| <= 1000`, `0 <= b <= 10`, `|ρ| < 1`, `|m| <= 10`,
    /// `10^-6 <= σ <= 10`, and a minimum total variance whose lower bound is `>= 0`).
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(unchecked_param_type = "bigint")] a: JsValue,
        #[wasm_bindgen(unchecked_param_type = "bigint")] b: JsValue,
        #[wasm_bindgen(unchecked_param_type = "bigint")] rho: JsValue,
        #[wasm_bindgen(unchecked_param_type = "bigint")] m: JsValue,
        #[wasm_bindgen(unchecked_param_type = "bigint")] sigma: JsValue,
    ) -> R<SviParams> {
        let a = int(a)?;
        let b = int(b)?;
        let rho = int(rho)?;
        let m = int(m)?;
        let sigma = int(sigma)?;
        Ok(Self {
            inner: RsSviParams::try_new(fx(a), fx(b), fx(rho), fx(m), fx(sigma))?,
        })
    }

    /// `a`, the level.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn a(&self) -> i128 {
        self.inner.a().raw()
    }

    /// `b`, the slope.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn b(&self) -> i128 {
        self.inner.b().raw()
    }

    /// `ρ`, the rotation.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn rho(&self) -> i128 {
        self.inner.rho().raw()
    }

    /// `m`, the shift.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn m(&self) -> i128 {
        self.inner.m().raw()
    }

    /// `σ`, the curvature.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn sigma(&self) -> i128 {
        self.inner.sigma().raw()
    }

    /// A rigorous lower bound of the minimum total variance `a + b σ sqrt(1 − ρ²)`
    /// (`SviParams::min_total_variance`).
    #[wasm_bindgen(js_name = minTotalVariance)]
    #[must_use]
    pub fn min_total_variance(&self) -> i128 {
        self.inner.min_total_variance().raw()
    }

    /// `w(k)`, rounded down (`SviParams::total_variance`).
    #[wasm_bindgen(js_name = totalVariance)]
    pub fn total_variance(
        &self,
        #[wasm_bindgen(unchecked_param_type = "bigint")] k: JsValue,
    ) -> R<i128> {
        let k = int(k)?;
        Ok(self.inner.total_variance(fx(k))?.raw())
    }

    /// `{ lo, hi }` enclosing `w(k)` (`SviParams::total_variance_bounds`).
    #[wasm_bindgen(js_name = totalVarianceBounds)]
    pub fn total_variance_bounds(
        &self,
        #[wasm_bindgen(unchecked_param_type = "bigint")] k: JsValue,
    ) -> R<JsValue> {
        let k = int(k)?;
        let (lo, hi) = self.inner.total_variance_bounds(fx(k))?;
        enclosure(lo, hi)
    }
}

/// `k̂ = ln K − ln S − (r T − q T)` (`optfix::log_forward_moneyness`).
#[wasm_bindgen(js_name = logForwardMoneyness)]
pub fn log_forward_moneyness(
    #[wasm_bindgen(unchecked_param_type = "bigint")] expiry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] spot: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] rate: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] carry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] strike: JsValue,
) -> R<i128> {
    let expiry = int(expiry)?;
    let spot = int(spot)?;
    let rate = int(rate)?;
    let carry = int(carry)?;
    let strike = int(strike)?;
    Ok(optfix::log_forward_moneyness(
        Years::try_new(fx(expiry))?,
        &svi_market(spot, rate, carry)?,
        Strike::try_new(fx(strike))?,
    )?
    .raw())
}

/// The arguments of `sviVol` and `sviVolBounds` after the slice.
fn vol_inputs(
    expiry: JsValue,
    spot: JsValue,
    rate: JsValue,
    carry: JsValue,
    strike: JsValue,
) -> R<(Years, SviMarket, Strike)> {
    let expiry = int(expiry)?;
    let spot = int(spot)?;
    let rate = int(rate)?;
    let carry = int(carry)?;
    let strike = int(strike)?;
    Ok((
        Years::try_new(fx(expiry))?,
        svi_market(spot, rate, carry)?,
        Strike::try_new(fx(strike))?,
    ))
}

/// The slice's implied volatility at `strike` (`optfix::svi_vol`).
#[wasm_bindgen(js_name = sviVol)]
pub fn svi_vol(
    slice: &SviParams,
    #[wasm_bindgen(unchecked_param_type = "bigint")] expiry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] spot: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] rate: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] carry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] strike: JsValue,
) -> R<i128> {
    let (t, m, k) = vol_inputs(expiry, spot, rate, carry, strike)?;
    Ok(optfix::svi_vol(&slice.inner, t, &m, k)?.get().raw())
}

/// `{ lo, hi }` enclosing the slice's volatility at the exact inputs
/// (`optfix::svi_vol_bounds`).
#[wasm_bindgen(js_name = sviVolBounds)]
pub fn svi_vol_bounds(
    slice: &SviParams,
    #[wasm_bindgen(unchecked_param_type = "bigint")] expiry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] spot: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] rate: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] carry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] strike: JsValue,
) -> R<JsValue> {
    let (t, m, k) = vol_inputs(expiry, spot, rate, carry, strike)?;
    let (lo, hi) = optfix::svi_vol_bounds(&slice.inner, t, &m, k)?;
    enclosure(lo, hi)
}

/// `{ lo, hi }` enclosing the premium at the slice's exact volatility
/// (`optfix::svi_price_bounds`).
#[wasm_bindgen(js_name = sviPriceBounds)]
pub fn svi_price_bounds(
    slice: &SviParams,
    kind_: &str,
    #[wasm_bindgen(unchecked_param_type = "bigint")] spot: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] strike: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] expiry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] rate: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] carry: JsValue,
) -> R<JsValue> {
    let spot = int(spot)?;
    let strike = int(strike)?;
    let expiry = int(expiry)?;
    let rate = int(rate)?;
    let carry = int(carry)?;
    let o = option(kind_, strike, expiry)?;
    let (lo, hi) = optfix::svi_price_bounds(&o, &svi_market(spot, rate, carry)?, &slice.inner)?;
    enclosure(lo.get(), hi.get())
}

/// The premiums of a strip whose legs take their volatility from the slice
/// (`optfix::price_strip_svi`): `kinds[i]` and the raw `bigint` `strikes[i]` per leg. A
/// failing leg throws its error (the prices before it are not returned).
#[wasm_bindgen(js_name = priceStripSvi)]
pub fn price_strip_svi(
    slice: &SviParams,
    #[wasm_bindgen(unchecked_param_type = "bigint")] expiry: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] spot: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] rate: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] carry: JsValue,
    kinds: Vec<String>,
    strikes: Vec<JsValue>,
) -> R<Vec<JsValue>> {
    let expiry = int(expiry)?;
    let spot = int(spot)?;
    let rate = int(rate)?;
    let carry = int(carry)?;
    let legs = strip_legs(&kinds, strikes)?;
    let mut out = vec![Price::MIN; legs.len()];
    optfix::price_strip_svi(
        Years::try_new(fx(expiry))?,
        &svi_market(spot, rate, carry)?,
        &slice.inner,
        &legs,
        &mut out,
    )?;
    Ok(out.iter().map(|p| JsValue::from(p.get().raw())).collect())
}

/// The wing condition `b (1 + |ρ|) < 2`, certain when `true` (`optfix::check_wings`).
#[wasm_bindgen(js_name = checkWings)]
#[must_use]
pub fn check_wings(slice: &SviParams) -> bool {
    optfix::check_wings(&slice.inner)
}

/// `{ lo, hi }` enclosing Durrleman's `g(k)` (`optfix::butterfly_g_bounds`).
#[wasm_bindgen(js_name = butterflyGBounds)]
pub fn butterfly_g_bounds(
    slice: &SviParams,
    #[wasm_bindgen(unchecked_param_type = "bigint")] k: JsValue,
) -> R<JsValue> {
    let k = int(k)?;
    let (lo, hi) = optfix::butterfly_g_bounds(&slice.inner, fx(k))?;
    enclosure(lo, hi)
}

/// Durrleman's condition at every node of the grid `kMin..kMax` with `nodes` nodes (a
/// `number`): `{ ok, firstFailure, min }` (`optfix::check_butterfly`).
#[wasm_bindgen(js_name = checkButterfly)]
pub fn check_butterfly(
    slice: &SviParams,
    #[wasm_bindgen(unchecked_param_type = "bigint")] k_min: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] k_max: JsValue,
    #[wasm_bindgen(unchecked_param_type = "number")] nodes: JsValue,
) -> R<JsValue> {
    let g = grid(k_min, k_max, nodes)?;
    let r = optfix::check_butterfly(&slice.inner, g)?;
    report(r.ok, r.first_failure, r.min)
}

/// The calendar condition `w_later(k) >= w_earlier(k)` at every node of the grid:
/// `{ ok, firstFailure, min }` (`optfix::check_calendar`).
#[wasm_bindgen(js_name = checkCalendar)]
pub fn check_calendar(
    earlier: &SviParams,
    #[wasm_bindgen(unchecked_param_type = "bigint")] expiry_earlier: JsValue,
    later: &SviParams,
    #[wasm_bindgen(unchecked_param_type = "bigint")] expiry_later: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] k_min: JsValue,
    #[wasm_bindgen(unchecked_param_type = "bigint")] k_max: JsValue,
    #[wasm_bindgen(unchecked_param_type = "number")] nodes: JsValue,
) -> R<JsValue> {
    let t_earlier = int(expiry_earlier)?;
    let t_later = int(expiry_later)?;
    let g = grid(k_min, k_max, nodes)?;
    let r = optfix::check_calendar(
        (&earlier.inner, Years::try_new(fx(t_earlier))?),
        (&later.inner, Years::try_new(fx(t_later))?),
        g,
    )?;
    report(r.ok, r.first_failure, r.min)
}
