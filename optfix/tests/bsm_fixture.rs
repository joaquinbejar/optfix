//! Codex's #11 sweep as a committed fixture (`tests/data/bsm_fixture.csv`, from
//! `tools/bsm_fixture/gen_sweep.py` and `expected.py`): 19,106 inputs with `d1` from −9 to 9 at
//! expiries from `10^-12` s to a year, and their Black-Scholes-Merton values from mpmath at 100
//! digits, rounded to raw Q64.64. Every row checks that
//!
//! - the 256-bit reference in `common/` agrees with mpmath (within one raw unit), and
//! - `price_and_greeks` is within its envelopes of the reference.
//!
//! `OPTFIX_FIXTURE_EXPORT=<path>` also writes optfix's outputs in the format of
//! `tools/bsm_fixture/check_mpmath.py`, the independent checker.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::float_arithmetic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::disallowed_macros,
    missing_docs
)]

mod common;

use std::fmt::Write as _;

use common::{bsm_inputs, bsm_ref, div, mul, ratio, to_ref};
use num_bigint::BigInt;
use optfix::{
    price_and_greeks, Fixed, DELTA_ERR, GAMMA_ERR_REL, GREEK_ERR_ABS, PRICE_ERR_ABS, PRICE_ERR_REL,
    THETA_ERR_REL, VEGA_ERR_REL,
};

/// `|got − want| <= rel × scale + abs` in Q.P, and the ratio of the two.
fn ratio_to_envelope(got: Fixed, want: &BigInt, rel: Fixed, scale: &BigInt, abs: Fixed) -> f64 {
    let err = (to_ref(got.raw()) - want).magnitude().clone();
    let allowed = mul(&to_ref(rel.raw()), scale) + to_ref(abs.raw());
    ratio(&BigInt::from(err)) / ratio(&allowed)
}

#[test]
fn test_fixture_within_envelopes() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/bsm_fixture.csv");
    let text = std::fs::read_to_string(path).unwrap();
    let mut export = String::new();
    let mut worst = [0f64; 5];
    let mut violations = [0usize; 5];
    let mut rows = 0;
    for line in text.lines() {
        let f: Vec<i128> = line.split(',').map(|v| v.parse().unwrap()).collect();
        let p = (f[0], f[1], f[2], f[3], f[4], f[5], f[6] == 0);
        let (s, k, t, sigma, r, q, call) = p;
        let rf = bsm_ref(
            call,
            &to_ref(s),
            &to_ref(k),
            &to_ref(t),
            &to_ref(sigma),
            &to_ref(r),
            &to_ref(q),
        );
        // The reference against mpmath: each value rounded to raw Q64.64 within one unit.
        for (i, want) in [&rf.price, &rf.delta, &rf.gamma, &rf.vega, &rf.theta]
            .iter()
            .enumerate()
        {
            let got: BigInt = (*want + (BigInt::from(1) << 191)) >> 192u32;
            let diff = (got - BigInt::from(f[7 + i])).magnitude().clone();
            assert!(
                diff <= BigInt::from(1u8).magnitude().clone(),
                "reference differs from mpmath at {line} (output {i})"
            );
        }
        let (opt, m) = bsm_inputs(p);
        let v = price_and_greeks(&opt, &m).unwrap();
        let g = v.greeks;
        writeln!(
            export,
            "{},{},{},{},{},{},{},{},{},{},{},{}",
            f[0],
            f[1],
            f[2],
            f[3],
            f[4],
            f[5],
            f[6],
            v.price.get().raw(),
            g.delta.raw(),
            g.gamma.raw(),
            g.vega.raw(),
            g.theta.raw()
        )
        .unwrap();
        let gamma_scale = div(&rf.dq, &mul(&to_ref(s), &rf.sst));
        let vega_scale = mul(&rf.s_dq, &rf.sqrt_t);
        let theta_scale = div(&mul(&rf.s_dq, &to_ref(sigma)), &rf.sqrt_t)
            + mul(&to_ref(r.abs()), &rf.k_dr)
            + mul(&to_ref(q.abs()), &rf.s_dq);
        let ratios = [
            ratio_to_envelope(
                v.price.get(),
                &rf.price,
                PRICE_ERR_REL,
                &to_ref(s.max(k)),
                PRICE_ERR_ABS,
            ),
            ratio_to_envelope(g.delta, &rf.delta, Fixed::ZERO, &BigInt::from(0), DELTA_ERR),
            ratio_to_envelope(
                g.gamma,
                &rf.gamma,
                GAMMA_ERR_REL,
                &gamma_scale,
                GREEK_ERR_ABS,
            ),
            ratio_to_envelope(g.vega, &rf.vega, VEGA_ERR_REL, &vega_scale, GREEK_ERR_ABS),
            ratio_to_envelope(
                g.theta,
                &rf.theta,
                THETA_ERR_REL,
                &theta_scale,
                GREEK_ERR_ABS,
            ),
        ];
        for (i, x) in ratios.iter().enumerate() {
            worst[i] = worst[i].max(*x);
            if *x > 1.0 {
                violations[i] += 1;
            }
        }
        rows += 1;
    }
    if let Ok(out) = std::env::var("OPTFIX_FIXTURE_EXPORT") {
        std::fs::write(out, export).unwrap();
    }
    println!("{rows} rows; worst envelope ratios price/delta/gamma/vega/theta {worst:?}; violations {violations:?}");
    assert_eq!(
        violations, [0; 5],
        "envelope violations (worst ratios {worst:?})"
    );
    assert!(rows >= 19_000);
}
