//! Input decoding and exact oracles shared by the fuzz targets (`docs/PLAN-improvements.md`,
//! milestone 16).
//!
//! Inputs are fixed little-endian fields read by [`Bytes`] (missing bytes read as zero), not
//! `arbitrary` structures, so a corpus entry can be written by hand: the failing control's
//! entry is one.

#[path = "../../optfix/tests/common/contract.rs"]
pub mod contract;

use num_bigint::BigInt;
use optfix::{
    EuropeanOption, Fixed, MarketInputs, OptionKind, Rate, Spot, Strike, Volatility, Years,
};

/// `2^64`, the raw value of `1`.
pub const ONE: i128 = 1 << 64;

/// A cursor over the fuzzer's bytes; reads past the end return zeros.
pub struct Bytes<'a>(pub &'a [u8]);

impl Bytes<'_> {
    fn take<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0u8; N];
        let n = self.0.len().min(N);
        out[..n].copy_from_slice(&self.0[..n]);
        self.0 = &self.0[n..];
        out
    }
    pub fn u8(&mut self) -> u8 {
        self.take::<1>()[0]
    }
    pub fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take())
    }
    pub fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take())
    }
    pub fn u128(&mut self) -> u128 {
        u128::from_le_bytes(self.take())
    }
    pub fn i128(&mut self) -> i128 {
        i128::from_le_bytes(self.take())
    }
}

/// `x` as a fraction of the `u64` range, in `[0, 1]`.
fn unit(x: u64) -> f64 {
    x as f64 / u64::MAX as f64
}

fn raw_of(v: f64) -> i128 {
    (v * ONE as f64) as i128
}

/// A point of the supported domain, raw `(S, K, T, σ, r, q)`, with extra weight on the edges
/// the samplers rarely draw: `S = K`, `T = 0`, a few raw units of `T`, the range ends of `σ`,
/// `r` and `q`, and `q = 0` or `q = r`. Mirrors `common::bsm_sample` otherwise.
pub fn bsm_point(b: &mut Bytes) -> (EuropeanOption, MarketInputs) {
    let edges = b.u32();
    let s = 10f64.powf(-6.0 + 15.0 * unit(b.u64()));
    let m = 10f64.powf(0.05f64.log10() + 400f64.log10() * unit(b.u64()));
    let t_bits = b.u64();
    let t = match edges & 7 {
        0 => 0.0,
        1 => (t_bits % 4096) as f64 / ONE as f64,
        2 => 10f64.powf(-12.0 + 6.0 * unit(t_bits)),
        3 => 10f64.powf(-6.0 + 7.5 * unit(t_bits)),
        _ => 30.0 * unit(t_bits),
    };
    let sigma = 10f64.powf(0.005f64.log10() + 3.0 * unit(b.u64()));
    let rate = -0.1 + 0.6 * unit(b.u64());
    let carry = -0.1 + 0.6 * unit(b.u64());
    let mut raw = [
        raw_of(s),
        raw_of(s / m),
        raw_of(t),
        raw_of(sigma),
        raw_of(rate),
        raw_of(carry),
    ];
    raw[0] = raw[0].clamp(Spot::MIN.get().raw(), Spot::MAX.get().raw());
    raw[2] = raw[2].clamp(0, Years::MAX.get().raw());
    raw[1] = raw[1]
        .clamp(raw[0] / 20 + 1, raw[0] * 20)
        .clamp(Strike::MIN.get().raw(), Strike::MAX.get().raw());
    if edges & 8 != 0 {
        raw[1] = raw[0];
    }
    raw[3] = match (edges >> 4) & 7 {
        0 => Volatility::MIN.get().raw(),
        1 => Volatility::MAX.get().raw(),
        _ => raw[3].clamp(Volatility::MIN.get().raw(), Volatility::MAX.get().raw()),
    };
    let (rmin, rmax) = (Rate::MIN.get().raw(), Rate::MAX.get().raw());
    raw[4] = match (edges >> 7) & 7 {
        0 => rmin,
        1 => rmax,
        _ => raw[4].clamp(rmin, rmax),
    };
    raw[5] = match (edges >> 10) & 7 {
        0 => rmin,
        1 => rmax,
        2 => 0,
        3 => raw[4],
        _ => raw[5].clamp(rmin, rmax),
    };
    let kind = if edges & (1 << 13) == 0 {
        OptionKind::Call
    } else {
        OptionKind::Put
    };
    inputs(kind, raw).expect("bsm_point stays in the domain")
}

/// The optfix inputs for raw `(S, K, T, σ, r, q)`, or `None` if a quantity is out of range.
pub fn inputs(kind: OptionKind, raw: [i128; 6]) -> Option<(EuropeanOption, MarketInputs)> {
    Some((
        EuropeanOption {
            kind,
            strike: Strike::try_new(Fixed::from_raw(raw[1])).ok()?,
            expiry: Years::try_new(Fixed::from_raw(raw[2])).ok()?,
        },
        MarketInputs {
            spot: Spot::try_new(Fixed::from_raw(raw[0])).ok()?,
            vol: Volatility::try_new(Fixed::from_raw(raw[3])).ok()?,
            rate: Rate::try_new(Fixed::from_raw(raw[4])).ok()?,
            carry: Rate::try_new(Fixed::from_raw(raw[5])).ok()?,
        },
    ))
}

/// `floor(n / d)` and `ceil(n / d)` for `d > 0`.
pub fn floor_ceil(n: &BigInt, d: &BigInt) -> (BigInt, BigInt) {
    let q = n / d;
    let r = n - &q * d;
    // BigInt division truncates toward zero.
    let floor = if r < BigInt::from(0) {
        &q - 1
    } else {
        q.clone()
    };
    let ceil = if r > BigInt::from(0) { &q + 1 } else { q };
    (floor, ceil)
}

/// `v` as an `i128` if it fits.
pub fn fits_i128(v: &BigInt) -> Option<i128> {
    i128::try_from(v).ok()
}
