//! No-arbitrage checks of raw SVI slices, for the host: an off-chain publisher or keeper checking
//! a slice before it is used (`docs/PLAN-phase2.md`, milestone 23). `no_std` like the rest of
//! the crate, but not benchmarked or budgeted for SBF.
//!
//! - [`check_wings`]: the slopes of `w` at `±∞`, `b (1 ± ρ)`, are below 2.
//! - [`check_butterfly`]: Durrleman's condition, in the form of Gatheral and Jacquier (2014),
//!   `g(k) >= 0` and `w(k) > 0`, at the nodes of a grid.
//! - [`check_calendar`]: `w_later(k) >= w_earlier(k)` at the nodes of a grid.
//!
//! The grid checks are conservative at the nodes: every bound is computed in interval
//! arithmetic with the directed primitives, so a node passes only if the condition holds there
//! for certain, and an interval that straddles 0 fails. They say nothing between nodes, so they
//! are not a certificate for the whole real line; [`check_wings`] covers `|k| → ∞`. There is no
//! default grid: pass the range of `k` the slice will be used on, plus a margin, with at least
//! four nodes per `σ` around `m`, where `w''` peaks.

use crate::svi::{abs, SviParams};
use crate::{Fixed, PricingError, Years};

/// `|k| <= 32`, as [`SviParams::total_variance`].
const K_MAX: Fixed = Fixed::from_int(32);
/// 2, the wing slope that [`check_wings`] refuses.
const TWO: Fixed = Fixed::from_int(2);
/// `1 / 2`, exact.
const HALF: Fixed = Fixed::from_raw(1 << 63);
/// `1 / 4`, exact.
const QUARTER: Fixed = Fixed::from_raw(1 << 62);

/// A uniform grid of log-forward moneyness `k`: `nodes` points from `k_min` to `k_max`, both
/// included. Node `i` is `k_min + floor((k_max − k_min) × i / (nodes − 1))` in raw units, so the
/// last node is `k_max` exactly; a single node is `k_min`.
///
/// A check refuses (`DomainError`) a grid with `nodes = 0`, `k_min > k_max` or an end outside
/// `|k| <= 32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SviGrid {
    /// The first node.
    pub k_min: Fixed,
    /// The last node.
    pub k_max: Fixed,
    /// The number of nodes, at least 1.
    pub nodes: u32,
}

impl SviGrid {
    /// Checks the grid (see [`SviGrid`]).
    fn validate(&self) -> Result<(), PricingError> {
        let neg = K_MAX.checked_neg()?;
        if self.nodes == 0 || self.k_min > self.k_max || self.k_min < neg || self.k_max > K_MAX {
            return Err(PricingError::DomainError);
        }
        Ok(())
    }

    /// Node `i < nodes` of a valid grid.
    fn node(&self, i: u32) -> Result<Fixed, PricingError> {
        let Some(last) = self.nodes.checked_sub(1).filter(|&l| l > 0) else {
            return Ok(self.k_min);
        };
        // At most 2^70 × 2^32: no overflow in i128.
        let off = self
            .k_max
            .raw()
            .checked_sub(self.k_min.raw())
            .and_then(|d| d.checked_mul(i128::from(i)))
            .and_then(|d| d.checked_div(i128::from(last)))
            .ok_or(PricingError::Overflow)?;
        self.k_min.checked_add(Fixed::from_raw(off))
    }
}

/// A node of a grid check and its bound.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeBound {
    /// The node's `k`.
    pub k: Fixed,
    /// The node's bound: a lower bound of the checked quantity, which passes when `>= 0`.
    pub bound: Fixed,
}

/// The result of [`check_butterfly`]. The bound at a node is the lower bound of `g(k)`, or
/// [`Fixed::MIN`] when the lower bound of `w(k)` is not positive (`g` is then unbounded below).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ButterflyReport {
    /// Every node passed: the lower bounds of `g` are `>= 0` and those of `w` are `> 0`.
    pub ok: bool,
    /// The first failing node, in grid order.
    pub first_failure: Option<NodeBound>,
    /// The node with the smallest bound (the first one on a tie).
    pub min: NodeBound,
}

/// The result of [`check_calendar`]. The bound at a node is `w_later`'s lower bound minus
/// `w_earlier`'s upper bound.
///
/// Both slices are in their own forward's log-moneyness `k = ln(K / F(T))`, and the condition
/// `w_later(k) >= w_earlier(k)` at equal `k` is the calendar condition under deterministic rates
/// and carry, where the forward-normalised call prices of both expiries are functions of `k`
/// and `w` alone.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CalendarReport {
    /// Every node passed: `w_later`'s lower bound is at least `w_earlier`'s upper bound.
    pub ok: bool,
    /// The first failing node, in grid order.
    pub first_failure: Option<NodeBound>,
    /// The node with the smallest bound (the first one on a tie).
    pub min: NodeBound,
}

/// The wing condition: `b (1 + |ρ|) < 2`, evaluated rounding up, so `true` is certain.
///
/// The slopes of `w` at `±∞` are `b (1 ± ρ)`. Lee's moment bound allows 2, but the slice is free
/// of butterfly arbitrage only if `d+(k) → −∞` as `k → +∞` (and `d−(k) → +∞` as `k → −∞`), and
/// a slope of exactly 2 gives `d+ → 0`.
#[must_use]
pub fn check_wings(params: &SviParams) -> bool {
    let slope = abs(params.rho())
        .and_then(|r| Fixed::ONE.checked_add(r))
        .and_then(|r| params.b().mul_up(r));
    // b <= 10 and 1 + |ρ| < 2: the chain cannot fail, and a failure would read as `false`.
    matches!(slope, Ok(s) if s < TWO)
}

/// A rigorous enclosure `lo <= g(k) <= hi` of Durrleman's function at an exact `k`:
///
/// ```text
/// g(k) = (1 − k w' / (2 w))² − (w'² / 4) (1 / w + 1 / 4) + w'' / 2,
/// w' = b (ρ + x / s),  w'' = b σ² / s³,  s = sqrt(x² + σ²),  x = k − m.
/// ```
///
/// One interval chain from [`SviParams::total_variance_bounds`]: every operation rounds its
/// lower end down and its upper end up, a subtracted term enters with its opposite end, a
/// divisor is a positive interval and the quotient takes its outward corners, and the square of
/// an interval that straddles 0 has a lower end of 0. `w''` is `(b / s) (σ / s)²`, so a small
/// `σ` keeps its precision.
///
/// # Errors
///
/// `DomainError` if `|k| > 32` or the lower bound of `w(k)` is not positive; `Overflow` if a
/// term does not fit (a `w` within about `10^-7` of 0, where `k w' / (2 w)` grows past `2^32`).
#[must_use = "the result carries a possible error"]
pub fn butterfly_g_bounds(params: &SviParams, k: Fixed) -> Result<(Fixed, Fixed), PricingError> {
    let g = g_interval(params, k)?.ok_or(PricingError::DomainError)?;
    Ok((g.lo, g.hi))
}

/// Durrleman's condition at every node of `grid` (see [`butterfly_g_bounds`]): a node passes
/// only if the lower bound of `g` is `>= 0` and the lower bound of `w` is `> 0`.
///
/// Conservative at the nodes and silent between them (see the module documentation); use it
/// with [`check_wings`].
///
/// # Errors
///
/// `DomainError` for an invalid grid (see [`SviGrid`]); `Overflow` as [`butterfly_g_bounds`].
/// An `Err` means the evaluation failed, never a violation.
#[must_use = "the result carries a possible error"]
pub fn check_butterfly(params: &SviParams, grid: SviGrid) -> Result<ButterflyReport, PricingError> {
    let (first_failure, min) = scan(grid, |k| {
        Ok(g_interval(params, k)?.map_or(Fixed::MIN, |g| g.lo))
    })?;
    Ok(ButterflyReport {
        ok: first_failure.is_none(),
        first_failure,
        min,
    })
}

/// The calendar condition at every node of `grid`: `w_later(k) >= w_earlier(k)`, a node passing
/// only if `w_later`'s lower bound is at least `w_earlier`'s upper bound
/// ([`SviParams::total_variance_bounds`]). See [`CalendarReport`] for the assumption.
///
/// # Errors
///
/// `DomainError` if `T_earlier >= T_later` or for an invalid grid (see [`SviGrid`]). An `Err`
/// means the evaluation failed, never a violation.
#[must_use = "the result carries a possible error"]
pub fn check_calendar(
    earlier: (&SviParams, Years),
    later: (&SviParams, Years),
    grid: SviGrid,
) -> Result<CalendarReport, PricingError> {
    if earlier.1.get() >= later.1.get() {
        return Err(PricingError::DomainError);
    }
    let (first_failure, min) = scan(grid, |k| {
        let (_, e_hi) = earlier.0.total_variance_bounds(k)?;
        let (l_lo, _) = later.0.total_variance_bounds(k)?;
        l_lo.checked_sub(e_hi)
    })?;
    Ok(CalendarReport {
        ok: first_failure.is_none(),
        first_failure,
        min,
    })
}

/// The bound at every node of a valid grid: the first negative one and the smallest one.
fn scan(
    grid: SviGrid,
    mut bound: impl FnMut(Fixed) -> Result<Fixed, PricingError>,
) -> Result<(Option<NodeBound>, NodeBound), PricingError> {
    grid.validate()?;
    let mut first = None;
    let mut min: Option<NodeBound> = None;
    for i in 0..grid.nodes {
        let k = grid.node(i)?;
        let here = NodeBound {
            k,
            bound: bound(k)?,
        };
        if first.is_none() && here.bound < Fixed::ZERO {
            first = Some(here);
        }
        match min {
            Some(m) if m.bound <= here.bound => {}
            _ => min = Some(here),
        }
    }
    // nodes >= 1, so `min` is set.
    Ok((first, min.ok_or(PricingError::DomainError)?))
}

/// A closed interval `[lo, hi]`, `lo <= hi`.
#[derive(Debug, Clone, Copy)]
struct Iv {
    lo: Fixed,
    hi: Fixed,
}

impl Iv {
    const fn point(v: Fixed) -> Self {
        Self { lo: v, hi: v }
    }

    fn add(self, o: Self) -> Result<Self, PricingError> {
        Ok(Self {
            lo: self.lo.checked_add(o.lo)?,
            hi: self.hi.checked_add(o.hi)?,
        })
    }

    fn sub(self, o: Self) -> Result<Self, PricingError> {
        Ok(Self {
            lo: self.lo.checked_sub(o.hi)?,
            hi: self.hi.checked_sub(o.lo)?,
        })
    }

    /// The product: bilinear, so its extremes are at the corners.
    fn mul(self, o: Self) -> Result<Self, PricingError> {
        let corners = [
            (self.lo, o.lo),
            (self.lo, o.hi),
            (self.hi, o.lo),
            (self.hi, o.hi),
        ];
        let mut lo = Fixed::MAX;
        let mut hi = Fixed::MIN;
        for (x, y) in corners {
            lo = lo.min(x.mul_down(y)?);
            hi = hi.max(x.mul_up(y)?);
        }
        Ok(Self { lo, hi })
    }

    /// The quotient by a positive interval (`o.lo > 0`): monotone in each argument, so its
    /// extremes are at the corners.
    fn div(self, o: Self) -> Result<Self, PricingError> {
        let corners = [
            (self.lo, o.lo),
            (self.lo, o.hi),
            (self.hi, o.lo),
            (self.hi, o.hi),
        ];
        let mut lo = Fixed::MAX;
        let mut hi = Fixed::MIN;
        for (x, y) in corners {
            lo = lo.min(x.div_down(y)?);
            hi = hi.max(x.div_up(y)?);
        }
        Ok(Self { lo, hi })
    }

    /// The square: a lower end of 0 when the interval straddles 0.
    fn sqr(self) -> Result<Self, PricingError> {
        if self.lo >= Fixed::ZERO {
            Ok(Self {
                lo: self.lo.mul_down(self.lo)?,
                hi: self.hi.mul_up(self.hi)?,
            })
        } else if self.hi <= Fixed::ZERO {
            Ok(Self {
                lo: self.hi.mul_down(self.hi)?,
                hi: self.lo.mul_up(self.lo)?,
            })
        } else {
            Ok(Self {
                lo: Fixed::ZERO,
                hi: self.lo.mul_up(self.lo)?.max(self.hi.mul_up(self.hi)?),
            })
        }
    }

    /// The intersection with `[lo, hi]`, for a value known to lie there.
    fn clamp(self, lo: Fixed, hi: Fixed) -> Self {
        Self {
            lo: self.lo.max(lo).min(hi),
            hi: self.hi.min(hi).max(lo),
        }
    }
}

/// `s = sqrt(x² + σ²)`, enclosed: `sqrt(x² + σ²)` when `M = max(|x|, σ) >= 1`, otherwise
/// `M sqrt(1 + (μ / M)²)` with `μ = min(|x|, σ)`, each end rounded its own way. `s >= M`.
fn root_interval(x: Fixed, sigma: Fixed) -> Result<Iv, PricingError> {
    let ax = abs(x)?;
    let (big, small) = if ax >= sigma {
        (ax, sigma)
    } else {
        (sigma, ax)
    };
    let (lo, hi) = if big >= Fixed::ONE {
        (
            x.mul_down(x)?
                .checked_add(sigma.mul_down(sigma)?)?
                .sqrt_down()?,
            x.mul_up(x)?.checked_add(sigma.mul_up(sigma)?)?.sqrt_up()?,
        )
    } else {
        // big >= σ > 0.
        let q_lo = small.div_down(big)?;
        let q_hi = small.div_up(big)?;
        let r_lo = Fixed::ONE.checked_add(q_lo.mul_down(q_lo)?)?.sqrt_down()?;
        let r_hi = Fixed::ONE.checked_add(q_hi.mul_up(q_hi)?)?.sqrt_up()?;
        (big.mul_down(r_lo)?, big.mul_up(r_hi)?)
    };
    Ok(Iv {
        lo: lo.max(big),
        hi,
    })
}

/// The enclosure of `g(k)`, or `None` when the lower bound of `w(k)` is not positive.
fn g_interval(params: &SviParams, k: Fixed) -> Result<Option<Iv>, PricingError> {
    let (w_lo, w_hi) = params.total_variance_bounds(k)?;
    if w_lo <= Fixed::ZERO {
        return Ok(None);
    }
    let w = Iv { lo: w_lo, hi: w_hi };
    let neg_one = Fixed::ONE.checked_neg()?;
    let b = Iv::point(params.b());
    let sigma = params.sigma();
    let x = k.checked_sub(params.m())?;
    let s = root_interval(x, sigma)?;
    // x / s and σ / s lie in [−1, 1] and (0, 1].
    let u = Iv::point(x).div(s)?.clamp(neg_one, Fixed::ONE);
    let t = Iv::point(sigma).div(s)?.clamp(Fixed::ZERO, Fixed::ONE);
    let w1 = b.mul(Iv::point(params.rho()).add(u)?)?;
    let w2 = b.div(s)?.mul(t.sqr()?)?;
    let first = Iv::point(Fixed::ONE)
        .sub(Iv::point(k).mul(w1)?.div(w.add(w)?)?)?
        .sqr()?;
    let inv = Iv::point(Fixed::ONE).div(w)?.add(Iv::point(QUARTER))?;
    let second = w1.sqr()?.mul(Iv::point(QUARTER))?.mul(inv)?;
    let third = w2.mul(Iv::point(HALF))?;
    Ok(Some(first.sub(second)?.add(third)?))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]
mod tests {
    use super::{butterfly_g_bounds, check_butterfly, check_calendar, check_wings, SviGrid, K_MAX};
    use crate::{Fixed, PricingError, Rounding, SviParams, Years};

    fn fx(num: i128, den: i128) -> Fixed {
        Fixed::from_ratio(num, den, Rounding::Down).unwrap()
    }

    fn slice(a: Fixed, b: Fixed, rho: Fixed, m: Fixed, s: Fixed) -> SviParams {
        SviParams::try_new(a, b, rho, m, s).unwrap()
    }

    /// Gatheral and Jacquier (2014), example 3.1 (Vogt).
    fn vogt() -> SviParams {
        slice(
            fx(-410, 10_000),
            fx(1331, 10_000),
            fx(3060, 10_000),
            fx(3586, 10_000),
            fx(4153, 10_000),
        )
    }

    fn grid(lo: i64, hi: i64, nodes: u32) -> SviGrid {
        SviGrid {
            k_min: Fixed::from_int(lo),
            k_max: Fixed::from_int(hi),
            nodes,
        }
    }

    fn years(num: i128, den: i128) -> Years {
        Years::try_new(fx(num, den)).unwrap()
    }

    #[test]
    fn test_grid_nodes_and_refusals() {
        let g = grid(-2, 2, 401);
        assert_eq!(g.node(0).unwrap(), Fixed::from_int(-2));
        assert_eq!(g.node(400).unwrap(), Fixed::from_int(2));
        assert_eq!(g.node(200).unwrap(), Fixed::ZERO);
        let one = grid(1, 1, 1);
        assert_eq!(one.node(0).unwrap(), Fixed::ONE);
        let s = vogt();
        for bad in [
            grid(-2, 2, 0),
            grid(2, -2, 3),
            SviGrid {
                k_min: K_MAX
                    .checked_neg()
                    .unwrap()
                    .checked_sub(Fixed::from_raw(1))
                    .unwrap(),
                k_max: Fixed::ZERO,
                nodes: 2,
            },
            SviGrid {
                k_min: Fixed::ZERO,
                k_max: K_MAX.checked_add(Fixed::from_raw(1)).unwrap(),
                nodes: 2,
            },
        ] {
            assert_eq!(check_butterfly(&s, bad), Err(PricingError::DomainError));
        }
        assert!(check_butterfly(&s, grid(-32, 32, 2)).is_ok());
    }

    #[test]
    fn test_wings() {
        let (a, m, s) = (fx(1, 10), Fixed::ZERO, fx(1, 10));
        let ok = |b, rho| check_wings(&slice(a, b, rho, m, s));
        let quarter = fx(1, 4);
        // b (1 + |ρ|) = 1.875 and 2 − 2^-64 × 1.25.
        assert!(ok(fx(3, 2), quarter));
        assert!(ok(fx(3, 2), quarter.checked_neg().unwrap()));
        // 1.6 rounded down times 1.25 is below 2 by 0.75 ulp: rounded up, it is 2 and fails.
        assert!(!ok(fx(8, 5), quarter));
        assert!(!ok(
            Fixed::from_ratio(8, 5, Rounding::Up).unwrap(),
            quarter.checked_neg().unwrap()
        ));
        assert!(!ok(Fixed::from_int(10), Fixed::ZERO));
        assert!(ok(Fixed::ZERO, fx(9, 10)));
        assert!(check_wings(&vogt()));
    }

    #[test]
    fn test_vogt_fails_only_by_butterfly() {
        let s = vogt();
        assert!(check_wings(&s));
        assert!(s.min_total_variance() > Fixed::ZERO);
        let r = check_butterfly(&s, grid(-2, 2, 401)).unwrap();
        assert!(!r.ok);
        let first = r.first_failure.unwrap();
        // mpmath: g < 0 for k in about [0.65, 1.25], with a minimum of about −0.033.
        assert!(first.k > fx(60, 100) && first.k < fx(70, 100));
        assert!(r.min.bound < fx(-3, 100) && r.min.bound > fx(-4, 100));
        let (lo, hi) = butterfly_g_bounds(&s, Fixed::ONE).unwrap();
        assert!(lo <= hi && hi < Fixed::ZERO);
    }

    #[test]
    fn test_symmetric_small_b_passes() {
        let s = slice(fx(4, 100), fx(1, 10), Fixed::ZERO, Fixed::ZERO, fx(2, 10));
        assert!(check_wings(&s));
        let r = check_butterfly(&s, grid(-5, 5, 1001)).unwrap();
        assert!(r.ok, "{r:?}");
        assert!(r.first_failure.is_none());
        assert!(r.min.bound > Fixed::ZERO);
    }

    #[test]
    fn test_zero_variance_node_fails() {
        // ρ = 0, a = −b σ: w(m) = 0, so its lower bound is not positive (try_new accepts it).
        let b = fx(1, 10);
        let s = fx(2, 10);
        // `total_variance(m)` is `a + b ⊗↓ σ`, the minimum's lower bound: exactly 0 here.
        let a = b.mul_down(s).unwrap().checked_neg().unwrap();
        let sl = slice(a, b, Fixed::ZERO, Fixed::ZERO, s);
        assert_eq!(
            butterfly_g_bounds(&sl, Fixed::ZERO),
            Err(PricingError::DomainError)
        );
        let r = check_butterfly(&sl, grid(-1, 1, 3)).unwrap();
        assert!(!r.ok);
        assert_eq!(r.first_failure.unwrap().k, Fixed::ZERO);
        assert_eq!(r.min.bound, Fixed::MIN);
    }

    #[test]
    fn test_calendar() {
        let base = vogt();
        let up = slice(
            base.a().checked_add(fx(1, 1_000_000)).unwrap(),
            base.b(),
            base.rho(),
            base.m(),
            base.sigma(),
        );
        let g = grid(-3, 3, 601);
        let (t1, t2) = (years(1, 2), years(1, 1));
        let r = check_calendar((&base, t1), (&up, t2), g).unwrap();
        assert!(r.ok, "{r:?}");
        // Against itself: equal w, and an interval of width E_w on each side fails.
        assert!(!check_calendar((&base, t1), (&base, t2), g).unwrap().ok);
        // Crossing slices: the later one is higher left of m and lower right of it.
        let tilted = slice(
            base.a(),
            base.b(),
            fx(-3060, 10_000),
            base.m(),
            base.sigma(),
        );
        let r = check_calendar((&base, t1), (&tilted, t2), g).unwrap();
        assert!(!r.ok);
        let k = r.first_failure.unwrap().k;
        assert!(k >= base.m(), "{k:?}");
        assert!(r.min.bound < Fixed::ZERO);
        // Expiry order.
        assert_eq!(
            check_calendar((&base, t2), (&up, t1), g),
            Err(PricingError::DomainError)
        );
        assert_eq!(
            check_calendar((&base, t1), (&up, t1), g),
            Err(PricingError::DomainError)
        );
    }
}
