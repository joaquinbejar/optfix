//! Fixed-point option pricing for Solana programs and off-chain Rust.
//!
//! `no_std`, no allocator, no floating point, no `unsafe` and no Solana dependency. Every
//! arithmetic step is checked and every rounding is explicit: primitive operations take a
//! direction (`mul_down`, `div_up`, a [`Rounding`] argument) and never round toward zero.
//!
//! [`Fixed`] is binary Q64.64 (`raw / 2^64`), chosen by measurement in milestone 3 of
//! `docs/PLAN.md`. This release has the number, its 256-bit intermediates, the quantity
//! newtypes, [`PricingError`], [`exp`] and [`ln`], the normal [`norm_cdf`] and [`norm_pdf`],
//! Black-Scholes-Merton [`price`] and [`price_and_greeks`] with a continuous carry, and
//! [`implied_vol`], each with its error bound.

#![cfg_attr(not(test), no_std)]

mod bsm;
#[cfg(feature = "bench-internals")]
#[doc(hidden)]
pub use bsm::baseline;
mod error;
mod explog;
mod fixed;
mod iv;
#[cfg(feature = "bench-internals")]
#[doc(hidden)]
pub use iv::bench as iv_bench;
mod iv_tables;
mod kernel;
mod norm;
mod quantity;
mod u256;
pub mod wide;

pub use bsm::{
    intrinsic, price, price_and_greeks, price_error_bound, EuropeanOption, Greeks, MarketInputs,
    OptionKind, Priced, DELTA_ERR, GAMMA_ERR_REL, GREEK_ERR_ABS, PRICE_ERR_ABS, PRICE_ERR_REL,
    THETA_ERR_REL, VEGA_ERR_REL,
};
pub use error::PricingError;
pub use explog::{exp, ln, EXP_MAX_ERROR_ULPS, LN_MAX_ERROR_ULPS};
pub use fixed::{Fixed, FixedBytes, Rounding};
pub use iv::{
    implied_vol, IvInputs, IV_ERR_PRICE_FACTOR, IV_ERR_REL, IV_MAX_ITERATIONS, IV_TOLERANCE,
    IV_VEGA_MIN_FACTOR,
};
pub use norm::{
    norm_cdf, norm_pdf, NORM_CDF_CUTOFF, NORM_CDF_MAX_ERROR_ULPS, NORM_CDF_TAIL_REL_ERROR,
    NORM_PDF_MAX_ERROR_ULPS,
};
pub use quantity::{Price, Rate, Spot, Strike, Volatility, Years, SECONDS_PER_YEAR};
