//! The test reference in `common/` agrees with Python's correctly rounded `decimal` module
//! (`tools/reference.txt`, from `tools/gen_reference.py`) on every committed point, for each
//! function it serves: `exp`, `ln`, `N(x)` and `n(x)`.

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

use common::{exp_ref, ln_ref, norm_cdf_ref, norm_pdf_ref, to_ref, P};
use num_bigint::BigInt;

/// The reference agrees with Python's correctly rounded `decimal` on every committed point.
#[test]
fn test_reference_matches_python_decimal() {
    let table = include_str!("../../tools/reference.txt");
    let mut n = 0;
    for line in table
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (func, repr, x, want) = (f[0], f[1], f[2].parse::<i128>().unwrap(), f[3]);
        let want: BigInt = want.parse().unwrap(); // y × 2^160, rounded
        let xr = match repr {
            "bin" => to_ref(x),
            "dec" => (BigInt::from(x) << P) / BigInt::from(10i128.pow(18)),
            _ => panic!("repr {repr}"),
        };
        let got = match func {
            "exp" => exp_ref(&xr),
            "ln" => ln_ref(&xr),
            "ncdf" => norm_cdf_ref(&xr),
            "npdf" => norm_pdf_ref(&xr),
            _ => panic!("func {func}"),
        };
        let got = (got + (BigInt::from(1) << (P - 161))) >> (P - 160);
        let diff = (&got - &want).magnitude().clone();
        let tol = std::cmp::max(
            want.magnitude().clone(),
            BigInt::from(1u8).magnitude() << 160u32,
        ) >> 120u32;
        assert!(diff <= tol, "{line}: reference {got}");
        n += 1;
    }
    assert!(n >= 700, "only {n} reference points");
}
