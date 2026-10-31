//! Precision report: the maximum error of every function over a deterministic sample per
//! domain, and the measured error of the OptionStratLib oracle (PLAN 4.7). `make precision`
//! writes it to `docs/precision.md`; the property tests assert the bounds.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::float_arithmetic,
    clippy::cast_precision_loss,
    clippy::disallowed_macros,
    missing_docs
)]

mod common;

use std::fmt::Write as _;

use common::oracle::oracle_stats;
use common::{
    bsm_inputs, bsm_ref, bsm_sample, div, exp_err, exp_ref, ln_ref, mul, norm_cdf_ref,
    norm_pdf_ref, one, ratio, to_ref, ulps, Rng, ONE, P,
};
use num_bigint::BigInt;
use optfix::{
    exp, ln, norm_cdf, norm_pdf, price_and_greeks, price_asset_or_nothing, price_cash_or_nothing,
    Fixed, Price, PricingError, Spot, BINARY_ERR_ABS, BINARY_ERR_REL, DELTA_ERR,
    EXP_MAX_ERROR_ULPS, GAMMA_ERR_REL, GREEK_ERR_ABS, IV_ERR_PRICE_FACTOR, IV_ERR_REL,
    IV_VEGA_MIN_FACTOR, LN_MAX_ERROR_ULPS, NORM_CDF_MAX_ERROR_ULPS, NORM_CDF_TAIL_REL_ERROR,
    NORM_PDF_MAX_ERROR_ULPS, PRICE_ERR_ABS, PRICE_ERR_REL, THETA_ERR_REL, VEGA_ERR_REL,
};
use rust_decimal::Decimal;

/// A function under test: name, bound (ulps), error of the result at `x` in ulps.
struct Func {
    name: &'static str,
    bound: u32,
    err: fn(i128) -> Option<f64>,
}

/// One sampled domain: label and raw-argument generator.
struct Domain {
    label: &'static str,
    gen: fn(&mut Rng) -> i128,
}

fn exp_e(x: i128) -> Option<f64> {
    let v = exp(Fixed::from_raw(x)).ok()?;
    Some(exp_err(v.raw(), &exp_ref(&to_ref(x))))
}

fn ln_e(x: i128) -> Option<f64> {
    let v = ln(Fixed::from_raw(x)).ok()?;
    Some(ulps(v.raw(), &ln_ref(&to_ref(x))).abs())
}

fn cdf_e(x: i128) -> Option<f64> {
    let v = norm_cdf(Fixed::from_raw(x)).ok()?;
    Some(ulps(v.raw(), &norm_cdf_ref(&to_ref(x))).abs())
}

fn pdf_e(x: i128) -> Option<f64> {
    let v = norm_pdf(Fixed::from_raw(x)).ok()?;
    Some(ulps(v.raw(), &norm_pdf_ref(&to_ref(x))).abs())
}

fn uniform(g: &mut Rng, lo: i128, hi: i128) -> i128 {
    g.range(lo, hi)
}

fn rows() -> Vec<(Func, Vec<Domain>)> {
    vec![
        (
            Func {
                name: "exp",
                bound: EXP_MAX_ERROR_ULPS,
                err: exp_e,
            },
            vec![
                Domain {
                    label: "x in [−1, 1]",
                    gen: |g| uniform(g, -ONE, ONE),
                },
                Domain {
                    label: "x in [−18, 18]",
                    gen: |g| uniform(g, -18 * ONE, 18 * ONE),
                },
                Domain {
                    label: "x in [−47, 47]",
                    gen: |g| uniform(g, -47 * ONE, 47 * ONE),
                },
            ],
        ),
        (
            Func {
                name: "ln",
                bound: LN_MAX_ERROR_ULPS,
                err: ln_e,
            },
            vec![
                Domain {
                    label: "x in [0.9, 1.1]",
                    gen: |g| uniform(g, ONE * 9 / 10, ONE * 11 / 10),
                },
                Domain {
                    label: "x in [0.05, 20]",
                    gen: |g| uniform(g, ONE / 20, 20 * ONE),
                },
                Domain {
                    label: "x in [10^-6, 10^9], log-uniform",
                    gen: |g| {
                        // 2^k × [1, 2) with k uniform over the binary exponents of the range.
                        let lo = ONE / 1_000_000;
                        let hi = ONE * 1_000_000_000;
                        let (kl, kh) = (
                            127 - lo.leading_zeros() as i32,
                            127 - hi.leading_zeros() as i32,
                        );
                        let k = kl + (g.next() % (kh - kl + 1) as u64) as i32;
                        let base = 1i128 << k;
                        (base + g.range(0, base)).clamp(lo, hi)
                    },
                },
                Domain {
                    label: "any positive raw value",
                    gen: |g| ((g.wide() >> 1) >> (g.next() % 127)).max(1) as i128,
                },
            ],
        ),
        (
            Func {
                name: "norm_cdf",
                bound: NORM_CDF_MAX_ERROR_ULPS,
                err: cdf_e,
            },
            vec![
                Domain {
                    label: "x in [−1, 1]",
                    gen: |g| uniform(g, -ONE, ONE),
                },
                Domain {
                    label: "x in [−9.25, 9.25]",
                    gen: |g| uniform(g, -37 * ONE / 4, 37 * ONE / 4),
                },
                Domain {
                    label: "x in [−8, −5]",
                    gen: |g| uniform(g, -8 * ONE, -5 * ONE),
                },
            ],
        ),
        (
            Func {
                name: "norm_pdf",
                bound: NORM_PDF_MAX_ERROR_ULPS,
                err: pdf_e,
            },
            vec![
                Domain {
                    label: "x in [−1, 1]",
                    gen: |g| uniform(g, -ONE, ONE),
                },
                Domain {
                    label: "x in [−10, 10]",
                    gen: |g| uniform(g, -10 * ONE, 10 * ONE),
                },
            ],
        ),
    ]
}

/// Max error (ulps) over `n` samples and the argument where it occurs.
fn max_error(f: &Func, d: &Domain, n: usize, seed: u64) -> (f64, i128) {
    let mut g = Rng(seed);
    let mut worst = (0.0, 0);
    for _ in 0..n {
        let x = (d.gen)(&mut g);
        if let Some(e) = (f.err)(x) {
            if e > worst.0 {
                worst = (e, x);
            }
        }
    }
    worst
}

/// Max relative error of `norm_cdf` on `[−8, −5]` beyond the final rounding (half an ulp).
fn tail_relative(n: usize) -> f64 {
    let mut g = Rng(0x0F7F_1C5E_7A11_0000);
    let mut worst = 0.0f64;
    for _ in 0..n {
        let x = g.range(-8 * ONE, -5 * ONE);
        let y = norm_cdf_ref(&to_ref(x));
        let e = ulps(norm_cdf(Fixed::from_raw(x)).unwrap().raw(), &y).abs();
        worst = worst.max((e - 0.5).max(0.0) / ratio(&(y << 64)));
    }
    worst
}

/// `d` as a Q.P reference value.
fn decimal_ref(d: Decimal) -> BigInt {
    (BigInt::from(d.mantissa()) << P) / BigInt::from(10u8).pow(d.scale())
}

/// The OptionStratLib oracle's `big_n` and `n` against the reference on `x = m / 10^9`:
/// (max absolute error, argument), and for `big_n` the max relative error on `[−8, −5]`.
fn oracle(n: usize) -> [(f64, f64, Option<f64>); 2] {
    use optionstratlib::greeks::{big_n, n as small_n};
    let mut g = Rng(0x0F7F_1C5E_05C1_0000);
    let (mut cdf_abs, mut cdf_at, mut cdf_rel) = (0.0f64, 0.0, 0.0f64);
    let (mut pdf_abs, mut pdf_at) = (0.0f64, 0.0);
    for i in 0..n {
        // Alternate the whole range and the tail.
        let m = if i % 2 == 0 {
            g.range(-9_250_000_000, 9_250_000_000)
        } else {
            g.range(-8_000_000_000, -5_000_000_000)
        };
        let x = Decimal::from_i128_with_scale(m, 9);
        let xr = decimal_ref(x);
        let cdf = norm_cdf_ref(&xr);
        let e = ratio(&(decimal_ref(big_n(x).unwrap()) - &cdf)).abs();
        if e > cdf_abs {
            (cdf_abs, cdf_at) = (e, m as f64 / 1e9);
        }
        if i % 2 == 1 {
            cdf_rel = cdf_rel.max(e / ratio(&cdf));
        }
        let p = norm_pdf_ref(&xr);
        let e = ratio(&(decimal_ref(small_n(x).unwrap()) - &p)).abs();
        if e > pdf_abs {
            (pdf_abs, pdf_at) = (e, m as f64 / 1e9);
        }
    }
    [(cdf_abs, cdf_at, Some(cdf_rel)), (pdf_abs, pdf_at, None)]
}

/// Black-Scholes errors over `n` points of the supported domain: for each output the largest
/// `(|error| − abs) / scale` with the envelope's own absolute part and scale, and the largest
/// `|error| / scale` measured against `S Dq` instead of `max(S, K)` for the price (question 4);
/// plus the number of `Overflow`s (a theta beyond the representation).
fn bsm_errors(n: usize) -> ([f64; 6], usize) {
    let mut g = Rng(0x0F7F_1C5E_B5B5_0002);
    let mut worst = [0f64; 6];
    let mut overflow = 0;
    let f = |x: &BigInt| ratio(x).abs();
    for _ in 0..n {
        let p = bsm_sample(&mut g);
        let (opt, m) = bsm_inputs(p);
        let (s, k, t, sigma, r, q, call) = p;
        let v = match price_and_greeks(&opt, &m) {
            Ok(v) => v,
            Err(PricingError::Overflow) => {
                overflow += 1;
                continue;
            }
            Err(e) => panic!("{e:?}"),
        };
        let rf = bsm_ref(
            call,
            &to_ref(s),
            &to_ref(k),
            &to_ref(t),
            &to_ref(sigma),
            &to_ref(r),
            &to_ref(q),
        );
        let err = |got: Fixed, want: &BigInt| f(&(to_ref(got.raw()) - want));
        let beyond = |e: f64, abs: Fixed| (e - ratio(&to_ref(abs.raw()))).max(0.0);
        let ep = err(v.price.get(), &rf.price);
        worst[0] = worst[0].max(beyond(ep, PRICE_ERR_ABS) / f(&to_ref(s.max(k))));
        worst[1] = worst[1].max(beyond(ep, PRICE_ERR_ABS) / f(&rf.s_dq).max(f(&rf.k_dr)));
        worst[2] = worst[2].max(err(v.greeks.delta, &rf.delta));
        if t != 0 {
            let gamma_scale = div(&rf.dq, &mul(&to_ref(s), &rf.sst));
            let vega_scale = mul(&rf.s_dq, &rf.sqrt_t);
            let theta_scale = div(&mul(&rf.s_dq, &to_ref(sigma)), &rf.sqrt_t)
                + mul(&to_ref(r.abs()), &rf.k_dr)
                + mul(&to_ref(q.abs()), &rf.s_dq);
            worst[3] = worst[3]
                .max(beyond(err(v.greeks.gamma, &rf.gamma), GREEK_ERR_ABS) / f(&gamma_scale));
            worst[4] =
                worst[4].max(beyond(err(v.greeks.vega, &rf.vega), GREEK_ERR_ABS) / f(&vega_scale));
            worst[5] = worst[5]
                .max(beyond(err(v.greeks.theta, &rf.theta), GREEK_ERR_ABS) / f(&theta_scale));
        }
    }
    (worst, overflow)
}

/// Largest `(|error| − BINARY_ERR_ABS) / scale` of the cash-or-nothing (`scale = payout`) and
/// asset-or-nothing (`scale = S`) prices against the 256-bit reference, on `n` points of
/// `bsm_sample` with a log-uniform payout in `[10^-6, 10^9]`; also the same error over the
/// discounted scale (`payout × Dr`, `S Dq`), which shows why the notional is the scale.
fn binary_errors(n: usize) -> [f64; 4] {
    let mut g = Rng(0x0F7F_1C5E_B1A2_0003);
    let mut worst = [0f64; 4];
    let f = |x: &BigInt| ratio(x).abs();
    for _ in 0..n {
        let p = bsm_sample(&mut g);
        let (opt, m) = bsm_inputs(p);
        let (s, k, t, sigma, r, q, call) = p;
        let pay_f = 10f64.powf(-6.0 + 15.0 * common::uniform(&mut g));
        let pay = ((pay_f * (1u128 << 64) as f64) as i128)
            .clamp(Spot::MIN.get().raw(), Spot::MAX.get().raw());
        let rf = bsm_ref(
            call,
            &to_ref(s),
            &to_ref(k),
            &to_ref(t),
            &to_ref(sigma),
            &to_ref(r),
            &to_ref(q),
        );
        let (cash_exact, asset_exact) = if t == 0 {
            let itm = if call { s > k } else { s < k };
            if itm {
                (to_ref(pay), to_ref(s))
            } else {
                (BigInt::from(0), BigInt::from(0))
            }
        } else if call {
            (
                mul(&mul(&to_ref(pay), &rf.dr), &rf.nd2),
                mul(&rf.s_dq, &rf.nd1),
            )
        } else {
            (
                mul(&mul(&to_ref(pay), &rf.dr), &(one() - &rf.nd2)),
                mul(&rf.s_dq, &(one() - &rf.nd1)),
            )
        };
        let cash =
            price_cash_or_nothing(&opt, &m, Price::try_new(Fixed::from_raw(pay)).unwrap()).unwrap();
        let asset = price_asset_or_nothing(&opt, &m).unwrap();
        let beyond = |got: Fixed, want: &BigInt| {
            (f(&(to_ref(got.raw()) - want)) - ratio(&to_ref(BINARY_ERR_ABS.raw()))).max(0.0)
        };
        let (ec, ea) = (
            beyond(cash.get(), &cash_exact),
            beyond(asset.get(), &asset_exact),
        );
        worst[0] = worst[0].max(ec / f(&to_ref(pay)));
        worst[1] = worst[1].max(ea / f(&to_ref(s)));
        worst[2] = worst[2].max(ec / f(&mul(&to_ref(pay), &rf.dr)));
        worst[3] = worst[3].max(ea / f(&rf.s_dq));
    }
    worst
}

fn binary_section(n: usize) -> String {
    let w = binary_errors(n);
    format!(
        "\n## Binary prices\n\n\
         `price_cash_or_nothing` and `price_asset_or_nothing` against the 256-bit reference \
         (`common::bsm_ref`'s `N(d1)`, `N(d2)` and discount factors) on {n} points of the whole \
         supported domain (`common::bsm_sample`), with a log-uniform payout in `[10^-6, 10^9]`. \
         The envelope is `BINARY_ERR_REL × scale + BINARY_ERR_ABS`; the table gives the largest \
         `(|error| − ABS) / scale`.\n\n\
         | Price | Scale | Max | Envelope `REL` | `ABS` |\n\
         | --- | --- | ---: | ---: | ---: |\n\
         | cash-or-nothing | `payout` | {:.3e} | {:.3e} | {} ulps |\n\
         | asset-or-nothing | `S` | {:.3e} | {:.3e} | {} ulps |\n\n\
         The scale is the undiscounted notional, as `max(S, K)` is for the price: over the \
         discounted scale (`payout × Dr`, `S Dq`) the same errors reach {:.3e} and {:.3e}, \
         because a discount factor down to `e^{{−15}}` holds only absolute precision in Q64.64. \
         The mpmath fixture (`tests/data/binary_fixture.csv`, \
         `tools/binary_fixture/gen_binary_fixture.py`: `d2` or `d1` from −9 to 9 at 26 expiries \
         from `10^-12` s to 30 years, at the money, both sides of the `d1` switch and `T = 0`) \
         is checked by `tests/binary_fixture.rs`.\n",
        w[0],
        ratio(&to_ref(BINARY_ERR_REL.raw())),
        BINARY_ERR_ABS.raw(),
        w[1],
        ratio(&to_ref(BINARY_ERR_REL.raw())),
        BINARY_ERR_ABS.raw(),
        w[2],
        w[3],
    )
}

/// Milestone 3 record, measured at commit `6adf034` with 1,000,000 samples per domain
/// (`make precision` there reproduces it); the decimal candidates no longer exist.
const COMPARISON: &str = "
## Representation comparison (milestone 3)

Maximum error over every domain above, measured at commit `6adf034` (`make precision` there \
reproduces the full table), for each candidate of `docs/PLAN.md` 4.1. Decimal ulps are \
`10^-18`. Q64.64 won on compute units (`docs/benchmarks.md`) and is the only one kept.

| Function | Representation | Max error | Unit | Absolute |
| --- | --- | ---: | --- | ---: |
| `exp` | Q64.64 | 1.974 | ulp `2^-64` of `max(e^x, 1)` | `1.1 × 10^-19` |
| `ln` | Q64.64 | 1.507 | ulp `2^-64` | `8.2 × 10^-20` |
| `ln` | Q64.64, atanh form | 1.502 | ulp `2^-64` | `8.1 × 10^-20` |
| `exp` | decimal `10^18`, binary core | 0.594 | ulp `10^-18` of `max(e^x, 1)` | `5.9 × 10^-19` |
| `ln` | decimal `10^18`, binary core | 0.597 | ulp `10^-18` | `6.0 × 10^-19` |
| `exp` | decimal `10^18`, native | 3.127 | ulp `10^-18` of `max(e^x, 1)` | `3.1 × 10^-18` |
| `ln` | decimal `10^18`, native | 3.032 | ulp `10^-18` | `3.0 × 10^-18` |
";

#[test]
fn test_precision_report() {
    let n: usize = std::env::var("OPTFIX_PRECISION_SAMPLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000);
    let mut s = String::new();
    s.push_str("# Precision\n\n");
    write!(
        s,
        "Generated by `make precision` (`optfix/tests/precision.rs`); do not edit by hand. \
         Maximum error over {n} deterministic samples per row (splitmix64), against a \
         256-bit reference (`num-bigint` series in `optfix/tests/common/`, itself checked \
         against Python's correctly rounded `decimal` in `tools/reference.txt`). One ulp is \
         `2^-64`, about `5.4 × 10^-20`. `exp` errors are in ulps of `max(e^x, 1)`, a \
         relative bound for results above 1 and an absolute one below; the other errors are \
         absolute. The bounds are the crate's `*_MAX_ERROR_ULPS` constants, which the \
         property tests (`make test`) assert over one million random cases each.\n\n"
    )
    .unwrap();
    s.push_str("| Function | Domain | Max error (ulp) | Bound (ulp) | At x |\n");
    s.push_str("| --- | --- | ---: | ---: | ---: |\n");
    for (fi, (f, domains)) in rows().iter().enumerate() {
        for (di, d) in domains.iter().enumerate() {
            let seed = 0x0F7F_1C5E_0000_0000 ^ ((fi as u64) << 16) ^ ((di as u64) << 8);
            let (e, x) = max_error(f, d, n, seed);
            let xv = ratio(&to_ref(x));
            writeln!(
                s,
                "| `{}` | {} | {e:.3} | {} | {xv:.6e} |",
                f.name, d.label, f.bound
            )
            .unwrap();
        }
    }
    let rel = tail_relative(n);
    let bound = ratio(&to_ref(NORM_CDF_TAIL_REL_ERROR.raw()));
    write!(
        s,
        "\n`norm_cdf` on `[−8, −5]`, relative error beyond the final rounding (half an ulp): \
         max {rel:.3e}, bound `NORM_CDF_TAIL_REL_ERROR` = {bound:.3e}. The rounding itself is \
         a relative `4.4 × 10^-5` at `x = −8` and `9.4 × 10^-14` at `x = −5`.\n"
    )
    .unwrap();
    let [(ca, cx, cr), (pa, px, _)] = oracle(n);
    write!(
        s,
        "\n## Oracle: OptionStratLib 0.23\n\n\
         `optionstratlib::greeks::big_n` and `n` (PLAN 4.7), measured on {n} arguments `x = m / \
         10^9`, half of them in `[−8, −5]`. `big_n` converts to `f64` and calls `statrs`; `n` \
         returns zero beyond `|x|` of about 4.84 (an `exp` cutoff at −11.7). Milestone 5 bounds are never claimed tighter than these.\n\n\
         | Function | Max absolute error | At x | Max relative error on `[−8, −5]` |\n\
         | --- | ---: | ---: | ---: |\n\
         | `big_n` | {ca:.3e} | {cx:.4} | {:.3e} |\n\
         | `n` | {pa:.3e} | {px:.4} | |\n",
        cr.unwrap_or(0.0)
    )
    .unwrap();
    let (w, overflow) = bsm_errors(n);
    let b = |x: Fixed| ratio(&to_ref(x.raw()));
    write!(
        s,
        "\n## Black-Scholes-Merton\n\n\
         `price_and_greeks` against the 256-bit reference (`common::bsm_ref`, the exact formulas, \
         including the true value at tiny `T`) on {n} points of the whole supported domain \
         (`common::bsm_sample`: every range, `T` from `0` and a few ulps to 30 years, `q > 0`, \
         `q < 0` and `q = 0`). Each envelope is `REL × scale + ABS`; the table gives the \
         largest `(|error| − ABS) / scale` against `REL`.\n\n\
         | Output | Scale | Max | Envelope `REL` | `ABS` |\n\
         | --- | --- | ---: | ---: | ---: |\n\
         | price | `max(S, K)` | {:.3e} | {:.3e} | 64 ulps |\n\
         | delta | 1 | {:.3e} | {:.3e} (absolute) | |\n\
         | gamma | `Dq / (S σ√T)` | {:.3e} | {:.3e} | 64 ulps |\n\
         | vega | `S Dq √T` | {:.3e} | {:.3e} | 64 ulps |\n\
         | theta | `S Dq σ / √T + abs(r) K Dr + abs(q) S Dq` | {:.3e} | {:.3e} | 64 ulps |\n\n\
         Question 4 (carry scale): the price error beyond 64 ulps divided by `max(S Dq, K Dr)` \
         instead of `max(S, K)` reaches {:.3e}, because `S Dq` falls to `e^{{−15}} S`; \
         `max(S, K)` is the scale that holds over the carry range. `Overflow`: {overflow} of {n} \
         (none is reachable over the supported domain).\n",
        w[0],
        b(PRICE_ERR_REL),
        w[2],
        b(DELTA_ERR),
        w[3],
        b(GAMMA_ERR_REL),
        w[4],
        b(VEGA_ERR_REL),
        w[5],
        b(THETA_ERR_REL),
        w[1]
    )
    .unwrap();
    write!(
        s,
        "\n### Against OptionStratLib 0.23\n\n\
         Garman-Kohlhagen with the foreign rate `q` (`common::oracle`), on {} exact decimal \
         points per carry set with `T` from one day; gamma, vega and theta only where \
         `abs(d1) <= 4.8` (beyond, the oracle's `n` is zero). The columns are the largest \
         difference over the oracle tolerance (`<= 1` passes): price `10^-9 × max(S, K)`, delta \
         `10^-9`, the other greeks `10^-9 × scale + 10^-18`.\n\n\
         | Carry | price | delta | gamma | vega | theta | `q = 0` price vs `black_scholes` |\n\
         | --- | ---: | ---: | ---: | ---: | ---: | ---: |\n",
        n.min(20_000)
    )
    .unwrap();
    for (carry, label) in [(1, "`q > 0`"), (-1, "`q < 0`"), (0, "`q = 0`")] {
        let st = oracle_stats(n.min(20_000), carry);
        let r = st.ratio;
        let last = if carry == 0 {
            format!("{:.3e}", r[5])
        } else {
            String::new()
        };
        writeln!(
            s,
            "| {label} | {:.3e} | {:.3e} | {:.3e} | {:.3e} | {:.3e} | {last} |",
            r[0], r[1], r[2], r[3], r[4]
        )
        .unwrap();
    }
    s.push_str(&binary_section(n));
    s.push_str(&implied_vol_section(n));
    s.push_str(COMPARISON);
    s.push_str("\nRun `make precision` to regenerate this file.\n");
    println!("{s}");
    if let Ok(path) = std::env::var("OPTFIX_PRECISION_OUT") {
        std::fs::write(path, s).unwrap();
    }
    let _ = one();
}

/// The implied-volatility section: the grid round trip, the steps taken, the initial guess and
/// the mpmath reference (`common::iv`, the same checks as `iv_round_trip.rs`, `iv_internals.rs`
/// and `iv_fixture.rs`).
fn implied_vol_section(n: usize) -> String {
    let mut s = String::new();
    let b = |x: Fixed| ratio(&to_ref(x.raw()));
    write!(
        s,
        "\n## Implied volatility\n\n\
         `implied_vol(price(σ))` on {n} points per carry set of the whole supported domain \
         (`common::bsm_sample` with the carry of each set). With `E` the price envelope, \
         `implied_vol` answers where vega at the root is at least `IV_VEGA_MIN_FACTOR` = \
         {:.3e} times `E`, within `IV_ERR_PRICE_FACTOR` = {} times `E / vega` plus \
         `IV_ERR_REL` = {:.3e} times `σ`; below the threshold it returns `IllConditioned`. \
         \"No time value\" is a target equal to a bound (the out-of-the-money value rounds to \
         zero), `ArbitrageBound`. The price column is `price(implied_vol(p))` against `p`, over \
         `(IV_ERR_PRICE_FACTOR + 1) × E + IV_ERR_REL × σ × vega`. `NoConvergence` does not \
         occur.\n\n",
        b(IV_VEGA_MIN_FACTOR),
        b(IV_ERR_PRICE_FACTOR),
        b(IV_ERR_REL)
    )
    .unwrap();
    s.push_str(&common::iv::round_trip_table(n, 0x1f60));
    #[cfg(feature = "bench-internals")]
    {
        let hist = common::iv::iteration_histogram(n, 0x2f60);
        write!(
            s,
            "\nSteps taken (at most `IV_MAX_ITERATIONS` = 3; a solve stops once a step is below \
             `IV_TOLERANCE`, `2^-16` in `ln σ`), over the same {} points by outcome:\n\n\
             | Outcome | 1 step | 2 steps | 3 steps |\n\
             | --- | ---: | ---: | ---: |\n",
            3 * n
        )
        .unwrap();
        for (name, h) in ["answered", "`IllConditioned`"].iter().zip(hist) {
            writeln!(s, "| {name} | {} | {} | {} |", h[1], h[2], h[3]).unwrap();
        }
        let (worst, count) = common::iv::guess_errors(n, 0x3f60, 0.0625);
        write!(
            s,
            "\nInitial guess (`tools/gen_iv_tables.py`), largest `|ln σ0 − ln σ|` where \
             `implied_vol` answers: {:.4} from the near-the-money table ({} points), {:.4} \
             from the deep out-of-the-money table ({} points).\n",
            worst[0], count[0], worst[1], count[1]
        )
        .unwrap();
    }
    let fx = common::iv::fixture_check("iv_fixture.csv");
    let th = common::iv::fixture_check("iv_threshold.csv");
    write!(
        s,
        "\nAgainst the independent mpmath reference (`tests/data/iv_fixture.csv`, \
         `tools/iv_fixture/gen_iv_fixture.py`: {} exact targets on a boundary set with `d1` \
         from −9 to 9, near the money at ±1/4 and 0, `T` from `10^-12` s to a year): {} \
         answered, the largest error {:.3e} of the envelope around the exact implied \
         volatility of each rounded target; the other {} rejected (`IllConditioned`, or no \
         root inside the bounds).\n",
        fx.rows, fx.answered, fx.worst, fx.rejected
    )
    .unwrap();
    write!(
        s,
        "\nAt the conditioning threshold (`tests/data/iv_threshold.csv`, \
         `tools/iv_fixture/gen_iv_threshold.py`: {} targets whose exact root's vega is the \
         threshold times `1 + ε`, `ε` from `−2^-6` to `2^-6` down to `±2^-26` and 0): no target \
         below the threshold is answered, every one at `1 + 2^-8` or above is; {} answered \
         (worst {:.3e} of the envelope), {} rejected, of which {} answered and {} rejected in \
         `[thr, thr (1 + 2^-8))`, where `implied_vol` widens the threshold by its own `δ`.\n",
        th.rows, th.answered, th.worst, th.rejected, th.band_answered, th.band_rejected
    )
    .unwrap();
    let cx = common::iv::fixture_check("iv_threshold_codex.csv");
    write!(
        s,
        "\nCodex's adversarial threshold set (`tests/data/iv_threshold_codex.csv`, \
         `tools/iv_fixture/codex/`, 100 mpmath digits, the exact ratio of the root's vega to \
         the threshold per target): {} targets, none with a ratio below 1 answered; {} \
         answered (worst {:.3e} of the envelope), {} rejected.\n",
        cx.rows, cx.answered, cx.worst, cx.rejected
    )
    .unwrap();
    s
}
