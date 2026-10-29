//! Property tests for `price` and `price_and_greeks` against the 256-bit Black-Scholes-Merton
//! reference in `common/`, over the whole supported domain (every range, `T` down to a few
//! ulps and `T = 0`, `q > 0`, `q < 0` and `q = 0`), plus put-call parity, the delta bounds
//! `[0, Dq]` / `[−Dq, 0]`, the domain checks and a bit-exact `q = 0` regression table.
//!
//! The reference costs about half a millisecond per case, so these properties default to
//! 100,000 cases (`OPTFIX_PROPTEST_CASES` overrides it).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::float_arithmetic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::panic_in_result_fn,
    clippy::disallowed_macros,
    missing_docs
)]

mod common;

use std::fmt::Write as _;

use common::{bsm_inputs, bsm_ref, bsm_sample, config_with, div, mul, ratio, to_ref, Rng};
use num_bigint::BigInt;
use optfix::{
    price, price_and_greeks, EuropeanOption, Fixed, OptionKind, PricingError, Rate, Spot, Strike,
    Years, DELTA_ERR, GAMMA_ERR_REL, GREEK_ERR_ABS, PRICE_ERR_ABS, PRICE_ERR_REL, THETA_ERR_REL,
    VEGA_ERR_REL,
};
use proptest::prelude::*;

const CASES: u32 = 100_000;

fn r(x: Fixed) -> BigInt {
    to_ref(x.raw())
}

/// `|got − want| <= rel × scale + abs`, all in Q.P.
fn within(got: Fixed, want: &BigInt, rel: Fixed, scale: &BigInt, abs: Fixed) -> bool {
    let err = (r(got) - want).magnitude().clone();
    let allowed = (mul(&r(rel), scale) + r(abs)).magnitude().clone();
    err <= allowed
}

/// Checks one point of the domain. `Overflow` is not reachable over the supported domain, so
/// it fails unless the reference itself is beyond `Fixed::MAX`; `Ok(false)` then.
fn check_point(p: (i128, i128, i128, i128, i128, i128, bool)) -> Result<bool, TestCaseError> {
    let (opt, m) = bsm_inputs(p);
    let (s, k, t, sigma, rate, q, call) = p;
    let rf = bsm_ref(
        call,
        &to_ref(s),
        &to_ref(k),
        &to_ref(t),
        &to_ref(sigma),
        &to_ref(rate),
        &to_ref(q),
    );
    let priced = match price_and_greeks(&opt, &m) {
        Ok(v) => v,
        Err(PricingError::Overflow) => {
            // Only theta can exceed the representation: its reference must be beyond MAX.
            prop_assert!(
                rf.theta.magnitude() > to_ref(i128::MAX).magnitude(),
                "unexpected Overflow at {p:?}"
            );
            prop_assert!(
                price(&opt, &m).is_ok(),
                "price() must not overflow at {p:?}"
            );
            return Ok(false);
        }
        Err(e) => return Err(TestCaseError::fail(format!("{e:?} at {p:?}"))),
    };
    let g = priced.greeks;
    let ctx = || format!("at {p:?}");
    // price() returns the same premium.
    prop_assert_eq!(
        price(&opt, &m).unwrap(),
        priced.price,
        "price() differs {}",
        ctx()
    );
    let max_sk = to_ref(s.max(k));
    prop_assert!(
        within(
            priced.price.get(),
            &rf.price,
            PRICE_ERR_REL,
            &max_sk,
            PRICE_ERR_ABS
        ),
        "price {} vs {} {}",
        ratio(&r(priced.price.get())),
        ratio(&rf.price),
        ctx()
    );
    prop_assert!(
        within(g.delta, &rf.delta, Fixed::ZERO, &BigInt::from(0), DELTA_ERR),
        "delta {}",
        ctx()
    );
    // Delta bounds: [0, Dq] for a call, [−Dq, 0] for a put, within the envelope.
    let dq = &rf.dq + r(DELTA_ERR);
    if call {
        prop_assert!(
            g.delta >= Fixed::ZERO && r(g.delta) <= dq,
            "call delta out of [0, Dq] {}",
            ctx()
        );
    } else {
        prop_assert!(
            g.delta <= Fixed::ZERO && -r(g.delta) <= dq,
            "put delta out of [−Dq, 0] {}",
            ctx()
        );
    }
    prop_assert!(
        g.gamma >= Fixed::ZERO && g.vega >= Fixed::ZERO,
        "gamma or vega negative {}",
        ctx()
    );
    if t != 0 {
        let gamma_scale = div(&rf.dq, &mul(&to_ref(s), &rf.sst));
        prop_assert!(
            within(
                g.gamma,
                &rf.gamma,
                GAMMA_ERR_REL,
                &gamma_scale,
                GREEK_ERR_ABS
            ),
            "gamma {}",
            ctx()
        );
        let vega_scale = mul(&rf.s_dq, &rf.sqrt_t);
        prop_assert!(
            within(g.vega, &rf.vega, VEGA_ERR_REL, &vega_scale, GREEK_ERR_ABS),
            "vega {}",
            ctx()
        );
        let theta_scale = div(&mul(&rf.s_dq, &to_ref(sigma)), &rf.sqrt_t)
            + mul(&to_ref(rate.abs()), &rf.k_dr)
            + mul(&to_ref(q.abs()), &rf.s_dq);
        prop_assert!(
            within(
                g.theta,
                &rf.theta,
                THETA_ERR_REL,
                &theta_scale,
                GREEK_ERR_ABS
            ),
            "theta {}",
            ctx()
        );
    } else {
        prop_assert_eq!((g.gamma, g.vega), (Fixed::ZERO, Fixed::ZERO));
        prop_assert!(
            within(
                g.theta,
                &rf.theta,
                Fixed::ZERO,
                &BigInt::from(0),
                GREEK_ERR_ABS
            ),
            "theta {}",
            ctx()
        );
    }
    Ok(true)
}

proptest! {
    #![proptest_config(config_with(CASES))]

    #[test]
    fn prop_price_and_greeks_within_envelopes(seed in any::<u64>()) {
        check_point(bsm_sample(&mut Rng(seed)))?;
    }

    /// `C − P = S Dq − K Dr` within the sum of the two price envelopes.
    #[test]
    fn prop_put_call_parity(seed in any::<u64>()) {
        let p = bsm_sample(&mut Rng(seed));
        let (opt, m) = bsm_inputs((p.0, p.1, p.2, p.3, p.4, p.5, true));
        let c = price(&opt, &m).unwrap().get();
        let put = price(&EuropeanOption { kind: OptionKind::Put, ..opt }, &m).unwrap().get();
        let rf = bsm_ref(true, &to_ref(p.0), &to_ref(p.1), &to_ref(p.2), &to_ref(p.3), &to_ref(p.4), &to_ref(p.5));
        let fwd = &rf.s_dq - &rf.k_dr;
        let diff = r(c) - r(put);
        let allowed: BigInt = mul(&r(PRICE_ERR_REL), &to_ref(p.0.max(p.1))) * 2 + r(PRICE_ERR_ABS) * 2;
        prop_assert!((diff - fwd).magnitude() <= allowed.magnitude(), "parity at {:?}", p);
    }

    /// `S/K` outside `[0.05, 20]` is a `DomainError`, for both functions.
    #[test]
    fn prop_moneyness_outside_domain_is_rejected(seed in any::<u64>(), factor in 21i128..400) {
        let p = bsm_sample(&mut Rng(seed));
        let s = Spot::MIN.get().raw().max(p.0);
        // K far above 20 S, or far below S / 20, kept inside the strike range.
        for k in [s.saturating_mul(factor), s / factor] {
            let k = k.clamp(Strike::MIN.get().raw(), Strike::MAX.get().raw());
            let inside = k <= s * 20 && k * 20 >= s;
            let (opt, m) = bsm_inputs((s, k, p.2, p.3, p.4, p.5, p.6));
            if !inside {
                prop_assert_eq!(price(&opt, &m), Err(PricingError::DomainError));
                prop_assert_eq!(price_and_greeks(&opt, &m).map(|v| v.price), Err(PricingError::DomainError));
            }
        }
    }
}

/// The carry sets of `docs/PLAN.md` 4.7 by construction, each over its own sample: `q > 0` up
/// to `q = 0.5, T = 30` (`Dq = e^{−15}`), `q < 0` down to `q = −0.1, T = 30` (`Dq = e^3`, so
/// `|delta| > 1`), and `q = 0`.
#[test]
fn test_carry_sets() {
    let mut g = Rng(0x0F7F_1C5E_CA44_0000);
    let cases: u32 = std::env::var("OPTFIX_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(CASES)
        / 10;
    let mut big_delta = 0;
    for (lo, hi) in [(1i128, 1i128 << 63), (Rate::MIN.get().raw(), -1), (0, 0)] {
        for i in 0..cases {
            let mut p = bsm_sample(&mut g);
            p.5 = if lo == hi {
                0
            } else {
                lo + (g.wide() % (hi - lo + 1) as u128) as i128
            };
            if i % 8 == 0 {
                // The extremes: q at the end of its range, T = 30.
                p.5 = if lo == hi {
                    0
                } else if hi < 0 {
                    Rate::MIN.get().raw()
                } else {
                    Rate::MAX.get().raw()
                };
                p.2 = Years::MAX.get().raw();
            }
            if !check_point(p).unwrap() {
                continue;
            }
            let (opt, m) = bsm_inputs(p);
            if price_and_greeks(&opt, &m)
                .unwrap()
                .greeks
                .delta
                .raw()
                .unsigned_abs()
                > 1 << 64
            {
                big_delta += 1;
            }
        }
    }
    assert!(big_delta > 0, "the q < 0 set never reached |delta| > 1");
}

/// Committed `q = 0` results, bit for bit: later carry changes must not move them.
/// `OPTFIX_REGEN_Q0=1` rewrites `tests/data/bsm_q0.txt`.
#[test]
fn test_q0_regression_table() {
    let mut g = Rng(0x0F7F_1C5E_0000_0000);
    let mut out = String::new();
    for _ in 0..64 {
        let mut p = bsm_sample(&mut g);
        p.5 = 0;
        let (opt, m) = bsm_inputs(p);
        let v = price_and_greeks(&opt, &m).unwrap();
        writeln!(
            out,
            "{} {} {} {} {} {} {} {} {} {} {} {}",
            p.0,
            p.1,
            p.2,
            p.3,
            p.4,
            u8::from(p.6),
            v.price.get().raw(),
            v.greeks.delta.raw(),
            v.greeks.gamma.raw(),
            v.greeks.vega.raw(),
            v.greeks.theta.raw(),
            price(&opt, &m).unwrap().get().raw()
        )
        .unwrap();
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/bsm_q0.txt");
    if std::env::var("OPTFIX_REGEN_Q0").is_ok() {
        std::fs::write(path, &out).unwrap();
    }
    let committed =
        std::fs::read_to_string(path).expect("tests/data/bsm_q0.txt (OPTFIX_REGEN_Q0=1 writes it)");
    assert_eq!(committed, out, "q = 0 results moved");
}
