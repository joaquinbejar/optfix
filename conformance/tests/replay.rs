//! Every committed generation `conformance/vectors-g<N>.json`, replayed natively: each case's
//! result, bit for bit, or its error code (`docs/PLAN-improvements.md`, milestone 17). A later
//! milestone that agrees a change to some outputs lists it in [`agreed_change`], which then
//! checks those cases its own way; every other case of every generation stays exact.

#![allow(clippy::unwrap_used, clippy::panic, clippy::disallowed_macros)]

use optfix_conformance::{eval, is_priced, load, priced_width, Case, FORMAT, SIGNATURES};

fn generations() -> Vec<(u32, String)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        if let Some(n) = name
            .strip_prefix("vectors-g")
            .and_then(|r| r.strip_suffix(".json"))
        {
            out.push((
                n.parse().unwrap(),
                std::fs::read_to_string(format!("{dir}{name}")).unwrap(),
            ));
        }
    }
    out.sort();
    out
}

/// A change agreed by a later milestone for `case` of generation `generation`: `Some(ok)` if
/// the case is checked here instead of exactly.
///
/// - Milestone 19 adds rho to every priced result: a generation-1 priced case (five values per
///   option) must equal today's result with rho dropped, so every other output stays exact.
fn agreed_change(generation: u32, case: &Case) -> Option<bool> {
    if !is_priced(&case.f) || priced_width(generation) == priced_width(u32::MAX) {
        return None;
    }
    let got = eval(&case.f, &case.a).ok()?;
    Some(match (got, &case.r, case.e) {
        (Ok(now), Some(want), None) => {
            let old = priced_width(generation);
            let kept: Vec<String> = now
                .chunks(priced_width(u32::MAX))
                .flat_map(|p| p.iter().take(old).cloned())
                .collect();
            &kept == want
        }
        (Err(e), None, Some(code)) => e as u8 == code,
        _ => false,
    })
}

#[test]
fn test_every_generation_replays_exactly() {
    let gens = generations();
    assert!(!gens.is_empty(), "no conformance generation");
    for (i, (n, text)) in gens.iter().enumerate() {
        assert_eq!(
            *n as usize,
            i + 1,
            "generations are numbered 1, 2, ... without gaps"
        );
        let v = load(text).unwrap();
        assert_eq!(v.format, FORMAT);
        assert_eq!(v.generation, *n);
        let mut mismatches = Vec::new();
        for c in &v.cases {
            if let Some(ok) = agreed_change(*n, c) {
                if !ok {
                    mismatches.push(format!("{c:?}\n  agreed change does not hold"));
                }
                continue;
            }
            let got = eval(&c.f, &c.a).unwrap_or_else(|e| panic!("g{n} malformed {c:?}: {e:?}"));
            let ok = match (&got, &c.r, c.e) {
                (Ok(r), Some(want), None) => r == want,
                (Err(e), None, Some(code)) => *e as u8 == code,
                _ => false,
            };
            if !ok {
                mismatches.push(format!("{c:?}\n  now: {got:?}"));
            }
        }
        assert!(
            mismatches.is_empty(),
            "g{n}: {} of {} cases differ, first:\n{}",
            mismatches.len(),
            v.cases.len(),
            mismatches
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

#[test]
fn test_latest_generation_covers_every_signature() {
    let gens = generations();
    let (_, text) = gens.last().unwrap();
    let v = load(text).unwrap();
    for (f, args) in SIGNATURES {
        let n = v.cases.iter().filter(|c| c.f == *f).count();
        assert!(n >= 100, "{f}: {n} cases");
        let sig = v.signatures.iter().find(|(g, _)| g == f).unwrap();
        assert_eq!(
            sig.1,
            args.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
            "{f}"
        );
    }
}
