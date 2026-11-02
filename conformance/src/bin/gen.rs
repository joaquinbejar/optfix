//! Generates a conformance generation (`docs/PLAN-improvements.md`, milestone 17):
//!
//!     cargo run -p optfix-conformance --bin gen -- <generation> <commit> <out.json>
//!
//! Inputs come from the existing fixtures in `optfix/tests/data` (the frozen 0.1 rows for the
//! arithmetic, prices, greeks and implied volatility; the binary fixture for binaries), from
//! them derived (Black-76 from the price rows, strips from a price row's market with four
//! strikes around its own, multiplications from the division pairs), and from a seeded
//! sampler with edge sets for the kernels and the decimal text. Every result is the native
//! [`optfix_conformance::eval`]. At most [`PER_FUNCTION`] cases per function, evenly spaced.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::disallowed_macros,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use optfix_conformance::{case, Case, Vectors, FORMAT, SIGNATURES};
use std::collections::BTreeMap;

/// The most cases kept per function.
const PER_FUNCTION: usize = 1_000;

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../optfix/tests/data/");

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn i128(&mut self) -> i128 {
        (i128::from(self.next()) << 64 | i128::from(self.next())) >> (self.next() % 120)
    }
}

const ONE: i128 = 1 << 64;

fn s(v: impl ToString) -> String {
    v.to_string()
}

fn kind01(v: &str) -> &'static str {
    if v == "0" {
        "call"
    } else {
        "put"
    }
}

fn rounding01(v: &str) -> &'static str {
    if v == "0" {
        "down"
    } else {
        "up"
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, generation, commit, out] = args.as_slice() else {
        panic!("usage: gen <generation> <commit> <out.json>");
    };
    let mut inputs: BTreeMap<&str, Vec<Vec<String>>> = BTreeMap::new();
    let mut push = |f: &'static str, a: Vec<String>| inputs.entry(f).or_default().push(a);

    // The frozen 0.1 rows: `op,args...,=,result`.
    let frozen = std::fs::read_to_string(format!("{DATA}frozen_0_1.csv")).unwrap();
    for line in frozen.lines() {
        let (head, _) = line.split_once(",=,").unwrap();
        let mut it = head.split(',');
        let op = it.next().unwrap();
        let a: Vec<&str> = it.collect();
        match op {
            "div_down" | "div_up" => {
                push(
                    if op == "div_down" {
                        "div_down"
                    } else {
                        "div_up"
                    },
                    vec![s(a[0]), s(a[1])],
                );
                push(
                    if op == "div_down" {
                        "mul_down"
                    } else {
                        "mul_up"
                    },
                    vec![s(a[0]), s(a[1])],
                );
                let (x, y) = (a[0].parse::<i128>().unwrap(), a[1].parse::<i128>().unwrap());
                push(
                    "mul_shr",
                    vec![
                        s(x.unsigned_abs()),
                        s(y.unsigned_abs()),
                        s(64),
                        s(rounding01(if op == "div_down" { "0" } else { "1" })),
                    ],
                );
            }
            "from_ratio" => push("from_ratio", vec![s(a[0]), s(a[1]), s(rounding01(a[2]))]),
            "from_scaled" => {
                push("from_scaled", vec![s(a[0]), s(a[1]), s(rounding01(a[2]))]);
                push("to_scaled", vec![s(a[0]), s(a[1]), s(rounding01(a[2]))]);
            }
            "sqrt_down" => push("sqrt_down", vec![s(a[0])]),
            "sqrt_up" => push("sqrt_up", vec![s(a[0])]),
            "price" | "price_and_greeks" => {
                let k = kind01(a[0]);
                let mut v = vec![s(k)];
                v.extend(a[1..7].iter().map(s));
                push(
                    if op == "price" {
                        "price"
                    } else {
                        "price_and_greeks"
                    },
                    v.clone(),
                );
                if op == "price" {
                    // Black-76 with F = S and r.
                    let b = vec![s(k), s(a[1]), s(a[2]), s(a[3]), s(a[4]), s(a[5])];
                    push("price_black76", b.clone());
                    push("price_and_greeks_black76", b);
                    // A strip: this market, strikes K, 0.8 K, 1.25 K, 2 K, kinds alternating.
                    let kr: i128 = a[2].parse().unwrap();
                    let mut st = vec![s(a[3]), s(a[1]), s(a[4]), s(a[5]), s(a[6])];
                    let (kmin, kmax) = (
                        optfix::Strike::MIN.get().raw(),
                        optfix::Strike::MAX.get().raw(),
                    );
                    for (i, kk) in [kr, kr / 5 * 4, kr / 4 * 5, kr * 2]
                        .map(|x| x.clamp(kmin, kmax))
                        .iter()
                        .enumerate()
                    {
                        st.push(s(if i % 2 == 0 {
                            k
                        } else if k == "call" {
                            "put"
                        } else {
                            "call"
                        }));
                        st.push(s(kk));
                    }
                    push("price_strip", st.clone());
                    push("price_and_greeks_strip", st);
                }
            }
            "implied_vol" => {
                let mut v = vec![s(kind01(a[0]))];
                v.extend(a[1..7].iter().map(s));
                push("implied_vol", v);
            }
            _ => panic!("unknown frozen op {op}"),
        }
    }

    // The binary fixture: kind (0 call), S, K, T, σ, r, q, payout, then results.
    let binary = std::fs::read_to_string(format!("{DATA}binary_fixture.csv")).unwrap();
    for line in binary.lines() {
        let a: Vec<&str> = line.split(',').collect();
        let k = kind01(a[0]);
        let mut v = vec![s(k)];
        v.extend(a[1..7].iter().map(s));
        push("price_asset_or_nothing", v.clone());
        v.push(s(a[7]));
        push("price_cash_or_nothing", v);
    }

    // Kernels, decimal text and Years::from_seconds from a seeded sampler plus edges.
    let mut g = Rng(0x0C0F_0A11_0000_0017);
    let edges: [i128; 9] = [
        0,
        1,
        -1,
        ONE,
        -ONE,
        i128::MAX,
        i128::MIN,
        42 * ONE,
        -45 * ONE,
    ];
    for e in edges {
        for f in ["exp", "ln", "norm_cdf", "norm_pdf", "to_decimal"] {
            push(f, vec![s(e)]);
        }
    }
    for i in 0..PER_FUNCTION {
        let u =
            |g: &mut Rng, lo: i128, hi: i128| lo + (((g.next() >> 16) as i128 * (hi - lo)) >> 48);
        push("exp", vec![s(u(&mut g, -45 * ONE, 44 * ONE))]);
        push(
            "ln",
            vec![s(
                g.i128().unsigned_abs().min(i128::MAX as u128) as i128 >> (i % 3)
            )],
        );
        push("norm_cdf", vec![s(u(&mut g, -40 * ONE, 40 * ONE))]);
        push("norm_pdf", vec![s(u(&mut g, -40 * ONE, 40 * ONE))]);
        let raw = g.i128();
        push("to_decimal", vec![s(raw)]);
        let text = optfix::Fixed::from_raw(raw).to_string();
        // Exact texts, and texts cut or extended to be inexact.
        let cut = if text.len() > 12 {
            text[..text.len() - 3].to_string()
        } else {
            format!("{text}.1")
        };
        let cut = if cut.ends_with('.') {
            format!("{cut}5")
        } else {
            cut
        };
        let r = if i % 2 == 0 { "down" } else { "up" };
        push("from_decimal_str", vec![text, s(r)]);
        push("from_decimal_str", vec![cut, s(r)]);
        let secs = (g.next() % 946_080_062) as i64 - 1;
        push("years_from_seconds", vec![s(secs), s(r)]);
        push(
            "mul_shr",
            vec![
                s(g.i128().unsigned_abs()),
                s(g.i128().unsigned_abs()),
                s(g.next() % 260),
                s(r),
            ],
        );
    }
    for bad in [
        "",
        "-",
        "1.",
        ".5",
        "1e3",
        "9223372036854775808",
        "-9223372036854775808",
    ] {
        push("from_decimal_str", vec![s(bad), s("down")]);
    }

    // Evenly spaced, at most PER_FUNCTION per function, in SIGNATURES order.
    let mut cases: Vec<Case> = Vec::new();
    for (f, _) in SIGNATURES {
        let all = inputs
            .remove(f)
            .unwrap_or_else(|| panic!("no inputs for {f}"));
        let step = all.len().div_ceil(PER_FUNCTION).max(1);
        for a in all.into_iter().step_by(step) {
            cases.push(case(f, a).unwrap_or_else(|e| panic!("{f}: {e:?}")));
        }
    }
    assert!(
        inputs.is_empty(),
        "inputs without a signature: {:?}",
        inputs.keys()
    );

    let v = Vectors {
        format: FORMAT,
        generation: generation.parse().unwrap(),
        optfix: env!("CARGO_PKG_VERSION").into(),
        commit: commit.clone(),
        signatures: SIGNATURES
            .iter()
            .map(|(f, a)| (f.to_string(), a.iter().map(|x| x.to_string()).collect()))
            .collect(),
        cases,
    };
    // One case per line, so diffs and reviews stay readable.
    let mut text = String::from("{\n");
    text.push_str(&format!(
        "\"format\": {},\n\"generation\": {},\n",
        v.format, v.generation
    ));
    text.push_str(&format!(
        "\"optfix\": {},\n\"commit\": {},\n",
        serde_json::to_string(&v.optfix).unwrap(),
        serde_json::to_string(&v.commit).unwrap()
    ));
    text.push_str(&format!(
        "\"signatures\": {},\n\"cases\": [\n",
        serde_json::to_string(&v.signatures).unwrap()
    ));
    for (i, c) in v.cases.iter().enumerate() {
        let sep = if i + 1 == v.cases.len() { "" } else { "," };
        text.push_str(&format!("{}{sep}\n", serde_json::to_string(c).unwrap()));
    }
    text.push_str("]\n}\n");
    std::fs::write(out, text).unwrap();
    let mut per: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for c in &v.cases {
        let e = per.entry(&c.f).or_default();
        if c.e.is_some() {
            e.1 += 1
        } else {
            e.0 += 1
        }
    }
    for (f, (ok, err)) in per {
        println!("{f}: {ok} results, {err} errors");
    }
}
