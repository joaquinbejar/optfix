//! `implied_vol` against the independent mpmath reference of `tests/data/iv_fixture.csv`
//! (`tools/iv_fixture/gen_iv_fixture.py`): 20,754 targets on a boundary set in the style of
//! codex's #11 sweep (`d1` from −9 to 9, near the money at ±1/4 and 0, expiries from `10^-12` s
//! to a year, `σ` from 0.005 to 5, four spot scales, three rate/carry sets), each the exact
//! price rounded to raw Q64.64, with the exact implied volatility `σ*` of that rounded target
//! and the exact vega at `σ*`. With `E` the price envelope and `thr = IV_VEGA_MIN_FACTOR × E`:
//!
//! - vega at `σ*` above `thr` (by `2^-8`): `Ok`, within
//!   `IV_ERR_PRICE_FACTOR × E / vega + IV_ERR_REL × σ*` of `σ*` (clamped to the range);
//! - below `thr` (by `2^-8`), or no root (`σ* = −1`): an error, `IllConditioned`,
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
    let st = common::iv::fixture_check();
    println!(
        "{} rows: {} answered (worst {:.3e} of the envelope, at {}), {} rejected",
        st.rows, st.answered, st.worst, st.worst_at, st.rejected
    );
    assert!(st.rows >= 20_000 && st.answered > 5_000);
}
