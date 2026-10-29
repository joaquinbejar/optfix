//! Black-Scholes-Merton prices and greeks of European options with a continuous carry `q`.
//!
//! `Dr = e^{−rT}`, `Dq = e^{−qT}`, `d1 = (ln(S/K) + (r − q + σ²/2) T) / (σ√T)`, `d2 = d1 − σ√T`;
//! `C = S Dq N(d1) − K Dr N(d2)`, `P = K Dr N(−d2) − S Dq N(−d1)` (`docs/PLAN.md` 4.5). The
//! option that is out of the money by forward moneyness (`S Dq` against `K Dr`) is priced
//! directly and the other through put-call parity, `C − P = S Dq − K Dr`.
//!
//! Nothing here divides or takes a square root: `√T`, `σ√T` and the reciprocals the formulas
//! need are `exp` of sums of `ln σ`, `ln T / 2` and `ln S`, and `ln(S/K) = ln S − ln K`. On SBF
//! that is several times cheaper than `sqrt_down` and `div_down` (`docs/benchmarks.md`).
//!
//! Results are rounded to nearest within published envelopes; a caller that needs a
//! conservative side widens them with the envelope (`PLAN`, rounding contract).

use crate::explog::{exp, ln};
use crate::norm::{norm_cdf, norm_cdf_pdf};
use crate::{Fixed, Price, PricingError, Rate, Spot, Strike, Volatility, Years};

/// Call or put.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum OptionKind {
    /// The right to buy at the strike.
    Call = 0,
    /// The right to sell at the strike.
    Put = 1,
}

/// A European option: exercised only at expiry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EuropeanOption {
    /// Call or put.
    pub kind: OptionKind,
    /// Strike, in the unit of the spot.
    pub strike: Strike,
    /// Time remaining to expiry, not an absolute timestamp.
    pub expiry: Years,
}

/// Market inputs of a price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MarketInputs {
    /// Price of the underlying.
    pub spot: Spot,
    /// Annualized volatility.
    pub vol: Volatility,
    /// Continuously compounded risk-free rate `r`.
    pub rate: Rate,
    /// Continuous carry `q` (dividend yield, foreign rate, or lease rate); zero for none.
    pub carry: Rate,
}

/// First- and second-order sensitivities.
///
/// With `Dq = e^{−qT}` in `[e^{−15}, e^3]` over the supported domain, `|delta|` can reach
/// about 20.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Greeks {
    /// `dV/dS`: a call's in `[0, Dq]`, a put's in `[−Dq, 0]`.
    pub delta: Fixed,
    /// `d²V/dS²`, per unit of spot; `>= 0`.
    pub gamma: Fixed,
    /// `dV/dσ` per 1.00 of volatility (divide by 100 for "per vol point"); `>= 0`.
    pub vega: Fixed,
    /// `dV/dt` in calendar time, per year (`−dV/dT`); either sign.
    pub theta: Fixed,
}

/// A price with its greeks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Priced {
    /// The option premium.
    pub price: Price,
    /// Its sensitivities.
    pub greeks: Greeks,
}

/// Price envelope, relative part: `|error| <= PRICE_ERR_REL × max(S, K) + PRICE_ERR_ABS`.
/// `2^-48`, about `3.6 × 10^-15` (measured: `3.3 × 10^-16`, `docs/precision.md`).
pub const PRICE_ERR_REL: Fixed = Fixed::from_raw(1 << 16);

/// Price envelope, absolute part: 64 ulps, about `3.5 × 10^-18`.
pub const PRICE_ERR_ABS: Fixed = Fixed::from_raw(64);

/// Delta envelope, absolute: `2^-49`, about `1.8 × 10^-15` (measured `1.1 × 10^-16`).
pub const DELTA_ERR: Fixed = Fixed::from_raw(1 << 15);

/// Gamma envelope: `|error| <= GAMMA_ERR_REL × Dq / (S σ√T) + GREEK_ERR_ABS` (the scale is
/// gamma's maximum times `√(2π)`). `2^-40`, about `9.1 × 10^-13` (measured `8.7 × 10^-14`).
pub const GAMMA_ERR_REL: Fixed = Fixed::from_raw(1 << 24);

/// Vega envelope: `|error| <= VEGA_ERR_REL × S Dq √T + GREEK_ERR_ABS`. `2^-43`, about
/// `1.1 × 10^-13` (measured `7.5 × 10^-15`).
pub const VEGA_ERR_REL: Fixed = Fixed::from_raw(1 << 21);

/// Theta envelope: `|error| <= THETA_ERR_REL × (S Dq σ / √T + |r| K Dr + |q| S Dq) +
/// GREEK_ERR_ABS`. `2^-42`, about `2.3 × 10^-13` (measured `1.5 × 10^-14`).
pub const THETA_ERR_REL: Fixed = Fixed::from_raw(1 << 22);

/// Absolute part of the gamma, vega and theta envelopes: 64 ulps.
pub const GREEK_ERR_ABS: Fixed = Fixed::from_raw(64);

/// `S/K` must lie in `[0.05, 20]`: `S × 20 >= K` and `K × 20 >= S`.
const MONEYNESS_LIMIT: i128 = 20;

/// The intrinsic value at spot: `max(S − K, 0)` for a call, `max(K − S, 0)` for a put. Exact.
#[must_use]
pub fn intrinsic(opt: &EuropeanOption, spot: Spot) -> Price {
    let (s, k) = (spot.get(), opt.strike.get());
    let value = match opt.kind {
        OptionKind::Call => s.checked_sub(k),
        OptionKind::Put => k.checked_sub(s),
    };
    // Spot and strike are in [10^-6, 10^9], so the difference always fits and the result is a
    // valid price; the fallback only satisfies the types.
    value
        .ok()
        .filter(|v| *v > Fixed::ZERO)
        .and_then(|v| Price::try_new(v).ok())
        .unwrap_or(Price::MIN)
}

/// The Black-Scholes-Merton premium, rounded to nearest within the price envelope of
/// `docs/precision.md` (`PRICE_ERR_REL × max(S, K) + PRICE_ERR_ABS`).
///
/// # Errors
///
/// `DomainError` if `S/K` is outside `[0.05, 20]`; `Overflow` only if an intermediate exceeds
/// [`Fixed::MAX`], which the supported domain excludes for the price.
#[must_use = "the result carries a possible error"]
pub fn price(opt: &EuropeanOption, m: &MarketInputs) -> Result<Price, PricingError> {
    evaluate(opt, m, false).map(|p| p.price)
}

/// The premium and its four greeks from one evaluation of `d1`, `d2`, `N`, `n` and the
/// discount factors, each within its envelope (`docs/precision.md`).
///
/// # Errors
///
/// `DomainError` if `S/K` is outside `[0.05, 20]`; `Overflow` if a greek exceeds
/// [`Fixed::MAX`]. That happens only for theta, `S Dq n(d1) σ / (2√T)`, with a large spot and
/// volatility a tiny fraction of a second before expiry, where the true value is beyond the
/// representation.
#[must_use = "the result carries a possible error"]
pub fn price_and_greeks(opt: &EuropeanOption, m: &MarketInputs) -> Result<Priced, PricingError> {
    evaluate(opt, m, true)
}

/// Shared evaluation; `greeks = false` stops after the price (the greeks are then zero).
fn evaluate(opt: &EuropeanOption, m: &MarketInputs, greeks: bool) -> Result<Priced, PricingError> {
    let (s, k, t) = (m.spot.get(), opt.strike.get(), opt.expiry.get());
    let (sigma, r, q) = (m.vol.get(), m.rate.get(), m.carry.get());
    check_moneyness(s, k)?;
    let rt = r.mul_down(t)?;
    let qt = q.mul_down(t)?;
    let dr = exp(rt.checked_neg()?)?;
    // q = 0 gives qT = 0 and Dq = exp(0) = 1 exactly, so plain Black-Scholes takes no extra
    // rounding.
    let dq = exp(qt.checked_neg()?)?;
    let s_dq = s.mul_down(dq)?;
    let k_dr = k.mul_down(dr)?;
    // The only degenerate case is T = 0: the smallest positive T is 2^-64 years, so σ√T is at
    // least 0.005 × 2^-32 ≈ 1.2 × 10^-12, where d1 stays below about 2 × 10^13 and gamma below
    // about 6.9 × 10^18, inside the representation.
    if t == Fixed::ZERO {
        return degenerate(opt.kind, r, q, dq, s_dq, k_dr);
    }
    let ln_t = ln(t)?;
    let half_ln_t = half(ln_t);
    let ln_sigma = ln(sigma)?;
    // ln(σ√T)
    let ls = ln_sigma.checked_add(half_ln_t)?;
    let sst = exp(ls)?;
    let inv_sst = exp(ls.checked_neg()?)?;
    let ln_s = ln(s)?;
    let x = ln_s.checked_sub(ln(k)?)?;
    // d1 = (ln(S/K) + (r − q) T + σ²T/2) / (σ√T)
    let num = x
        .checked_add(rt)?
        .checked_sub(qt)?
        .checked_add(half(sst.mul_down(sst)?))?;
    let d1 = num.mul_down(inv_sst)?;
    let d2 = d1.checked_sub(sst)?;
    // Price the forward out-of-the-money side directly (small values, tail-accurate N), the
    // other through parity. `nd1` is N(d1) and `nd2` N(d2); `pdf` is n(d1).
    let call_otm = s_dq <= k_dr;
    let (nd1, nd2, pdf, call, put) = if call_otm {
        let (nd1, pdf) = norm_cdf_pdf(d1)?;
        let nd2 = norm_cdf(d2)?;
        let call = non_negative(s_dq.mul_down(nd1)?.checked_sub(k_dr.mul_down(nd2)?)?);
        let put = call.checked_add(k_dr.checked_sub(s_dq)?)?;
        (nd1, nd2, pdf, call, put)
    } else {
        let (nmd1, pdf) = norm_cdf_pdf(d1.checked_neg()?)?;
        let nmd2 = norm_cdf(d2.checked_neg()?)?;
        let put = non_negative(k_dr.mul_down(nmd2)?.checked_sub(s_dq.mul_down(nmd1)?)?);
        let call = put.checked_add(s_dq.checked_sub(k_dr)?)?;
        (
            Fixed::ONE.checked_sub(nmd1)?,
            Fixed::ONE.checked_sub(nmd2)?,
            pdf,
            call,
            put,
        )
    };
    let premium = match opt.kind {
        OptionKind::Call => call,
        OptionKind::Put => put,
    };
    let price = Price::try_new(premium)?;
    if !greeks {
        return Ok(Priced {
            price,
            greeks: ZERO_GREEKS,
        });
    }
    // N(−d1) = 1 − N(d1) and N(−d2) = 1 − N(d2), exactly.
    let nmd1 = Fixed::ONE.checked_sub(nd1)?;
    let nmd2 = Fixed::ONE.checked_sub(nd2)?;
    let delta = match opt.kind {
        OptionKind::Call => dq.mul_down(nd1)?,
        OptionKind::Put => dq.mul_down(nmd1)?.checked_neg()?,
    };
    // gamma = Dq n(d1) / (S σ√T), with 1 / (S σ√T) = exp(−(ln S + ln σ√T)).
    let gamma = dq
        .mul_down(pdf)?
        .mul_down(exp(ln_s.checked_add(ls)?.checked_neg()?)?)?;
    // vega = S Dq n(d1) √T
    let s_dq_pdf = s_dq.mul_down(pdf)?;
    let vega = s_dq_pdf.mul_down(exp(half_ln_t)?)?;
    // theta = −S Dq n(d1) σ / (2√T) ∓ r K Dr N(±d2) ± q S Dq N(±d1)
    let decay = half(s_dq_pdf.mul_down(exp(ln_sigma.checked_sub(half_ln_t)?)?)?);
    let theta = match opt.kind {
        OptionKind::Call => decay
            .checked_neg()?
            .checked_sub(r.mul_down(k_dr.mul_down(nd2)?)?)?
            .checked_add(q.mul_down(s_dq.mul_down(nd1)?)?)?,
        OptionKind::Put => decay
            .checked_neg()?
            .checked_add(r.mul_down(k_dr.mul_down(nmd2)?)?)?
            .checked_sub(q.mul_down(s_dq.mul_down(nmd1)?)?)?,
    };
    Ok(Priced {
        price,
        greeks: Greeks {
            delta,
            gamma,
            vega,
            theta,
        },
    })
}

/// All-zero greeks, for `price` (which does not compute them).
const ZERO_GREEKS: Greeks = Greeks {
    delta: Fixed::ZERO,
    gamma: Fixed::ZERO,
    vega: Fixed::ZERO,
    theta: Fixed::ZERO,
};

/// Intrinsic value and its greeks at `T = 0` (where `Dq = Dr = 1`): the call
/// is worth `max(S Dq − K Dr, 0)` and the put `max(K Dr − S Dq, 0)`; delta is `±Dq` in the
/// money and 0 otherwise, gamma and vega are 0, and theta is the derivative of that intrinsic
/// value (`q S Dq − r K Dr` for an in-the-money call, its negative for an in-the-money put).
fn degenerate(
    kind: OptionKind,
    r: Fixed,
    q: Fixed,
    dq: Fixed,
    s_dq: Fixed,
    k_dr: Fixed,
) -> Result<Priced, PricingError> {
    let call_value = s_dq.checked_sub(k_dr)?;
    let itm = match kind {
        OptionKind::Call => call_value > Fixed::ZERO,
        OptionKind::Put => call_value < Fixed::ZERO,
    };
    if !itm {
        return Ok(Priced {
            price: Price::MIN,
            greeks: ZERO_GREEKS,
        });
    }
    let call_theta = q.mul_down(s_dq)?.checked_sub(r.mul_down(k_dr)?)?;
    let (value, delta, theta) = match kind {
        OptionKind::Call => (call_value, dq, call_theta),
        OptionKind::Put => (
            call_value.checked_neg()?,
            dq.checked_neg()?,
            call_theta.checked_neg()?,
        ),
    };
    Ok(Priced {
        price: Price::try_new(value)?,
        greeks: Greeks {
            delta,
            gamma: Fixed::ZERO,
            vega: Fixed::ZERO,
            theta,
        },
    })
}

/// `DomainError` unless `S/K` is in `[0.05, 20]`, checked exactly as `20 S >= K` and
/// `20 K >= S` (no division).
fn check_moneyness(s: Fixed, k: Fixed) -> Result<(), PricingError> {
    let s20 = s
        .raw()
        .checked_mul(MONEYNESS_LIMIT)
        .ok_or(PricingError::Overflow)?;
    let k20 = k
        .raw()
        .checked_mul(MONEYNESS_LIMIT)
        .ok_or(PricingError::Overflow)?;
    if s20 >= k.raw() && k20 >= s.raw() {
        Ok(())
    } else {
        Err(PricingError::DomainError)
    }
}

/// `x / 2`, rounded down.
fn half(x: Fixed) -> Fixed {
    Fixed::from_raw(x.raw() >> 1)
}

/// `max(x, 0)`: an out-of-the-money price is positive, and rounding must not make it negative.
fn non_negative(x: Fixed) -> Fixed {
    if x < Fixed::ZERO {
        Fixed::ZERO
    } else {
        x
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::disallowed_macros)]
mod tests {
    use super::*;
    use crate::Rounding;

    fn fx(num: i128, den: i128) -> Fixed {
        Fixed::from_ratio(num, den, Rounding::Down).unwrap()
    }

    fn inputs(s: i128, k: i128, t: Fixed, kind: OptionKind) -> (EuropeanOption, MarketInputs) {
        (
            EuropeanOption {
                kind,
                strike: Strike::try_new(Fixed::from_int(k as i64)).unwrap(),
                expiry: Years::try_new(t).unwrap(),
            },
            MarketInputs {
                spot: Spot::try_new(Fixed::from_int(s as i64)).unwrap(),
                vol: Volatility::try_new(fx(1, 5)).unwrap(),
                rate: Rate::try_new(fx(1, 20)).unwrap(),
                carry: Rate::try_new(Fixed::ZERO).unwrap(),
            },
        )
    }

    #[test]
    fn test_textbook_value() {
        // S = K = 100, T = 1, σ = 0.2, r = 0.05: C = 10.450583572185565, P = 5.573526022256971.
        let (opt, m) = inputs(100, 100, Fixed::ONE, OptionKind::Call);
        let c = price(&opt, &m).unwrap().get();
        let p = price(
            &EuropeanOption {
                kind: OptionKind::Put,
                ..opt
            },
            &m,
        )
        .unwrap()
        .get();
        let close = |v: Fixed, want: i128, den: i128| {
            let want = fx(want, den);
            (v.raw() - want.raw()).abs() < 1 << 20
        };
        assert!(
            close(c, 10_450_583_572_185_565, 1_000_000_000_000_000),
            "{c:?}"
        );
        assert!(
            close(p, 5_573_526_022_256_971, 1_000_000_000_000_000),
            "{p:?}"
        );
    }

    #[test]
    fn test_moneyness_and_expiry_edges() {
        let (opt, m) = inputs(100, 2001, Fixed::ONE, OptionKind::Call);
        assert_eq!(price(&opt, &m), Err(PricingError::DomainError));
        let (opt, m) = inputs(100, 2000, Fixed::ONE, OptionKind::Call);
        assert!(price(&opt, &m).is_ok());
        let (opt, m) = inputs(120, 100, Fixed::ZERO, OptionKind::Call);
        let pg = price_and_greeks(&opt, &m).unwrap();
        assert_eq!(pg.price.get(), Fixed::from_int(20));
        assert_eq!(pg.greeks.delta, Fixed::ONE);
        assert_eq!(intrinsic(&opt, m.spot).get(), Fixed::from_int(20));
        let put = EuropeanOption {
            kind: OptionKind::Put,
            ..opt
        };
        assert_eq!(price(&put, &m).unwrap(), Price::MIN);
        assert_eq!(intrinsic(&put, m.spot), Price::MIN);
    }
}
