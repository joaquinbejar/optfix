//! The implied-volatility profile of `docs/PLAN-improvements.md`, milestone 20: over the
//! milestone 5 grid (the seed and carry sets of `iv_internals.rs`), for every answered target,
//! the initial guess's error, the steps taken and each step's size, through
//! `iv_bench::trace`. It also replays two stopping rules on the recorded steps (stop after the
//! first step of at most `τ`), the gate's second candidate, and reports what each would save
//! and the error it would leave.
//!
//! `OPTFIX_IV_SAMPLES` points per carry set (default 100,000; the plan's figures use
//! 1,000,000). `OPTFIX_IV_PROFILE_OUT=<path>` also writes the report.

#![cfg(feature = "iv-profile")]
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::float_arithmetic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::disallowed_macros,
    missing_docs
)]

mod common;

use std::fmt::Write as _;

use common::iv::{carry_sample, f, iv_inputs, CARRY_SETS};
use common::{bsm_inputs, Rng};
use optfix::iv_bench::trace;
use optfix::{implied_vol, price, Fixed, IV_ERR_REL, IV_MAX_ITERATIONS, IV_TOLERANCE};

fn samples() -> usize {
    std::env::var("OPTFIX_IV_SAMPLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100_000)
}

/// The value at quantile `q` of sorted `v`.
fn quantile(v: &[f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v[((v.len() - 1) as f64 * q).round() as usize]
}

/// Candidate step thresholds for the replayed stopping rule, as powers of two.
const TAUS: [i32; 5] = [-16, -14, -12, -10, -8];

#[test]
fn test_trace_holds_every_step() {
    assert_eq!(optfix::iv_bench::TRACE_STEPS, IV_MAX_ITERATIONS as usize);
}

#[test]
fn test_iv_profile() {
    let n = samples();
    // Per answered solve: seed error |v0 − v_final|, and the trace.
    let mut seed_err: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
    let mut hist = [[0usize; 4]; 2];
    // |Δ_k| by step k, over solves that took more than k steps (non-final steps) and the final
    // step of each solve.
    let mut nonfinal: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut last: Vec<f64> = Vec::new();
    // Largest e_k / |Δ_k|^3 over non-final steps with |Δ_k| >= 2^-30.
    let mut contraction = 0f64;
    // Per τ: steps taken under the rule [1, 2, 3], the largest error left (in ln σ), solves
    // that would stop on a step of a different trajectory (none by construction).
    let mut rule_steps = [[0usize; 4]; TAUS.len()];
    let mut rule_err = [0f64; TAUS.len()];
    let mut answered = 0usize;
    // The seed errors that separate step counts: the largest answered within two steps and the
    // smallest that needed three.
    let (mut max_two, mut min_three) = (0f64, f64::INFINITY);
    for (carry, _, offset) in CARRY_SETS {
        let mut g = Rng(0x2f60 + offset);
        for _ in 0..n {
            let p = carry_sample(&mut g, carry);
            let (opt, m) = bsm_inputs(p);
            let target = price(&opt, &m).unwrap();
            let im = iv_inputs(&m);
            let Ok(t) = trace(&opt, &im, target) else {
                continue;
            };
            assert_eq!(
                t.result,
                implied_vol(&opt, &im, target),
                "trace differs at {p:?}"
            );
            if t.result.is_err() {
                continue;
            }
            answered += 1;
            let taken = t.taken as usize;
            assert!((1..=IV_MAX_ITERATIONS as usize).contains(&taken));
            hist[usize::from(t.deep)][taken] += 1;
            // The iterates v_0 = v0, v_k = v_{k−1} + Δ_k (no clamp is reached where it answers:
            // checked by the final iterate matching the sum).
            let steps = &t.steps[..taken];
            let total: i128 = steps.iter().map(|s| s.raw()).sum();
            let e0 = f(Fixed::from_raw(total)).abs();
            seed_err[usize::from(t.deep)].push(e0);
            if taken <= 2 {
                max_two = max_two.max(e0);
            } else {
                min_three = min_three.min(e0);
            }
            // e_k = |v_final − v_k| = |Σ_{j>k} Δ_j|.
            let left = |k: usize| -> f64 {
                f(Fixed::from_raw(steps[k..].iter().map(|s| s.raw()).sum())).abs()
            };
            for (k, s) in steps.iter().enumerate() {
                let d = f(*s).abs();
                if k + 1 < taken {
                    nonfinal[k].push(d);
                    if d >= 2f64.powi(-30) {
                        contraction = contraction.max(left(k + 1) / d.powi(3));
                    }
                } else {
                    last.push(d);
                }
            }
            for (i, tau) in TAUS.iter().enumerate() {
                let bound = 2f64.powi(*tau);
                let stop = steps
                    .iter()
                    .position(|s| f(*s).abs() <= bound)
                    .map_or(taken, |k| k + 1);
                rule_steps[i][stop] += 1;
                if stop < taken {
                    rule_err[i] = rule_err[i].max(left(stop));
                }
            }
        }
    }
    for v in seed_err
        .iter_mut()
        .chain(nonfinal.iter_mut())
        .chain(std::iter::once(&mut last))
    {
        v.sort_by(f64::total_cmp);
    }
    let all_seed: Vec<f64> = {
        let mut v: Vec<f64> = seed_err.concat();
        v.sort_by(f64::total_cmp);
        v
    };
    let qs = |v: &[f64]| {
        format!(
            "{:.2e} | {:.2e} | {:.2e} | {:.2e} | {:.2e}",
            quantile(v, 0.5),
            quantile(v, 0.9),
            quantile(v, 0.99),
            quantile(v, 0.999),
            v.last().copied().unwrap_or(f64::NAN)
        )
    };
    let mut s = String::new();
    writeln!(
        s,
        "Profile over {} grid points ({n} per carry set, seed `0x2f60`), {answered} answered.\n",
        3 * n
    )
    .unwrap();
    writeln!(
        s,
        "| Quantity | Median | 90% | 99% | 99.9% | Max |\n| --- | ---: | ---: | ---: | ---: | ---: |"
    )
    .unwrap();
    writeln!(
        s,
        "| Seed error `|v0 − ln σ|`, near table | {} |",
        qs(&seed_err[0])
    )
    .unwrap();
    writeln!(s, "| Seed error, deep table | {} |", qs(&seed_err[1])).unwrap();
    writeln!(s, "| Seed error, all | {} |", qs(&all_seed)).unwrap();
    for (k, v) in nonfinal.iter().enumerate() {
        if !v.is_empty() {
            writeln!(
                s,
                "| `|Δ{}|`, not the last step ({}) | {} |",
                k + 1,
                v.len(),
                qs(v)
            )
            .unwrap();
        }
    }
    writeln!(
        s,
        "| `|Δ|` of the last step ({}) | {} |",
        last.len(),
        qs(&last)
    )
    .unwrap();
    writeln!(
        s,
        "\nSteps taken (1, 2, 3): near table {:?}, deep table {:?}. Largest `e_k / |Δ_k|^3` \
         (error left after a non-final step over its size cubed, `|Δ_k| >= 2^-30`): {contraction:.3}. \
         `IV_TOLERANCE` = 2^{}, `IV_ERR_REL` = {:.3e}. Seed errors: the largest answered within \
         two steps {max_two:.3e}, the smallest that needed three {min_three:.3e}.\n",
        &hist[0][1..],
        &hist[1][1..],
        IV_TOLERANCE.raw().trailing_zeros() as i32 - 64,
        f(IV_ERR_REL)
    )
    .unwrap();
    writeln!(
        s,
        "| Stop after the first step `|Δ| <= τ` | 1 step | 2 steps | 3 steps | Largest error left in `ln σ` |\n| --- | ---: | ---: | ---: | ---: |"
    )
    .unwrap();
    for (i, tau) in TAUS.iter().enumerate() {
        writeln!(
            s,
            "| `τ = 2^{tau}` | {} | {} | {} | {:.3e} |",
            rule_steps[i][1], rule_steps[i][2], rule_steps[i][3], rule_err[i]
        )
        .unwrap();
    }
    println!("{s}");
    if let Ok(path) = std::env::var("OPTFIX_IV_PROFILE_OUT") {
        std::fs::write(path, &s).unwrap();
    }
    // The replay of today's rule reproduces the solve.
    assert_eq!(
        rule_steps[0][1..],
        [&hist[0][1..], &hist[1][1..]]
            .iter()
            .fold([0usize; 3], |mut acc, h| {
                for (a, x) in acc.iter_mut().zip(h.iter()) {
                    *a += x;
                }
                acc
            })
    );
}
