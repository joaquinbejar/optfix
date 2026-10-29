//! `implied_vol` against the independent mpmath reference of `tests/data/iv_fixture.csv`
//! (`tools/iv_fixture/gen_iv_fixture.py`): 20,754 targets on a boundary set in the style of
//! codex's #11 sweep (`d1` from −9 to 9, near the money at ±1/4 and 0, expiries from `10^-12` s
//! to a year, `σ` from 0.005 to 5, four spot scales, three rate/carry sets), each the exact
//! price rounded to raw Q64.64, with the exact implied volatility `σ*` of that rounded target
//! and the exact vega at `σ*`. With `E` the price envelope and `thr = IV_VEGA_MIN_FACTOR × E`:
//!
//! - vega at `σ*` at least `thr (1 + 2^-8)`: `Ok`, within
//!   `IV_ERR_PRICE_FACTOR × E / vega + IV_ERR_REL × σ*` of `σ*` (clamped to the range);
//! - below `thr` (strictly, no band), or no root (`σ* = −1`): an error, `IllConditioned`,
//!   `ArbitrageBound` or `DomainError`, never a value and never `NoConvergence`.
//!
//! The checks live in `common::iv::fixture_check`, which `make precision` also runs.

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

#[test]
fn test_against_the_mpmath_reference() {
    let st = common::iv::fixture_check("iv_fixture.csv");
    println!(
        "{} rows: {} answered (worst {:.3e} of the envelope, at {}), {} rejected",
        st.rows, st.answered, st.worst, st.worst_at, st.rejected
    );
    assert!(st.rows >= 20_000 && st.answered > 5_000);
}

/// `tests/data/iv_threshold.csv` (`tools/iv_fixture/gen_iv_threshold.py`): targets whose exact
/// root's vega is within `±2^-6` of the threshold, down to `2^-26` and exactly at it. None
/// below the threshold is answered, and every one at `thr (1 + 2^-8)` or above is.
#[test]
fn test_threshold_fixture() {
    let st = common::iv::fixture_check("iv_threshold.csv");
    println!(
        "{} rows: {} answered (worst {:.3e} of the envelope), {} rejected; within [thr, thr (1 + 2^-8)): {} answered, {} rejected",
        st.rows, st.answered, st.worst, st.rejected, st.band_answered, st.band_rejected
    );
    assert!(st.rows >= 4_000 && st.answered > 1_000 && st.rejected > 1_000);
}

/// Codex's #12 adversarial set (`tests/data/iv_threshold_codex.csv`, from
/// `tools/iv_fixture/codex/`, 100 mpmath digits): 10,005 targets straddling the threshold at
/// five `(σ, T)` pairs with strikes stepped by `10^-9` in `ln K` around the threshold, each with
/// the exact ratio of the root's vega to the threshold. None below 1 is answered.
#[test]
fn test_codex_threshold_set() {
    let st = common::iv::fixture_check("iv_threshold_codex.csv");
    println!(
        "{} rows: {} answered (worst {:.3e} of the envelope), {} rejected; within [thr, thr (1 + 2^-8)): {} answered, {} rejected",
        st.rows, st.answered, st.worst, st.rejected, st.band_answered, st.band_rejected
    );
    assert_eq!(st.rows, 10_005);
}
