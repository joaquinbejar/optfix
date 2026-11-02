//! A minimal on-chain optfix consumer, written to be copied (`docs/PLAN-improvements.md`,
//! milestone 18). One instruction, no accounts: it prices a European option with
//! [`optfix::price_and_greeks`] and returns the result through return data.
//!
//! Instruction data ([`DATA_LEN`] bytes): a kind byte (`0` call, `1` put), then `S`, `K`, `T`
//! (years), `σ`, `r` and `q`, each a [`FixedBytes`] (a Q64.64 value, 16 bytes little endian).
//!
//! Return data ([`RETURN_LEN`] bytes): price, delta, gamma, vega, theta and the price envelope
//! [`optfix::price_error_bound`], each a `FixedBytes`. A protocol that charges or pays this
//! price widens it by the envelope toward itself: the price is within the envelope of the exact
//! Black-Scholes-Merton value, rounding in either direction.
//!
//! Errors fail the transaction: malformed data with `InvalidInstructionData`, an input or
//! result optfix rejects with `Custom(`[`OPTFIX_ERROR_BASE`]` + e as u8)`, so the
//! `PricingError` is recoverable from the program error.

#![no_std]

use optfix::{
    EuropeanOption, Fixed, FixedBytes, MarketInputs, OptionKind, PricingError, Rate, Spot, Strike,
    Volatility, Years,
};
use pinocchio::cpi::set_return_data;
use pinocchio::error::ProgramError;
use pinocchio::{AccountView, Address, ProgramResult};

// The macros expand to the `extern "C"` entrypoint and the target's allocator and panic hooks;
// that generated code is the only `unsafe` here, and the only `panic!` (the handler itself).
#[allow(unsafe_code, clippy::disallowed_macros)]
mod entry {
    pinocchio::program_entrypoint!(super::process_instruction);
    pinocchio::no_allocator!();
    pinocchio::nostd_panic_handler!();
}

/// Instruction data length: a kind byte and six 16-byte values.
pub const DATA_LEN: usize = 1 + 6 * 16;

/// Return data length: six 16-byte values.
pub const RETURN_LEN: usize = 6 * 16;

/// `Custom` program error of an optfix `PricingError` `e`: this plus `e as u8`.
pub const OPTFIX_ERROR_BASE: u32 = 6_000;

/// The program error for an optfix error.
#[must_use]
pub fn optfix_error(e: PricingError) -> ProgramError {
    ProgramError::Custom(OPTFIX_ERROR_BASE.saturating_add(u32::from(e as u8)))
}

/// Decodes the instruction data into the option and its market.
///
/// # Errors
///
/// `InvalidInstructionData` for a wrong length or kind byte; the quantity range checks as
/// [`optfix_error`].
pub fn decode(data: &[u8]) -> Result<(EuropeanOption, MarketInputs), ProgramError> {
    if data.len() != DATA_LEN {
        return Err(ProgramError::InvalidInstructionData);
    }
    let (&kind, values) = data
        .split_first()
        .ok_or(ProgramError::InvalidInstructionData)?;
    let kind = match kind {
        0 => OptionKind::Call,
        1 => OptionKind::Put,
        _ => return Err(ProgramError::InvalidInstructionData),
    };
    // Account data is only 8-aligned, and so is instruction data: read each value as
    // `FixedBytes` (alignment 1), never as a `Fixed`.
    let v: &[FixedBytes] =
        bytemuck::try_cast_slice(values).map_err(|_| ProgramError::InvalidInstructionData)?;
    let [s, k, t, sigma, r, q] = <[FixedBytes; 6]>::try_from(v)
        .map_err(|_| ProgramError::InvalidInstructionData)?
        .map(FixedBytes::get);
    let checked = || -> Result<(EuropeanOption, MarketInputs), PricingError> {
        Ok((
            EuropeanOption {
                kind,
                strike: Strike::try_new(k)?,
                expiry: Years::try_new(t)?,
            },
            MarketInputs {
                spot: Spot::try_new(s)?,
                vol: Volatility::try_new(sigma)?,
                rate: Rate::try_new(r)?,
                carry: Rate::try_new(q)?,
            },
        ))
    };
    checked().map_err(optfix_error)
}

/// Prices the option and encodes the return data.
///
/// # Errors
///
/// optfix's errors, as [`optfix_error`].
pub fn price(opt: &EuropeanOption, m: &MarketInputs) -> Result<[u8; RETURN_LEN], ProgramError> {
    let p = optfix::price_and_greeks(opt, m).map_err(optfix_error)?;
    let envelope = optfix::price_error_bound(opt, m).map_err(optfix_error)?;
    let mut out = [0u8; RETURN_LEN];
    let values: [Fixed; 6] = [
        p.price.get(),
        p.greeks.delta,
        p.greeks.gamma,
        p.greeks.vega,
        p.greeks.theta,
        envelope,
    ];
    for (chunk, v) in out.as_chunks_mut::<16>().0.iter_mut().zip(values) {
        *chunk = v.to_bytes().0;
    }
    Ok(out)
}

/// The entrypoint body.
///
/// # Errors
///
/// As [`decode`] and [`price`].
pub fn process_instruction(
    _program_id: &Address,
    _accounts: &mut [AccountView],
    data: &[u8],
) -> ProgramResult {
    let (opt, m) = decode(data)?;
    set_return_data(&price(&opt, &m)?);
    Ok(())
}
