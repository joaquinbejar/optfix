//! Fixed-point option pricing for Solana programs and off-chain Rust.
//!
//! `no_std`, no allocator, no floating point, no `unsafe` and no Solana dependency. Every
//! arithmetic step is checked and every rounding is explicit: primitive operations take a
//! direction (`mul_down`, `div_up`, a [`Rounding`] argument) and never round toward zero.
//!
//! [`Fixed`] is binary Q64.64 (`raw / 2^64`), chosen by measurement in milestone 3 of
//! `docs/PLAN.md`. This release has the number, its 256-bit intermediates, the quantity
//! newtypes, [`PricingError`], [`exp`] and [`ln`], and the normal [`norm_cdf`] and [`norm_pdf`],
//! each with its error bound.

#![cfg_attr(not(test), no_std)]

mod error;
mod explog;
mod fixed;
mod kernel;
mod norm;
mod quantity;
mod u256;

pub use error::PricingError;
pub use explog::{exp, ln, EXP_MAX_ERROR_ULPS, LN_MAX_ERROR_ULPS};
pub use fixed::{Fixed, FixedBytes, Rounding};
pub use norm::{
    norm_cdf, norm_pdf, NORM_CDF_CUTOFF, NORM_CDF_MAX_ERROR_ULPS, NORM_CDF_TAIL_REL_ERROR,
    NORM_PDF_MAX_ERROR_ULPS,
};
pub use quantity::{Price, Rate, Spot, Strike, Volatility, Years, SECONDS_PER_YEAR};
