//! `implied_vol` through its `bench-internals` hooks: the iteration cap (never exceeded,
//! reached, and the measured distribution over the milestone 5 grid), `NoConvergence` when the
//! cap is lowered, the initial guess's error over the grid, and dense samples on both sides of
//! the seam between the near-the-money and deep out-of-the-money guess tables.
//!
//! `OPTFIX_IV_SAMPLES` points per carry set (default 100,000).

#![cfg(feature = "bench-internals")]
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::float_arithmetic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::disallowed_macros,
    missing_docs
)]

mod common;

use common::iv::{carry_sample, envelope, f, guess_errors, iteration_histogram, iv_inputs};
use common::{bsm_inputs, Rng};
use optfix::iv_bench::{all_iterations, initial_guess, with_cap};
use optfix::{
    exp, implied_vol, ln, price, price_and_greeks, price_error_bound, EuropeanOption, Fixed,
    IvInputs, MarketInputs, OptionKind, Price, PricingError, Rate, Spot, Strike, Volatility, Years,
    IV_MAX_ITERATIONS,
};

/// The guess is within this of `ln σ` wherever `implied_vol` answers (the generator measures
/// at most 0.06 over the normalized domain).
const GUESS_BOUND: f64 = 0.0625;

/// `SWITCH` of `iv_tables.rs`: the guess comes from the deep table above it.
const SWITCH: f64 = 10.0;

fn samples() -> usize {
    std::env::var("OPTFIX_IV_SAMPLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100_000)
}

#[test]
fn test_iteration_cap_and_distribution() {
    let n = samples();
    let hist = iteration_histogram(n, 0x2f60);
    println!("steps taken (1, 2, 3) by outcome over {} points:", 3 * n);
    for (name, h) in ["Ok", "IllConditioned", "other"].iter().zip(hist) {
        println!("  {name:<15} {:>8} {:>8} {:>8}", h[1], h[2], h[3]);
    }
    let at_cap: usize = hist.iter().map(|h| h[3]).sum();
    assert!(at_cap > 0, "no solve reached the cap");
}

/// With every step taken (no early exit, the worst-case CU row), each answer is still within
/// the round-trip envelope.
#[test]
fn test_all_iterations_stays_within_the_envelope() {
    let mut g = Rng(0x2f70);
    for _ in 0..samples() / 10 {
        let p = carry_sample(&mut g, 1);
        let (opt, m) = bsm_inputs(p);
        let target = price(&opt, &m).unwrap();
        let im = iv_inputs(&m);
        if implied_vol(&opt, &im, target).is_err() {
            continue;
        }
        let full = all_iterations(&opt, &im, target).unwrap();
        let e = price_error_bound(&opt, &m).unwrap();
        let vega = price_and_greeks(&opt, &m).unwrap().greeks.vega;
        let sigma = m.vol.get();
        let err = f(full.get().checked_sub(sigma).unwrap()).abs();
        assert!(
            err <= envelope(e, vega, sigma),
            "all_iterations outside the envelope at {p:?}"
        );
    }
}

fn atm() -> (EuropeanOption, MarketInputs) {
    let k = Fixed::from_int(100);
    (
        EuropeanOption {
            kind: OptionKind::Call,
            strike: Strike::try_new(k).unwrap(),
            expiry: Years::try_new(Fixed::ONE).unwrap(),
        },
        MarketInputs {
            spot: Spot::try_new(Fixed::from_int(90)).unwrap(),
            vol: Volatility::try_new(Fixed::from_raw(1 << 62)).unwrap(),
            rate: Rate::try_new(Fixed::ZERO).unwrap(),
            carry: Rate::try_new(Fixed::ZERO).unwrap(),
        },
    )
}

/// One step from the guess (within 6%) cannot meet the tolerance: `NoConvergence`, after
/// exactly one step; with the documented cap the same input converges.
#[test]
fn test_no_convergence_when_the_cap_is_lowered() {
    let (opt, m) = atm();
    let target = price(&opt, &m).unwrap();
    let im = iv_inputs(&m);
    assert_eq!(
        with_cap(&opt, &im, target, 1),
        (Err(PricingError::NoConvergence), 1)
    );
    assert!(with_cap(&opt, &im, target, IV_MAX_ITERATIONS).0.is_ok());
}

#[test]
fn test_initial_guess_error_over_the_grid() {
    let (worst, count) = guess_errors(samples(), 0x3f60, GUESS_BOUND);
    println!(
        "initial guess |error| in ln σ: near table {:.4} over {}, deep table {:.4} over {}",
        worst[0], count[0], worst[1], count[1]
    );
    assert!(count[1] > 0, "no answered point used the deep table");
}

/// Targets placed on a dense grid of `u = ln a + L` across `SWITCH` (from `SWITCH − 1` to
/// `SWITCH + 1` in steps of 1/64), for out-of-the-money calls and puts with `a` from `2^-4` to 2.9
/// and expiries from `10^-4` to 25 years: the guess table switches exactly at `SWITCH`, every
/// solve answers or is rejected as `IllConditioned` or outside the `Volatility` range (never
/// `NoConvergence`), and each answer's guess is within `GUESS_BOUND`.
#[test]
fn test_dense_samples_across_the_table_seam() {
    let k = Fixed::from_int(100);
    let mut answered = [0usize; 2];
    let mut worst = [0f64; 2];
    let zero = Rate::try_new(Fixed::ZERO).unwrap();
    for a_step in 0..=48 {
        // a from 2^-4 to 2.9 (S/K stays inside [0.05, 20]), geometrically
        let a = 0.0625 * (46.4f64).powf(f64::from(a_step) / 48.0);
        for kind in [OptionKind::Call, OptionKind::Put] {
            // Out of the money: S = K e^(∓a).
            let sign = if kind == OptionKind::Call { -1.0 } else { 1.0 };
            let s = exp(Fixed::from_raw((sign * a * common::ONE as f64) as i128))
                .unwrap()
                .mul_down(k)
                .unwrap();
            let spot = Spot::try_new(s).unwrap();
            let a_exact = f(ln(s).unwrap().checked_sub(ln(k).unwrap()).unwrap()).abs();
            let bound = if kind == OptionKind::Call { s } else { k };
            for t in [1e-4, 1e-2, 1.0, 25.0] {
                let opt = EuropeanOption {
                    kind,
                    strike: Strike::try_new(k).unwrap(),
                    expiry: Years::try_new(Fixed::from_raw((t * common::ONE as f64) as i128))
                        .unwrap(),
                };
                let im = IvInputs {
                    spot,
                    rate: zero,
                    carry: zero,
                };
                for d in -64i32..=64 {
                    let u = SWITCH + f64::from(d) / 64.0;
                    let big_l = u - a_exact.ln();
                    let y = (-big_l).exp();
                    let target = bound.mul_down(Fixed::from_raw((y * common::ONE as f64) as i128));
                    let Ok(target) = target.and_then(Price::try_new) else {
                        continue;
                    };
                    if target.get() == Fixed::ZERO {
                        continue;
                    }
                    let (v0, deep) = match initial_guess(&opt, &im, target) {
                        Ok(g) => g,
                        Err(e) => panic!("{e:?} at a {a}, T {t}, u {u}, {kind:?}"),
                    };
                    // The rounded target moves u by far less than 1/64 except near the edges.
                    if d.abs() > 1 {
                        assert_eq!(deep, d > 0, "table side at a {a}, T {t}, u {u}");
                    }
                    match implied_vol(&opt, &im, target) {
                        Ok(iv) => {
                            let err = (f(v0) - f(iv.get()).ln()).abs();
                            assert!(
                                err <= GUESS_BOUND,
                                "guess off by {err:.4} at a {a}, T {t}, u {u}, {kind:?}"
                            );
                            answered[usize::from(deep)] += 1;
                            worst[usize::from(deep)] = worst[usize::from(deep)].max(err);
                        }
                        Err(PricingError::IllConditioned | PricingError::DomainError) => {}
                        Err(e) => panic!("{e:?} at a {a}, T {t}, u {u}, {kind:?}"),
                    }
                }
            }
        }
    }
    println!(
        "seam: answered {} below / {} above SWITCH, worst guess error {:.4} / {:.4}",
        answered[0], answered[1], worst[0], worst[1]
    );
    assert!(answered[0] > 1000 && answered[1] > 1000, "{answered:?}");
}
