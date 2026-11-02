//! `implied_vol(price(σ))` against the round trip contract of `common::contract`
//! (`docs/PLAN-improvements.md`, milestone 16, target 5), and `implied_vol` on any target never
//! panicking.
#![no_main]

use libfuzzer_sys::fuzz_target;
use optfix::{implied_vol, price_and_greeks, Fixed, IvInputs, Price};
use optfix_fuzz::contract::iv_round_trip;
use optfix_fuzz::{bsm_point, Bytes};

fuzz_target!(|data: &[u8]| {
    let mut b = Bytes(data);
    let (opt, m) = bsm_point(&mut b);
    let im = IvInputs { spot: m.spot, rate: m.rate, carry: m.carry };
    let priced = price_and_greeks(&opt, &m).expect("the sampler stays in the domain");
    let got = implied_vol(&opt, &im, priced.price);
    if let Err(why) = iv_round_trip(&opt, &m, &priced, got) {
        panic!("{why} at {opt:?} {m:?}: {got:?}");
    }
    // Any target: an answer or an error, never a panic.
    let target = Price::try_new(Fixed::from_raw(b.i128().unsigned_abs().min(i128::MAX as u128) as i128));
    if let Ok(t) = target {
        let _ = implied_vol(&opt, &im, t);
    }
});
