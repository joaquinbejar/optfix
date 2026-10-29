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

use common::{
    exp_err, exp_ref, ln_ref, norm_cdf_ref, norm_pdf_ref, one, ratio, to_ref, ulps, Rng, ONE, P,
};
use num_bigint::BigInt;
use optfix::{
    exp, ln, norm_cdf, norm_pdf, Fixed, EXP_MAX_ERROR_ULPS, LN_MAX_ERROR_ULPS,
    NORM_CDF_MAX_ERROR_ULPS, NORM_CDF_TAIL_REL_ERROR, NORM_PDF_MAX_ERROR_ULPS,
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
    s.push_str(COMPARISON);
    s.push_str("\nRun `make precision` to regenerate this file.\n");
    println!("{s}");
    if let Ok(path) = std::env::var("OPTFIX_PRECISION_OUT") {
        std::fs::write(path, s).unwrap();
    }
    let _ = one();
}
