//! Raw SVI reference in Q.P (`docs/PLAN-phase2.md`, milestone 22) and the sampler shared by
//! `svi_props.rs` and `precision.rs`. Nothing here calls the crate's SVI code.

use num_bigint::BigInt;
use optfix::{Fixed, Rate, Spot, Strike, SviMarket, SviParams, Years};

use super::{div, ln_ref, mul, one, to_ref, uniform, Rng, ONE, P};

/// `sqrt(v)` in Q.P for `v >= 0`, truncated.
pub fn sqrt_ref(v: &BigInt) -> BigInt {
    assert!(v >= &BigInt::from(0), "sqrt_ref of a negative value");
    (v << P).sqrt()
}

/// Raw slice `(a, b, ρ, m, σ)`.
pub type RawSlice = [i128; 5];

/// `w(k)` in Q.P for a raw slice and a Q.P argument `k`.
pub fn w_ref(p: &RawSlice, k: &BigInt) -> BigInt {
    let [a, b, rho, m, sigma] = p.map(to_ref);
    let x = k - m;
    let s = sqrt_ref(&(mul(&x, &x) + mul(&sigma, &sigma)));
    a + mul(&b, &(mul(&rho, &x) + s))
}

/// The true minimum `a + b σ sqrt(1 − ρ²)` in Q.P.
pub fn min_ref(p: &RawSlice) -> BigInt {
    let [a, b, rho, _, sigma] = p.map(to_ref);
    a + mul(&mul(&b, &sigma), &sqrt_ref(&(one() - mul(&rho, &rho))))
}

/// Durrleman's `g(k)` in Q.P (`docs/PLAN-phase2.md`, milestone 23), or `None` when
/// `w(k) <= 0`:
/// `(1 − k w' / (2 w))² − (w'² / 4) (1 / w + 1 / 4) + w'' / 2`, `w' = b (ρ + x / s)`,
/// `w'' = b σ² / s³`, `s = sqrt(x² + σ²)`, `x = k − m`.
pub fn g_ref(p: &RawSlice, k: &BigInt) -> Option<BigInt> {
    let w = w_ref(p, k);
    if w <= BigInt::from(0) {
        return None;
    }
    let [_, b, rho, m, sigma] = p.map(to_ref);
    let x = k - m;
    let s = sqrt_ref(&(mul(&x, &x) + mul(&sigma, &sigma)));
    let w1 = mul(&b, &(rho + div(&x, &s)));
    let s3 = mul(&mul(&s, &s), &s);
    let w2 = div(&mul(&b, &mul(&sigma, &sigma)), &s3);
    let c = one() - div(&mul(k, &w1), &(&w << 1));
    let quarter = one() >> 2;
    let f = mul(&(mul(&w1, &w1) >> 2), &(div(&one(), &w) + quarter));
    Some(mul(&c, &c) - f + (w2 >> 1))
}

/// `k* = ln(K / S) − (r − q) T` in Q.P at raw inputs.
pub fn k_star(s: i128, k: i128, t: i128, r: i128, q: i128) -> BigInt {
    ln_ref(&to_ref(k)) - ln_ref(&to_ref(s)) - mul(&(to_ref(r) - to_ref(q)), &to_ref(t))
}

/// `σ* = sqrt(w(k*) / T)` in Q.P, for `T > 0` and `w(k*) >= 0`.
pub fn vol_ref(w: &BigInt, t: i128) -> BigInt {
    sqrt_ref(&div(w, &to_ref(t)))
}

fn log_uniform(g: &mut Rng, lo: f64, hi: f64) -> f64 {
    (lo.ln() + (hi.ln() - lo.ln()) * uniform(g)).exp()
}

fn raw(v: f64) -> i128 {
    (v * ONE as f64) as i128
}

fn sign(g: &mut Rng) -> i128 {
    if g.next() & 1 == 0 {
        1
    } else {
        -1
    }
}

/// `a` such that the true minimum is `margin` above 0 (to within the Q.P truncation): the
/// sampler's way of keeping slices inside the domain.
fn a_for(b: i128, rho: i128, sigma: i128, margin: i128) -> i128 {
    let part = min_ref(&[0, b, rho, 0, sigma]);
    let part_raw = i128::try_from(&(part >> (P - 64))).unwrap();
    (margin - part_raw).clamp(-1000 * ONE, 1000 * ONE)
}

/// Strata of [`svi_sample`].
pub const STRATA: [&str; 4] = [
    "uniform over the domain",
    "near k = m, σ <= 10^-3",
    "wings, 1 − |ρ| <= 10^-2, cancelling",
    "around max(|x|, σ) = 1 and the domain edges",
];

/// A random slice and an exact argument `k` (raw) from stratum `st` of [`STRATA`]. The slice
/// may still be refused by `try_new` (a minimum within the bound's slack of 0).
pub fn svi_sample(g: &mut Rng, st: usize) -> (RawSlice, i128) {
    let mut b = raw(log_uniform(g, 1e-4, 10.0));
    if g.next().is_multiple_of(64) {
        b = 0;
    }
    let mut sigma = raw(log_uniform(g, 1e-6, 10.0));
    let mut m = raw(-10.0 + 20.0 * uniform(g));
    let mut rho = g.range(-ONE + 1, ONE);
    let mut k = g.range(-32 * ONE, 32 * ONE + 1);
    match st {
        1 => {
            sigma = raw(log_uniform(g, 1e-6, 1e-3));
            k = m + sign(g) * raw(log_uniform(g, 1e-12, 1e-2));
        }
        2 => {
            // 1 − |ρ| from one ulp to 10^-2, with the sign that cancels against x.
            let gap = raw(log_uniform(g, 1.0 / ONE as f64, 1e-2)).max(1);
            let x_sign = if k >= m { 1 } else { -1 };
            rho = -x_sign * (ONE - gap);
        }
        3 => match g.next() % 6 {
            0 => {
                let d = sign(g) * raw(log_uniform(g, 1e-18, 1e-2));
                k = (m + sign(g) * (ONE + d)).clamp(-32 * ONE, 32 * ONE);
            }
            1 => sigma = (ONE + sign(g) * raw(log_uniform(g, 1e-18, 1e-2))).max(1),
            2 => {
                b = 10 * ONE;
                sigma = 10 * ONE;
            }
            3 => sigma = 18_446_744_073_709,
            4 => m = sign(g) * 10 * ONE,
            _ => k = sign(g) * 32 * ONE,
        },
        _ => {}
    }
    let margin = if g.next().is_multiple_of(4) {
        g.range(0, 1 << 20)
    } else {
        raw(log_uniform(g, 1e-6, 100.0))
    };
    let a = a_for(b, rho, sigma, margin);
    ([a, b, rho, m, sigma], k)
}

/// The crate's slice for a raw one.
pub fn params(p: &RawSlice) -> Option<SviParams> {
    let [a, b, rho, m, sigma] = p.map(Fixed::from_raw);
    SviParams::try_new(a, b, rho, m, sigma).ok()
}

/// A market point for a slice: raw `(S, K, T, r, q)` with `S/K` in `[0.05, 20]`, `|k*| <~ 3 +
/// |(r − q) T|`, and `T` log-uniform in `[10^-3, 30]` (one point in eight below, to `10^-8`).
pub fn market_sample(g: &mut Rng) -> (i128, i128, i128, i128, i128) {
    let s = raw(log_uniform(g, 1e-6, 1e9)).clamp(Spot::MIN.get().raw(), Spot::MAX.get().raw());
    let t = if g.next().is_multiple_of(8) {
        log_uniform(g, 1e-8, 1e-3)
    } else {
        log_uniform(g, 1e-3, 30.0)
    };
    let t = raw(t).clamp(1, Years::MAX.get().raw());
    let r = raw(-0.1 + 0.6 * uniform(g)).clamp(Rate::MIN.get().raw(), Rate::MAX.get().raw());
    let q = if g.next().is_multiple_of(3) {
        0
    } else {
        raw(-0.1 + 0.6 * uniform(g)).clamp(Rate::MIN.get().raw(), Rate::MAX.get().raw())
    };
    let lk = -2.99 + 5.98 * uniform(g);
    let k = raw(s as f64 / ONE as f64 * lk.exp())
        .clamp(s / 20 + 1, s.saturating_mul(20))
        .clamp(Strike::MIN.get().raw(), Strike::MAX.get().raw());
    (s, k, t, r, q)
}

/// A slice whose minimum variance is a volatility `v0` log-uniform in `[0.01, 2]` at `T`, so
/// that most strikes near the money have a volatility in `[0.005, 5]`.
pub fn market_slice(g: &mut Rng, t: i128) -> RawSlice {
    let st = (g.next() % 4) as usize;
    let (mut p, _) = svi_sample(g, st);
    let v0 = log_uniform(g, 0.01, 2.0);
    let level = raw(v0 * v0 * (t as f64 / ONE as f64)).max(1);
    // Keep b's contribution comparable to the level at short expiries.
    let tf = (t as f64 / ONE as f64).min(1.0);
    p[1] = raw((p[1] as f64 / ONE as f64) * tf).max(if p[1] == 0 { 0 } else { 1 });
    p[0] = a_for(p[1], p[2], p[4], level);
    p
}

/// The crate's market for raw `(S, K, T, r, q)`: `(T, market, K)`.
pub fn market(s: i128, k: i128, t: i128, r: i128, q: i128) -> (Years, SviMarket, Strike) {
    (
        Years::try_new(Fixed::from_raw(t)).unwrap(),
        SviMarket {
            spot: Spot::try_new(Fixed::from_raw(s)).unwrap(),
            rate: Rate::try_new(Fixed::from_raw(r)).unwrap(),
            carry: Rate::try_new(Fixed::from_raw(q)).unwrap(),
        },
        Strike::try_new(Fixed::from_raw(k)).unwrap(),
    )
}

/// A row of `tests/data/svi_g.csv` (`tools/svi_fixture/gen_arb_fixture.py`, mpmath at 80
/// digits): the group tag, the raw slice and `k`, then `nint(w × 2^96)` and `nint(g × 2^96)`
/// (`None` when `w <= 0`).
pub struct GRow {
    pub tag: String,
    pub p: RawSlice,
    pub k: i128,
    pub w96: BigInt,
    pub g96: Option<BigInt>,
}

/// The rows of `tests/data/svi_g.csv`.
pub fn g_fixture() -> Vec<GRow> {
    include_str!("../data/svi_g.csv")
        .lines()
        .map(|line| {
            let f: Vec<&str> = line.split(',').collect();
            let int = |i: usize| -> i128 { f[i].parse().unwrap() };
            GRow {
                tag: f[0].to_string(),
                p: [int(1), int(2), int(3), int(4), int(5)],
                k: int(6),
                w96: f[7].parse().unwrap(),
                g96: (f[8] != "-").then(|| f[8].parse().unwrap()),
            }
        })
        .collect()
}
