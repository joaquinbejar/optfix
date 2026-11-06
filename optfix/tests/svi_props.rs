//! Milestone 22 (`docs/PLAN-phase2.md`): raw SVI slices against the 256-bit reference in
//! `common/svi.rs`.
//!
//! - `total_variance` at an exact `k`: never above `w(k)` and within `E_w` below it, in every
//!   stratum of `common::svi::STRATA`; `total_variance_bounds` encloses `w(k)`, and its upper
//!   end is at least `min_total_variance`.
//! - Reflection: `(a, b, −ρ, −m, σ)` at `−k` equals `(a, b, ρ, m, σ)` at `k`, bit for bit.
//! - `min_total_variance` is never above the true minimum, and `try_new` refuses every slice
//!   whose true minimum is negative, including those built so that `ρ²` rounded down would
//!   accept them.
//! - End to end from `S, K, r, q, T`: `|k̂ − k*| < SVI_K_ERR`; `svi_vol_bounds` encloses `σ*` and
//!   `svi_vol`; `svi_vol` is within the documented two-sided bound; `svi_price_bounds` encloses
//!   the price at `σ*`.
//!
//! Cases: 200,000 per stratum for `w`, 20,000 end to end (`OPTFIX_PROPTEST_CASES` overrides
//! both).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::float_arithmetic,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::disallowed_macros,
    missing_docs
)]

mod common;

use common::svi::{
    k_star, market, market_sample, market_slice, min_ref, params, svi_sample, vol_ref, w_ref,
    RawSlice, STRATA,
};
use common::{bsm_ref, ratio, to_ref, Rng, ONE};
use num_bigint::BigInt;
use optfix::{
    log_forward_moneyness, svi_price_bounds, svi_vol, svi_vol_bounds, EuropeanOption, Fixed,
    OptionKind, PricingError, SviParams, Volatility, SVI_K_ERR,
};

fn cases(default: usize) -> usize {
    std::env::var("OPTFIX_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// `y − raw` in ulps (`2^-64`).
fn ulps_below(y: &BigInt, raw: i128) -> f64 {
    ratio(&((y - to_ref(raw)) << 64))
}

#[test]
fn prop_total_variance_one_sided_envelope() {
    let n = cases(200_000);
    for (st, name) in STRATA.iter().enumerate() {
        let mut g = Rng(0x5F1_0000 + st as u64);
        let (mut worst, mut refused) = (0f64, 0usize);
        for _ in 0..n {
            let (p, k) = svi_sample(&mut g, st);
            let Some(s) = params(&p) else {
                refused += 1;
                continue;
            };
            let kf = Fixed::from_raw(k);
            let w = s.total_variance(kf).unwrap();
            let (lo, hi) = s.total_variance_bounds(kf).unwrap();
            assert_eq!(lo, w);
            let exact = w_ref(&p, &to_ref(k));
            assert!(
                to_ref(w.raw()) <= exact,
                "{name}: total_variance above w(k) at {p:?}, k {k}"
            );
            assert!(
                exact <= to_ref(hi.raw()),
                "{name}: w(k) above hi at {p:?}, k {k}: {} ulps below, E_w {}",
                ulps_below(&exact, w.raw()),
                hi.raw() - lo.raw()
            );
            assert!(hi >= s.min_total_variance(), "{name}: hi below the minimum");
            worst = worst.max(ulps_below(&exact, w.raw()) / (hi.raw() - lo.raw()) as f64);
        }
        println!("{name}: worst (w − computed) / E_w = {worst:.4}, refused {refused} of {n}");
        assert!(refused < n / 4, "{name}: the sampler refuses too often");
    }
}

#[test]
fn prop_reflection_is_bit_identical() {
    let n = cases(200_000);
    let mut g = Rng(0x5F1_1000);
    for i in 0..n {
        let (p, k) = svi_sample(&mut g, i % STRATA.len());
        let Some(s) = params(&p) else { continue };
        let [a, b, rho, m, sigma] = p;
        let r = params(&[a, b, -rho, -m, sigma]).expect("the reflected slice is valid too");
        assert_eq!(s.min_total_variance(), r.min_total_variance());
        assert_eq!(
            s.total_variance(Fixed::from_raw(k)),
            r.total_variance(Fixed::from_raw(-k)),
            "reflection at {p:?}, k {k}"
        );
    }
}

/// `a` such that the wrong-direction bound (`ρ²` rounded down) is exactly 0.
fn a_for_rho2_down(b: i128, rho: i128, sigma: i128) -> i128 {
    let r = Fixed::from_raw(rho);
    let root = Fixed::ONE
        .checked_sub(r.mul_down(r).unwrap())
        .unwrap()
        .sqrt_down()
        .unwrap();
    let bs = Fixed::from_raw(b).mul_down(Fixed::from_raw(sigma)).unwrap();
    -bs.mul_down(root).unwrap().raw()
}

#[test]
fn prop_min_total_variance_never_above_true_minimum() {
    let n = cases(200_000);
    let mut g = Rng(0x5F1_2000);
    let (mut negative, mut attacked) = (0usize, 0usize);
    for i in 0..n {
        let (mut p, _) = svi_sample(&mut g, i % STRATA.len());
        if i % 2 == 0 {
            // 1 − |ρ| from one ulp to 2^-1, either sign, a at the wrong-direction bound.
            let gap = 1i128 << (g.next() % 63);
            let gap = (gap + (g.next() as i128 % gap)).min(ONE / 2);
            p[2] = if g.next() & 1 == 0 {
                ONE - gap
            } else {
                gap - ONE
            };
            p[1] = p[1].max(1);
            p[0] = a_for_rho2_down(p[1], p[2], p[4]);
            attacked += 1;
        }
        let truth = min_ref(&p);
        let accepted = params(&p);
        if let Some(s) = accepted {
            assert!(
                to_ref(s.min_total_variance().raw()) <= truth,
                "min_total_variance above the true minimum at {p:?}"
            );
            assert!(s.min_total_variance() >= Fixed::ZERO);
        }
        if truth < BigInt::from(0) {
            assert!(
                accepted.is_none(),
                "accepted a negative true minimum at {p:?}"
            );
            negative += 1;
        }
    }
    println!(
        "minimum: {attacked} slices at the rho^2-down bound; {negative} had a negative true \
         minimum, every one refused"
    );
    assert!(negative > 0, "the attack never produced a negative minimum");
}

#[test]
fn test_named_negative_minimum_is_refused() {
    let ten = 10 * ONE;
    for rho in [ONE - 2, 2 - ONE] {
        let p: RawSlice = [-858_993_459_200, ten, rho, 0, ten];
        assert!(min_ref(&p) < BigInt::from(0));
        assert_eq!(a_for_rho2_down(ten, rho, ten), -858_993_459_200);
        assert!(params(&p).is_none());
    }
}

/// `(σ*, ŵ)` checks of one end-to-end point; returns `σ̂ − σ*` in ulps when `svi_vol` answers.
fn check_point(
    p: &RawSlice,
    s: &SviParams,
    pt: (i128, i128, i128, i128, i128),
    call: bool,
) -> Option<f64> {
    let (sp, k, t, r, q) = pt;
    let (expiry, m, strike) = market(sp, k, t, r, q);
    let kh = log_forward_moneyness(expiry, &m, strike).unwrap();
    let ks = k_star(sp, k, t, r, q);
    let kerr = ratio(&((to_ref(kh.raw()) - &ks) << 64)).abs();
    assert!(
        kerr < SVI_K_ERR.raw() as f64,
        "|k̂ − k*| = {kerr} ulps at {pt:?}"
    );
    let ws = w_ref(p, &ks);
    assert!(ws >= BigInt::from(0), "negative w(k*) on an accepted slice");
    let bounds = svi_vol_bounds(s, expiry, &m, strike);
    let vol = svi_vol(s, expiry, &m, strike);
    let (lo, hi) = match bounds {
        Ok(b) => b,
        Err(e) => {
            assert_eq!(
                vol.map(Volatility::get),
                Err(e),
                "svi_vol and the bounds disagree"
            );
            return None;
        }
    };
    let sig = vol_ref(&ws, t);
    assert!(
        to_ref(lo.raw()) <= sig && sig <= to_ref(hi.raw()),
        "σ* outside [{lo:?}, {hi:?}] at {p:?} {pt:?}"
    );
    let inside = lo >= Volatility::MIN.get() && hi <= Volatility::MAX.get();
    let diff = match vol {
        Ok(v) => {
            let v = v.get();
            assert!(
                lo <= v && v <= hi,
                "svi_vol outside its bounds at {p:?} {pt:?}"
            );
            // The closed form: −(δ− / (2σ̂) + 1) <= σ̂ − σ* <= δ+ / σ̂, in ulps.
            let d = -ulps_below(&sig, v.raw());
            let tf = t as f64 / ONE as f64;
            let ew = (5.0 * s.b().raw() as f64 / ONE as f64).ceil() + 1.0;
            let ek = (5.0
                * (s.b().raw() as f64 / ONE as f64)
                * (1.0 + s.rho().raw().abs() as f64 / ONE as f64))
                .ceil()
                + 1.0;
            let vf = v.raw() as f64 / ONE as f64;
            let up = ek / tf / vf * 1.000_001 + 1e-6;
            let down = ((ew + ek) / tf + 1.0) / (2.0 * vf) * 1.000_001 + 1.0;
            assert!(
                d <= up && -d <= down,
                "closed form: {d} ulps, allowed −{down} to {up}"
            );
            Some(d)
        }
        Err(e) => {
            assert_eq!(e, PricingError::DomainError);
            assert!(
                !inside,
                "svi_vol refused a volatility whose enclosure is in range"
            );
            None
        }
    };
    let opt = EuropeanOption {
        kind: if call {
            OptionKind::Call
        } else {
            OptionKind::Put
        },
        strike,
        expiry,
    };
    let moneyness = sp * 20 >= k && k * 20 >= sp;
    match svi_price_bounds(&opt, &m, s) {
        Ok((pl, ph)) => {
            let rf = bsm_ref(
                call,
                &to_ref(sp),
                &to_ref(k),
                &to_ref(t),
                &sig,
                &to_ref(r),
                &to_ref(q),
            );
            assert!(
                to_ref(pl.get().raw()) <= rf.price && rf.price <= to_ref(ph.get().raw()),
                "price at σ* outside [{pl:?}, {ph:?}] at {p:?} {pt:?}"
            );
        }
        Err(e) => {
            assert_eq!(e, PricingError::DomainError);
            assert!(
                !(inside && moneyness),
                "svi_price_bounds refused an in-range point"
            );
        }
    }
    diff
}

#[test]
fn prop_end_to_end_enclosures() {
    let n = cases(20_000);
    let mut g = Rng(0x5F1_3000);
    let (mut above, mut answered, mut worst_up, mut worst_down) = (0usize, 0usize, 0f64, 0f64);
    for _ in 0..n {
        let pt = market_sample(&mut g);
        let p = market_slice(&mut g, pt.2);
        let Some(s) = params(&p) else { continue };
        if let Some(d) = check_point(&p, &s, pt, g.next() & 1 == 0) {
            answered += 1;
            if d > 0.0 {
                above += 1;
            }
            worst_up = worst_up.max(d);
            worst_down = worst_down.max(-d);
        }
    }
    println!(
        "end to end: {answered} of {n} answered; svi_vol above σ* at {above}, by up to \
         {worst_up:.3} ulps; below by up to {worst_down:.3}"
    );
    assert!(answered > n / 2, "too few points answered");
    // svi_vol above σ* is rare with the crate's own ln (round to nearest, 1.5 ulps): 4 of the
    // 20,000 default points. `test_codex_counterexample_is_enclosed` pins one such point.
}

#[test]
fn test_codex_counterexample_is_enclosed() {
    // codex, bus message 53498: T = 1, r = q = 0, a = b = 1, ρ = 0.9, m = −1, σ = 0.1.
    let p: RawSlice = [
        ONE,
        ONE,
        16_602_069_666_338_596_454,
        -ONE,
        1_844_674_407_370_955_161,
    ];
    let s = params(&p).unwrap();
    let pt = (
        9_818_482_447_416_719_652_683_776,
        179_040_027_428_643_882_866_284_137,
        ONE,
        0,
        0,
    );
    let (expiry, m, strike) = market(pt.0, pt.1, pt.2, pt.3, pt.4);
    assert_eq!(
        log_forward_moneyness(expiry, &m, strike).unwrap().raw(),
        53_557_222_037_377_178_528
    );
    let d = check_point(&p, &s, pt, true).unwrap();
    // svi_vol is above σ* here, by about 0.1245 ulps.
    assert!(d > 0.12 && d < 0.13, "σ̂ − σ* = {d} ulps");
}
