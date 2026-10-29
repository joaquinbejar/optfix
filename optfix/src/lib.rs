//! Fixed-point option pricing for Solana programs and off-chain Rust.
//!
//! `no_std`, no allocator, no floating point, no `unsafe` and no Solana dependency. Every
//! arithmetic step is checked and every rounding is explicit: primitive operations take a
//! direction (`mul_down`, `div_up`, a [`Rounding`] argument) and never round toward zero.
//!
//! This is milestone 1 of `docs/PLAN.md`: the [`Fixed`] number, its 256-bit intermediates, the
//! quantity newtypes and [`PricingError`]. The representation (decimal `10^18` here) is
//! provisional until milestone 3.

#![cfg_attr(not(test), no_std)]

#[cfg(feature = "bench-internals")]
#[doc(hidden)]
pub mod bench_internals;
#[cfg(feature = "bench-internals")]
mod bench_u128;
mod error;
mod fixed;
mod quantity;
mod u256;

pub use error::PricingError;
pub use fixed::{Fixed, FixedBytes, Rounding};
pub use quantity::{Price, Rate, Spot, Strike, Volatility, Years, SECONDS_PER_YEAR};
