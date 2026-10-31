//! Stateless Pinocchio program that runs one optfix function per instruction, so LiteSVM can
//! measure its compute-unit cost. It takes no accounts and is never deployed.
//!
//! Instruction data is a one-byte [`Instruction`] tag followed by the arguments as
//! [`FixedBytes`] (16 bytes, little endian). The result goes back through return data as
//! [`RESULT_LEN`] bytes: a status byte (`0` for success, otherwise `1 +` the
//! [`PricingError`] discriminant) and the result as `FixedBytes` (zero on error). An error is
//! reported in the return data rather than by failing the transaction, so its cost is measured
//! too.
//!
//! The strip instructions (milestone 11) take `[T, S, σ, r, q]` and up to [`MAX_LEGS`] legs of
//! a kind byte and a strike. `PriceStrip` returns the status byte and every price;
//! `PriceAndGreeksStrip` returns the status byte and a 32-byte [`digest`] of every `Priced`
//! (16 of them would not fit the 1,024 bytes of return data), and `Digest` measures that
//! digest alone over as many zero values, so its cost can be subtracted.

#![no_std]

use bytemuck::Pod;
use optfix::{
    EuropeanOption, Fixed, FixedBytes, Greeks, IvInputs, MarketInputs, OptionKind, Price, Priced,
    PricingError, Rate, Rounding, Spot, Strike, Volatility, Years,
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
    /// `optfix::implied_vol`: an option-kind byte and six `FixedBytes`: spot, strike, expiry,
    /// target price, rate, carry; returns the volatility.
    ImpliedVol = 12,
    /// `implied_vol` taking all `IV_MAX_ITERATIONS` steps (`optfix::iv_bench::all_iterations`,
    /// bench-only): its worst-case cost. Same arguments and return data as `ImpliedVol`.
    ImpliedVolAllIterations = 13,
    /// `optfix::price_strip` (milestone 11).
    PriceStrip = 14,
    /// `optfix::price_and_greeks_strip`, returning a [`digest`] of the outputs.
    PriceAndGreeksStrip = 15,
    /// The [`digest`] alone, over `n` zero values (bench-only, subtracted from the row above).
    Digest = 16,
    /// `optfix::price_black76` (milestone 12): an option-kind byte and five `FixedBytes`:
    /// forward, strike, expiry, volatility, rate; returns the price.
    PriceBlack76 = 17,
    /// `optfix::price_and_greeks_black76`: as `PriceBlack76`; returns the price and greeks.
    PriceAndGreeksBlack76 = 18,
    /// `optfix::price_cash_or_nothing` (milestone 13): as `Price` plus a seventh `FixedBytes`,
    /// the payout; returns the price.
    CashOrNothing = 19,
    /// `optfix::price_asset_or_nothing`: as `Price`; returns the price.
    AssetOrNothing = 20,
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
            12 => Ok(Self::ImpliedVol),
            13 => Ok(Self::ImpliedVolAllIterations),
            14 => Ok(Self::PriceStrip),
            15 => Ok(Self::PriceAndGreeksStrip),
            16 => Ok(Self::Digest),
            17 => Ok(Self::PriceBlack76),
            18 => Ok(Self::PriceAndGreeksBlack76),
            19 => Ok(Self::CashOrNothing),
            20 => Ok(Self::AssetOrNothing),
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
        PricingError::IllConditioned => 5,
        // `PricingError` is `#[non_exhaustive]`: a later variant keeps the `1 +` discriminant
        // rule.
        _ => (e as u8).saturating_add(1),
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
    match ix {
        Instruction::PriceStrip => return price_strip(args),
        Instruction::PriceAndGreeksStrip => return greeks_strip(args),
        Instruction::Digest => return digest_only(args),
        Instruction::PriceAndGreeksBlack76 => {
            set_return_data(&encode_priced(futures(
                args,
                optfix::price_and_greeks_black76,
            )?));
            return Ok(());
        }
        _ => {}
    }
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
        Instruction::PriceBlack76 => futures(args, black76_price_only)?.map(|p| p.price.get()),
        Instruction::CashOrNothing => cash_or_nothing(args)?,
        Instruction::AssetOrNothing => bsm(args, asset_or_nothing_only)?.map(|p| p.price.get()),
        Instruction::ImpliedVol => iv(args, optfix::implied_vol)?,
        Instruction::ImpliedVolAllIterations => iv(args, optfix::iv_bench::all_iterations)?,
        Instruction::PriceAndGreeks
        | Instruction::Baseline
        | Instruction::PriceStrip
        | Instruction::PriceAndGreeksStrip
        | Instruction::Digest
        | Instruction::PriceAndGreeksBlack76 => return Err(BenchError::InvalidArguments.into()),
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
    f: impl Fn(&EuropeanOption, &MarketInputs) -> Result<Priced, PricingError>,
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

/// `price_asset_or_nothing` with zero greeks, so it shares [`bsm`]'s decoding.
fn asset_or_nothing_only(opt: &EuropeanOption, m: &MarketInputs) -> Result<Priced, PricingError> {
    Ok(Priced {
        price: optfix::price_asset_or_nothing(opt, m)?,
        greeks: EMPTY_PRICED.greeks,
    })
}

/// Decodes `[kind, S, K, T, σ, r, q, payout]` and runs `price_cash_or_nothing`; an argument
/// outside its newtype's range is reported as a `DomainError` result.
#[inline(never)]
fn cash_or_nothing(args: &[u8]) -> Result<Result<Fixed, PricingError>, BenchError> {
    let (head, payout) = args
        .split_at_checked(args.len().saturating_sub(16))
        .ok_or(BenchError::InvalidArguments)?;
    let payout: FixedBytes = decode(payout)?;
    let pay = match Price::try_new(payout.get()) {
        Ok(p) => p,
        Err(e) => return Ok(Err(e)),
    };
    Ok(bsm(head, |opt, m| {
        Ok(Priced {
            price: optfix::price_cash_or_nothing(opt, m, pay)?,
            greeks: EMPTY_PRICED.greeks,
        })
    })?
    .map(|p| p.price.get()))
}

/// `price_black76` with zero greeks, so both Black-76 rows share [`futures`].
fn black76_price_only(
    opt: &EuropeanOption,
    m: &optfix::FuturesInputs,
) -> Result<Priced, PricingError> {
    Ok(Priced {
        price: optfix::price_black76(opt, m)?,
        greeks: EMPTY_PRICED.greeks,
    })
}

/// Decodes `[kind, F, K, T, σ, r]` and runs `f` (a Black-76 function); an argument outside its
/// newtype's range is reported as a `DomainError` result.
#[inline(never)]
fn futures(
    args: &[u8],
    f: fn(&EuropeanOption, &optfix::FuturesInputs) -> Result<Priced, PricingError>,
) -> Result<Result<Priced, PricingError>, BenchError> {
    let (&kind, rest) = args.split_first().ok_or(BenchError::InvalidArguments)?;
    let kind = match kind {
        0 => OptionKind::Call,
        1 => OptionKind::Put,
        _ => return Err(BenchError::InvalidArguments),
    };
    let [forward, k, t, sigma, r]: [FixedBytes; 5] = decode(rest)?;
    let inputs = || -> Result<(EuropeanOption, optfix::FuturesInputs), PricingError> {
        Ok((
            EuropeanOption {
                kind,
                strike: Strike::try_new(k.get())?,
                expiry: Years::try_new(t.get())?,
            },
            optfix::FuturesInputs {
                forward: Spot::try_new(forward.get())?,
                vol: Volatility::try_new(sigma.get())?,
                rate: Rate::try_new(r.get())?,
            },
        ))
    };
    Ok(inputs().and_then(|(opt, m)| f(&opt, &m)))
}

/// Decodes `[kind, S, K, T, target, r, q]` and runs `f` (an implied-volatility solver); an
/// argument outside its newtype's range is reported as a `DomainError` result.
#[inline(never)]
fn iv(
    args: &[u8],
    f: fn(&EuropeanOption, &IvInputs, Price) -> Result<Volatility, PricingError>,
) -> Result<Result<Fixed, PricingError>, BenchError> {
    let (&kind, rest) = args.split_first().ok_or(BenchError::InvalidArguments)?;
    let kind = match kind {
        0 => OptionKind::Call,
        1 => OptionKind::Put,
        _ => return Err(BenchError::InvalidArguments),
    };
    let [s, k, t, target, r, q]: [FixedBytes; 6] = decode(rest)?;
    let solve = || -> Result<Fixed, PricingError> {
        let opt = EuropeanOption {
            kind,
            strike: Strike::try_new(k.get())?,
            expiry: Years::try_new(t.get())?,
        };
        let m = IvInputs {
            spot: Spot::try_new(s.get())?,
            rate: Rate::try_new(r.get())?,
            carry: Rate::try_new(q.get())?,
        };
        f(&opt, &m, Price::try_new(target.get())?).map(Volatility::get)
    };
    Ok(solve())
}

/// Most legs a strip instruction takes.
pub const MAX_LEGS: usize = 16;

/// Bytes of one strip leg: a kind byte and a `FixedBytes` strike.
const LEG_LEN: usize = 17;

/// A 32-byte fold of `Priced` values (price and the four greeks, each as two 64-bit halves)
/// into four 64-bit lanes with native multiplies: not cryptographic, only a compact equality
/// check of the outputs against the host's.
#[must_use]
pub fn digest<'a>(values: impl Iterator<Item = &'a Priced>) -> [u8; 32] {
    const K: u64 = 0x9E37_79B9_7F4A_7C15;
    let (mut a, mut b, mut c, mut d) = (K, K.rotate_left(16), K.rotate_left(32), K.rotate_left(48));
    for p in values {
        for v in [
            p.price.get(),
            p.greeks.delta,
            p.greeks.gamma,
            p.greeks.vega,
            p.greeks.theta,
        ] {
            let x = v.raw().to_le_bytes();
            let (lo_bytes, hi_bytes) = x.split_at(8);
            let lo = u64::from_le_bytes(lo_bytes.try_into().unwrap_or([0; 8]));
            let hi = u64::from_le_bytes(hi_bytes.try_into().unwrap_or([0; 8]));
            a = (a ^ lo).wrapping_mul(K);
            b = (b ^ hi).wrapping_mul(K).rotate_left(27);
            c = c.wrapping_add(lo).rotate_left(13) ^ hi;
            d ^= a ^ b;
        }
    }
    let mut out = [0u8; 32];
    for (chunk, lane) in out.as_chunks_mut::<8>().0.iter_mut().zip([a, b, c, d]) {
        chunk.copy_from_slice(&lane.to_le_bytes());
    }
    out
}

/// Decodes a strip: `[T, S, σ, r, q]` and the legs into `legs`; returns the leg count. An
/// argument outside its newtype's range is a `DomainError` result.
fn decode_strip(
    args: &[u8],
    legs: &mut [optfix::StripLeg; MAX_LEGS],
) -> Result<Result<(Years, MarketInputs, usize), PricingError>, BenchError> {
    let head = args.get(..80).ok_or(BenchError::InvalidArguments)?;
    let rest = args.get(80..).ok_or(BenchError::InvalidArguments)?;
    let [t, s, sigma, r, q]: [FixedBytes; 5] = decode(head)?;
    let (chunks, tail) = rest.as_chunks::<LEG_LEN>();
    if !tail.is_empty() || chunks.len() > MAX_LEGS {
        return Err(BenchError::InvalidArguments);
    }
    let n = chunks.len();
    for (slot, raw) in legs.iter_mut().zip(chunks) {
        let (&kind, strike) = raw.split_first().ok_or(BenchError::InvalidArguments)?;
        let kind = match kind {
            0 => OptionKind::Call,
            1 => OptionKind::Put,
            _ => return Err(BenchError::InvalidArguments),
        };
        let strike: FixedBytes = decode(strike)?;
        match Strike::try_new(strike.get()) {
            Ok(strike) => *slot = optfix::StripLeg { kind, strike },
            Err(e) => return Ok(Err(e)),
        }
    }
    let inputs = || -> Result<(Years, MarketInputs), PricingError> {
        Ok((
            Years::try_new(t.get())?,
            MarketInputs {
                spot: Spot::try_new(s.get())?,
                vol: Volatility::try_new(sigma.get())?,
                rate: Rate::try_new(r.get())?,
                carry: Rate::try_new(q.get())?,
            },
        ))
    };
    Ok(inputs().map(|(t, m)| (t, m, n)))
}

const EMPTY_LEG: optfix::StripLeg = optfix::StripLeg {
    kind: OptionKind::Call,
    strike: Strike::MIN,
};

const EMPTY_PRICED: Priced = Priced {
    price: Price::MIN,
    greeks: Greeks {
        delta: Fixed::ZERO,
        gamma: Fixed::ZERO,
        vega: Fixed::ZERO,
        theta: Fixed::ZERO,
    },
};

#[inline(never)]
fn price_strip(args: &[u8]) -> ProgramResult {
    let mut legs = [EMPTY_LEG; MAX_LEGS];
    let mut prices = [Price::MIN; MAX_LEGS];
    let mut out = [0u8; 1 + 16 * MAX_LEGS];
    let result = decode_strip(args, &mut legs)?.and_then(|(t, m, n)| {
        let legs = legs.get(..n).ok_or(PricingError::DomainError)?;
        let prices = prices.get_mut(..n).ok_or(PricingError::DomainError)?;
        optfix::price_strip(t, &m, legs, prices)?;
        Ok(n)
    });
    match result {
        Ok(n) => {
            for (chunk, p) in out
                .get_mut(1..)
                .unwrap_or(&mut [])
                .as_chunks_mut::<16>()
                .0
                .iter_mut()
                .zip(prices.iter())
            {
                chunk.copy_from_slice(&p.get().to_bytes().0);
            }
            let len = n
                .checked_mul(16)
                .and_then(|b| b.checked_add(1))
                .ok_or(BenchError::InvalidArguments)?;
            set_return_data(out.get(..len).ok_or(BenchError::InvalidArguments)?);
        }
        Err(e) => set_return_data(&[error_status(e)]),
    }
    Ok(())
}

#[inline(never)]
fn greeks_strip(args: &[u8]) -> ProgramResult {
    let mut legs = [EMPTY_LEG; MAX_LEGS];
    let mut priced = [EMPTY_PRICED; MAX_LEGS];
    let result = decode_strip(args, &mut legs)?.and_then(|(t, m, n)| {
        let legs = legs.get(..n).ok_or(PricingError::DomainError)?;
        let out = priced.get_mut(..n).ok_or(PricingError::DomainError)?;
        optfix::price_and_greeks_strip(t, &m, legs, out)?;
        Ok(n)
    });
    match result {
        Ok(n) => {
            let mut out = [0u8; 33];
            let (_, tail) = out.split_at_mut(1);
            tail.copy_from_slice(&digest(priced.iter().take(n)));
            set_return_data(&out);
        }
        Err(e) => set_return_data(&[error_status(e)]),
    }
    Ok(())
}

/// The digest of `n` zero `Priced` values (instruction data: one byte `n`), the cost that
/// `PriceAndGreeksStrip` adds to pricing.
#[inline(never)]
fn digest_only(args: &[u8]) -> ProgramResult {
    let n = usize::from(*args.first().ok_or(BenchError::InvalidArguments)?);
    if n > MAX_LEGS {
        return Err(BenchError::InvalidArguments.into());
    }
    let priced = [EMPTY_PRICED; MAX_LEGS];
    let mut out = [0u8; 33];
    let (_, tail) = out.split_at_mut(1);
    tail.copy_from_slice(&digest(priced.iter().take(n)));
    set_return_data(&out);
    Ok(())
}
