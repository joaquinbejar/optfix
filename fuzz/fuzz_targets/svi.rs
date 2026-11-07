//! Raw SVI slices (`docs/PLAN-phase2.md`, milestone 22): `SviParams::try_new` agrees with the
//! documented domain, with the minimum's lower bound never above the true minimum and within
//! its documented slack; `total_variance` at an exact `k` is never above `w(k)` and within
//! `E_w` below it; `svi_vol` equals its documented steps and lies inside `svi_vol_bounds`; and
//! nothing panics. Half the inputs are any raw values, half are drawn inside the domain.
#![no_main]

use libfuzzer_sys::fuzz_target;
use num_bigint::BigInt;
use optfix::{
    log_forward_moneyness, svi_vol, svi_vol_bounds, Fixed, Rate, Spot, Strike, SviMarket,
    SviParams, Volatility, Years,
};
use optfix_fuzz::{Bytes, ONE};

/// Fractional bits of the reference.
const P: u32 = 192;

fn big(raw: i128) -> BigInt {
    BigInt::from(raw) << (P - 64)
}

fn mul(a: &BigInt, b: &BigInt) -> BigInt {
    (a * b) >> P
}

fn sqrt(v: &BigInt) -> BigInt {
    (v << P).sqrt()
}

/// `w(k)` in Q.P, truncated toward −∞ at `2^-192`.
fn w_ref(p: [i128; 5], k: i128) -> BigInt {
    let [a, b, rho, m, s] = p.map(big);
    let x = big(k) - m;
    a + mul(&b, &(mul(&rho, &x) + sqrt(&(mul(&x, &x) + mul(&s, &s)))))
}

/// `a + b σ sqrt(1 − ρ²)` in Q.P, truncated (within a few units of `2^-192`).
fn min_ref(p: [i128; 5]) -> BigInt {
    let [a, b, rho, _, s] = p.map(big);
    a + mul(
        &mul(&b, &s),
        &sqrt(&((BigInt::from(1) << P) - mul(&rho, &rho))),
    )
}

/// `lo + (hi − lo) × x / 2^64`, inside `[lo, hi]`, for `hi − lo < 2^127`.
fn span(x: u64, lo: i128, hi: i128) -> i128 {
    let d = (hi - lo) as u128;
    let x = u128::from(x);
    lo + ((d >> 64) * x + (((d & u128::from(u64::MAX)) * x) >> 64)) as i128
}

fn in_ranges(p: [i128; 5]) -> bool {
    let [a, b, rho, m, s] = p;
    (-1000 * ONE..=1000 * ONE).contains(&a)
        && (0..=10 * ONE).contains(&b)
        && rho > -ONE
        && rho < ONE
        && (-10 * ONE..=10 * ONE).contains(&m)
        && (18_446_744_073_709..=10 * ONE).contains(&s)
}

fuzz_target!(|data: &[u8]| {
    let mut g = Bytes(data);
    let mode = g.u8();
    let (p, k) = if mode & 1 == 0 {
        ([g.i128(), g.i128(), g.i128(), g.i128(), g.i128()], g.i128())
    } else {
        let b = span(g.u64(), 0, 10 * ONE) >> (g.u8() % 64);
        let s = span(g.u64(), 18_446_744_073_709, 10 * ONE) >> (g.u8() % 24);
        let rho = match g.u8() % 4 {
            0 => ONE - 1 - i128::from(g.u64() >> (g.u8() % 64)),
            1 => 1 - ONE + i128::from(g.u64() >> (g.u8() % 64)),
            _ => span(g.u64(), 1 - ONE, ONE - 1),
        };
        let m = span(g.u64(), -10 * ONE, 10 * ONE);
        let k = span(g.u64(), -32 * ONE, 32 * ONE);
        // a near −(b σ sqrt(1 − ρ²)): the minimum's sign is in play.
        let part = i128::try_from(&(min_ref([0, b, rho, 0, s.max(1)]) >> (P - 64))).unwrap_or(0);
        let a = (i128::from(g.u32() as i32) - part).clamp(-1000 * ONE, 1000 * ONE);
        ([a, b, rho, m, s], k)
    };
    let [a, b, rho, m, s] = p.map(Fixed::from_raw);
    let slice = SviParams::try_new(a, b, rho, m, s);
    if !in_ranges(p) {
        assert!(slice.is_err(), "accepted a slice outside the ranges: {p:?}");
        return;
    }
    let truth = min_ref(p);
    let slice = match slice {
        Ok(sl) => {
            let lb = big(sl.min_total_variance().raw());
            assert!(
                lb <= truth,
                "the minimum's bound is above the true minimum at {p:?}"
            );
            assert!(lb >= BigInt::from(0));
            sl
        }
        Err(_) => {
            // Refused: the true minimum is negative, or within the bound's slack of 0,
            // `b σ (1 / (2 sqrt(δ)) + 1) + 2` ulps with `δ = 1 − ρ²` rounded down.
            let r = Fixed::from_raw(p[2]);
            let delta = Fixed::ONE.checked_sub(r.mul_up(r).unwrap()).unwrap();
            let bs = (p[1] as f64 / ONE as f64) * (p[4] as f64 / ONE as f64);
            let slack = bs * (1.0 / (2.0 * (delta.raw() as f64 / ONE as f64).sqrt()) + 1.0) + 2.0;
            let truth_ulps = (&truth >> (P - 64)).to_string().parse::<f64>().unwrap();
            assert!(
                truth_ulps <= slack * 1.0001 + 1.0,
                "refused a slice whose minimum is {truth_ulps} ulps, slack {slack}: {p:?}"
            );
            return;
        }
    };
    let kf = Fixed::from_raw(k);
    match slice.total_variance_bounds(kf) {
        Ok((lo, hi)) => {
            assert!((-32 * ONE..=32 * ONE).contains(&k));
            let w = w_ref(p, k);
            assert!(
                big(lo.raw()) <= w,
                "total_variance above w(k) at {p:?}, k {k}"
            );
            assert!(
                w <= big(hi.raw()),
                "w(k) above the envelope at {p:?}, k {k}"
            );
        }
        Err(_) => assert!(!(-32 * ONE..=32 * ONE).contains(&k)),
    }
    // A market: any raw values, refused by the newtypes when out of range.
    let raw = [g.i128(), g.i128(), g.i128(), g.i128(), g.i128()];
    let (Ok(spot), Ok(strike), Ok(expiry), Ok(rate), Ok(carry)) = (
        Spot::try_new(Fixed::from_raw(raw[0])),
        Strike::try_new(Fixed::from_raw(raw[1])),
        Years::try_new(Fixed::from_raw(raw[2])),
        Rate::try_new(Fixed::from_raw(raw[3])),
        Rate::try_new(Fixed::from_raw(raw[4])),
    ) else {
        return;
    };
    let market = SviMarket { spot, rate, carry };
    let vol = svi_vol(&slice, expiry, &market, strike);
    let steps = || -> Result<Volatility, optfix::PricingError> {
        if expiry.get() == Fixed::ZERO {
            return Err(optfix::PricingError::DomainError);
        }
        let k = log_forward_moneyness(expiry, &market, strike)?;
        let w = slice.total_variance(k)?;
        Volatility::try_new(w.div_down(expiry.get())?.sqrt_down()?)
    };
    assert_eq!(vol, steps(), "svi_vol differs from its documented steps");
    let bounds = svi_vol_bounds(&slice, expiry, &market, strike);
    if let (Ok(v), Ok((lo, hi))) = (vol, bounds) {
        assert!(lo <= v.get() && v.get() <= hi, "svi_vol outside its bounds");
    }
    if let Ok((lo, hi)) = bounds {
        assert!(lo <= hi);
    }
});
