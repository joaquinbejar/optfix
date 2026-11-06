//! The SVI fixtures of milestone 22 (`docs/PLAN-phase2.md`), from
//! `tools/svi_fixture/gen_fixture.py` (mpmath at 80 digits, values as `nint(v × 2^96)`):
//!
//! - `tests/data/svi_w.csv`: `total_variance` at an exact `k` is never above `w(k)` and within
//!   `E_w` below it, in four strata;
//! - `tests/data/svi_e2e.csv`: from `S, K, r, q, T`, `|k̂ − k*| < SVI_K_ERR`, `svi_vol_bounds`
//!   encloses `σ*` and `svi_vol`, `svi_price_bounds` encloses the premium at `σ*`,
//!   `min_total_variance` is never above the true minimum, and every slice with a negative true
//!   minimum is refused (1,000 rows built so that `ρ²` rounded down would accept them).
//!
//! The 256-bit reference of `common/svi.rs` agrees with every mpmath value but the premium
//! within one unit of `2^-96` (`bsm_ref` saturates `N` beyond 12, so the premium is mpmath's
//! alone), and the comparisons with mpmath allow that unit (`2^-32` ulp).
//! `OPTFIX_FIXTURE_EXPORT=<dir>` writes optfix's outputs for
//! `tools/svi_fixture/check_mpmath.py`, the independent checker.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::float_arithmetic,
    clippy::cast_precision_loss,
    clippy::disallowed_macros,
    missing_docs
)]

mod common;

use std::fmt::Write as _;

use common::svi::{k_star, market, min_ref, params, vol_ref, w_ref, RawSlice};
use common::{to_ref, P};
use num_bigint::BigInt;
use optfix::{
    log_forward_moneyness, svi_price_bounds, svi_vol, svi_vol_bounds, EuropeanOption, Fixed,
    OptionKind, SVI_K_ERR,
};

/// A raw Q64.64 value at `2^-96` resolution.
fn x96(raw: i128) -> BigInt {
    BigInt::from(raw) << 32
}

/// A Q.P reference value rounded to nearest at `2^-96`.
fn ref96(v: &BigInt) -> BigInt {
    (v + (BigInt::from(1) << (P - 97))) >> (P - 96)
}

fn close(reference: &BigInt, mpmath: &BigInt, what: &str, line: &str) {
    let d = (ref96(reference) - mpmath).magnitude().clone();
    assert!(
        d <= BigInt::from(1).magnitude().clone(),
        "reference differs from mpmath ({what}) at {line}"
    );
}

fn parse(line: &str) -> Vec<BigInt> {
    line.split(',').map(|v| v.parse().unwrap()).collect()
}

fn small(v: &BigInt) -> i128 {
    i128::try_from(v).unwrap()
}

#[test]
fn test_w_fixture_one_sided() {
    let text = include_str!("data/svi_w.csv");
    let (mut export, mut rows, mut refused, mut worst) = (String::new(), 0, 0, 0f64);
    for line in text.lines() {
        let f = parse(line);
        let p: RawSlice = [0, 1, 2, 3, 4].map(|i| small(&f[i]));
        let k = small(&f[5]);
        let want = &f[6];
        close(&w_ref(&p, &to_ref(k)), want, "w", line);
        let Some(s) = params(&p) else {
            refused += 1;
            continue;
        };
        let (lo, hi) = s.total_variance_bounds(Fixed::from_raw(k)).unwrap();
        // lo <= w <= hi, to within the fixture's unit of 2^-96.
        assert!(
            x96(lo.raw()) <= want + 1,
            "total_variance above w(k) at {line}"
        );
        assert!(
            want - 1 <= x96(hi.raw()),
            "w(k) above the envelope at {line}"
        );
        let below = (want - x96(lo.raw())).to_string().parse::<f64>().unwrap() / 2f64.powi(32);
        worst = worst.max(below / (hi.raw() - lo.raw()) as f64);
        writeln!(export, "{line},{},{}", lo.raw(), hi.raw()).unwrap();
        rows += 1;
    }
    println!("svi_w: {rows} rows, {refused} refused; worst (w − computed) / E_w {worst:.4}");
    assert!(refused * 50 < rows, "too many refused slices");
    if let Ok(dir) = std::env::var("OPTFIX_FIXTURE_EXPORT") {
        std::fs::write(format!("{dir}/svi_w_out.csv"), export).unwrap();
    }
}

#[test]
fn test_e2e_fixture_enclosures() {
    let text = include_str!("data/svi_e2e.csv");
    let mut export = String::new();
    let (mut checked, mut refused_negative, mut answered) = (0, 0, 0);
    for line in text.lines() {
        let f = parse(line);
        let p: RawSlice = [0, 1, 2, 3, 4].map(|i| small(&f[i]));
        let [sp, k, t, r, q] = [5, 6, 7, 8, 9].map(|i| small(&f[i]));
        let call = f[10] == BigInt::from(0);
        let (ks, w, sig, prem, min) = (&f[11], &f[12], &f[13], &f[14], &f[15]);
        close(&min_ref(&p), min, "minimum", line);
        let Some(s) = params(&p) else {
            if min < &BigInt::from(0) {
                refused_negative += 1;
            }
            writeln!(export, "{line},REFUSED").unwrap();
            continue;
        };
        assert!(
            min >= &BigInt::from(-1),
            "accepted a negative true minimum at {line}"
        );
        assert!(
            x96(s.min_total_variance().raw()) <= min + 1,
            "min_total_variance above the true minimum at {line}"
        );
        let kr = k_star(sp, k, t, r, q);
        close(&kr, ks, "k*", line);
        let wr = w_ref(&p, &kr);
        close(&wr, w, "w(k*)", line);
        let sr = vol_ref(&wr, t);
        close(&sr, sig, "σ*", line);
        let (expiry, m, strike) = market(sp, k, t, r, q);
        let kh = log_forward_moneyness(expiry, &m, strike).unwrap();
        let kerr = (x96(kh.raw()) - ks).magnitude().clone();
        assert!(
            kerr < x96(SVI_K_ERR.raw()).magnitude().clone(),
            "|k̂ − k*| too large at {line}"
        );
        let mut out = format!("{line},{},{}", s.min_total_variance().raw(), kh.raw());
        match svi_vol_bounds(&s, expiry, &m, strike) {
            Ok((lo, hi)) => {
                assert!(
                    x96(lo.raw()) <= sig + 1 && sig - 1 <= x96(hi.raw()),
                    "σ* at {line}"
                );
                write!(out, ",{},{}", lo.raw(), hi.raw()).unwrap();
                match svi_vol(&s, expiry, &m, strike) {
                    Ok(v) => {
                        assert!(lo <= v.get() && v.get() <= hi, "svi_vol at {line}");
                        write!(out, ",{}", v.get().raw()).unwrap();
                        answered += 1;
                    }
                    Err(e) => write!(out, ",ERR{e:?}").unwrap(),
                }
            }
            Err(e) => write!(out, ",ERR{e:?},,").unwrap(),
        }
        let opt = EuropeanOption {
            kind: if call {
                OptionKind::Call
            } else {
                OptionKind::Put
            },
            strike,
            expiry,
        };
        match svi_price_bounds(&opt, &m, &s) {
            Ok((pl, ph)) => {
                // The premium comes from mpmath only: `bsm_ref` saturates `N` beyond 12.
                assert!(
                    x96(pl.get().raw()) <= prem + 1 && prem - 1 <= x96(ph.get().raw()),
                    "premium at σ* outside the price bounds at {line}"
                );
                write!(out, ",{},{}", pl.get().raw(), ph.get().raw()).unwrap();
            }
            Err(e) => write!(out, ",ERR{e:?},").unwrap(),
        }
        writeln!(export, "{out}").unwrap();
        checked += 1;
    }
    println!(
        "svi_e2e: {checked} accepted rows ({answered} with a volatility), {refused_negative} \
         refused with a negative true minimum"
    );
    assert!(answered * 2 > checked && refused_negative > 0);
    if let Ok(dir) = std::env::var("OPTFIX_FIXTURE_EXPORT") {
        std::fs::write(format!("{dir}/svi_e2e_out.csv"), export).unwrap();
    }
}
