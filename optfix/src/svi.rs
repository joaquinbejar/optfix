//! Raw SVI volatility slices (Gatheral 2004): the total implied variance
//!
//! ```text
//! w(k) = a + b (ρ (k − m) + sqrt((k − m)² + σ²)),   k = ln(K / F),   F = S e^{(r − q) T}
//! ```
//!
//! and the implied volatility `sqrt(w(k) / T)` of a strike (`docs/PLAN-phase2.md`, milestone
//! 22).
//!
//! Every step of [`SviParams::total_variance`] rounds down and `b >= 0`, so at an exact
//! argument `k` the result is never above `w(k)` and at most [`SVI_W_ERR_REL`] `× b +`
//! [`SVI_W_ERR_ABS`] below it. From market inputs the error has both signs: `k` itself is
//! within [`SVI_K_ERR`] of `ln(K / S) − (r − q) T`, and [`svi_vol_bounds`] and
//! [`svi_price_bounds`] carry that into rigorous enclosures of the volatility and the price at
//! the exact inputs.

use crate::bsm::{price, price_error_bound, EuropeanOption, MarketInputs};
use crate::explog::ln;
use crate::{Fixed, Price, PricingError, Rate, Spot, Strike, Volatility, Years};

/// `w` envelope, relative part: at an exact `k`, `0 <= w(k) − total_variance(k) <=
/// SVI_W_ERR_REL × b + SVI_W_ERR_ABS`. 5 ulps per unit of `b` (the derivation gives 4.5).
pub const SVI_W_ERR_REL: Fixed = Fixed::from_raw(5);

/// `w` envelope, absolute part: 1 ulp, the final rounding of `b × (…)`.
pub const SVI_W_ERR_ABS: Fixed = Fixed::from_raw(1);

/// `|k̂ − k*| < SVI_K_ERR` for [`log_forward_moneyness`]: 5 ulps, from two `ln` within
/// [`LN_MAX_ERROR_ULPS`](crate::LN_MAX_ERROR_ULPS) each and the difference of two products
/// rounded down, which is inside `(−1, 1)` ulps. Either sign occurs.
pub const SVI_K_ERR: Fixed = Fixed::from_raw(5);

/// `|a| <= 1000`.
const A_MAX: Fixed = Fixed::from_int(1000);
/// `b <= 10`.
const B_MAX: Fixed = Fixed::from_int(10);
/// `|m| <= 10`.
const M_MAX: Fixed = Fixed::from_int(10);
/// `σ >= 10^-6`: `floor(10^-6 × 2^64)`, rounded outward as the quantity bounds are.
const SIGMA_MIN: Fixed = Fixed::from_raw(18_446_744_073_709);
/// `σ <= 10`.
const SIGMA_MAX: Fixed = Fixed::from_int(10);
/// `|k| <= 32` for [`SviParams::total_variance`].
const K_MAX: Fixed = Fixed::from_int(32);

/// A raw SVI slice `(a, b, ρ, m, σ)`, valid by construction.
///
/// Built only by [`SviParams::try_new`], which checks the domain:
///
/// - `|a| <= 1000`, `0 <= b <= 10`, `|ρ| < 1` (strictly, in raw units), `|m| <= 10` and
///   `10^-6 <= σ <= 10` (the lower bound rounded outward, as the quantity bounds are);
/// - [`SviParams::min_total_variance`] `>= 0`: a rigorous lower bound of the slice's minimum
///   total variance `a + b σ sqrt(1 − ρ²)`, so an accepted slice has `w >= 0` everywhere. A
///   slice whose true minimum is just above 0 can be refused: the bound is low by at most
///   `b σ (1 / (2 sqrt(δ)) + 1) + 2` ulps, `δ = 1 − ρ²` rounded down (3.5 ulps for
///   `1 − |ρ| = 0.01`, 354 for `10^-6`, `2^31` at one ulp).
///
/// The ranges cover any slice whose volatilities lie in [`Volatility`]'s `[0.005, 5]` up to
/// 30 years (`w <= 750`) and rule out overflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SviParams {
    a: Fixed,
    b: Fixed,
    rho: Fixed,
    m: Fixed,
    sigma: Fixed,
    /// The lower bound of the minimum, computed once by `try_new`.
    min_w: Fixed,
}

impl SviParams {
    /// Checks the domain (see [`SviParams`]) and builds the slice.
    ///
    /// # Errors
    ///
    /// `DomainError` if a parameter is outside its range or the lower bound of the minimum
    /// total variance is negative.
    #[must_use = "the result carries a possible error"]
    pub fn try_new(
        a: Fixed,
        b: Fixed,
        rho: Fixed,
        m: Fixed,
        sigma: Fixed,
    ) -> Result<Self, PricingError> {
        let in_range = a >= A_MAX.checked_neg()?
            && a <= A_MAX
            && b >= Fixed::ZERO
            && b <= B_MAX
            && rho > Fixed::ONE.checked_neg()?
            && rho < Fixed::ONE
            && m >= M_MAX.checked_neg()?
            && m <= M_MAX
            && sigma >= SIGMA_MIN
            && sigma <= SIGMA_MAX;
        if !in_range {
            return Err(PricingError::DomainError);
        }
        let min_w = min_lower_bound(a, b, rho, sigma)?;
        if min_w < Fixed::ZERO {
            return Err(PricingError::DomainError);
        }
        Ok(Self {
            a,
            b,
            rho,
            m,
            sigma,
            min_w,
        })
    }

    /// `a`, the level.
    #[must_use]
    pub const fn a(&self) -> Fixed {
        self.a
    }

    /// `b`, the slope scale (`>= 0`).
    #[must_use]
    pub const fn b(&self) -> Fixed {
        self.b
    }

    /// `ρ`, the skew (`|ρ| < 1`).
    #[must_use]
    pub const fn rho(&self) -> Fixed {
        self.rho
    }

    /// `m`, the horizontal shift.
    #[must_use]
    pub const fn m(&self) -> Fixed {
        self.m
    }

    /// `σ`, the curvature at the minimum (`> 0`).
    #[must_use]
    pub const fn sigma(&self) -> Fixed {
        self.sigma
    }

    /// A rigorous lower bound of the minimum total variance `a + b σ sqrt(1 − ρ²)`, never
    /// negative for a slice that [`SviParams::try_new`] accepted.
    ///
    /// One interval chain: `ρ²` rounded up (so `1 − ρ²` is low; it stays positive since
    /// `|ρ| < 1` strictly), its complement's root rounded down, `b σ` and the product rounded
    /// down, then `a +` exactly.
    #[must_use]
    pub const fn min_total_variance(&self) -> Fixed {
        self.min_w
    }

    /// `w(k)`, rounded down once per operation in a fixed order: at an exact `k`,
    /// `0 <= w(k) − total_variance(k) <= E_w` with `E_w =` [`SVI_W_ERR_REL`] `× b +`
    /// [`SVI_W_ERR_ABS`].
    ///
    /// The root `sqrt(x² + σ²)`, `x = k − m`, is `sqrt(x² + σ²)` when `max(|x|, σ) >= 1`
    /// (at most 2 ulps low) and `M sqrt(1 + (μ / M)²)` with `M = max(|x|, σ)`,
    /// `μ = min(|x|, σ)` below it (under 3.5 ulps low), so a tiny `σ` keeps its precision.
    ///
    /// # Errors
    ///
    /// `DomainError` if `|k| > 32`.
    #[must_use = "the result carries a possible error"]
    pub fn total_variance(&self, k: Fixed) -> Result<Fixed, PricingError> {
        if k > K_MAX || k < K_MAX.checked_neg()? {
            return Err(PricingError::DomainError);
        }
        let x = k.checked_sub(self.m)?;
        let ax = abs(x)?;
        let (big, small) = if ax >= self.sigma {
            (ax, self.sigma)
        } else {
            (self.sigma, ax)
        };
        let root = if big >= Fixed::ONE {
            x.mul_down(x)?
                .checked_add(self.sigma.mul_down(self.sigma)?)?
                .sqrt_down()?
        } else {
            // big >= σ >= 10^-6 > 0, and q = small / big is in [0, 1].
            let q = small.div_down(big)?;
            let r = Fixed::ONE.checked_add(q.mul_down(q)?)?.sqrt_down()?;
            big.mul_down(r)?
        };
        let inner = self.rho.mul_down(x)?.checked_add(root)?;
        self.a.checked_add(self.b.mul_down(inner)?)
    }

    /// A rigorous enclosure `lo <= w(k) <= hi` at an exact `k`: `lo` is
    /// [`SviParams::total_variance`] and `hi` adds the envelope `E_w`, rounded up.
    ///
    /// # Errors
    ///
    /// As [`SviParams::total_variance`].
    #[must_use = "the result carries a possible error"]
    pub fn total_variance_bounds(&self, k: Fixed) -> Result<(Fixed, Fixed), PricingError> {
        let lo = self.total_variance(k)?;
        Ok((lo, lo.checked_add(self.w_err()?)?))
    }

    /// `E_w = SVI_W_ERR_REL × b + SVI_W_ERR_ABS`, rounded up.
    fn w_err(&self) -> Result<Fixed, PricingError> {
        SVI_W_ERR_REL.mul_up(self.b)?.checked_add(SVI_W_ERR_ABS)
    }

    /// `E_k = SVI_K_ERR × b (1 + |ρ|)`, rounded up: the largest change of `w` over `k`'s error,
    /// since `|w'| = b |ρ + x / s| < b (1 + |ρ|)` everywhere.
    fn k_err(&self) -> Result<Fixed, PricingError> {
        let slope = self.b.mul_up(Fixed::ONE.checked_add(abs(self.rho)?)?)?;
        SVI_K_ERR.mul_up(slope)
    }
}

/// The lower bound of `a + b σ sqrt(1 − ρ²)` (see [`SviParams::min_total_variance`]).
fn min_lower_bound(a: Fixed, b: Fixed, rho: Fixed, sigma: Fixed) -> Result<Fixed, PricingError> {
    let rho2 = rho.mul_up(rho)?;
    let root = Fixed::ONE.checked_sub(rho2)?.sqrt_down()?;
    a.checked_add(b.mul_down(sigma)?.mul_down(root)?)
}

/// `|x|`.
fn abs(x: Fixed) -> Result<Fixed, PricingError> {
    if x < Fixed::ZERO {
        x.checked_neg()
    } else {
        Ok(x)
    }
}

/// Market inputs of an SVI slice: [`MarketInputs`] without the volatility, which the slice
/// gives per strike.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SviMarket {
    /// Price of the underlying.
    pub spot: Spot,
    /// Continuously compounded risk-free rate `r`.
    pub rate: Rate,
    /// Continuous carry `q`; zero for none.
    pub carry: Rate,
}

/// `k̂ = ln K − ln S − (r ⊗ T − q ⊗ T)`, both products rounded down, as the strip computes
/// it: within [`SVI_K_ERR`] of `k* = ln(K / S) − (r − q) T` at the exact inputs, either sign.
///
/// # Errors
///
/// None over the quantity ranges; the `Result` carries the checked arithmetic.
#[must_use = "the result carries a possible error"]
pub fn log_forward_moneyness(
    expiry: Years,
    market: &SviMarket,
    strike: Strike,
) -> Result<Fixed, PricingError> {
    let t = expiry.get();
    let rt = market.rate.get().mul_down(t)?;
    let qt = market.carry.get().mul_down(t)?;
    ln(strike.get())?
        .checked_sub(ln(market.spot.get())?)?
        .checked_sub(rt.checked_sub(qt)?)
}

/// `ŵ = total_variance(k̂)` and `T`, refusing `T = 0`.
fn variance_at(
    params: &SviParams,
    expiry: Years,
    market: &SviMarket,
    strike: Strike,
) -> Result<(Fixed, Fixed), PricingError> {
    let t = expiry.get();
    if t == Fixed::ZERO {
        return Err(PricingError::DomainError);
    }
    let k = log_forward_moneyness(expiry, market, strike)?;
    Ok((params.total_variance(k)?, t))
}

/// The implied volatility of `strike`: `sqrt(ŵ / T)` with `ŵ = total_variance(k̂)` and both the
/// division and the root rounded down.
///
/// Against `σ* = sqrt(w(k*) / T)` at the exact inputs the error has both signs, since `k̂` does
/// (see [`SVI_K_ERR`]); [`svi_vol_bounds`] encloses `σ*`. With `δ− = (E_w + E_k) / T + 1 ulp`
/// and `δ+ = E_k / T`, `E_k =` [`SVI_K_ERR`] `× b (1 + |ρ|)`:
/// `−(δ− / (2 σ̂) + 1 ulp) <= σ̂ − σ* <= δ+ / σ̂`.
///
/// # Errors
///
/// `DomainError` if `T = 0`, if `|k̂| > 32`, if `ŵ < 0`, or if the result is outside
/// [`Volatility`]'s range; `Overflow` if `ŵ / T` does not fit (a very small `T`).
#[must_use = "the result carries a possible error"]
pub fn svi_vol(
    params: &SviParams,
    expiry: Years,
    market: &SviMarket,
    strike: Strike,
) -> Result<Volatility, PricingError> {
    let (w, t) = variance_at(params, expiry, market, strike)?;
    Volatility::try_new(w.div_down(t)?.sqrt_down()?)
}

/// A rigorous enclosure `lo <= σ* <= hi` of `σ* = sqrt(w(k*) / T)` at the exact inputs, which
/// also holds [`svi_vol`]'s result: `lo = sqrt((ŵ − E_k) / T)` rounded down, or 0 when
/// `ŵ − E_k <= 0`, and `hi = sqrt((ŵ + E_w + E_k) / T)` rounded up.
///
/// Plain `Fixed`, since either end may leave [`Volatility`]'s range by a few ulps.
///
/// # Errors
///
/// As [`svi_vol`], without the [`Volatility`] range check.
#[must_use = "the result carries a possible error"]
pub fn svi_vol_bounds(
    params: &SviParams,
    expiry: Years,
    market: &SviMarket,
    strike: Strike,
) -> Result<(Fixed, Fixed), PricingError> {
    let (w, t) = variance_at(params, expiry, market, strike)?;
    let ek = params.k_err()?;
    let w_lo = w.checked_sub(ek)?;
    // w(k*) >= 0 on an accepted slice, so 0 is a valid lower bound.
    let lo = if w_lo <= Fixed::ZERO {
        Fixed::ZERO
    } else {
        w_lo.div_down(t)?.sqrt_down()?
    };
    let w_hi = w.checked_add(params.w_err()?)?.checked_add(ek)?;
    Ok((lo, w_hi.div_up(t)?.sqrt_up()?))
}

/// A rigorous enclosure of the Black-Scholes-Merton premium at `σ*`, the slice's volatility at
/// the exact inputs: the price at each end of [`svi_vol_bounds`], widened by
/// [`price_error_bound`] outward ([`Price::lower`], [`Price::upper`]). Sound because the
/// premium never decreases in `σ` (vega `S Dq n(d1) √T >= 0` for calls and puts).
///
/// # Errors
///
/// `DomainError` if `T = 0`, if an end of the volatility enclosure is outside [`Volatility`]'s
/// range, or as [`price`] (`S/K` outside `[0.05, 20]`); otherwise as [`svi_vol_bounds`].
#[must_use = "the result carries a possible error"]
pub fn svi_price_bounds(
    opt: &EuropeanOption,
    market: &SviMarket,
    params: &SviParams,
) -> Result<(Price, Price), PricingError> {
    let (lo, hi) = svi_vol_bounds(params, opt.expiry, market, opt.strike)?;
    let at = |vol: Fixed| -> Result<(Price, Fixed), PricingError> {
        let m = MarketInputs {
            spot: market.spot,
            vol: Volatility::try_new(vol)?,
            rate: market.rate,
            carry: market.carry,
        };
        Ok((price(opt, &m)?, price_error_bound(opt, &m)?))
    };
    let (p_lo, e_lo) = at(lo)?;
    let (p_hi, e_hi) = at(hi)?;
    Ok((p_lo.lower(e_lo)?, p_hi.upper(e_hi)?))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]
mod tests {
    use super::{SviMarket, SviParams, SIGMA_MIN};
    use crate::{Fixed, PricingError, Rate, Rounding, Spot};

    fn fx(num: i128, den: i128) -> Fixed {
        Fixed::from_ratio(num, den, Rounding::Down).unwrap()
    }

    fn slice() -> SviParams {
        SviParams::try_new(fx(4, 100), fx(1, 10), fx(-3, 10), Fixed::ZERO, fx(2, 10)).unwrap()
    }

    #[test]
    fn test_try_new_domain_edges() {
        let ok = |a, b, rho, m, s| SviParams::try_new(a, b, rho, m, s).is_ok();
        let (a, b, rho, m, s) = (fx(1, 10), fx(1, 10), Fixed::ZERO, Fixed::ZERO, fx(1, 10));
        assert!(ok(a, b, rho, m, s));
        let ulp = Fixed::from_raw(1);
        let one = Fixed::ONE;
        assert!(ok(a, b, one.checked_sub(ulp).unwrap(), m, s));
        assert!(!ok(a, b, one, m, s));
        assert!(!ok(a, b, one.checked_neg().unwrap(), m, s));
        assert!(ok(a, Fixed::ZERO, rho, m, s));
        assert!(!ok(a, Fixed::from_raw(-1), rho, m, s));
        assert!(ok(a, Fixed::from_int(10), rho, m, s));
        assert!(!ok(
            a,
            Fixed::from_int(10).checked_add(ulp).unwrap(),
            rho,
            m,
            s
        ));
        assert!(ok(a, b, rho, m, SIGMA_MIN));
        assert!(!ok(a, b, rho, m, SIGMA_MIN.checked_sub(ulp).unwrap()));
        assert!(ok(a, b, rho, m, Fixed::from_int(10)));
        assert!(!ok(
            a,
            b,
            rho,
            m,
            Fixed::from_int(10).checked_add(ulp).unwrap()
        ));
        assert!(ok(a, b, rho, Fixed::from_int(-10), s));
        assert!(!ok(
            a,
            b,
            rho,
            Fixed::from_int(10).checked_add(ulp).unwrap(),
            s
        ));
        assert!(ok(Fixed::from_int(1000), b, rho, m, s));
        assert!(!ok(
            Fixed::from_int(1000).checked_add(ulp).unwrap(),
            b,
            rho,
            m,
            s
        ));
        // A negative minimum is refused.
        assert!(!ok(
            fx(-2, 10),
            fx(1, 10),
            Fixed::ZERO,
            Fixed::ZERO,
            fx(1, 10)
        ));
    }

    #[test]
    fn test_min_total_variance_refuses_negative_true_minimum_near_unit_rho() {
        // ρ = 1 − 2 ulps, b = σ = 10, a raw −858993459200: the true minimum is −1.26e-27,
        // and ρ² rounded down would give exactly 0 and accept it.
        let ten = Fixed::from_int(10);
        let a = Fixed::from_raw(-858_993_459_200);
        for rho in [
            Fixed::from_raw((1 << 64) - 2),
            Fixed::from_raw(2 - (1 << 64)),
        ] {
            assert_eq!(
                SviParams::try_new(a, ten, rho, Fixed::ZERO, ten),
                Err(PricingError::DomainError)
            );
        }
    }

    #[test]
    fn test_total_variance_k_range() {
        let s = slice();
        assert!(s.total_variance(Fixed::from_int(32)).is_ok());
        assert!(s.total_variance(Fixed::from_int(-32)).is_ok());
        let ulp = Fixed::from_raw(1);
        assert_eq!(
            s.total_variance(Fixed::from_int(32).checked_add(ulp).unwrap()),
            Err(PricingError::DomainError)
        );
        assert_eq!(
            s.total_variance(Fixed::from_int(-32).checked_sub(ulp).unwrap()),
            Err(PricingError::DomainError)
        );
    }

    #[test]
    fn test_svi_vol_refuses_zero_expiry() {
        let market = SviMarket {
            spot: Spot::try_new(Fixed::from_int(100)).unwrap(),
            rate: Rate::try_new(Fixed::ZERO).unwrap(),
            carry: Rate::try_new(Fixed::ZERO).unwrap(),
        };
        let k = crate::Strike::try_new(Fixed::from_int(100)).unwrap();
        let t = crate::Years::try_new(Fixed::ZERO).unwrap();
        assert_eq!(
            super::svi_vol(&slice(), t, &market, k),
            Err(PricingError::DomainError)
        );
        assert_eq!(
            super::svi_vol_bounds(&slice(), t, &market, k),
            Err(PricingError::DomainError)
        );
    }
}
