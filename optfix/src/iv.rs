//! Implied volatility: the `σ` at which the Black-Scholes-Merton price of `bsm` equals a target
//! (`docs/PLAN.md` 4.6).
//!
//! The solve works on the option that is out of the money by forward moneyness (`S Dq` against
//! `K Dr`, as `price` does), whose price `p` lies in `(0, bound)` with `bound = S Dq` for a call
//! and `K Dr` for a put; an in-the-money target is moved there by put-call parity, which `price`
//! applies exactly the other way. In normalized form (`a = |ln(S Dq / K Dr)|`, `s = σ√T`,
//! `y = p / bound`, `L = −ln y`) the initial guess of `ln s` is a bilinear interpolation in the
//! tables of `tools/gen_iv_tables.py`, within 6% of the root over the supported domain.
//!
//! Up to [`IV_MAX_ITERATIONS`] third-order (Chebyshev) steps then refine `v = ln σ` on a log
//! objective: `g = ln p(v) − ln p` below the inflection point `s = √(2a)`, where the price is
//! small and convex in `σ`, and `g = ln(bound − p(v)) − ln(bound − p)` above it, where the price
//! nears its bound. With `p' = s S Dq n(d1)`, `g'' / g' = 1 + d1 d2 − g'`, so the step
//! `−(g / g')(1 + (g / g') g'' / (2 g'))` needs `g / g'` only, which is `g e^(−ln |g'|)` with
//! `ln |g'|` known in closed form (`ln n(d1) = −d1² / 2 − ln √(2π)`): no division. Each step
//! prices with the same operations as `price`, so the solve inverts the function `price`
//! computes.
//!
//! The answer is only as good as the target determines it: a price error `E` moves the root by
//! `E / vega`. So `implied_vol` answers only where vega at the root is at least
//! [`IV_VEGA_MIN_FACTOR`] times the price envelope, and returns `IllConditioned` elsewhere
//! (deep in or out of the money, or close to expiry), never a best effort.

use crate::bsm::{
    check_moneyness, d1_terms, half, non_negative, price_envelope, EuropeanOption, LN2_RAW,
};
use crate::explog::{exp, ln};
use crate::iv_tables::{
    DEEP, DEEP_W_MIN, DEEP_Z_MIN, NEAR, NEAR_U_MIN, NEAR_Z_MIN, SWITCH, Z_FINE,
};
use crate::norm::norm_cdf;
use crate::{Fixed, OptionKind, Price, PricingError, Rate, Spot, Volatility};

/// Market inputs of an implied volatility: those of a price, without the volatility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IvInputs {
    /// Price of the underlying.
    pub spot: Spot,
    /// Continuously compounded risk-free rate `r`.
    pub rate: Rate,
    /// Continuous carry `q` (dividend yield, foreign rate, or lease rate); zero for none.
    pub carry: Rate,
}

/// The most refinement steps [`implied_vol`] takes; it stops earlier once a step is below
/// [`IV_TOLERANCE`]. A fixed cap keeps the CU cost bounded (`docs/benchmarks.md`).
pub const IV_MAX_ITERATIONS: u32 = 3;

/// Convergence: a step in `ln σ` of at most `2^-16`. The steps converge cubically, so the error
/// left after such a step is of order `2^-48` in relative `σ`, below the round-trip envelope's
/// [`IV_ERR_REL`].
pub const IV_TOLERANCE: Fixed = Fixed::from_raw(1 << 48);

/// The conditioning threshold: `implied_vol` answers only where `vega >= IV_VEGA_MIN_FACTOR ×
/// E` at the exact root, with `vega = ∂price/∂σ` and `E` the price envelope of
/// [`crate::price_error_bound`]; below it the price's rounding moves the root by more than
/// `2^-28` in `σ`, and the result is `IllConditioned`. `2^28`. The computed vega at the returned
/// root must clear the threshold by a relative margin `δ` (about `|d1 d2|` times the round-trip
/// envelope over `σ`, plus `2^-20`), so a root just above the threshold may also be rejected,
/// and none below it is answered.
pub const IV_VEGA_MIN_FACTOR: Fixed = Fixed::from_raw(1 << 92);

/// Round-trip envelope, price part: where `implied_vol` answers,
/// `|implied_vol(price(σ)) − σ| <= IV_ERR_PRICE_FACTOR × E / vega + IV_ERR_REL × σ`. 4: the
/// target and every evaluation are each within `E` of the exact price.
pub const IV_ERR_PRICE_FACTOR: Fixed = Fixed::from_raw(4 << 64);

/// Round-trip envelope, relative part (see [`IV_ERR_PRICE_FACTOR`]): `2^-40`.
pub const IV_ERR_REL: Fixed = Fixed::from_raw(1 << 24);

/// The volatility whose Black-Scholes-Merton price (as [`crate::price`] computes it) equals
/// `target`, within the round-trip envelope of [`IV_ERR_PRICE_FACTOR`].
///
/// # Errors
///
/// - `ArbitrageBound` unless `max(0, S Dq − K Dr) < target < S Dq` for a call or
///   `max(0, K Dr − S Dq) < target < K Dr` for a put: at the bounds the volatility is zero or
///   infinite.
/// - `IllConditioned` if vega at the root is below [`IV_VEGA_MIN_FACTOR`] times the price
///   envelope (or within its margin above it): the target does not determine `σ` to `2^-28`
///   (deep in or out of the money, or close to expiry). Checked at the returned root, and at
///   the smallest-vega evaluation when the steps did not converge.
/// - `NoConvergence` if no step falls below [`IV_TOLERANCE`] within [`IV_MAX_ITERATIONS`] at a
///   well-conditioned target (none over the milestone 5 grid, `docs/precision.md`).
/// - `DomainError` if `S/K` is outside `[0.05, 20]`, if `T = 0` (the price no longer depends
///   on `σ`), or if the implied volatility is outside the [`Volatility`] range by more than its
///   round-trip envelope (within it, the result is that range end).
/// - `Overflow` only carries the checked arithmetic.
#[must_use = "the result carries a possible error"]
pub fn implied_vol(
    opt: &EuropeanOption,
    m: &IvInputs,
    target: Price,
) -> Result<Volatility, PricingError> {
    solve(opt, m, target, IV_MAX_ITERATIONS, true).0
}

/// The step cap in `ln σ`: a factor of `e` per step.
const MAX_STEP: Fixed = Fixed::ONE;

/// The iterate stays within `[−6.5, 2.75]` in `ln σ`, about `[0.0015, 15.6]`: past the
/// [`Volatility`] range by more than one step on either side.
const V_MIN: Fixed = Fixed::from_raw(-13 << 63);
const V_MAX: Fixed = Fixed::from_raw(11 << 62);

/// The relative allowance for the rounding of the vega that judges conditioning: `2^-20`.
const VEGA_JUDGE_REL: Fixed = Fixed::from_raw(1 << 44);

/// `ln √(2π)`, rounded to nearest.
const LN_SQRT_2PI: Fixed = Fixed::from_raw(16_951_423_941_496_644_917);

/// `1 / |g'|` is capped at `e^40`: past it the Newton step is beyond [`MAX_STEP`] anyway.
const INV_SLOPE_LN_CAP: Fixed = Fixed::from_raw(40 << 64);

/// `|d1|` from which `d1²` is not formed (it would not fit): `n(d1)` is then far below any
/// representable value.
const D1_LIMIT: u128 = 1 << 88;

/// `ln a` for `a = 0` (at the money forward): any value below the tables' first column.
const LN_A_ZERO: Fixed = Fixed::from_raw(-1000 << 64);

/// The smallest `L` the guess uses (`Z = ln L >= −44`): `y` closer to 1 than this rounds away.
const L_MIN: Fixed = Fixed::from_raw(1);

/// What the solve needs from the inputs, computed once.
struct Setup {
    s: Fixed,
    k: Fixed,
    r: Fixed,
    q: Fixed,
    ln_s: Fixed,
    ln_k: Fixed,
    half_ln_t: Fixed,
    ln_s_dq: Fixed,
    s_dq: Fixed,
    k_dr: Fixed,
    call_otm: bool,
    /// Above the inflection point: the objective is `ln(bound − p)`.
    upper: bool,
    /// `ln` of the objective's target value.
    target_ln: Fixed,
    /// The initial guess of `ln σ`.
    v0: Fixed,
    /// The guess came from the deep out-of-the-money table (read by the bench-internals hook).
    #[cfg_attr(not(feature = "bench-internals"), allow(dead_code))]
    deep: bool,
    /// The price envelope `E`.
    envelope: Fixed,
    /// `IV_VEGA_MIN_FACTOR × E`.
    vega_min: Fixed,
}

/// Checks the inputs and the bounds, moves the target to the out-of-the-money side and forms
/// the initial guess.
fn setup(opt: &EuropeanOption, m: &IvInputs, target: Price) -> Result<Setup, PricingError> {
    let (s, k, t) = (m.spot.get(), opt.strike.get(), opt.expiry.get());
    let (r, q) = (m.rate.get(), m.carry.get());
    check_moneyness(s, k)?;
    if t == Fixed::ZERO {
        return Err(PricingError::DomainError);
    }
    // The same discount factors and forward comparison as `price`.
    let rt = r.mul_down(t)?;
    let qt = q.mul_down(t)?;
    let dr = exp(rt.checked_neg()?)?;
    let dq = exp(qt.checked_neg()?)?;
    let s_dq = s.mul_down(dq)?;
    let k_dr = k.mul_down(dr)?;
    let call_otm = s_dq <= k_dr;
    // `price` computes the out-of-the-money side and adds the parity term exactly; undo it.
    let p = match (opt.kind, call_otm) {
        (OptionKind::Call, true) | (OptionKind::Put, false) => target.get(),
        (OptionKind::Call, false) => target.get().checked_sub(s_dq.checked_sub(k_dr)?)?,
        (OptionKind::Put, true) => target.get().checked_sub(k_dr.checked_sub(s_dq)?)?,
    };
    let bound = if call_otm { s_dq } else { k_dr };
    if p <= Fixed::ZERO || p >= bound {
        return Err(PricingError::ArbitrageBound);
    }

    let ln_s = ln(s)?;
    let ln_k = ln(k)?;
    let half_ln_t = half(ln(t)?);
    let ln_s_dq = ln_s.checked_sub(qt)?;
    let ln_bound = if call_otm {
        ln_s_dq
    } else {
        ln_k.checked_sub(rt)?
    };
    let ln_p = ln(p)?;
    // a = |ln(S Dq / K Dr)|
    let a = Fixed::from_raw(
        ln_s.checked_sub(ln_k)?
            .checked_add(rt.checked_sub(qt)?)?
            .raw()
            .checked_abs()
            .ok_or(PricingError::Overflow)?,
    );
    let ln_a = if a > Fixed::ZERO { ln(a)? } else { LN_A_ZERO };
    let (ln_s0, deep) = guess(ln_a, ln_bound.checked_sub(ln_p)?.max(L_MIN))?;
    // Above the inflection point s = √(2a) when ln s > (ln 2 + ln a) / 2.
    let upper = ln_s0 > half(Fixed::from_raw(LN2_RAW).checked_add(ln_a)?);
    let target_ln = if upper {
        ln(bound.checked_sub(p)?)?
    } else {
        ln_p
    };
    let envelope = price_envelope(s, k)?;
    Ok(Setup {
        s,
        k,
        r,
        q,
        ln_s,
        ln_k,
        half_ln_t,
        ln_s_dq,
        s_dq,
        k_dr,
        call_otm,
        upper,
        target_ln,
        v0: ln_s0.checked_sub(half_ln_t)?.clamp(V_MIN, V_MAX),
        deep,
        envelope,
        vega_min: IV_VEGA_MIN_FACTOR.mul_up(envelope)?,
    })
}

/// The solve with at most `cap` steps; `early_exit = false` takes all of them (the worst-case
/// CU row of the bench) and judges convergence by the last. Returns the steps taken.
fn solve(
    opt: &EuropeanOption,
    m: &IvInputs,
    target: Price,
    cap: u32,
    early_exit: bool,
) -> (Result<Volatility, PricingError>, u32) {
    let mut steps = 0;
    let result = setup(opt, m, target).and_then(|st| iterate(&st, cap, early_exit, &mut steps));
    (result, steps)
}

fn iterate(
    st: &Setup,
    cap: u32,
    early_exit: bool,
    steps: &mut u32,
) -> Result<Volatility, PricingError> {
    let mut v = st.v0;
    let mut converged = false;
    // The smallest ln vega over the evaluations (None: below any representable value).
    let mut ln_vega_min: Option<Fixed> = None;
    while *steps < cap {
        *steps = steps.checked_add(1).ok_or(PricingError::Overflow)?;
        let Eval {
            ls,
            d1,
            d2,
            ln_pdf,
            ln_vega,
        } = evaluate(st, v)?;
        ln_vega_min = match (*steps, ln_vega, ln_vega_min) {
            (1, now, _) => now,
            (_, Some(now), Some(low)) => Some(now.min(low)),
            _ => None,
        };
        // The objective's argument: bound − p(v) above the inflection point, p(v) below, each
        // formed without cancellation (bound − p = S Dq N(−d1) + K Dr N(d2) for both kinds).
        let value = if st.upper {
            st.s_dq
                .mul_down(norm_cdf(d1.checked_neg()?)?)?
                .checked_add(st.k_dr.mul_down(norm_cdf(d2)?)?)?
        } else if st.call_otm {
            non_negative(
                st.s_dq
                    .mul_down(norm_cdf(d1)?)?
                    .checked_sub(st.k_dr.mul_down(norm_cdf(d2)?)?)?,
            )
        } else {
            non_negative(
                st.k_dr
                    .mul_down(norm_cdf(d2.checked_neg()?)?)?
                    .checked_sub(st.s_dq.mul_down(norm_cdf(d1.checked_neg()?)?)?)?,
            )
        };
        let step = match ln_pdf {
            Some(lp) if value > Fixed::ZERO => {
                let lv = ln(value)?;
                let g = lv.checked_sub(st.target_ln)?;
                // ln |g'| = ln(s S Dq n(d1) / value)
                let ln_slope = st
                    .ln_s_dq
                    .checked_add(ls)?
                    .checked_add(lp)?
                    .checked_sub(lv)?;
                chebyshev_step(g, ln_slope, d1, d2, st.upper)?
            }
            // The price underflowed or n(d1) vanished: σ is far too low below the inflection
            // point, or far too high above it.
            _ if st.upper => MAX_STEP.checked_neg()?,
            _ => MAX_STEP,
        };
        // The objective is monotone in v, so a step out of [V_MIN, V_MAX] from its end means
        // the root lies beyond it, outside the Volatility range.
        if (v == V_MAX && step > Fixed::ZERO) || (v == V_MIN && step < Fixed::ZERO) {
            return Err(PricingError::DomainError);
        }
        v = v.checked_add(step)?.clamp(V_MIN, V_MAX);
        converged = step.raw().unsigned_abs() <= IV_TOLERANCE.raw().unsigned_abs();
        if converged && early_exit {
            break;
        }
    }
    if !converged {
        // A solve that wandered away from an ill-conditioned root still reports it: judged at
        // the evaluation where vega was smallest.
        let well_conditioned = match ln_vega_min {
            Some(lv) => exp(lv)? >= st.vega_min,
            None => false,
        };
        return Err(if well_conditioned {
            PricingError::NoConvergence
        } else {
            PricingError::IllConditioned
        });
    }
    // Conditioning, judged at the returned root (codex's #12 review): vega = S Dq n(d1) √T there,
    // against the threshold widened by `δ` (below) so that a true-root vega under it cannot
    // pass.
    let root = evaluate(st, v)?;
    let Some(ln_vega) = root.ln_vega else {
        return Err(PricingError::IllConditioned);
    };
    let vega = exp(ln_vega)?;
    if vega < st.vega_min {
        return Err(PricingError::IllConditioned);
    }
    let sigma = exp(v)?;
    // slack: the round-trip envelope at the root, `IV_ERR_PRICE_FACTOR × E / vega + IV_ERR_REL ×
    // σ`, the documented bound on |σ − σ*| between the returned σ and the exact root σ* of the
    // target: the target and every evaluation are each within E of the exact price, so the
    // computed root is within about 2E / vega of σ* (factor 4 for margin), plus the cubic
    // remainder of the last step (`IV_ERR_REL`). 1 / vega = e^(−ln vega), at most 2^30 here
    // since vega >= 2^28 E >= 2^28 × 64 ulps. Measured against exact mpmath roots
    // (`tests/iv_fixture.rs`): at most 0.8% of it.
    let slack = IV_ERR_PRICE_FACTOR
        .mul_up(st.envelope)?
        .mul_up(exp(ln_vega.checked_neg()?)?)?
        .checked_add(IV_ERR_REL.mul_up(sigma)?)?;
    // δ = (|d1 d2| + 1) × slack / σ + 2^-20, the vega derivative factor times the distance to
    // the root, plus the rounding of vega itself:
    //
    // - ∂d1/∂σ = −d2 / σ (from d1 = ln(F/K) / (σ√T) + σ√T / 2), so ∂ ln vega / ∂ σ =
    //   −d1 ∂d1/∂σ = d1 d2 / σ, that is d ln vega / d ln σ = d1 d2;
    // - |ln σ − ln σ*| <= slack / σ (to first order; slack / σ <= 2^-20 here), so
    //   |ln vega(σ) − ln vega(σ*)| <= |d1 d2| × slack / σ, with d1 d2 at the returned σ; the `+ 1`
    //   covers d1 d2's own change over that interval, which is far below 1 at a relative
    //   width of 2^-20;
    // - `VEGA_JUDGE_REL` (2^-20) covers the rounding of the computed vega (`d1`, `ln S`, `qT`,
    //   `ln T` and `exp`, each orders of magnitude smaller) and the second-order terms of
    //   e^(±δ').
    //
    // Then vega >= thr (1 + δ) implies vega(σ*) >= thr. Checked on the threshold fixtures:
    // none of the targets whose exact-root vega is below the threshold is answered
    // (`tests/iv_fixture.rs`).
    let spread = Fixed::from_raw(
        root.d1
            .mul_up(root.d2)?
            .raw()
            .checked_abs()
            .ok_or(PricingError::Overflow)?,
    )
    .checked_add(Fixed::ONE)?;
    let delta = spread
        .mul_up(slack.mul_up(exp(v.checked_neg()?)?)?)?
        .checked_add(VEGA_JUDGE_REL)?;
    if vega < st.vega_min.mul_up(Fixed::ONE.checked_add(delta)?)? {
        return Err(PricingError::IllConditioned);
    }
    to_volatility(sigma, slack)
}

/// One evaluation at `v = ln σ`: `ln σ√T`, `d1`, `d2`, `ln n(d1)` and `ln vega`, the last two
/// `None` when `d1²` would not fit (`n(d1)` is then zero).
struct Eval {
    ls: Fixed,
    d1: Fixed,
    d2: Fixed,
    ln_pdf: Option<Fixed>,
    ln_vega: Option<Fixed>,
}

fn evaluate(st: &Setup, v: Fixed) -> Result<Eval, PricingError> {
    let ls = v.checked_add(st.half_ln_t)?;
    let sst = exp(ls)?;
    let d1 = d1_terms(
        st.s,
        st.k,
        st.r,
        st.q,
        st.ln_s,
        st.ln_k,
        v,
        st.half_ln_t,
        ls,
        sst,
    )?;
    let d2 = d1.checked_sub(sst)?;
    // ln n(d1) = −d1² / 2 − ln √(2π)
    let ln_pdf = if d1.raw().unsigned_abs() < D1_LIMIT {
        Some(
            half(d1.mul_down(d1)?)
                .checked_add(LN_SQRT_2PI)?
                .checked_neg()?,
        )
    } else {
        None
    };
    // ln vega = ln S Dq + ln n(d1) + ln √T
    let ln_vega = match ln_pdf {
        Some(lp) => Some(st.ln_s_dq.checked_add(lp)?.checked_add(st.half_ln_t)?),
        None => None,
    };
    Ok(Eval {
        ls,
        d1,
        d2,
        ln_pdf,
        ln_vega,
    })
}

/// The Chebyshev step in `v = ln σ`, capped at [`MAX_STEP`]: `−n (1 + c / 2)` with
/// `n = g / g'` and `c = n g'' / g' = n (1 + d1 d2) − g`, from `ln |g'|`; `g'` is negative
/// above the inflection point.
fn chebyshev_step(
    g: Fixed,
    ln_slope: Fixed,
    d1: Fixed,
    d2: Fixed,
    upper: bool,
) -> Result<Fixed, PricingError> {
    let n = g.mul_down(exp(ln_slope.checked_neg()?.min(INV_SLOPE_LN_CAP))?)?;
    let n = if upper { n.checked_neg()? } else { n };
    if n.raw().unsigned_abs() >= MAX_STEP.raw().unsigned_abs() {
        // Far from the root: a capped Newton step.
        return if n > Fixed::ZERO {
            MAX_STEP.checked_neg()
        } else {
            Ok(MAX_STEP)
        };
    }
    // c / 2, kept within ±1/2 so the correction never reverses the Newton direction.
    let c_half = half(
        n.mul_down(Fixed::ONE.checked_add(d1.mul_down(d2)?)?)?
            .checked_sub(g)?,
    )
    .clamp(Fixed::from_raw(-(1 << 63)), Fixed::from_raw(1 << 63));
    n.mul_down(Fixed::ONE.checked_add(c_half)?)?.checked_neg()
}

/// The initial guess of `ln s` from `ln a` and `L = −ln y` (`tools/gen_iv_tables.py`), and
/// whether it came from the deep out-of-the-money table.
fn guess(ln_a: Fixed, big_l: Fixed) -> Result<(Fixed, bool), PricingError> {
    let z = ln(big_l)?;
    let u = ln_a.checked_add(big_l)?;
    if u > Fixed::from_int(i64::from(SWITCH)) {
        let w = ln_a.checked_sub(half(Fixed::from_raw(LN2_RAW).checked_add(z)?))?;
        let col = w.checked_sub(Fixed::from_int(i64::from(DEEP_W_MIN)))?;
        let row = quadruple(z.checked_sub(Fixed::from_int(i64::from(DEEP_Z_MIN)))?)?;
        return Ok((bilinear(&DEEP, col, row)?.checked_add(w)?, true));
    }
    let col = u.checked_sub(Fixed::from_int(i64::from(NEAR_U_MIN)))?;
    let z_fine = Fixed::from_int(i64::from(Z_FINE));
    let row = if z <= z_fine {
        z.checked_sub(Fixed::from_int(i64::from(NEAR_Z_MIN)))?
    } else {
        Fixed::from_int(i64::from(Z_FINE - NEAR_Z_MIN))
            .checked_add(quadruple(z.checked_sub(z_fine)?)?)?
    };
    Ok((bilinear(&NEAR, col, row)?.checked_sub(big_l)?, false))
}

/// `4x`, exact.
fn quadruple(x: Fixed) -> Result<Fixed, PricingError> {
    x.raw()
        .checked_mul(4)
        .map(Fixed::from_raw)
        .ok_or(PricingError::Overflow)
}

/// Bilinear interpolation at fractional `(row, col)` (unit steps), clamped to the table.
/// Entries are at `2^-24`.
fn bilinear<const C: usize, const R: usize>(
    table: &[[i32; C]; R],
    col: Fixed,
    row: Fixed,
) -> Result<Fixed, PricingError> {
    let (i, fu) = cell(col, C)?;
    let (j, fv) = cell(row, R)?;
    let at = |r: usize, c: usize| -> Result<Fixed, PricingError> {
        table
            .get(r)
            .and_then(|line| line.get(c))
            .map(|v| Fixed::from_raw(i128::from(*v) << 40))
            .ok_or(PricingError::Overflow)
    };
    let next_i = i.checked_add(1).ok_or(PricingError::Overflow)?;
    let next_j = j.checked_add(1).ok_or(PricingError::Overflow)?;
    let (t00, t01) = (at(j, i)?, at(j, next_i)?);
    let (t10, t11) = (at(next_j, i)?, at(next_j, next_i)?);
    // t00 + fu (t01 − t00) + fv ((t10 − t00) + fu (t11 − t10 − t01 + t00))
    let across = t11.checked_sub(t10)?.checked_sub(t01)?.checked_add(t00)?;
    let down = t10.checked_sub(t00)?.checked_add(fu.mul_down(across)?)?;
    t00.checked_add(fu.mul_down(t01.checked_sub(t00)?)?)?
        .checked_add(fv.mul_down(down)?)
}

/// The cell index and the fraction within it of a coordinate on `0..n` unit steps, clamped to
/// `[0, n − 1]` (the last cell takes fraction 1 at the end).
fn cell(x: Fixed, n: usize) -> Result<(usize, Fixed), PricingError> {
    let last = n.checked_sub(2).ok_or(PricingError::Overflow)?;
    if x <= Fixed::ZERO {
        return Ok((0, Fixed::ZERO));
    }
    let whole = usize::try_from(x.raw() >> 64).map_err(|_| PricingError::Overflow)?;
    if whole > last {
        return Ok((last, Fixed::ONE));
    }
    Ok((whole, Fixed::from_raw(x.raw() & ((1 << 64) - 1))))
}

/// `σ` as a [`Volatility`]: a root outside the range by at most `slack`, the solve's own
/// round-trip envelope, is that end (the round trip of a price at `σ = 0.005` or `5` may land
/// just outside); past it, `DomainError`.
fn to_volatility(sigma: Fixed, slack: Fixed) -> Result<Volatility, PricingError> {
    let (lo, hi) = (Volatility::MIN.get(), Volatility::MAX.get());
    if sigma < lo && sigma >= lo.checked_sub(slack)? {
        return Ok(Volatility::MIN);
    }
    if sigma > hi && sigma <= hi.checked_add(slack)? {
        return Ok(Volatility::MAX);
    }
    Volatility::try_new(sigma)
}

/// Benchmark and test hooks: not public API (feature `bench-internals`).
#[cfg(feature = "bench-internals")]
#[doc(hidden)]
pub mod bench {
    use super::{setup, solve, EuropeanOption, Fixed, IvInputs, Price, PricingError, Volatility};

    /// `implied_vol` without the early exit: always [`super::IV_MAX_ITERATIONS`] steps, the
    /// worst-case CU row of `docs/benchmarks.md`.
    ///
    /// # Errors
    ///
    /// As `implied_vol`.
    pub fn all_iterations(
        opt: &EuropeanOption,
        m: &IvInputs,
        target: Price,
    ) -> Result<Volatility, PricingError> {
        solve(opt, m, target, super::IV_MAX_ITERATIONS, false).0
    }

    /// `implied_vol` with at most `cap` steps, and the number it took.
    pub fn with_cap(
        opt: &EuropeanOption,
        m: &IvInputs,
        target: Price,
        cap: u32,
    ) -> (Result<Volatility, PricingError>, u32) {
        solve(opt, m, target, cap, true)
    }

    /// The initial guess of `ln σ`, and whether it came from the deep out-of-the-money table.
    ///
    /// # Errors
    ///
    /// As `implied_vol` before its first step.
    pub fn initial_guess(
        opt: &EuropeanOption,
        m: &IvInputs,
        target: Price,
    ) -> Result<(Fixed, bool), PricingError> {
        setup(opt, m, target).map(|st| (st.v0, st.deep))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::disallowed_macros)]
mod tests {
    use super::*;
    use crate::{price, MarketInputs, Rounding, Strike, Years};

    fn fx(num: i128, den: i128) -> Fixed {
        Fixed::from_ratio(num, den, Rounding::Down).unwrap()
    }

    fn setup(
        kind: OptionKind,
        s: i64,
        k: i64,
        t: Fixed,
        sigma: Fixed,
    ) -> (EuropeanOption, MarketInputs) {
        (
            EuropeanOption {
                kind,
                strike: Strike::try_new(Fixed::from_int(k)).unwrap(),
                expiry: Years::try_new(t).unwrap(),
            },
            MarketInputs {
                spot: Spot::try_new(Fixed::from_int(s)).unwrap(),
                vol: Volatility::try_new(sigma).unwrap(),
                rate: Rate::try_new(fx(1, 20)).unwrap(),
                carry: Rate::try_new(fx(1, 50)).unwrap(),
            },
        )
    }

    fn iv_inputs(m: &MarketInputs) -> IvInputs {
        IvInputs {
            spot: m.spot,
            rate: m.rate,
            carry: m.carry,
        }
    }

    #[test]
    fn test_constants() {
        // n(0) = 1 / √(2π)
        let ln_sqrt_2pi = crate::ln(crate::norm_pdf(Fixed::ZERO).unwrap()).unwrap();
        assert!((ln_sqrt_2pi.raw() + LN_SQRT_2PI.raw()).abs() <= 4);
        assert!(
            V_MIN
                < crate::ln(Volatility::MIN.get())
                    .unwrap()
                    .checked_sub(MAX_STEP)
                    .unwrap()
        );
        assert!(
            V_MAX
                > crate::ln(Volatility::MAX.get())
                    .unwrap()
                    .checked_add(MAX_STEP)
                    .unwrap()
        );
    }

    #[test]
    fn test_round_trip_textbook() {
        for kind in [OptionKind::Call, OptionKind::Put] {
            for (s, k) in [(100, 100), (80, 100), (125, 100)] {
                let sigma = fx(1, 5);
                let (opt, m) = setup(kind, s, k, Fixed::ONE, sigma);
                let p = price(&opt, &m).unwrap();
                let iv = implied_vol(&opt, &iv_inputs(&m), p).unwrap().get();
                assert!(
                    (iv.raw() - sigma.raw()).abs() < 1 << 30,
                    "{kind:?} {s} {k}: {iv:?}"
                );
            }
        }
    }

    #[test]
    fn test_bounds_and_degenerate_inputs() {
        let (opt, m) = setup(OptionKind::Call, 120, 100, Fixed::ONE, fx(1, 5));
        let im = iv_inputs(&m);
        // A call is worth more than S Dq − K Dr and less than S Dq.
        let s_dq = Fixed::from_int(120)
            .mul_down(crate::exp(m.carry.get().checked_neg().unwrap()).unwrap())
            .unwrap();
        assert_eq!(
            implied_vol(&opt, &im, Price::try_new(s_dq).unwrap()),
            Err(PricingError::ArbitrageBound)
        );
        assert_eq!(
            implied_vol(&opt, &im, Price::MIN),
            Err(PricingError::ArbitrageBound)
        );
        let zero_t = EuropeanOption {
            expiry: Years::try_new(Fixed::ZERO).unwrap(),
            ..opt
        };
        let p = price(&opt, &m).unwrap();
        assert_eq!(implied_vol(&zero_t, &im, p), Err(PricingError::DomainError));
    }
}
