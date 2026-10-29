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
use optfix::repr::{Binary, Decimal, Repr};
use optfix::{Fixed, FixedBytes, PricingError};
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
    /// `exp` in Q64.64 (milestone 3 comparison).
    ExpBinary = 4,
    /// `ln` in Q64.64 (milestone 3 comparison).
    LnBinary = 5,
    /// `exp` in decimal `10^18` (milestone 3 comparison).
    ExpDecimal = 6,
    /// `ln` in decimal `10^18` (milestone 3 comparison).
    LnDecimal = 7,
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
            4 => Ok(Self::ExpBinary),
            5 => Ok(Self::LnBinary),
            6 => Ok(Self::ExpDecimal),
            7 => Ok(Self::LnDecimal),
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
    let result = match Instruction::from_tag(tag)? {
        Instruction::Noop => noop(args)?,
        Instruction::MulDown => binary(args, Fixed::mul_down)?,
        Instruction::DivDown => binary(args, Fixed::div_down)?,
        Instruction::SqrtDown => unary(args, Fixed::sqrt_down)?,
        Instruction::ExpBinary => unary(args, raw_fn::<Binary, true>)?,
        Instruction::LnBinary => unary(args, raw_fn::<Binary, false>)?,
        Instruction::ExpDecimal => unary(args, raw_fn::<Decimal, true>)?,
        Instruction::LnDecimal => unary(args, raw_fn::<Decimal, false>)?,
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

/// `R::exp` (`EXP = true`) or `R::ln` on the raw value of a `Fixed`.
fn raw_fn<R: Repr, const EXP: bool>(x: Fixed) -> Result<Fixed, PricingError> {
    let f = if EXP { R::exp } else { R::ln };
    f(x.raw()).map(Fixed::from_raw)
}
