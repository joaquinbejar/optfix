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
//!
//! The SVI functions of generation 4 (`docs/PLAN-phase2.md`, milestone 25) take their inputs
//! from the fixtures of milestones 22 and 23 (`svi_w.csv`, `svi_e2e.csv`, `svi_g.csv`) and
//! from named cases (domain edges, failures partway through a strip, the Vogt slice's
//! butterfly report), which are always kept, ahead of the evenly spaced fixture cases. Nothing
//! there draws from the samplers above, so every earlier function keeps generation 3's inputs.

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

/// Raw SVI slice `a, b, ρ, m, σ`.
type Slice = [i128; 5];

/// The parsed rows of a fixture CSV whose fields are integers (`tag` columns excluded).
fn rows(name: &str, skip: usize) -> Vec<Vec<i128>> {
    std::fs::read_to_string(format!("{DATA}{name}"))
        .unwrap()
        .lines()
        .map(|l| {
            l.split(',')
                .skip(skip)
                .map(|v| if v == "-" { 0 } else { v.parse().unwrap() })
                .collect()
        })
        .collect()
}

fn with(p: &Slice, rest: &[i128]) -> Vec<String> {
    p.iter().chain(rest).map(s).collect()
}

/// Strikes of a strip around `k`, kinds alternating from `first`, as for `price_strip`.
fn strip_legs(first: &str, k: i128) -> Vec<String> {
    let (kmin, kmax) = (
        optfix::Strike::MIN.get().raw(),
        optfix::Strike::MAX.get().raw(),
    );
    let other = if first == "call" { "put" } else { "call" };
    let mut out = Vec::new();
    for (i, kk) in [k, k / 5 * 4, k / 4 * 5, k * 2]
        .map(|x| x.clamp(kmin, kmax))
        .iter()
        .enumerate()
    {
        out.push(s(if i % 2 == 0 { first } else { other }));
        out.push(s(kk));
    }
    out
}

/// Argument lists per function.
type Inputs = BTreeMap<&'static str, Vec<Vec<String>>>;

/// The SVI inputs of generation 4: `(named, fixture)`.
#[allow(clippy::too_many_lines)]
fn svi_inputs() -> (Inputs, Inputs) {
    let mut named = Inputs::new();
    let mut fixture = Inputs::new();
    let pct = |n: i128| ONE / 100 * n;
    let (sigma_min, years_max) = (18_446_744_073_709, optfix::Years::MAX.get().raw());

    // svi_w.csv: a, b, ρ, m, σ, k, then w(k).
    for r in rows("svi_w.csv", 0) {
        let p: Slice = [r[0], r[1], r[2], r[3], r[4]];
        let f = fixture.entry("svi_min_total_variance").or_default();
        f.push(with(&p, &[]));
        fixture
            .entry("svi_total_variance")
            .or_default()
            .push(with(&p, &[r[5]]));
        fixture
            .entry("svi_total_variance_bounds")
            .or_default()
            .push(with(&p, &[r[5]]));
    }

    // svi_e2e.csv: a, b, ρ, m, σ, S, K, T, r, q, kind (0 call), then the references.
    let e2e = rows("svi_e2e.csv", 0);
    for (i, r) in e2e.iter().enumerate() {
        let p: Slice = [r[0], r[1], r[2], r[3], r[4]];
        let [sp, k, t, rate, carry] = [r[5], r[6], r[7], r[8], r[9]];
        let kind = kind01(&r[10].to_string());
        let at = |p: &Slice| with(p, &[t, sp, rate, carry, k]);
        fixture
            .entry("log_forward_moneyness")
            .or_default()
            .push(vec![s(t), s(sp), s(rate), s(carry), s(k)]);
        fixture.entry("svi_vol").or_default().push(at(&p));
        fixture.entry("svi_vol_bounds").or_default().push(at(&p));
        let mut b = vec![s(kind)];
        b.extend([sp, k, t, rate, carry].map(s));
        fixture
            .entry("svi_price_bounds")
            .or_default()
            .push(with(&p, &[]).into_iter().chain(b).collect());
        let mut st = with(&p, &[t, sp, rate, carry]);
        st.extend(strip_legs(kind, k));
        fixture.entry("price_strip_svi").or_default().push(st);
        // A calendar pair: this slice raised by 0.01 at twice its expiry (it passes unless an
        // end is clamped), or the next row's slice and expiry (anything, a DomainError for a
        // later expiry that is not later).
        let (later, t_later) = if i % 2 == 0 {
            (
                [(p[0] + pct(1)).min(1000 * ONE), p[1], p[2], p[3], p[4]],
                (t * 2).min(years_max),
            )
        } else {
            let n = &e2e[(i + 1) % e2e.len()];
            ([n[0], n[1], n[2], n[3], n[4]], n[7])
        };
        let (k_lo, k_hi, nodes) = if i % 4 < 2 {
            (-ONE, ONE, 21)
        } else {
            (-3 * ONE, 3 * ONE, 61)
        };
        let mut c = with(&p, &[t]);
        c.extend(with(&later, &[t_later, k_lo, k_hi, nodes]));
        fixture.entry("check_calendar").or_default().push(c);
    }

    // svi_g.csv: tag, a, b, ρ, m, σ, k, then the references. The slices in file order, once
    // each, for check_wings and check_butterfly.
    let g_rows = rows("svi_g.csv", 1);
    let mut seen = std::collections::BTreeSet::new();
    let mut vogt: Option<Slice> = None;
    for r in &g_rows {
        let p: Slice = [r[0], r[1], r[2], r[3], r[4]];
        fixture
            .entry("butterfly_g_bounds")
            .or_default()
            .push(with(&p, &[r[5]]));
        if !seen.insert(p) {
            continue;
        }
        vogt.get_or_insert(p);
        fixture
            .entry("check_wings")
            .or_default()
            .push(with(&p, &[]));
        let grid = match seen.len() % 4 {
            0 => [-3 * ONE, 3 * ONE, 61],
            1 => [-ONE, ONE, 21],
            2 => [-32 * ONE, 32 * ONE, 65],
            _ => [r[5], r[5], 1],
        };
        fixture
            .entry("check_butterfly")
            .or_default()
            .push(with(&p, &grid));
    }

    // Named cases.
    let mut name = |f: &'static str, a: Vec<String>| named.entry(f).or_default().push(a);
    // The first svi_g.csv slice is Gatheral and Jacquier's example 3.1 (Vogt): it fails the
    // butterfly check on [−2, 2], where its g is negative on about [0.65, 1.25].
    let vogt = vogt.unwrap();
    name("check_butterfly", with(&vogt, &[-2 * ONE, 2 * ONE, 401]));
    name("check_wings", with(&vogt, &[]));
    // A steep left wing at T = 0.01 (the milestone 24 test): σ(K = 100) ≈ 2.83, but at K = 8
    // (S/K = 12.5, inside the moneyness range) σ ≈ 12 > 5.
    let steep: Slice = [pct(4), ONE / 10 * 4, -ONE / 2, 0, ONE / 10];
    let mkt = [pct(1), 100 * ONE, pct(4), pct(1)];
    let leg = |kind: &str, k: i128| [s(kind), s(k * ONE)];
    let strip = |p: &Slice, m: &[i128; 4], legs: &[[String; 2]]| -> Vec<String> {
        let mut a = with(p, m);
        a.extend(legs.iter().flatten().cloned());
        a
    };
    // The leg at K = 8 fails partway (DomainError from svi_vol).
    name(
        "price_strip_svi",
        strip(
            &steep,
            &mkt,
            &[
                leg("call", 100),
                leg("put", 95),
                leg("put", 8),
                leg("call", 105),
            ],
        ),
    );
    // The same strip without that leg prices in full.
    name(
        "price_strip_svi",
        strip(
            &steep,
            &mkt,
            &[leg("call", 100), leg("put", 95), leg("call", 105)],
        ),
    );
    // S/K = 0.04 at the third leg: DomainError from price's moneyness check.
    let calm: Slice = [pct(4), ONE / 10, -ONE / 10 * 3, 0, ONE / 10 * 2];
    let year = [ONE, 100 * ONE, pct(4), pct(1)];
    name(
        "price_strip_svi",
        strip(
            &calm,
            &year,
            &[leg("call", 100), leg("put", 90), leg("call", 2_500)],
        ),
    );
    // T = 0 (DomainError before any leg) and T = 2^-64 (w / T overflows at the first leg).
    name(
        "price_strip_svi",
        strip(&calm, &[0, 100 * ONE, pct(4), pct(1)], &[leg("call", 100)]),
    );
    let wide: Slice = [ONE, ONE / 10, 0, 0, ONE / 10];
    name(
        "price_strip_svi",
        strip(
            &wide,
            &[1, 100 * ONE, pct(4), pct(1)],
            &[leg("call", 100), leg("put", 90)],
        ),
    );
    for (p, t, k) in [
        (steep, pct(1), 8),   // σ > 5
        (steep, pct(1), 100), // σ ≈ 2.83
        (calm, 0, 100),       // T = 0
        (wide, 1, 100),       // Overflow
        // σ = sqrt(10^-8) = 10^-4 < 0.005
        ([ONE / 100_000_000, 0, 0, 0, ONE / 10], ONE, 100),
    ] {
        let a = with(&p, &[t, 100 * ONE, pct(4), pct(1), k * ONE]);
        name("svi_vol", a.clone());
        name("svi_vol_bounds", a);
    }
    for (kind, k) in [("call", 100), ("put", 2_500), ("call", 8)] {
        let a = with(&calm, &[0, 100 * ONE, k * ONE, ONE, pct(4), pct(1)]);
        let mut a = a;
        a[5] = s(kind);
        name("svi_price_bounds", a);
    }
    // The steep slice at T = 0.01 and K = 8: an end of the volatility enclosure above 5.
    {
        let mut a = with(&steep, &[0, 100 * ONE, 8 * ONE, pct(1), pct(4), pct(1)]);
        a[5] = s("put");
        name("svi_price_bounds", a);
    }
    // Domain edges of SviParams::try_new, each side of every bound.
    let base: Slice = [pct(4), ONE / 10 * 4, -ONE / 2, 0, ONE / 10];
    let mut edges: Vec<Slice> = vec![base];
    let mut set = |i: usize, v: i128| {
        let mut p = base;
        p[i] = v;
        edges.push(p);
    };
    for (i, v) in [
        (0, 1000 * ONE),
        (0, 1000 * ONE + 1),
        (0, -1000 * ONE),
        (1, 0),
        (1, -1),
        (1, 10 * ONE),
        (1, 10 * ONE + 1),
        (2, ONE - 1),
        (2, ONE),
        (2, -ONE + 1),
        (2, -ONE),
        (3, 10 * ONE),
        (3, 10 * ONE + 1),
        (3, -10 * ONE),
        (3, -10 * ONE - 1),
        (4, sigma_min),
        (4, sigma_min - 1),
        (4, 10 * ONE),
        (4, 10 * ONE + 1),
    ] {
        set(i, v);
    }
    // The minimum a + b σ sqrt(1 − ρ²) = a + 0.04 sqrt(0.75) ≈ a + 0.034641: just above and
    // just below 0.
    edges.push([-pct(3) - ONE / 10_000 * 46, base[1], base[2], 0, base[4]]);
    edges.push([-pct(3) - ONE / 10_000 * 47, base[1], base[2], 0, base[4]]);
    // The wing slope b (1 + |ρ|) each side of 2 (it cannot equal 2 in binary).
    let third = ONE / 3;
    edges.push([pct(4), ONE / 2 * 3, third - 1_000, 0, ONE / 10]);
    edges.push([pct(4), ONE / 2 * 3, -(third + 1), 0, ONE / 10]);
    for p in &edges {
        name("svi_min_total_variance", with(p, &[]));
        name("check_wings", with(p, &[]));
    }
    for k in [0, 32 * ONE, -32 * ONE, 32 * ONE + 1, -32 * ONE - 1] {
        name("svi_total_variance", with(&base, &[k]));
        name("svi_total_variance_bounds", with(&base, &[k]));
        name("butterfly_g_bounds", with(&base, &[k]));
    }
    // A slice whose w is 0 at k = m (a = 0, b σ sqrt(1 − ρ²) = 0 with b = 0): g undefined.
    name("butterfly_g_bounds", with(&[0, 0, 0, 0, ONE / 10], &[0]));
    // Grids: valid, one node, and each refusal.
    for grid in [
        [-ONE, ONE, 21],
        [ONE, ONE, 1],
        [-ONE, ONE, 0],
        [ONE, -ONE, 3],
        [-32 * ONE, 32 * ONE + 1, 3],
        [-32 * ONE - 1, 0, 3],
    ] {
        name("check_butterfly", with(&base, &grid));
        let mut c = with(&base, &[ONE]);
        c.extend(with(
            &[base[0] + pct(1), base[1], base[2], base[3], base[4]],
            &[2 * ONE],
        ));
        c.extend(grid.map(s));
        name("check_calendar", c);
    }
    // Equal slices fail (the bounds overlap); equal expiries and reversed ones are refused.
    for (te, tl) in [(ONE, 2 * ONE), (ONE, ONE), (2 * ONE, ONE)] {
        let mut c = with(&base, &[te]);
        c.extend(with(&base, &[tl, -ONE, ONE, 21]));
        name("check_calendar", c);
    }
    for (t, k) in [(ONE, 100), (0, 100), (years_max, 2_000), (pct(1), 8)] {
        name(
            "log_forward_moneyness",
            vec![s(t), s(100 * ONE), s(pct(4)), s(pct(1)), s(k * ONE)],
        );
    }
    (named, fixture)
}

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
    // Generation 2 (milestone 19): `from_bps` draws from its own stream, so the cases above
    // keep generation 1's inputs. Quoted rates and volatilities, then any `i64`.
    let mut bps = Rng(0x0C0F_0A11_0000_0B05);
    for b in [
        0,
        1,
        -1,
        25,
        50,
        -1_000,
        5_000,
        10_000,
        50_000,
        i64::MAX,
        i64::MIN,
    ] {
        push("from_bps", vec![s(b), s("down")]);
        push("from_bps", vec![s(b), s("up")]);
    }
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
        let quoted = (bps.next() % 60_001) as i64 - 10_000;
        let any = bps.next() as i64 >> (bps.next() % 64);
        push(
            "from_bps",
            vec![s(if i % 2 == 0 { quoted } else { any }), s(r)],
        );
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

    let (mut named, svi) = svi_inputs();
    for (f, a) in svi {
        inputs.entry(f).or_default().extend(a);
    }

    // The named cases, then the others evenly spaced, at most PER_FUNCTION per function, in
    // SIGNATURES order. Only the SVI functions have named cases, so every earlier function's
    // selection is generation 3's.
    let mut cases: Vec<Case> = Vec::new();
    for (f, _) in SIGNATURES {
        let first = named.remove(f).unwrap_or_default();
        let all = inputs
            .remove(f)
            .unwrap_or_else(|| panic!("no inputs for {f}"));
        let room = PER_FUNCTION - first.len();
        let step = all.len().div_ceil(room).max(1);
        for a in first.into_iter().chain(all.into_iter().step_by(step)) {
            cases.push(case(f, a).unwrap_or_else(|e| panic!("{f}: {e:?}")));
        }
    }
    assert!(
        named.is_empty(),
        "named cases without a signature: {:?}",
        named.keys()
    );
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
