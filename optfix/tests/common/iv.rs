//! Shared implied-volatility checks: the grid round trip (`iv_round_trip.rs`, and the table of
//! `docs/precision.md`), the mpmath fixture (`iv_fixture.rs`), and, with `bench-internals`, the
//! iteration distribution and the initial guess's error (`iv_internals.rs`).

use super::{bsm_inputs, bsm_sample, raw_of, uniform, Rng, ONE};
use optfix::{
    implied_vol, price, price_and_greeks, price_error_bound, Fixed, IvInputs, MarketInputs, Price,
    PricingError, Rate, Volatility, IV_ERR_PRICE_FACTOR, IV_ERR_REL, IV_VEGA_MIN_FACTOR,
};

pub fn f(x: Fixed) -> f64 {
    x.raw() as f64 / ONE as f64
}

pub fn iv_inputs(m: &MarketInputs) -> IvInputs {
    IvInputs {
        spot: m.spot,
        rate: m.rate,
        carry: m.carry,
    }
}

/// A grid point (`bsm_sample`) with the carry of a PLAN 4.7 set: `q` in `(0, 0.5]` for
/// `carry = 1`, `[−0.1, 0)` for `−1`, zero for `0`.
pub fn carry_sample(g: &mut Rng, carry: i32) -> (i128, i128, i128, i128, i128, i128, bool) {
    let mut p = bsm_sample(g);
    p.5 = match carry {
        1 => raw_of(0.5 * uniform(g)).max(1),
        -1 => -raw_of(0.1 * uniform(g)).clamp(1, Rate::MIN.get().raw().abs()),
        _ => 0,
    };
    p
}

/// The three carry sets: `(carry, label, seed offset)`.
pub const CARRY_SETS: [(i32, &str, u64); 3] = [(1, "q > 0", 1), (-1, "q < 0", 2), (0, "q = 0", 3)];

/// Vega against the conditioning threshold `IV_VEGA_MIN_FACTOR × E`: `Some(true)` above it by
/// `2^-8`, `Some(false)` below it by `2^-8`, `None` in between (where `implied_vol`, which
/// judges vega at its last evaluation, may go either way).
pub fn conditioned(vega: Fixed, e: Fixed) -> Option<bool> {
    let thr = f(IV_VEGA_MIN_FACTOR) * f(e);
    let v = f(vega);
    if v >= thr * (1.0 + 1.0 / 256.0) {
        Some(true)
    } else if v < thr * (1.0 - 1.0 / 256.0) {
        Some(false)
    } else {
        None
    }
}

/// `IV_ERR_PRICE_FACTOR × E / vega + IV_ERR_REL × σ`.
pub fn envelope(e: Fixed, vega: Fixed, sigma: Fixed) -> f64 {
    f(IV_ERR_PRICE_FACTOR) * f(e) / f(vega) + f(IV_ERR_REL) * f(sigma)
}

#[derive(Default)]
pub struct RoundTrip {
    pub points: usize,
    pub expired: usize,
    pub no_time_value: usize,
    pub answered: usize,
    pub worst_sigma: f64,
    pub worst_price: f64,
    pub worst_rel: f64,
    pub worst_at: String,
    pub ill: usize,
    pub band: usize,
    pub domain: usize,
}

/// `implied_vol(price(σ))` on `n` grid points of one carry set, asserting the contract of
/// `iv_round_trip.rs` at each.
pub fn round_trip(n: usize, carry: i32, seed: u64) -> RoundTrip {
    let mut g = Rng(seed);
    let mut st = RoundTrip::default();
    while st.points < n {
        let p = carry_sample(&mut g, carry);
        st.points += 1;
        let (opt, m) = bsm_inputs(p);
        let priced = price_and_greeks(&opt, &m).unwrap();
        let got = implied_vol(&opt, &iv_inputs(&m), priced.price);
        if p.2 == 0 {
            st.expired += 1;
            assert_eq!(got, Err(PricingError::DomainError), "T = 0 at {p:?}");
            continue;
        }
        let e = price_error_bound(&opt, &m).unwrap();
        let vega = priced.greeks.vega;
        let cond = conditioned(vega, e);
        match got {
            Err(PricingError::ArbitrageBound) => {
                // The out-of-the-money value rounded to zero: the target is a bound.
                st.no_time_value += 1;
                assert!(
                    cond != Some(true),
                    "ArbitrageBound at a well-conditioned point {p:?}"
                );
            }
            Ok(iv) => {
                assert!(cond != Some(false), "a value below the threshold at {p:?}");
                st.answered += 1;
                let sigma = m.vol.get();
                let err = f(iv.get().checked_sub(sigma).unwrap()).abs();
                let ratio = err / envelope(e, vega, sigma);
                assert!(
                    ratio <= 1.0,
                    "round trip at {ratio:.3} of its envelope at {p:?}: {iv:?}"
                );
                if ratio > st.worst_sigma {
                    st.worst_sigma = ratio;
                    st.worst_at = format!("{p:?}");
                }
                st.worst_rel = st.worst_rel.max(err / f(sigma));
                // price → implied vol → price
                let back = price(&opt, &MarketInputs { vol: iv, ..m }).unwrap();
                let perr = f(back.get().checked_sub(priced.price.get()).unwrap()).abs();
                let pallowed =
                    (f(IV_ERR_PRICE_FACTOR) + 1.0) * f(e) + f(IV_ERR_REL) * f(sigma) * f(vega);
                let pratio = perr / pallowed;
                assert!(
                    pratio <= 1.0,
                    "price round trip at {pratio:.3} of its envelope at {p:?}"
                );
                st.worst_price = st.worst_price.max(pratio);
            }
            Err(PricingError::IllConditioned) => {
                assert!(
                    cond != Some(true),
                    "IllConditioned above the threshold at {p:?}"
                );
                st.ill += 1;
                if cond.is_none() {
                    st.band += 1;
                }
            }
            Err(PricingError::DomainError) => st.domain += 1,
            Err(e) => panic!("{e:?} at {p:?} (conditioning {cond:?})"),
        }
    }
    st
}

/// The round trip over the three carry sets as a markdown table.
pub fn round_trip_table(n: usize, seed: u64) -> String {
    let mut out = String::from(
        "| Carry | Points | `T = 0` | No time value | Answered | Worst σ error / envelope | Worst price error / envelope | Worst relative σ error | `IllConditioned` | `NoConvergence` | `DomainError` |\n\
         | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n",
    );
    for (carry, name, offset) in CARRY_SETS {
        let st = round_trip(n, carry, seed + offset);
        out.push_str(&format!(
            "| `{name}` | {} | {} | {} | {} | {:.2e} | {:.2e} | {:.2e} | {} | 0 | {} |\n",
            st.points,
            st.expired,
            st.no_time_value,
            st.answered,
            st.worst_sigma,
            st.worst_price,
            st.worst_rel,
            st.ill,
            st.domain
        ));
    }
    out
}

#[derive(Default)]
pub struct FixtureStats {
    pub rows: usize,
    pub answered: usize,
    pub rejected: usize,
    pub worst: f64,
    pub worst_at: String,
}

/// `implied_vol` against `tests/data/iv_fixture.csv`, asserting the contract of
/// `iv_fixture.rs` at each row.
pub fn fixture_check() -> FixtureStats {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/iv_fixture.csv");
    let text = std::fs::read_to_string(path).unwrap();
    let mut st = FixtureStats::default();
    for line in text.lines() {
        let v: Vec<i128> = line.split(',').map(|x| x.parse().unwrap()).collect();
        st.rows += 1;
        let p = (v[0], v[1], v[2], v[3], v[4], v[5], v[6] == 0);
        let (opt, m) = bsm_inputs(p);
        let target = Price::try_new(Fixed::from_raw(v[7])).unwrap();
        let got = implied_vol(&opt, &iv_inputs(&m), target);
        let e = price_error_bound(&opt, &m).unwrap();
        let (sigma_star, vega_star) = (v[8], v[9]);
        let cond = if sigma_star < 0 {
            Some(false)
        } else {
            conditioned(Fixed::from_raw(vega_star), e)
        };
        match (got, cond) {
            (Ok(iv), Some(true) | None) => {
                st.answered += 1;
                let star = Fixed::from_raw(sigma_star);
                let clamped = star.clamp(Volatility::MIN.get(), Volatility::MAX.get());
                let err = f(iv.get().checked_sub(star).unwrap()).abs();
                let allowed = envelope(e, Fixed::from_raw(vega_star), star)
                    + f(clamped.checked_sub(star).unwrap()).abs();
                let ratio = err / allowed;
                assert!(ratio <= 1.0, "{ratio:.3} of the envelope at {line}: {iv:?}");
                if ratio > st.worst {
                    st.worst = ratio;
                    st.worst_at = line.to_string();
                }
            }
            (Ok(iv), Some(false)) => panic!("a value ({iv:?}) at an ill-conditioned row {line}"),
            (Err(PricingError::DomainError), Some(true)) => {
                assert!(
                    Volatility::try_new(Fixed::from_raw(sigma_star)).is_err(),
                    "DomainError at an in-range root, {line}"
                );
                st.rejected += 1;
            }
            (
                Err(
                    PricingError::IllConditioned
                    | PricingError::ArbitrageBound
                    | PricingError::DomainError,
                ),
                Some(false) | None,
            ) => st.rejected += 1,
            (other, c) => panic!("{other:?} with conditioning {c:?} at {line}"),
        }
    }
    st
}

/// Steps taken by `implied_vol` over `n` grid points per carry set, `[outcome][steps]` with
/// outcome 0 `Ok`, 1 `IllConditioned`, 2 any other error after the first step; asserts the cap.
#[cfg(feature = "bench-internals")]
pub fn iteration_histogram(n: usize, seed: u64) -> [[usize; 4]; 3] {
    use optfix::iv_bench::with_cap;
    use optfix::IV_MAX_ITERATIONS;
    let mut hist = [[0usize; 4]; 3];
    for (carry, _, offset) in CARRY_SETS {
        let mut g = Rng(seed + offset);
        for _ in 0..n {
            let p = carry_sample(&mut g, carry);
            let (opt, m) = bsm_inputs(p);
            let target = price(&opt, &m).unwrap();
            let im = iv_inputs(&m);
            let (got, steps) = with_cap(&opt, &im, target, IV_MAX_ITERATIONS);
            assert!(steps <= IV_MAX_ITERATIONS, "{steps} steps at {p:?}");
            assert_eq!(got, implied_vol(&opt, &im, target), "hook differs at {p:?}");
            if steps == 0 {
                continue;
            }
            let outcome = match got {
                Ok(_) => 0,
                Err(PricingError::IllConditioned) => 1,
                Err(_) => 2,
            };
            hist[outcome][steps as usize] += 1;
        }
    }
    hist
}

/// The initial guess's largest `|v0 − ln σ|` wherever `implied_vol` answers, per table (near,
/// deep), and how many points used each; asserts `bound`.
#[cfg(feature = "bench-internals")]
pub fn guess_errors(n: usize, seed: u64, bound: f64) -> ([f64; 2], [usize; 2]) {
    use optfix::iv_bench::initial_guess;
    let mut worst = [0f64; 2];
    let mut count = [0usize; 2];
    for (carry, _, offset) in CARRY_SETS {
        let mut g = Rng(seed + offset);
        for _ in 0..n {
            let p = carry_sample(&mut g, carry);
            let (opt, m) = bsm_inputs(p);
            let target = price(&opt, &m).unwrap();
            let im = iv_inputs(&m);
            if implied_vol(&opt, &im, target).is_err() {
                continue;
            }
            let (v0, deep) = initial_guess(&opt, &im, target).unwrap();
            let err = (f(v0) - f(m.vol.get()).ln()).abs();
            assert!(err <= bound, "guess off by {err:.4} at {p:?}");
            worst[usize::from(deep)] = worst[usize::from(deep)].max(err);
            count[usize::from(deep)] += 1;
        }
    }
    (worst, count)
}
