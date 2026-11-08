//! The fixture of milestone 23 (`docs/PLAN-phase2.md`), `tests/data/svi_g.csv` from
//! `tools/svi_fixture/gen_arb_fixture.py` (mpmath at 80 digits, values as `nint(v × 2^96)`):
//!
//! - at every node, `total_variance_bounds` encloses `w(k)` and `butterfly_g_bounds` encloses
//!   Durrleman's `g(k)`, so a node where `g` is above the interval's width passes and one where
//!   it is below minus the width fails; the nodes inside that band are counted;
//! - the slice of Gatheral and Jacquier (2014), example 3.1 (Vogt), fails `check_butterfly` on
//!   `[−2, 2]` at the first node where mpmath's `g` is negative, with `g < 0` on about
//!   `[0.65, 1.25]` and a minimum of about −0.033, and passes `check_wings`;
//! - the clean slices (SSVI slices under theorem 4.2 of the same paper, and symmetric slices
//!   with a small `b`) pass `check_butterfly` on `[−3, 3]` and `check_wings`.
//!
//! The 256-bit reference of `common/svi.rs` (`g_ref`) agrees with every mpmath `g` within two
//! units of `2^-96`, and the comparisons with mpmath allow one unit (`2^-32` ulp).

#![allow(
    clippy::unwrap_used,
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

use common::svi::{g_fixture, g_ref, params, GRow};
use common::{to_ref, P};
use num_bigint::BigInt;
use optfix::{
    butterfly_g_bounds, check_butterfly, check_wings, Fixed, PricingError, SviGrid, SviParams,
};

/// A raw Q64.64 value at `2^-96` resolution.
fn x96(raw: i128) -> BigInt {
    BigInt::from(raw) << 32
}

/// A Q.P reference value rounded to nearest at `2^-96`.
fn ref96(v: &BigInt) -> BigInt {
    (v + (BigInt::from(1) << (P - 97))) >> (P - 96)
}

fn grid(lo: i64, hi: i64, nodes: u32) -> SviGrid {
    SviGrid {
        k_min: Fixed::from_int(lo),
        k_max: Fixed::from_int(hi),
        nodes,
    }
}

fn slice(r: &GRow) -> SviParams {
    params(&r.p).unwrap_or_else(|| panic!("fixture slice refused: {:?}", r.p))
}

fn real96(v: &BigInt) -> f64 {
    v.to_string().parse::<f64>().unwrap() / 2f64.powi(96)
}

/// Checks one node; returns whether it is inside the band `|g| <= width`.
fn check_node(r: &GRow) -> bool {
    let s = slice(r);
    let k = Fixed::from_raw(r.k);
    let (w_lo, w_hi) = s.total_variance_bounds(k).unwrap();
    assert!(
        x96(w_lo.raw()) <= &r.w96 + 1,
        "w below its enclosure: {:?} {}",
        r.p,
        r.k
    );
    assert!(
        &r.w96 - 1 <= x96(w_hi.raw()),
        "w above its enclosure: {:?} {}",
        r.p,
        r.k
    );
    let Some(g) = &r.g96 else {
        assert_eq!(butterfly_g_bounds(&s, k), Err(PricingError::DomainError));
        return false;
    };
    let d = (ref96(&g_ref(&r.p, &to_ref(r.k)).unwrap()) - g)
        .magnitude()
        .clone();
    assert!(
        d <= BigInt::from(2).magnitude().clone(),
        "g_ref differs from mpmath: {:?} {}",
        r.p,
        r.k
    );
    let (lo, hi) = butterfly_g_bounds(&s, k).unwrap();
    assert!(
        x96(lo.raw()) <= g + 1,
        "g below its enclosure: {:?} {}",
        r.p,
        r.k
    );
    assert!(
        g - 1 <= x96(hi.raw()),
        "g above its enclosure: {:?} {}",
        r.p,
        r.k
    );
    let width = x96(hi.raw() - lo.raw());
    if g > &width {
        assert!(lo >= Fixed::ZERO);
    }
    if g < &-&width {
        assert!(hi < Fixed::ZERO);
    }
    g.magnitude() <= width.magnitude()
}

#[test]
fn test_g_fixture_enclosures_and_band() {
    let rows = g_fixture();
    let mut groups: Vec<(String, usize, usize, i128, i128)> = Vec::new();
    for r in &rows {
        let group = r.tag.trim_end_matches(char::is_numeric).to_string();
        let in_band = check_node(r);
        let s = slice(r);
        let k = Fixed::from_raw(r.k);
        let (w_lo, w_hi) = s.total_variance_bounds(k).unwrap();
        let gw = butterfly_g_bounds(&s, k).map_or(0, |(lo, hi)| hi.raw() - lo.raw());
        match groups.iter_mut().find(|g| g.0 == group) {
            Some(g) => {
                g.1 += 1;
                g.2 += usize::from(in_band);
                g.3 = g.3.max(w_hi.raw() - w_lo.raw());
                g.4 = g.4.max(gw);
            }
            None => groups.push((group, 1, usize::from(in_band), w_hi.raw() - w_lo.raw(), gw)),
        }
    }
    for (name, n, band, ww, gw) in &groups {
        println!(
            "{name}: {n} nodes, {band} inside the band; widest w {ww} ulps, widest g {gw} ulps"
        );
    }
    assert_eq!(groups.iter().map(|g| g.1).sum::<usize>(), rows.len());
}

#[test]
fn test_vogt_fails_where_mpmath_is_negative() {
    let rows: Vec<GRow> = g_fixture()
        .into_iter()
        .filter(|r| r.tag == "vogt")
        .collect();
    assert_eq!(rows.len(), 401);
    let s = slice(&rows[0]);
    assert!(check_wings(&s));
    assert!(s.min_total_variance() > Fixed::ZERO);
    let report = check_butterfly(&s, grid(-2, 2, 401)).unwrap();
    assert!(!report.ok);
    let negative: Vec<&GRow> = rows
        .iter()
        .filter(|r| r.g96.as_ref().is_some_and(|g| g < &BigInt::from(0)))
        .collect();
    let first = negative.first().unwrap();
    let last = negative.last().unwrap();
    assert_eq!(report.first_failure.unwrap().k.raw(), first.k);
    let (k0, k1) = (
        first.k as f64 / 2f64.powi(64),
        last.k as f64 / 2f64.powi(64),
    );
    assert!(
        (0.6..0.7).contains(&k0) && (1.2..1.3).contains(&k1),
        "{k0} {k1}"
    );
    let min = negative
        .iter()
        .map(|r| real96(r.g96.as_ref().unwrap()))
        .fold(f64::MAX, f64::min);
    assert!((-0.04..-0.03).contains(&min), "{min}");
    let bound = report.min.bound.raw() as f64 / 2f64.powi(64);
    assert!(
        (bound - min).abs() < 1e-12,
        "report min {bound} against mpmath {min}"
    );
    // Every node with g < 0 fails and every other passes: the band is empty here.
    for r in &rows {
        let (lo, _) = butterfly_g_bounds(&s, Fixed::from_raw(r.k)).unwrap();
        let neg = r.g96.as_ref().unwrap() < &BigInt::from(0);
        assert_eq!(lo < Fixed::ZERO, neg, "node {}", r.k);
    }
    println!(
        "vogt: g < 0 at {} nodes, k in [{k0:.4}, {k1:.4}], min g {min:.6}",
        negative.len()
    );
}

#[test]
fn test_clean_slices_pass() {
    let rows = g_fixture();
    let mut seen = 0;
    for i in 0.. {
        let tag = format!("clean{i}");
        let Some(r) = rows.iter().find(|r| r.tag == tag) else {
            break;
        };
        let s = slice(r);
        assert!(check_wings(&s), "{tag}");
        let report = check_butterfly(&s, grid(-3, 3, 601)).unwrap();
        assert!(report.ok, "{tag}: {report:?}");
        println!(
            "{tag}: min g lower bound {:.6}",
            report.min.bound.raw() as f64 / 2f64.powi(64)
        );
        seen += 1;
    }
    assert_eq!(seen, 6);
}
