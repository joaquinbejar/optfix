//! `implied_vol`'s outcome at every point of the milestone 5 grid (3,000,000 points, seed
//! `0x2f60`) and of the three mpmath fixtures (`iv_fixture.csv`, `iv_threshold.csv`,
//! `iv_threshold_codex.csv`): one byte per point, 0 for an answer and `1 + e as u8` for an error.
//! Milestone 20 (`docs/PLAN-improvements.md`) changed the initial guess and the step cap; this
//! pins that every point answers or fails exactly as it did before, so an answer lost (or
//! gained) anywhere fails the test, not only a wrong answer where it still answers.
//!
//! The baseline is that file run unchanged at a250ed9 (0.2's tables, `IV_MAX_ITERATIONS = 3`):
//!
//! ```text
//! git worktree add /tmp/base a250ed9
//! cp optfix/tests/iv_classification.rs /tmp/base/optfix/tests/
//! cd /tmp/base && OPTFIX_IV_CLASSIFY_OUT=base.bin cargo test --release -p optfix \
//!     --test iv_classification -- --ignored --nocapture
//! ```
//!
//! which prints the digest asserted below and writes the bytes (`shasum -a 256 base.bin`).
//! Ignored by default (3,035,763 solves): `make iv-profile` runs it in release.

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

use common::iv::{carry_sample, iv_inputs, CARRY_SETS};
use common::{bsm_inputs, Rng};
use optfix::{implied_vol, price, Fixed, Price, PricingError, Volatility};

/// The baseline at a250ed9: points per outcome byte (0 answered, else
/// `1 + e as u8`), and the FNV-1a 64 of the outcome bytes in order (sha256 of the bytes:
/// d9bed92198e3051422187e5fb420aa5d4dcca097e6fdf4e3aa3484d98e239c71).
const BASELINE_COUNTS: [usize; 6] = [791_782, 0, 300_138, 0, 1_603_370, 340_473];
const BASELINE_FNV: u64 = 0x4814_a49c_a2c8_116e;

fn code(r: Result<Volatility, PricingError>) -> u8 {
    match r {
        Ok(_) => 0,
        Err(e) => 1 + e as u8,
    }
}

#[test]
#[ignore = "3,035,763 solves: make iv-profile"]
fn test_every_point_answers_or_fails_as_before() {
    let mut out: Vec<u8> = Vec::new();
    for (carry, _, offset) in CARRY_SETS {
        let mut g = Rng(0x2f60 + offset);
        for _ in 0..1_000_000 {
            let p = carry_sample(&mut g, carry);
            let (opt, m) = bsm_inputs(p);
            let target = price(&opt, &m).unwrap();
            out.push(code(implied_vol(&opt, &iv_inputs(&m), target)));
        }
    }
    for name in [
        "iv_fixture.csv",
        "iv_threshold.csv",
        "iv_threshold_codex.csv",
    ] {
        let path = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
        for line in std::fs::read_to_string(path).unwrap().lines() {
            let v: Vec<i128> = line
                .split(',')
                .take(10)
                .map(|x| x.parse().unwrap())
                .collect();
            let (opt, m) = bsm_inputs((v[0], v[1], v[2], v[3], v[4], v[5], v[6] == 0));
            let target = Price::try_new(Fixed::from_raw(v[7])).unwrap();
            out.push(code(implied_vol(&opt, &iv_inputs(&m), target)));
        }
    }
    let mut counts = [0usize; 6];
    for c in &out {
        counts[usize::from(*c)] += 1;
    }
    let fnv = out.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    });
    println!(
        "{} points; counts {counts:?}; FNV-1a 64 {fnv:#018x}",
        out.len()
    );
    if let Ok(path) = std::env::var("OPTFIX_IV_CLASSIFY_OUT") {
        std::fs::write(path, &out).unwrap();
    }
    assert_eq!(counts, BASELINE_COUNTS, "outcome counts moved");
    assert_eq!(fnv, BASELINE_FNV, "an outcome moved");
}
