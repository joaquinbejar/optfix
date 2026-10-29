//! Stateless Pinocchio program that runs one optfix function per instruction, so LiteSVM can
//! measure its compute-unit cost. It takes no accounts and is never deployed.
//!
//! Instruction data is a one-byte [`Instruction`] tag followed by the arguments as
//! [`FixedBytes`] (16 bytes, little endian). The result goes back through return data as
//! [`RESULT_LEN`] bytes: a status byte (`0` for success, otherwise `1 +` the
//! [`PricingError`] discriminant) and the result as `FixedBytes` (zero on error). An error is
//! reported in the return data rather than by failing the transaction, so its cost is measured
//! too.

#![no_std]

use bytemuck::Pod;
use optfix::{
    EuropeanOption, Fixed, FixedBytes, Greeks, MarketInputs, OptionKind, Priced, PricingError,
    Rate, Rounding, Spot, Strike, Volatility, Years,
};
use pinocchio::cpi::set_return_data;
use pinocchio::error::ProgramError;
use pinocchio::{AccountView, Address, ProgramResult};

// The macros expand to the `extern "C"` entrypoint and the target's allocator and panic hooks;
// that generated code is the only `unsafe` in this crate. `no_allocator!` also defines
// `allocate_unchecked`, whose offset helper asserts its bounds; nothing here calls it.
#[allow(unsafe_code, clippy::disallowed_macros)]
mod entry {
    pinocchio::program_entrypoint!(super::process_instruction);
    pinocchio::no_allocator!();
    pinocchio::nostd_panic_handler!();
}

/// Length of the return data: status byte plus one `FixedBytes`.
pub const RESULT_LEN: usize = 17;

/// Instruction tags (the first byte of the instruction data).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Instruction {
    /// Baseline: decodes one `FixedBytes` and returns it unchanged. Its cost is subtracted
    /// from every other row.
    Noop = 0,
    /// `Fixed::mul_down`.
    MulDown = 1,
    /// `Fixed::div_down`.
    DivDown = 2,
    /// `Fixed::sqrt_down`.
    SqrtDown = 3,
    /// `optfix::exp`.
    Exp = 4,
    /// `optfix::ln`.
    Ln = 5,
    /// `optfix::norm_cdf`.
    NormCdf = 6,
    /// `optfix::norm_pdf`.
    NormPdf = 7,
    /// `optfix::wide::mul_shr`: two `u128` (16 bytes each), the shift (`u32`, 4 bytes) and the
    /// rounding (1 byte, `0` down, `1` up), all little endian. Tag 8 leaves 6 and 7 to the
    /// normal CDF and PDF of milestone 4.
    MulShr = 8,
    /// `optfix::price`: an option-kind byte (`0` call, `1` put) and six `FixedBytes`: spot,
    /// strike, expiry, volatility, rate, carry.
    Price = 9,
    /// `optfix::price_and_greeks`, same arguments; returns the price and four greeks.
    PriceAndGreeks = 10,
    /// The benchmark-only baseline (`optfix::baseline::separately`): price and each greek
    /// computed separately; same arguments and return data as `PriceAndGreeks`.
    Baseline = 11,
}

impl Instruction {
    /// Decodes a tag byte.
    ///
    /// # Errors
    ///
    /// [`BenchError::UnknownInstruction`] for an unassigned tag.
    #[must_use = "the result carries a possible error"]
    pub fn from_tag(tag: u8) -> Result<Self, BenchError> {
        match tag {
            0 => Ok(Self::Noop),
            1 => Ok(Self::MulDown),
            2 => Ok(Self::DivDown),
            3 => Ok(Self::SqrtDown),
            4 => Ok(Self::Exp),
            5 => Ok(Self::Ln),
            6 => Ok(Self::NormCdf),
            7 => Ok(Self::NormPdf),
            8 => Ok(Self::MulShr),
            9 => Ok(Self::Price),
            10 => Ok(Self::PriceAndGreeks),
            11 => Ok(Self::Baseline),
            _ => Err(BenchError::UnknownInstruction),
        }
    }
}

/// Errors of the bench program, from code 100 (matchbook convention).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum BenchError {
    /// Empty instruction data or an unassigned tag.
    UnknownInstruction = 100,
    /// The arguments have the wrong length for the instruction.
    InvalidArguments = 101,
}

impl From<BenchError> for ProgramError {
    #[cold]
    fn from(e: BenchError) -> Self {
        ProgramError::Custom(e as u32)
    }
}

/// Length of the return data of `PriceAndGreeks` and `Baseline`: status byte and five values.
pub const PRICED_LEN: usize = 81;

/// Encodes a priced result: status byte, then price, delta, gamma, vega, theta.
#[must_use]
pub fn encode_priced(result: Result<Priced, PricingError>) -> [u8; PRICED_LEN] {
    let mut out = [0u8; PRICED_LEN];
    let (status, values) = match result {
        Ok(p) => (
            0u8,
            [
                p.price.get(),
                p.greeks.delta,
                p.greeks.gamma,
                p.greeks.vega,
                p.greeks.theta,
            ],
        ),
        Err(e) => (error_status(e), [Fixed::ZERO; 5]),
    };
    let (head, mut tail) = out.split_at_mut(1);
    head.copy_from_slice(&[status]);
    for v in values {
        let (chunk, rest) = tail.split_at_mut(16);
        chunk.copy_from_slice(&v.to_bytes().0);
        tail = rest;
    }
    out
}

/// Encodes a result as return data: status byte, then the value.
#[must_use]
pub fn encode_result(result: Result<Fixed, PricingError>) -> [u8; RESULT_LEN] {
    let (status, value) = match result {
        Ok(v) => (0u8, v),
        Err(e) => (error_status(e), Fixed::ZERO),
    };
    let mut out = [0u8; RESULT_LEN];
    let (head, tail) = out.split_at_mut(1);
    head.copy_from_slice(&[status]);
    tail.copy_from_slice(&value.to_bytes().0);
    out
}

/// `1 +` the `PricingError` discriminant.
#[must_use]
pub const fn error_status(e: PricingError) -> u8 {
    match e {
        PricingError::Overflow => 1,
        PricingError::DomainError => 2,
        PricingError::NoConvergence => 3,
        PricingError::ArbitrageBound => 4,
    }
}

/// Program entrypoint body.
///
/// # Errors
///
/// [`BenchError`] for malformed instruction data; optfix errors are reported in the return
/// data instead.
pub fn process_instruction(
    _program_id: &Address,
    _accounts: &mut [AccountView],
    data: &[u8],
) -> ProgramResult {
    let (&tag, args) = data.split_first().ok_or(BenchError::UnknownInstruction)?;
    let ix = Instruction::from_tag(tag)?;
    if let Instruction::PriceAndGreeks | Instruction::Baseline = ix {
        let f = if ix == Instruction::Baseline {
            optfix::baseline::separately
        } else {
            optfix::price_and_greeks
        };
        set_return_data(&encode_priced(bsm(args, f)?));
        return Ok(());
    }
    let result = match ix {
        Instruction::Noop => noop(args)?,
        Instruction::MulDown => binary(args, Fixed::mul_down)?,
        Instruction::DivDown => binary(args, Fixed::div_down)?,
        Instruction::SqrtDown => unary(args, Fixed::sqrt_down)?,
        Instruction::Exp => unary(args, optfix::exp)?,
        Instruction::Ln => unary(args, optfix::ln)?,
        Instruction::NormCdf => unary(args, optfix::norm_cdf)?,
        Instruction::NormPdf => unary(args, optfix::norm_pdf)?,
        Instruction::MulShr => mul_shr(args)?,
        Instruction::Price => bsm(args, price_only)?.map(|p| p.price.get()),
        Instruction::PriceAndGreeks | Instruction::Baseline => {
            return Err(BenchError::InvalidArguments.into())
        }
    };
    set_return_data(&encode_result(result));
    Ok(())
}

/// Reads exactly one `T` from `args`.
fn decode<T: Pod>(args: &[u8]) -> Result<T, BenchError> {
    bytemuck::try_pod_read_unaligned(args).map_err(|_| BenchError::InvalidArguments)
}

#[inline(never)]
fn noop(args: &[u8]) -> Result<Result<Fixed, PricingError>, BenchError> {
    // Noop ignores trailing bytes, so tests can make otherwise identical transactions unique.
    let first = args.get(..16).ok_or(BenchError::InvalidArguments)?;
    let x: FixedBytes = decode(first)?;
    Ok(Ok(x.get()))
}

#[inline(never)]
fn binary(
    args: &[u8],
    f: fn(Fixed, Fixed) -> Result<Fixed, PricingError>,
) -> Result<Result<Fixed, PricingError>, BenchError> {
    let [a, b]: [FixedBytes; 2] = decode(args)?;
    Ok(f(a.get(), b.get()))
}

#[inline(never)]
fn unary(
    args: &[u8],
    f: fn(Fixed) -> Result<Fixed, PricingError>,
) -> Result<Result<Fixed, PricingError>, BenchError> {
    let a: FixedBytes = decode(args)?;
    Ok(f(a.get()))
}

#[inline(never)]
fn mul_shr(args: &[u8]) -> Result<Result<Fixed, PricingError>, BenchError> {
    let bytes = <[u8; 37]>::try_from(args).map_err(|_| BenchError::InvalidArguments)?;
    let (a, rest) = bytes.split_at(16);
    let (b, rest) = rest.split_at(16);
    let (shift, rest) = rest.split_at(4);
    let word = |s: &[u8]| -> Result<[u8; 16], BenchError> {
        s.try_into().map_err(|_| BenchError::InvalidArguments)
    };
    let a = u128::from_le_bytes(word(a)?);
    let b = u128::from_le_bytes(word(b)?);
    let shift = u32::from_le_bytes(shift.try_into().map_err(|_| BenchError::InvalidArguments)?);
    let r = match rest {
        [0] => Rounding::Down,
        [1] => Rounding::Up,
        _ => return Err(BenchError::InvalidArguments),
    };
    // The u128 result travels in the 16 bytes of a `FixedBytes`, bit for bit.
    Ok(optfix::wide::mul_shr(a, b, shift, r)
        .map(|v| Fixed::from_raw(i128::from_le_bytes(v.to_le_bytes()))))
}

/// `optfix::price` in the shape of `price_and_greeks` (greeks zero).
fn price_only(opt: &EuropeanOption, m: &MarketInputs) -> Result<Priced, PricingError> {
    let zero = Fixed::ZERO;
    Ok(Priced {
        price: optfix::price(opt, m)?,
        greeks: Greeks {
            delta: zero,
            gamma: zero,
            vega: zero,
            theta: zero,
        },
    })
}

/// Decodes `[kind, S, K, T, σ, r, q]` and runs `f`; an argument outside its newtype's range is
/// reported as a `DomainError` result, like any other optfix error.
#[inline(never)]
fn bsm(
    args: &[u8],
    f: fn(&EuropeanOption, &MarketInputs) -> Result<Priced, PricingError>,
) -> Result<Result<Priced, PricingError>, BenchError> {
    let (&kind, rest) = args.split_first().ok_or(BenchError::InvalidArguments)?;
    let kind = match kind {
        0 => OptionKind::Call,
        1 => OptionKind::Put,
        _ => return Err(BenchError::InvalidArguments),
    };
    let [s, k, t, sigma, r, q]: [FixedBytes; 6] = decode(rest)?;
    let inputs = || -> Result<(EuropeanOption, MarketInputs), PricingError> {
        Ok((
            EuropeanOption {
                kind,
                strike: Strike::try_new(k.get())?,
                expiry: Years::try_new(t.get())?,
            },
            MarketInputs {
                spot: Spot::try_new(s.get())?,
                vol: Volatility::try_new(sigma.get())?,
                rate: Rate::try_new(r.get())?,
                carry: Rate::try_new(q.get())?,
            },
        ))
    };
    Ok(inputs().and_then(|(opt, m)| f(&opt, &m)))
}
