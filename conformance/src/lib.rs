//! Conformance vectors for optfix (`docs/PLAN-improvements.md`, milestone 17): the file format,
//! the native evaluator that defines each case's result, and the loader the replays use.
//!
//! A generation `conformance/vectors-g<N>.json` is immutable once merged. Each case names a
//! function, its arguments and its result: raw Q64.64 values and other integers as decimal
//! strings (JSON numbers cannot hold an `i128`), `call`/`put` for the option kind,
//! `down`/`up` for a rounding, and an error as its `PricingError` code (`e as u8`). The
//! argument order of every function is in [`SIGNATURES`], which the file repeats. A priced
//! result (`price_and_greeks`, its Black-76 and strip forms) is [`priced_width`] values per
//! option: price, delta, gamma, vega, theta, and from generation 2 rho.

#![allow(clippy::indexing_slicing, clippy::missing_errors_doc)]

use optfix::wide::mul_shr;
use optfix::{
    exp, implied_vol, ln, norm_cdf, norm_pdf, price, price_and_greeks, price_and_greeks_black76,
    price_and_greeks_strip, price_asset_or_nothing, price_black76, price_cash_or_nothing,
    price_strip, EuropeanOption, Fixed, FuturesInputs, IvInputs, MarketInputs, OptionKind, Price,
    Priced, PricingError, Rate, Rounding, Spot, Strike, StripLeg, Volatility, Years,
};
use serde::{Deserialize, Serialize};

/// The file format version.
pub const FORMAT: u32 = 1;

/// Every function and its argument names, in order. A strip's legs follow its five market
/// arguments as `kind, strike` pairs.
pub const SIGNATURES: &[(&str, &[&str])] = &[
    ("mul_down", &["a", "b"]),
    ("mul_up", &["a", "b"]),
    ("div_down", &["a", "b"]),
    ("div_up", &["a", "b"]),
    ("sqrt_down", &["a"]),
    ("sqrt_up", &["a"]),
    ("from_ratio", &["num", "den", "rounding"]),
    ("from_bps", &["bps", "rounding"]),
    ("from_scaled", &["amount", "decimals", "rounding"]),
    ("to_scaled", &["value", "decimals", "rounding"]),
    ("from_decimal_str", &["text", "rounding"]),
    ("to_decimal", &["value"]),
    ("mul_shr", &["a", "b", "shift", "rounding"]),
    ("years_from_seconds", &["seconds", "rounding"]),
    ("exp", &["x"]),
    ("ln", &["x"]),
    ("norm_cdf", &["x"]),
    ("norm_pdf", &["x"]),
    (
        "price",
        &["kind", "spot", "strike", "expiry", "vol", "rate", "carry"],
    ),
    (
        "price_and_greeks",
        &["kind", "spot", "strike", "expiry", "vol", "rate", "carry"],
    ),
    (
        "price_black76",
        &["kind", "forward", "strike", "expiry", "vol", "rate"],
    ),
    (
        "price_and_greeks_black76",
        &["kind", "forward", "strike", "expiry", "vol", "rate"],
    ),
    (
        "price_cash_or_nothing",
        &[
            "kind", "spot", "strike", "expiry", "vol", "rate", "carry", "payout",
        ],
    ),
    (
        "price_asset_or_nothing",
        &["kind", "spot", "strike", "expiry", "vol", "rate", "carry"],
    ),
    (
        "implied_vol",
        &[
            "kind", "spot", "strike", "expiry", "rate", "carry", "target",
        ],
    ),
    (
        "price_strip",
        &["expiry", "spot", "vol", "rate", "carry", "legs..."],
    ),
    (
        "price_and_greeks_strip",
        &["expiry", "spot", "vol", "rate", "carry", "legs..."],
    ),
];

/// A generation file.
#[derive(Debug, Serialize, Deserialize)]
pub struct Vectors {
    /// [`FORMAT`].
    pub format: u32,
    /// The generation number `N` of `vectors-g<N>.json`.
    pub generation: u32,
    /// The optfix version that generated it.
    pub optfix: String,
    /// The commit it was generated from.
    pub commit: String,
    /// [`SIGNATURES`] at generation time.
    pub signatures: Vec<(String, Vec<String>)>,
    /// The cases.
    pub cases: Vec<Case>,
}

/// One case: `f(a...)` returns `r` (raw values as decimal strings) or fails with code `e`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Case {
    /// The function.
    pub f: String,
    /// Its arguments.
    pub a: Vec<String>,
    /// The result values, if it succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r: Option<Vec<String>>,
    /// The `PricingError` code, if it fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e: Option<u8>,
}

/// Why a case could not be evaluated (a malformed case, not a `PricingError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed(pub String);

type Out = Result<Result<Vec<String>, PricingError>, Malformed>;

fn bad(what: &str) -> Malformed {
    Malformed(what.into())
}

fn int<T: core::str::FromStr>(a: &[String], i: usize) -> Result<T, Malformed> {
    a.get(i)
        .ok_or_else(|| bad("missing argument"))?
        .parse()
        .map_err(|_| bad("not an integer"))
}

fn fx(a: &[String], i: usize) -> Result<Fixed, Malformed> {
    int::<i128>(a, i).map(Fixed::from_raw)
}

fn rounding(a: &[String], i: usize) -> Result<Rounding, Malformed> {
    match a.get(i).map(String::as_str) {
        Some("down") => Ok(Rounding::Down),
        Some("up") => Ok(Rounding::Up),
        _ => Err(bad("rounding")),
    }
}

fn kind(a: &[String], i: usize) -> Result<OptionKind, Malformed> {
    match a.get(i).map(String::as_str) {
        Some("call") => Ok(OptionKind::Call),
        Some("put") => Ok(OptionKind::Put),
        _ => Err(bad("kind")),
    }
}

/// A quantity argument; an out-of-range value is a malformed case (vectors only carry valid
/// quantities, as the typed API does).
fn q<T>(a: &[String], i: usize, new: fn(Fixed) -> Result<T, PricingError>) -> Result<T, Malformed> {
    new(fx(a, i)?).map_err(|_| bad("quantity out of range"))
}

fn raws(v: &[Fixed]) -> Vec<String> {
    v.iter().map(|x| x.raw().to_string()).collect()
}

fn one(r: Result<Fixed, PricingError>) -> Result<Vec<String>, PricingError> {
    r.map(|x| raws(&[x]))
}

/// Values per priced result in generation `generation`: five (price, delta, gamma, vega,
/// theta) in generation 1, six (rho added, `docs/PLAN-improvements.md` milestone 19) from
/// generation 2. [`eval`] always returns six.
#[must_use]
pub const fn priced_width(generation: u32) -> usize {
    if generation < 2 {
        5
    } else {
        6
    }
}

/// The first generation whose `implied_vol` results are exact today: milestone 20
/// (`docs/PLAN-improvements.md`) refined the initial guess and lowered `IV_MAX_ITERATIONS` to 2,
/// so earlier generations' answers may move within the round-trip envelope (error codes are
/// unchanged).
pub const IV_EXACT_FROM: u32 = 3;

/// Whether `f` returns priced results ([`priced_width`] values per option).
#[must_use]
pub fn is_priced(f: &str) -> bool {
    matches!(
        f,
        "price_and_greeks" | "price_and_greeks_black76" | "price_and_greeks_strip"
    )
}

fn priced(p: &Priced) -> Vec<String> {
    raws(&[
        p.price.get(),
        p.greeks.delta,
        p.greeks.gamma,
        p.greeks.vega,
        p.greeks.theta,
        p.greeks.rho,
    ])
}

fn option(
    a: &[String],
    k: usize,
    strike: usize,
    expiry: usize,
) -> Result<EuropeanOption, Malformed> {
    Ok(EuropeanOption {
        kind: kind(a, k)?,
        strike: q(a, strike, Strike::try_new)?,
        expiry: q(a, expiry, Years::try_new)?,
    })
}

fn market(a: &[String], s: usize, v: usize, r: usize, c: usize) -> Result<MarketInputs, Malformed> {
    Ok(MarketInputs {
        spot: q(a, s, Spot::try_new)?,
        vol: q(a, v, Volatility::try_new)?,
        rate: q(a, r, Rate::try_new)?,
        carry: q(a, c, Rate::try_new)?,
    })
}

fn legs(a: &[String]) -> Result<Vec<StripLeg>, Malformed> {
    let rest = a.get(5..).ok_or_else(|| bad("strip without legs"))?;
    if rest.is_empty() || rest.len() % 2 != 0 {
        return Err(bad("strip legs are kind, strike pairs"));
    }
    rest.as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            Ok(StripLeg {
                kind: kind(pair.as_slice(), 0)?,
                strike: q(pair.as_slice(), 1, Strike::try_new)?,
            })
        })
        .collect()
}

/// `f(a...)` natively: the definition of every case's result.
pub fn eval(f: &str, a: &[String]) -> Out {
    Ok(match f {
        "mul_down" => one(fx(a, 0)?.mul_down(fx(a, 1)?)),
        "mul_up" => one(fx(a, 0)?.mul_up(fx(a, 1)?)),
        "div_down" => one(fx(a, 0)?.div_down(fx(a, 1)?)),
        "div_up" => one(fx(a, 0)?.div_up(fx(a, 1)?)),
        "sqrt_down" => one(fx(a, 0)?.sqrt_down()),
        "sqrt_up" => one(fx(a, 0)?.sqrt_up()),
        "from_ratio" => one(Fixed::from_ratio(int(a, 0)?, int(a, 1)?, rounding(a, 2)?)),
        "from_bps" => one(Fixed::from_bps(int(a, 0)?, rounding(a, 1)?)),
        "from_scaled" => one(Fixed::from_scaled(int(a, 0)?, int(a, 1)?, rounding(a, 2)?)),
        "to_scaled" => fx(a, 0)?
            .to_scaled(int(a, 1)?, rounding(a, 2)?)
            .map(|v| vec![v.to_string()]),
        "from_decimal_str" => one(Fixed::from_decimal_str(
            a.first().ok_or_else(|| bad("missing text"))?,
            rounding(a, 1)?,
        )),
        "to_decimal" => Ok(vec![fx(a, 0)?.to_string()]),
        "mul_shr" => mul_shr(int(a, 0)?, int(a, 1)?, int(a, 2)?, rounding(a, 3)?)
            .map(|v| vec![v.to_string()]),
        "years_from_seconds" => {
            Years::from_seconds(int(a, 0)?, rounding(a, 1)?).map(|y| raws(&[y.get()]))
        }
        "exp" => one(exp(fx(a, 0)?)),
        "ln" => one(ln(fx(a, 0)?)),
        "norm_cdf" => one(norm_cdf(fx(a, 0)?)),
        "norm_pdf" => one(norm_pdf(fx(a, 0)?)),
        "price" => price(&option(a, 0, 2, 3)?, &market(a, 1, 4, 5, 6)?).map(|p| raws(&[p.get()])),
        "price_and_greeks" => {
            price_and_greeks(&option(a, 0, 2, 3)?, &market(a, 1, 4, 5, 6)?).map(|p| priced(&p))
        }
        "price_black76" | "price_and_greeks_black76" => {
            let o = option(a, 0, 2, 3)?;
            let m = FuturesInputs {
                forward: q(a, 1, Spot::try_new)?,
                vol: q(a, 4, Volatility::try_new)?,
                rate: q(a, 5, Rate::try_new)?,
            };
            if f == "price_black76" {
                price_black76(&o, &m).map(|p| raws(&[p.get()]))
            } else {
                price_and_greeks_black76(&o, &m).map(|p| priced(&p))
            }
        }
        "price_cash_or_nothing" => price_cash_or_nothing(
            &option(a, 0, 2, 3)?,
            &market(a, 1, 4, 5, 6)?,
            q(a, 7, Price::try_new)?,
        )
        .map(|p| raws(&[p.get()])),
        "price_asset_or_nothing" => {
            price_asset_or_nothing(&option(a, 0, 2, 3)?, &market(a, 1, 4, 5, 6)?)
                .map(|p| raws(&[p.get()]))
        }
        "implied_vol" => {
            let m = IvInputs {
                spot: q(a, 1, Spot::try_new)?,
                rate: q(a, 4, Rate::try_new)?,
                carry: q(a, 5, Rate::try_new)?,
            };
            implied_vol(&option(a, 0, 2, 3)?, &m, q(a, 6, Price::try_new)?)
                .map(|v| raws(&[v.get()]))
        }
        "price_strip" | "price_and_greeks_strip" => {
            let expiry = q(a, 0, Years::try_new)?;
            let m = market(a, 1, 2, 3, 4)?;
            let legs = legs(a)?;
            if f == "price_strip" {
                let mut out = vec![Price::MIN; legs.len()];
                price_strip(expiry, &m, &legs, &mut out)
                    .map(|()| out.iter().map(|p| p.get().raw().to_string()).collect())
            } else {
                let mut out = vec![Priced::ZERO; legs.len()];
                price_and_greeks_strip(expiry, &m, &legs, &mut out)
                    .map(|()| out.iter().flat_map(priced).collect())
            }
        }
        _ => return Err(bad("unknown function")),
    })
}

/// The case for `f(a...)`, evaluated natively.
pub fn case(f: &str, a: Vec<String>) -> Result<Case, Malformed> {
    let out = eval(f, &a)?;
    Ok(match out {
        Ok(r) => Case {
            f: f.into(),
            a,
            r: Some(r),
            e: None,
        },
        Err(e) => Case {
            f: f.into(),
            a,
            r: None,
            e: Some(e as u8),
        },
    })
}

/// Parses a generation file.
pub fn load(text: &str) -> Result<Vectors, serde_json::Error> {
    serde_json::from_str(text)
}
