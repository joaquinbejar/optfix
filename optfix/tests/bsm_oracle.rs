//! `price_and_greeks` against the OptionStratLib 0.23 oracle on the three carry sets of PLAN
//! 4.7 (`q > 0`, `q < 0`, `q = 0`, the last also against plain Black-Scholes), within the
//! oracle tolerances of `common/oracle.rs`. `OPTFIX_ORACLE_SAMPLES` points per set (default
//! 20,000).

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

use common::oracle::oracle_stats;

#[test]
fn test_price_and_greeks_match_the_oracle() {
    let n: usize = std::env::var("OPTFIX_ORACLE_SAMPLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000);
    let names = [
        "price",
        "delta",
        "gamma",
        "vega",
        "theta",
        "price vs black_scholes",
    ];
    for carry in [1, -1, 0] {
        let st = oracle_stats(n, carry);
        for (i, name) in names.iter().enumerate() {
            println!(
                "q {carry:+}: {name:<22} {:.3e} of tolerance  {}",
                st.ratio[i], st.at[i]
            );
            assert!(
                st.ratio[i] <= 1.0,
                "q {carry:+}: {name} outside the oracle tolerance at {}",
                st.at[i]
            );
        }
        assert!(st.skipped_n < n, "every point had |d1| > 4.8");
    }
}
