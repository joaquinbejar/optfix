//! Prints the frozen 0.1.0 fixture (see `Cargo.toml`). Each line is
//! `op,inputs...,=,outputs...`: raw `i128` values (`Fixed::raw`), kinds and roundings as
//! `0`/`1`, an error as `E<discriminant>`. Inputs are deterministic (splitmix64, fixed seed).

use optfix::{
    implied_vol, price, price_and_greeks, EuropeanOption, Fixed, IvInputs, MarketInputs,
    OptionKind, Price, PricingError, Rate, Rounding, Spot, Strike, Volatility, Years,
};

const SEED: u64 = 0x0F7F_1C5E_ED00_0101;
const ONE: f64 = 18_446_744_073_709_551_616.0;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)` at `2^-53` resolution; IEEE arithmetic only (no libm), so the
    /// inputs are the same on every platform.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A signed raw value with a log-uniform magnitude over the whole `i128` range.
    fn raw(&mut self) -> i128 {
        let mag = (u128::from(self.next()) << 64 | u128::from(self.next())) >> (self.next() % 128);
        let v = (mag >> 1) as i128;
        if self.next() & 1 == 0 { v } else { -v }
    }

    /// `lo × (hi/lo)^u` from integer powers: avoids `powf`, whose last bits are platform
    /// dependent. Exact enough for input sampling.
    fn log_between(&mut self, lo: f64, hi: f64) -> f64 {
        let steps = 4096u32;
        let k = (self.next() % u64::from(steps)) as i32;
        let ratio = hi / lo;
        // ratio^(k/steps) by repeated square roots: sqrt is correctly rounded in IEEE 754.
        let mut r = ratio;
        let mut acc = 1.0;
        let mut bit = steps >> 1;
        let mut k = k;
        while bit > 0 {
            r = r.sqrt();
            if k as u32 >= bit {
                acc *= r;
                k -= bit as i32;
            }
            bit >>= 1;
        }
        lo * acc
    }
}

fn fx(v: f64) -> Fixed {
    Fixed::from_raw((v * ONE) as i128)
}

fn res(r: Result<Fixed, PricingError>) -> String {
    match r {
        Ok(v) => v.raw().to_string(),
        Err(e) => format!("E{}", e as u8),
    }
}

fn rounding(bit: u64) -> (Rounding, u8) {
    if bit & 1 == 0 { (Rounding::Down, 0) } else { (Rounding::Up, 1) }
}

const SPECIAL: [i128; 12] = [
    0,
    1,
    -1,
    1 << 64,
    -(1 << 64),
    i128::MAX,
    i128::MIN,
    i128::MIN + 1,
    (1 << 96) - 1,
    1 << 96,
    u64::MAX as i128,
    56_022_770_974_786_139_918_731_938_227,
];

fn primitives(rng: &mut Rng) {
    let pick = |rng: &mut Rng| -> i128 {
        if rng.next() % 8 == 0 {
            SPECIAL[(rng.next() % SPECIAL.len() as u64) as usize]
        } else {
            rng.raw()
        }
    };
    for _ in 0..1500 {
        let (a, b) = (pick(rng), pick(rng));
        let (fa, fb) = (Fixed::from_raw(a), Fixed::from_raw(b));
        println!("div_down,{a},{b},=,{}", res(fa.div_down(fb)));
        println!("div_up,{a},{b},=,{}", res(fa.div_up(fb)));
        let (r, rb) = rounding(rng.next());
        println!("from_ratio,{a},{b},{rb},=,{}", res(Fixed::from_ratio(a, b, r)));
        let decimals = (rng.next() % 41) as u8;
        println!(
            "from_scaled,{a},{decimals},{rb},=,{}",
            res(Fixed::from_scaled(a, decimals, r))
        );
        println!("sqrt_down,{a},=,{}", res(fa.sqrt_down()));
        println!("sqrt_up,{a},=,{}", res(fa.sqrt_up()));
    }
}

/// In-domain pricing inputs (docs/PLAN.md supported domain), a few with `T = 0` and a few
/// with `S/K` outside `[0.05, 20]`.
fn inputs(rng: &mut Rng) -> (u8, [Fixed; 6]) {
    let s = rng.log_between(1e-6, 1e9);
    let m = match rng.next() % 10 {
        0 => 1.0 + (rng.unit() - 0.5) * 0.0078,
        1 => rng.log_between(0.01, 40.0),
        _ => rng.log_between(0.05, 20.0),
    };
    let k = (s / m).clamp(1.01e-6, 0.99e9);
    let t = if rng.next() % 20 == 0 { 0.0 } else { 30.0 * rng.unit() };
    let sigma = rng.log_between(0.005, 5.0);
    let r = -0.1 + 0.6 * rng.unit();
    let q = match rng.next() % 4 {
        0 => 0.0,
        _ => -0.1 + 0.6 * rng.unit(),
    };
    ((rng.next() & 1) as u8, [fx(s), fx(k), fx(t), fx(sigma), fx(r), fx(q)])
}

fn build(kind: u8, v: &[Fixed; 6]) -> Option<(EuropeanOption, MarketInputs)> {
    Some((
        EuropeanOption {
            kind: if kind == 0 { OptionKind::Call } else { OptionKind::Put },
            strike: Strike::try_new(v[1]).ok()?,
            expiry: Years::try_new(v[2]).ok()?,
        },
        MarketInputs {
            spot: Spot::try_new(v[0]).ok()?,
            vol: Volatility::try_new(v[3]).ok()?,
            rate: Rate::try_new(v[4]).ok()?,
            carry: Rate::try_new(v[5]).ok()?,
        },
    ))
}

fn pricing(rng: &mut Rng) {
    let mut n = 0;
    while n < 2000 {
        let (kind, v) = inputs(rng);
        let Some((o, m)) = build(kind, &v) else { continue };
        n += 1;
        let ins = v.iter().map(|x| x.raw().to_string()).collect::<Vec<_>>().join(",");
        println!("price,{kind},{ins},=,{}", res(price(&o, &m).map(|p| p.get())));
        match price_and_greeks(&o, &m) {
            Ok(p) => println!(
                "price_and_greeks,{kind},{ins},=,{},{},{},{},{}",
                p.price.get().raw(),
                p.greeks.delta.raw(),
                p.greeks.gamma.raw(),
                p.greeks.vega.raw(),
                p.greeks.theta.raw()
            ),
            Err(e) => println!("price_and_greeks,{kind},{ins},=,E{}", e as u8),
        }
        // Implied volatility of the price itself, and of the price moved by a few ulps to
        // a few percent (some outside the arbitrage bounds or ill-conditioned).
        let Ok(p) = price(&o, &m) else { continue };
        let iv = IvInputs { spot: m.spot, rate: m.rate, carry: m.carry };
        let bump = match rng.next() % 4 {
            0 => 0,
            1 => (rng.next() % 64) as i128 - 32,
            _ => (p.get().raw() >> (4 + rng.next() % 40)) * if rng.next() & 1 == 0 { 1 } else { -1 },
        };
        let Some(target) = p.get().raw().checked_add(bump) else { continue };
        let Ok(tp) = Price::try_new(Fixed::from_raw(target)) else { continue };
        let ins = [v[0], v[1], v[2], v[4], v[5]]
            .iter()
            .map(|x| x.raw().to_string())
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "implied_vol,{kind},{ins},{target},=,{}",
            res(implied_vol(&o, &iv, tp).map(|s| s.get()))
        );
    }
}

fn main() {
    let mut rng = Rng(SEED);
    primitives(&mut rng);
    pricing(&mut rng);
}
