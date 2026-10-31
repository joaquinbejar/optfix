//! Keeps `README.md` honest: its example is the code below (compiled and run here), and its
//! error and CU tables are generated from the crate's constants and from
//! `docs/benchmarks.md` (the `make bench` output). A stale README fails this test;
//! `OPTFIX_README_WRITE=1` (`make readme`) rewrites the generated sections in place.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::panic_in_result_fn,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::float_arithmetic,
    clippy::cast_precision_loss,
    clippy::disallowed_macros,
    missing_docs
)]

use std::fmt::Write as _;

use optfix::{
    DELTA_ERR, EXP_MAX_ERROR_ULPS, GAMMA_ERR_REL, GREEK_ERR_ABS, IV_ERR_PRICE_FACTOR, IV_ERR_REL,
    IV_MAX_ITERATIONS, IV_VEGA_MIN_FACTOR, LN_MAX_ERROR_ULPS, NORM_CDF_MAX_ERROR_ULPS,
    NORM_CDF_TAIL_REL_ERROR, NORM_PDF_MAX_ERROR_ULPS, PRICE_ERR_ABS, PRICE_ERR_REL, THETA_ERR_REL,
    VEGA_ERR_REL,
};

const README: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../README.md");
const BENCHMARKS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/benchmarks.md");

// README example start
use optfix::{
    implied_vol, price_and_greeks, price_error_bound, EuropeanOption, Fixed, IvInputs,
    MarketInputs, OptionKind, PricingError, Rate, Rounding, Spot, Strike, Volatility, Years,
};

fn example() -> Result<(), PricingError> {
    let percent = |n| Fixed::from_ratio(n, 100, Rounding::Down);
    let opt = EuropeanOption {
        kind: OptionKind::Call,
        strike: Strike::try_new(Fixed::from_int(100))?,
        expiry: Years::from_seconds(90 * 86_400, Rounding::Down)?,
    };
    let m = MarketInputs {
        spot: Spot::try_new(Fixed::from_int(105))?,
        vol: Volatility::try_new(percent(20)?)?,
        rate: Rate::try_new(percent(5)?)?,
        carry: Rate::try_new(Fixed::ZERO)?,
    };
    let priced = price_and_greeks(&opt, &m)?;
    // Results are rounded to nearest within a published envelope; widen for a safe side.
    let ask = priced.price.upper(price_error_bound(&opt, &m)?)?;
    // Back to token units (6 decimals), rounding explicitly.
    let ask_units = ask.get().to_scaled(6, Rounding::Up)?;
    assert_eq!(ask_units, 7_887_779);
    // And back from the price to the volatility.
    let im = IvInputs {
        spot: m.spot,
        rate: m.rate,
        carry: m.carry,
    };
    let sigma = implied_vol(&opt, &im, priced.price)?;
    assert!((sigma.get().raw() - m.vol.get().raw()).abs() < 1 << 30);
    Ok(())
}
// README example end

#[test]
fn test_readme_example_runs() {
    example().unwrap();
}

fn v(x: Fixed) -> f64 {
    x.raw() as f64 / 18_446_744_073_709_551_616.0
}

/// `2^k` for a power of two, else the value.
fn pow2(x: Fixed) -> String {
    let raw = x.raw();
    if raw > 0 && raw & (raw - 1) == 0 {
        format!("`2^{}`", i64::from(raw.trailing_zeros()) - 64)
    } else {
        format!("`{}`", v(x))
    }
}

fn errors_table() -> String {
    let mut s = String::new();
    s.push_str("| Function | Error bound |\n| --- | --- |\n");
    s.push_str(
        "| `mul_*`, `div_*`, `sqrt_*`, `from_ratio`, `to_scaled`, `Years::from_seconds` | exact, \
         rounded in the direction asked (within 1 ulp, `2^-64`) |\n",
    );
    writeln!(
        s,
        "| `wide::mul_shr` | exact, rounded in the direction asked ({} ulp) |",
        optfix::wide::MUL_SHR_MAX_ERROR_ULPS
    )
    .unwrap();
    writeln!(
        s,
        "| `exp` | {EXP_MAX_ERROR_ULPS} ulps of `max(e^x, 1)` |\n\
         | `ln` | {LN_MAX_ERROR_ULPS} ulps |\n\
         | `norm_cdf` | {NORM_CDF_MAX_ERROR_ULPS} ulps; relative {} on `[−8, −5]` beyond the final rounding |\n\
         | `norm_pdf` | {NORM_PDF_MAX_ERROR_ULPS} ulps |",
        pow2(NORM_CDF_TAIL_REL_ERROR)
    )
    .unwrap();
    writeln!(
        s,
        "| `price` | {} × `max(S, K)` + {} ulps |\n\
         | delta | {} |\n\
         | gamma | {} × `Dq / (S σ√T)` + {} ulps |\n\
         | vega | {} × `S Dq √T` + {} ulps |\n\
         | theta | {} × `(S Dq σ / √T + abs(r) K Dr + abs(q) S Dq)` + {} ulps |",
        pow2(PRICE_ERR_REL),
        PRICE_ERR_ABS.raw(),
        pow2(DELTA_ERR),
        pow2(GAMMA_ERR_REL),
        GREEK_ERR_ABS.raw(),
        pow2(VEGA_ERR_REL),
        GREEK_ERR_ABS.raw(),
        pow2(THETA_ERR_REL),
        GREEK_ERR_ABS.raw()
    )
    .unwrap();
    writeln!(
        s,
        "| `implied_vol` | {} × `E / vega` + {} × `σ`, where vega at the root is at least {} × `E` \
         (`E` the price bound); `IllConditioned` below that |",
        v(IV_ERR_PRICE_FACTOR),
        pow2(IV_ERR_REL),
        pow2(IV_VEGA_MIN_FACTOR)
    )
    .unwrap();
    s
}

/// `(median, max)` net of the `Noop` median for a row of `docs/benchmarks.md`.
fn cu(bench: &str, row: &str) -> (u64, u64) {
    let cells =
        |line: &str| -> Vec<String> { line.split('|').map(|c| c.trim().to_string()).collect() };
    let find = |name: &str| -> Vec<String> {
        bench
            .lines()
            .map(cells)
            .find(|c| c.len() > 5 && c[1] == format!("`{name}`"))
            .unwrap_or_else(|| panic!("no row {name} in docs/benchmarks.md"))
    };
    let noop: u64 = find("Noop")[3].parse().unwrap();
    let c = find(row);
    let median: u64 = c[3].parse().unwrap();
    let max: u64 = c[4].parse().unwrap();
    (median - noop, max - noop)
}

fn cu_table(bench: &str) -> String {
    let mut s = String::from("| Function | Median CU | Max CU |\n| --- | ---: | ---: |\n");
    for (name, row) in [
        ("`Fixed::mul_down`", "MulDown"),
        ("`Fixed::div_down`", "DivDown"),
        ("`Fixed::sqrt_down`", "SqrtDown"),
        ("`exp`", "Exp"),
        ("`ln`", "Ln"),
        ("`norm_cdf`", "NormCdf"),
        ("`norm_pdf`", "NormPdf"),
        ("`wide::mul_shr`", "MulShr"),
        ("`price`", "Price"),
        ("`price`, near the money", "Price, near the money"),
        ("`price_and_greeks`", "PriceAndGreeks"),
        (
            "`price_and_greeks`, near the money",
            "PriceAndGreeks, near the money",
        ),
        ("`implied_vol`", "ImpliedVol"),
        (
            "`implied_vol`, near the money",
            "ImpliedVol, near the money",
        ),
        (
            "`implied_vol`, worst case (every step taken)",
            "ImpliedVol, all iterations, near the money",
        ),
        ("`price_strip`, 1 leg", "PriceStrip, 1 leg"),
        ("`price_strip`, 16 legs", "PriceStrip, 16 legs"),
    ] {
        let (median, max) = cu(bench, row);
        writeln!(s, "| {name} | {median} | {max} |").unwrap();
    }
    // The greeks strip rows include the bench's output digest, measured on its own.
    for n in ["1 leg", "16 legs"] {
        let (median, max) = cu(bench, &format!("PriceAndGreeksStrip, {n}"));
        let (digest, _) = cu(bench, &format!("Digest, {n}"));
        writeln!(
            s,
            "| `price_and_greeks_strip`, {n} | {} | {} |",
            median - digest,
            max - digest
        )
        .unwrap();
    }
    writeln!(
        s,
        "\n`implied_vol` takes at most {IV_MAX_ITERATIONS} steps. A strip shares one expiry and \
         market: each extra leg costs about {} CU in `price_strip` and {} in \
         `price_and_greeks_strip` (medians, from 1 to 16 legs).",
        (cu(bench, "PriceStrip, 16 legs").0 - cu(bench, "PriceStrip, 1 leg").0) / 15,
        ((cu(bench, "PriceAndGreeksStrip, 16 legs").0 - cu(bench, "Digest, 16 legs").0)
            - (cu(bench, "PriceAndGreeksStrip, 1 leg").0 - cu(bench, "Digest, 1 leg").0))
            / 15,
    )
    .unwrap();
    s
}

/// Replaces the text between `<!-- {tag} -->` and `<!-- /{tag} -->`.
fn splice(doc: &str, tag: &str, body: &str) -> String {
    let (open, close) = (format!("<!-- {tag} -->\n"), format!("<!-- /{tag} -->"));
    let start = doc
        .find(&open)
        .unwrap_or_else(|| panic!("README has no {open}"))
        + open.len();
    let end = doc
        .find(&close)
        .unwrap_or_else(|| panic!("README has no {close}"));
    format!("{}{body}{}", &doc[..start], &doc[end..])
}

fn example_block() -> String {
    let src = include_str!("readme.rs");
    let start = src.find("// README example start\n").unwrap() + "// README example start\n".len();
    let end = src.find("// README example end").unwrap();
    format!(
        "```rust\n{}\nfn main() {{\n    example().unwrap();\n}}\n```\n",
        &src[start..end]
    )
}

#[test]
fn test_readme_is_generated() {
    let readme = std::fs::read_to_string(README).unwrap();
    let bench = std::fs::read_to_string(BENCHMARKS).unwrap();
    let fresh = splice(&readme, "generated:example", &example_block());
    let fresh = splice(&fresh, "generated:errors", &errors_table());
    let fresh = splice(&fresh, "generated:cu", &cu_table(&bench));
    if std::env::var("OPTFIX_README_WRITE").is_ok() {
        std::fs::write(README, &fresh).unwrap();
    } else {
        assert!(
            fresh == readme,
            "README.md is stale: run `make readme` (it regenerates the example and the tables)"
        );
    }
}

/// Failing control: a README whose generated section differs from the generator's output is
/// caught (the comparison above is not vacuous).
#[test]
fn test_a_stale_section_is_detected() {
    let readme = std::fs::read_to_string(README).unwrap();
    let stale = splice(&readme, "generated:errors", "| stale | table |\n");
    assert_ne!(splice(&stale, "generated:errors", &errors_table()), stale);
    assert_eq!(splice(&stale, "generated:errors", &errors_table()), readme);
}
