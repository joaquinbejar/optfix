//! Milestone 23 (`docs/PLAN-phase2.md`): the host-side no-arbitrage checks against the 256-bit
//! reference in `common/svi.rs`.
//!
//! - `butterfly_g_bounds` encloses Durrleman's `g(k)` at an exact `k`, in every stratum of
//!   `common::svi::STRATA` and near `k = m`; it refuses only where the lower bound of `w` is not
//!   positive, and overflows only where `w < 10^-6`.
//! - `check_butterfly` agrees node by node with `butterfly_g_bounds`, never passes a grid with a
//!   node where `g < 0` or `w <= 0`, and its report (first failure, minimum) matches the nodes.
//! - `check_calendar` never passes a node where `w_later < w_earlier`, and passes every node
//!   where the gap exceeds both envelopes.
//! - `check_wings` is `true` whenever `b (1 + |ρ|) <= 2 − 2^-64` and `false` whenever it is
//!   `>= 2`.
//!
//! Cases: 200,000 per stratum for `g`, 20,000 grids, 50,000 calendar pairs and wing slices
//! (`OPTFIX_PROPTEST_CASES` overrides all).

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::float_arithmetic,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::disallowed_macros,
    missing_docs
)]

mod common;

use common::svi::{g_ref, params, svi_sample, w_ref, RawSlice, STRATA};
use common::{to_ref, Rng, ONE, P};
use num_bigint::BigInt;
use optfix::{
    butterfly_g_bounds, check_butterfly, check_calendar, check_wings, Fixed, PricingError, SviGrid,
    SviParams, Years,
};

fn cases(default: usize) -> usize {
    std::env::var("OPTFIX_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// The reference's own error allowance: `2^-180`, far below an ulp (`2^-64`) and far above the
/// truncations of a 256-bit chain whose divisors are at least `2^-64`.
fn slack() -> BigInt {
    BigInt::from(1) << (P - 180)
}

/// One case of the enclosure: `Some(width / max(|g|, 1))`, in ulps, when bounded; `None` when
/// refused.
fn enclose(p: &RawSlice, sl: &SviParams, k: i128) -> Option<f64> {
    let kf = Fixed::from_raw(k);
    match butterfly_g_bounds(sl, kf) {
        Ok((lo, hi)) => {
            let g =
                g_ref(p, &to_ref(k)).unwrap_or_else(|| panic!("bounded with w <= 0: {p:?} {k}"));
            assert!(
                to_ref(lo.raw()) <= &g + slack(),
                "g below its lower bound at {p:?}, k {k}: lo {lo:?}"
            );
            assert!(
                g <= to_ref(hi.raw()) + slack(),
                "g above its upper bound at {p:?}, k {k}: hi {hi:?}"
            );
            let scale = common::ratio(&g).abs().max(1.0);
            Some((hi.raw() - lo.raw()) as f64 / scale)
        }
        Err(PricingError::DomainError) => {
            let (w_lo, _) = sl.total_variance_bounds(kf).unwrap();
            assert!(w_lo <= Fixed::ZERO, "refused with w_lo > 0: {p:?}, k {k}");
            None
        }
        Err(PricingError::Overflow) => {
            let (w_lo, _) = sl.total_variance_bounds(kf).unwrap();
            assert!(
                w_lo.raw() < ONE / 1_000_000,
                "overflow with w_lo {w_lo:?}: {p:?}, k {k}"
            );
            None
        }
        Err(e) => panic!("{e:?} at {p:?}, k {k}"),
    }
}

#[test]
fn prop_g_bounds_enclose_durrleman() {
    let n = cases(200_000);
    for (st, name) in STRATA.iter().enumerate() {
        let mut g = Rng(0x5F1_2300 + st as u64);
        let (mut bounded, mut refused, mut widest) = (0usize, 0usize, 0f64);
        for i in 0..n {
            let (p, mut k) = svi_sample(&mut g, st);
            let Some(sl) = params(&p) else { continue };
            if i % 4 == 0 {
                // Around m, where w'' peaks: within a few σ.
                let span = (p[4].saturating_mul(4)).max(1);
                k = (p[3] + g.range(-span, span + 1)).clamp(-32 * ONE, 32 * ONE);
            }
            match enclose(&p, &sl, k) {
                Some(w) => {
                    bounded += 1;
                    widest = widest.max(w);
                }
                None => refused += 1,
            }
        }
        println!("{name}: {bounded} bounded, {refused} refused or overflowed, widest {widest:.3e} ulps of max(|g|, 1)");
        assert!(bounded > n / 2, "{name}: too few bounded cases");
    }
}

fn grid_sample(g: &mut Rng, m: i128) -> SviGrid {
    let lo = (m + g.range(-6 * ONE, ONE)).clamp(-32 * ONE, 32 * ONE);
    let hi = (lo + g.range(0, 8 * ONE)).clamp(-32 * ONE, 32 * ONE);
    SviGrid {
        k_min: Fixed::from_raw(lo),
        k_max: Fixed::from_raw(hi),
        nodes: g.range(1, 41) as u32,
    }
}

/// Node `i`, as `SviGrid` documents it.
fn node(gr: &SviGrid, i: u32) -> i128 {
    if gr.nodes == 1 {
        return gr.k_min.raw();
    }
    gr.k_min.raw() + (gr.k_max.raw() - gr.k_min.raw()) * i128::from(i) / i128::from(gr.nodes - 1)
}

#[test]
fn prop_check_butterfly_is_sound_and_consistent() {
    let n = cases(20_000);
    let mut g = Rng(0x5F1_2310);
    let (mut passed, mut failed, mut errors) = (0usize, 0usize, 0usize);
    for i in 0..n {
        let (p, _) = svi_sample(&mut g, i % STRATA.len());
        let Some(sl) = params(&p) else { continue };
        let gr = grid_sample(&mut g, p[3]);
        let Ok(r) = check_butterfly(&sl, gr) else {
            errors += 1;
            continue;
        };
        let mut first = None;
        let mut min: Option<(i128, i128)> = None;
        let mut any_negative = false;
        for j in 0..gr.nodes {
            let k = node(&gr, j);
            let kf = Fixed::from_raw(k);
            let bound = match butterfly_g_bounds(&sl, kf) {
                Ok((lo, _)) => lo,
                Err(PricingError::DomainError) => Fixed::MIN,
                Err(e) => panic!("{e:?} at a node of a grid that evaluated"),
            };
            match g_ref(&p, &to_ref(k)) {
                Some(gv) if gv >= BigInt::from(0) => {}
                _ => any_negative = true,
            }
            if first.is_none() && bound < Fixed::ZERO {
                first = Some((k, bound.raw()));
            }
            if min.is_none_or(|(_, b)| bound.raw() < b) {
                min = Some((k, bound.raw()));
            }
        }
        assert_eq!(r.ok, first.is_none());
        assert_eq!(r.first_failure.map(|f| (f.k.raw(), f.bound.raw())), first);
        assert_eq!(Some((r.min.k.raw(), r.min.bound.raw())), min);
        if any_negative {
            assert!(
                !r.ok,
                "passed a grid with g < 0 or w <= 0 at a node: {p:?} {gr:?}"
            );
        }
        if r.ok {
            passed += 1;
        } else {
            failed += 1;
        }
    }
    println!("grids: {passed} passed, {failed} failed, {errors} errors");
    assert!(passed > 0 && failed > 0);
}

#[test]
fn prop_check_calendar_is_sound() {
    let n = cases(50_000);
    let mut g = Rng(0x5F1_2320);
    let (mut passed, mut failed) = (0usize, 0usize);
    for i in 0..n {
        let (p, _) = svi_sample(&mut g, i % STRATA.len());
        let Some(early) = params(&p) else { continue };
        // A later slice: a raised or lowered, the other parameters nudged.
        let mut q = p;
        q[0] = (q[0] + g.range(-ONE / 100, ONE / 10)).clamp(-1000 * ONE, 1000 * ONE);
        q[1] = (q[1] + g.range(-ONE / 100, ONE / 100)).clamp(0, 10 * ONE);
        q[2] = (q[2] + g.range(-ONE / 10, ONE / 10)).clamp(1 - ONE, ONE - 1);
        let Some(late) = params(&q) else { continue };
        let gr = grid_sample(&mut g, p[3]);
        let t1 = Years::try_new(Fixed::from_raw(ONE / 2)).unwrap();
        let t2 = Years::try_new(Fixed::ONE).unwrap();
        let r = check_calendar((&early, t1), (&late, t2), gr).unwrap();
        // Both envelopes, plus an ulp.
        let band = 5 * (p[1] + q[1]) / ONE + 12;
        let mut all_clear = true;
        for j in 0..gr.nodes {
            let k = to_ref(node(&gr, j));
            let gap = w_ref(&q, &k) - w_ref(&p, &k);
            assert!(
                !(r.ok && gap < -slack()),
                "passed a node where w_later < w_earlier: {p:?} {q:?} {gr:?}"
            );
            if gap <= to_ref(band) {
                all_clear = false;
            }
        }
        if all_clear {
            assert!(
                r.ok,
                "failed a grid whose gaps all exceed the envelopes: {p:?} {q:?} {gr:?}"
            );
        }
        if r.ok {
            passed += 1;
        } else {
            failed += 1;
        }
    }
    println!("calendar pairs: {passed} passed, {failed} failed");
    assert!(passed > 0 && failed > 0);
}

#[test]
fn prop_check_wings_is_exact_but_for_one_ulp() {
    let n = cases(50_000);
    let mut g = Rng(0x5F1_2330);
    let two = BigInt::from(2) << 128;
    let (mut yes, mut no) = (0usize, 0usize);
    for i in 0..n {
        let rho = g.range(1 - ONE, ONE);
        let b = if i % 2 == 0 {
            // Around b (1 + |ρ|) = 2.
            let exact = (BigInt::from(2) << 128) / BigInt::from(ONE + rho.abs());
            (i128::try_from(&exact).unwrap() + g.range(-4, 5)).clamp(0, 10 * ONE)
        } else {
            g.range(0, 10 * ONE + 1)
        };
        let sl = SviParams::try_new(
            Fixed::from_int(1000),
            Fixed::from_raw(b),
            Fixed::from_raw(rho),
            Fixed::ZERO,
            Fixed::ONE,
        )
        .unwrap();
        let slope = BigInt::from(b) * BigInt::from(ONE + rho.abs());
        let got = check_wings(&sl);
        if slope >= two {
            assert!(!got, "b (1 + |ρ|) >= 2 passed: b {b}, ρ {rho}");
        }
        if slope <= &two - (BigInt::from(1) << 64) {
            assert!(got, "b (1 + |ρ|) <= 2 − 2^-64 failed: b {b}, ρ {rho}");
        }
        if got {
            yes += 1;
        } else {
            no += 1;
        }
    }
    println!("wings: {yes} true, {no} false");
}
