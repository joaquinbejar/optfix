//! The frozen 0.1.0 reference (`docs/PLAN.md` section 8): `tests/data/frozen_0_1.csv` holds
//! inputs and outputs of the published optfix 0.1.0 (`tools/frozen_0_1`, which depends on
//! `optfix = "=0.1.0"`), and this crate must reproduce every row bit for bit, results and
//! error variants alike: `div_*`, `from_ratio`, `from_scaled`, `sqrt_*`, `price`,
//! `price_and_greeks` and `implied_vol`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::disallowed_macros,
    missing_docs
)]

use optfix::{
    implied_vol, price, price_and_greeks, EuropeanOption, Fixed, IvInputs, MarketInputs,
    OptionKind, Price, PricingError, Rate, Rounding, Spot, Strike, Volatility, Years,
};

const FIXTURE: &str = include_str!("data/frozen_0_1.csv");

fn out(r: Result<Fixed, PricingError>) -> String {
    match r {
        Ok(v) => v.raw().to_string(),
        Err(e) => format!("E{}", e as u8),
    }
}

fn raw(s: &str) -> i128 {
    s.parse().unwrap()
}

fn fx(s: &str) -> Fixed {
    Fixed::from_raw(raw(s))
}

fn rounding(s: &str) -> Rounding {
    match s {
        "0" => Rounding::Down,
        "1" => Rounding::Up,
        _ => panic!("rounding {s}"),
    }
}

fn kind(s: &str) -> OptionKind {
    match s {
        "0" => OptionKind::Call,
        "1" => OptionKind::Put,
        _ => panic!("kind {s}"),
    }
}

/// What the current crate computes for one fixture row's inputs.
fn evaluate(op: &str, a: &[&str]) -> String {
    match op {
        "div_down" => out(fx(a[0]).div_down(fx(a[1]))),
        "div_up" => out(fx(a[0]).div_up(fx(a[1]))),
        "from_ratio" => out(Fixed::from_ratio(raw(a[0]), raw(a[1]), rounding(a[2]))),
        "from_scaled" => out(Fixed::from_scaled(
            raw(a[0]),
            a[1].parse().unwrap(),
            rounding(a[2]),
        )),
        "sqrt_down" => out(fx(a[0]).sqrt_down()),
        "sqrt_up" => out(fx(a[0]).sqrt_up()),
        "price" | "price_and_greeks" => {
            let o = EuropeanOption {
                kind: kind(a[0]),
                strike: Strike::try_new(fx(a[2])).unwrap(),
                expiry: Years::try_new(fx(a[3])).unwrap(),
            };
            let m = MarketInputs {
                spot: Spot::try_new(fx(a[1])).unwrap(),
                vol: Volatility::try_new(fx(a[4])).unwrap(),
                rate: Rate::try_new(fx(a[5])).unwrap(),
                carry: Rate::try_new(fx(a[6])).unwrap(),
            };
            if op == "price" {
                out(price(&o, &m).map(Price::get))
            } else {
                match price_and_greeks(&o, &m) {
                    Ok(p) => [
                        p.price.get(),
                        p.greeks.delta,
                        p.greeks.gamma,
                        p.greeks.vega,
                        p.greeks.theta,
                    ]
                    .iter()
                    .map(|v| v.raw().to_string())
                    .collect::<Vec<_>>()
                    .join(","),
                    Err(e) => format!("E{}", e as u8),
                }
            }
        }
        "implied_vol" => {
            let o = EuropeanOption {
                kind: kind(a[0]),
                strike: Strike::try_new(fx(a[2])).unwrap(),
                expiry: Years::try_new(fx(a[3])).unwrap(),
            };
            let m = IvInputs {
                spot: Spot::try_new(fx(a[1])).unwrap(),
                rate: Rate::try_new(fx(a[4])).unwrap(),
                carry: Rate::try_new(fx(a[5])).unwrap(),
            };
            let target = Price::try_new(fx(a[6])).unwrap();
            out(implied_vol(&o, &m, target).map(Volatility::get))
        }
        _ => panic!("unknown op {op}"),
    }
}

#[test]
fn test_every_fixture_row_matches_0_1_0() {
    let mut rows = 0usize;
    let mut mismatches = Vec::new();
    for line in FIXTURE.lines() {
        let (inputs, expected) = line.split_once(",=,").expect("row without ,=,");
        let mut fields = inputs.split(',');
        let op = fields.next().unwrap();
        let args: Vec<&str> = fields.collect();
        let got = evaluate(op, &args);
        if got != expected {
            mismatches.push(format!("{line}\n  now: {got}"));
        }
        rows += 1;
    }
    assert!(rows >= 14_000, "{rows} rows");
    assert!(
        mismatches.is_empty(),
        "{} of {rows} rows differ from 0.1.0, first:\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn test_fixture_covers_every_operation_and_outcome() {
    for op in [
        "div_down",
        "div_up",
        "from_ratio",
        "from_scaled",
        "sqrt_down",
        "sqrt_up",
        "price",
        "price_and_greeks",
        "implied_vol",
    ] {
        let rows: Vec<&str> = FIXTURE
            .lines()
            .filter(|l| l.starts_with(&format!("{op},")))
            .collect();
        assert!(rows.len() >= 1000, "{op}: {} rows", rows.len());
        assert!(rows.iter().any(|l| !l.contains(",=,E")), "{op}: no result");
    }
    for (op, code) in [
        ("div_down", "E0"),
        ("div_down", "E1"),
        ("sqrt_down", "E1"),
        ("price", "E1"),
        ("implied_vol", "E3"),
        ("implied_vol", "E4"),
    ] {
        let (head, tail) = (format!("{op},"), format!(",=,{code}"));
        assert!(
            FIXTURE
                .lines()
                .any(|l| l.starts_with(&head) && l.ends_with(&tail)),
            "{op}: no {code}"
        );
    }
}
