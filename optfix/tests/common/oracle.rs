//! The OptionStratLib 0.23 oracle (PLAN 4.7): Garman-Kohlhagen with its foreign rate set to
//! `q`, and plain Black-Scholes for `q = 0`. Inputs are exact decimals given to both libraries
//! (optfix rounds them to Q64.64, an input change below `2^-64` relative). Oracle greeks are
//! rescaled to optfix units: theta per year (OptionStratLib's is per day) and vega per 1.00 of
//! volatility (OptionStratLib's is per 1%).

use super::{ratio, to_ref, uniform, Rng};
use optfix::{
    price_and_greeks, EuropeanOption, Fixed, MarketInputs, OptionKind, Rate, Rounding, Spot,
    Strike, Volatility, Years,
};
use optionstratlib::model::option::ExoticParams;
use optionstratlib::model::{ExpirationDate, Positive};
use optionstratlib::{OptionStyle, OptionType, Options, Side};
use rust_decimal::Decimal;

/// A decimal `m / 10^scale`.
#[derive(Clone, Copy, Debug)]
pub struct Dec {
    pub m: i128,
    pub scale: u32,
}

impl Dec {
    pub fn fixed(self) -> Fixed {
        Fixed::from_ratio(self.m, 10i128.pow(self.scale), Rounding::Down).unwrap()
    }
    pub fn decimal(self) -> Decimal {
        Decimal::from_i128_with_scale(self.m, self.scale)
    }
    pub fn f64(self) -> f64 {
        self.m as f64 / 10f64.powi(self.scale as i32)
    }
}

/// `v` to 12 significant digits as a decimal.
pub fn dec(v: f64) -> Dec {
    let e = v.abs().log10().floor() as i32;
    let scale = (11 - e).clamp(0, 24) as u32;
    Dec {
        m: (v * 10f64.powi(scale as i32)).round() as i128,
        scale,
    }
}

pub fn d2f(d: Decimal) -> f64 {
    d.mantissa() as f64 / 10f64.powi(d.scale() as i32)
}

/// One oracle point: S, K, T (≥ one day: OptionStratLib converts days to years in f64), σ, r, q.
pub fn oracle_sample(g: &mut Rng, carry: i32) -> (Dec, Dec, Dec, Dec, Dec, Dec, bool) {
    let s = 10f64.powf(-6.0 + 15.0 * uniform(g));
    let m = 10f64.powf((0.05f64).log10() + (400f64).log10() * uniform(g));
    let k = (s / m).clamp(1.0001e-6, 0.9999e9);
    let t = 10f64.powf((1.0f64 / 365.0).log10() + (30.0f64 * 365.0).log10() * uniform(g));
    let sigma = 10f64.powf((0.005f64).log10() + 3.0 * uniform(g)).min(5.0);
    let r = -0.1 + 0.6 * uniform(g);
    let q = match carry {
        1 => 0.5 * uniform(g).max(1e-6),
        -1 => -0.1 * uniform(g).max(1e-6),
        _ => 0.0,
    };
    let zero = Dec { m: 0, scale: 0 };
    (
        dec(s),
        dec(k),
        dec(t.min(30.0)),
        dec(sigma),
        dec(r),
        if q == 0.0 { zero } else { dec(q) },
        g.next() & 1 == 0,
    )
}

pub fn oracle_options(p: &(Dec, Dec, Dec, Dec, Dec, Dec, bool)) -> Options {
    let (s, k, t, sigma, r, q, call) = *p;
    let pos = |d: Decimal| Positive::new_decimal(d).unwrap();
    Options::new(
        OptionType::European,
        Side::Long,
        "X".to_string(),
        pos(k.decimal()),
        ExpirationDate::Days(pos(t.decimal() * Decimal::from(365))),
        pos(sigma.decimal()),
        pos(Decimal::ONE),
        pos(s.decimal()),
        r.decimal(),
        if call {
            OptionStyle::Call
        } else {
            OptionStyle::Put
        },
        Positive::ZERO,
        Some(ExoticParams {
            foreign_rate: Some(q.decimal()),
            ..Default::default()
        }),
    )
}

pub fn ours(p: &(Dec, Dec, Dec, Dec, Dec, Dec, bool)) -> [f64; 5] {
    let (s, k, t, sigma, r, q, call) = *p;
    let opt = EuropeanOption {
        kind: if call {
            OptionKind::Call
        } else {
            OptionKind::Put
        },
        strike: Strike::try_new(k.fixed()).unwrap(),
        expiry: Years::try_new(t.fixed()).unwrap(),
    };
    let m = MarketInputs {
        spot: Spot::try_new(s.fixed()).unwrap(),
        vol: Volatility::try_new(sigma.fixed()).unwrap(),
        rate: Rate::try_new(r.fixed()).unwrap(),
        carry: Rate::try_new(q.fixed()).unwrap(),
    };
    let v = price_and_greeks(&opt, &m).unwrap();
    let f = |x: Fixed| ratio(&to_ref(x.raw()));
    [
        f(v.price.get()),
        f(v.greeks.delta),
        f(v.greeks.gamma),
        f(v.greeks.vega),
        f(v.greeks.theta),
    ]
}

/// Oracle tolerance for the price: the decided budget, `10^-9 × max(S, K)` (PLAN question 1).
pub const ORACLE_PRICE_REL: f64 = 1e-9;
/// Oracle tolerance for delta, absolute (PLAN question 1).
pub const ORACLE_DELTA: f64 = 1e-9;
/// Oracle tolerance for gamma, vega and theta: `10^-9 ×` the envelope scale of
/// `optfix::bsm` plus `10^-18` absolute (about 18 ulps, for values a few ulps wide).
pub const ORACLE_GREEK_REL: f64 = 1e-9;
/// Absolute part of the gamma, vega and theta oracle tolerance.
pub const ORACLE_GREEK_ABS: f64 = 1e-18;

/// Largest differences from the oracle over `n` points of one carry set (`1`: `q > 0`, `−1`:
/// `q < 0`, `0`: `q = 0`), each divided by its tolerance (so `<= 1` passes), with the raw
/// normalized differences: price / max(S, K), delta, and gamma, vega, theta beyond the
/// absolute tolerance per scale; the last entry is `q = 0` against plain `black_scholes`.
pub struct OracleStats {
    pub ratio: [f64; 6],
    pub diff: [f64; 6],
    pub at: Vec<String>,
    pub skipped_n: usize,
}

pub fn oracle_stats(n: usize, carry: i32) -> OracleStats {
    use optionstratlib::greeks::{delta_gk, gamma_gk, theta_gk, vega_gk};
    use optionstratlib::pricing::black_scholes_model::black_scholes;
    use optionstratlib::pricing::garman_kohlhagen::garman_kohlhagen;
    let mut g = Rng(0x0F7F_1C5E_05C1_0000 ^ carry as u64);
    let mut st = OracleStats {
        ratio: [0.0; 6],
        diff: [0.0; 6],
        at: vec![String::new(); 6],
        skipped_n: 0,
    };
    for _ in 0..n {
        let p = oracle_sample(&mut g, carry);
        let o = oracle_options(&p);
        let ours = ours(&p);
        let (s, k, t, sig, r, q) = (
            p.0.f64(),
            p.1.f64(),
            p.2.f64(),
            p.3.f64(),
            p.4.f64(),
            p.5.f64(),
        );
        let dq = (-q * t).exp();
        let sst = sig * t.sqrt();
        let scale = [
            s.max(k),
            1.0,
            dq / (s * sst),
            s * dq * t.sqrt(),
            s * dq * sig / t.sqrt() + r.abs() * k * (-r * t).exp() + q.abs() * s * dq,
        ];
        let oracle = [
            d2f(garman_kohlhagen(&o).unwrap()),
            d2f(delta_gk(&o).unwrap()),
            d2f(gamma_gk(&o).unwrap()),
            d2f(vega_gk(&o).unwrap()) * 100.0,
            d2f(theta_gk(&o).unwrap()) * 365.0,
        ];
        // OptionStratLib's n(x) is zero beyond |x| ≈ 4.84 (PLAN 4.7): its gamma, vega and
        // theta are no oracle there; bsm_props.rs checks those against the 256-bit reference.
        let d1 = ((s / k).ln() + (r - q + sig * sig / 2.0) * t) / sst;
        let n_ok = d1.abs() <= 4.8;
        if !n_ok {
            st.skipped_n += 1;
        }
        for i in 0..5 {
            if i >= 2 && !n_ok {
                continue;
            }
            let e = (ours[i] - oracle[i]).abs();
            let (diff, ratio) = match i {
                0 => (e / scale[0], e / (ORACLE_PRICE_REL * scale[0])),
                1 => (e, e / ORACLE_DELTA),
                _ => (
                    (e - ORACLE_GREEK_ABS).max(0.0) / scale[i],
                    e / (ORACLE_GREEK_REL * scale[i] + ORACLE_GREEK_ABS),
                ),
            };
            if ratio > st.ratio[i] {
                st.ratio[i] = ratio;
                st.at[i] = format!("{:?}", (s, k, t, sig, r, q, p.6, ours[i], oracle[i]));
            }
            st.diff[i] = st.diff[i].max(diff);
        }
        if carry == 0 {
            let bs = d2f(black_scholes(&o).unwrap());
            let e = (ours[0] - bs).abs();
            st.diff[5] = st.diff[5].max(e / scale[0]);
            st.ratio[5] = st.ratio[5].max(e / (ORACLE_PRICE_REL * scale[0]));
        }
    }
    st
}
