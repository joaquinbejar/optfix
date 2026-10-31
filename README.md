[![CI](https://github.com/joaquinbejar/optfix/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/joaquinbejar/optfix/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/optfix.svg)](https://crates.io/crates/optfix)
[![Docs.rs](https://docs.rs/optfix/badge.svg)](https://docs.rs/optfix)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://github.com/joaquinbejar/optfix/blob/main/LICENSE)
[![Stars](https://img.shields.io/github/stars/joaquinbejar/optfix.svg)](https://github.com/joaquinbejar/optfix/stargazers)
[![Issues](https://img.shields.io/github/issues/joaquinbejar/optfix.svg)](https://github.com/joaquinbejar/optfix/issues)
[![PRs](https://img.shields.io/github/issues-pr/joaquinbejar/optfix.svg)](https://github.com/joaquinbejar/optfix/pulls)
[![no_std](https://img.shields.io/badge/no__std-yes-green.svg)](https://github.com/joaquinbejar/optfix/blob/main/docs/PLAN.md)

## optfix v0.1.0: Fixed-Point Option Pricing for Solana

### Table of Contents
1. [Introduction](#introduction)
2. [Features](#features)
3. [Workspace Crates](#workspace-crates)
4. [Numbers and Rounding](#numbers-and-rounding)
5. [Setup Instructions](#setup-instructions)
6. [Usage Example](#usage-example)
7. [Error Bounds](#error-bounds)
8. [Compute Units](#compute-units)
9. [Testing and Benchmarks](#testing-and-benchmarks)
10. [Documentation](#documentation)
11. [Used by](#used-by)
12. [Contribution and Contact](#contribution-and-contact)

### Introduction

optfix is fixed-point option pricing for Solana programs and off-chain Rust: `no_std`, no
allocator, no floating point, no `unsafe`, and no Solana dependency. The same code runs inside
an SBF program and on a host, and gives the same result bit for bit.

It computes Black-Scholes-Merton prices, greeks and implied volatility of European options with
a continuous carry, in integer arithmetic, with `ln`, `exp` and the normal CDF built to fit a
program's compute budget. Every function has a published error bound, tested against
arbitrary-precision references and against OptionStratLib, and a cost in compute units,
measured in LiteSVM.

### Features

#### 1. **Fixed-point arithmetic**
- `Fixed`: binary Q64.64 (`raw / 2^64` in an `i128`), one ulp is `2^-64`.
- Checked arithmetic everywhere; nothing panics.
- Directed rounding: `mul_down`/`mul_up`, `div_down`/`div_up`, `sqrt_down`/`sqrt_up`, and a
  `Rounding` argument for `from_ratio`, `from_scaled` and `to_scaled` (token units).
- `wide::mul_shr`: a full 256-bit product shifted right with exact directed rounding.
- `FixedBytes`: an alignment-1 storage form for account data.

#### 2. **Elementary functions**
- `exp` (3 ulps) and `ln` (2 ulps).
- `norm_cdf` and `norm_pdf` (2 ulps; `norm_cdf` relative to `2^-56` on `[−8, −5]`).

#### 3. **Black-Scholes-Merton**
- European calls and puts with a risk-free rate `r` and a continuous carry `q`.
- `intrinsic`, `price`, and `price_and_greeks` (delta, gamma, vega, theta from one evaluation).
- `price_error_bound`, `Price::upper` and `Price::lower` to widen a result to a safe side.
- Published envelopes: `PRICE_ERR_REL`, `DELTA_ERR`, `GAMMA_ERR_REL`, `VEGA_ERR_REL`,
  `THETA_ERR_REL` and the absolute parts.

#### 4. **Implied volatility**
- `implied_vol` with `IvInputs`: a table-based initial guess and at most three third-order steps.
- Answers within `4 E / vega + 2^-40 σ` where vega is at least `2^28 E`; returns
  `IllConditioned` below that.

#### 5. **Type safety**
- Range-checked quantity newtypes `Spot`, `Strike`, `Volatility`, `Rate`, `Years` (with
  `Years::from_seconds`, 365-day years) and `Price`.
- `PricingError` (`#[repr(u8)]`): `Overflow`, `DomainError`, `NoConvergence`,
  `ArbitrageBound`, `IllConditioned`.

### Workspace Crates

| Crate | Purpose |
| --- | --- |
| `optfix` | The library; its only runtime dependency is `bytemuck`. |
| `bench-program` | A Pinocchio SBF program that exists only to measure compute units; nobody deploys it. |
| `bench` (`optfix-bench`) | Runs `bench-program` in LiteSVM and writes `docs/benchmarks.md`. |
| `lint-control` | A failing control proving the workspace lints fire. |

### Numbers and Rounding
`Fixed` is binary Q64.64 (`raw / 2^64`, an `i128`), chosen by measurement against a decimal
`10^18` representation (`docs/PLAN.md`, decision log). One ulp is `2^-64`, about
`5.4 × 10^-20`. Every arithmetic step is checked; nothing panics.

- **Primitive operations round in the direction you ask**: `mul_down`, `mul_up`, `div_down`,
  `div_up`, `sqrt_down`, `sqrt_up`, and a `Rounding` argument for conversions
  (`from_ratio`, `from_scaled`, `to_scaled` to token units, `Years::from_seconds`).
- **Compound functions** (`price`, the greeks, `implied_vol`) round to nearest within a
  published envelope. A caller that needs a conservative side widens the result with
  `price_error_bound`, `Price::upper` and `Price::lower`.
- **Inputs are range-checked newtypes** (`Spot`, `Strike`, `Volatility`, `Rate`, `Years`,
  `Price`), so a `Volatility` cannot be passed as a `Strike`. `FixedBytes` is the
  alignment-1 storage form for account data.

| Input | Supported range |
| --- | --- |
| `Spot`, `Strike` | `[10^-6, 10^9]` each, `S/K` in `[0.05, 20]` |
| `Volatility` | `[0.005, 5]` |
| `Rate` | `[−0.1, 0.5]`, for both the rate `r` and the carry `q` |
| `Years` | `[0, 30]` (365-day years) |

Errors are a `PricingError` (`#[repr(u8)]`): `Overflow`, `DomainError`, `NoConvergence`,
`ArbitrageBound`, `IllConditioned`.

### Setup Instructions

#### Prerequisites
- Rust (the toolchain is pinned in `rust-toolchain.toml`).
- For the benchmarks only: the Agave CLI with `cargo build-sbf`.

#### Install

Add optfix from crates.io:

```toml
[dependencies]
optfix = "0.1"
```

#### Building from Source

```shell
git clone https://github.com/joaquinbejar/optfix
cd optfix
make build
```

### Usage Example

Price a call, widen the price to a conservative ask, convert it to token units, and recover
the volatility from the price. This block is compiled and run by `optfix/tests/readme.rs`.

<!-- generated:example -->
```rust
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

fn main() {
    example().unwrap();
}
```
<!-- /generated:example -->

### Error Bounds


Generated from the crate's constants (the tests assert each one; measured maxima are in
`docs/precision.md`). `E` is the price bound at the inputs (`price_error_bound`).

<!-- generated:errors -->
| Function | Error bound |
| --- | --- |
| `mul_*`, `div_*`, `sqrt_*`, `from_ratio`, `to_scaled`, `Years::from_seconds` | exact, rounded in the direction asked (within 1 ulp, `2^-64`) |
| `wide::mul_shr` | exact, rounded in the direction asked (1 ulp) |
| `exp` | 3 ulps of `max(e^x, 1)` |
| `ln` | 2 ulps |
| `norm_cdf` | 2 ulps; relative `2^-56` on `[−8, −5]` beyond the final rounding |
| `norm_pdf` | 2 ulps |
| `price` | `2^-48` × `max(S, K)` + 64 ulps |
| delta | `2^-49` |
| gamma | `2^-40` × `Dq / (S σ√T)` + 64 ulps |
| vega | `2^-43` × `S Dq √T` + 64 ulps |
| theta | `2^-42` × `(S Dq σ / √T + abs(r) K Dr + abs(q) S Dq)` + 64 ulps |
| `implied_vol` | 4 × `E / vega` + `2^-40` × `σ`, where vega at the root is at least `2^28` × `E` (`E` the price bound); `IllConditioned` below that |
<!-- /generated:errors -->

### Compute Units

Generated from `docs/benchmarks.md` (`make bench`: LiteSVM, SBF release build, 1,000 random
inputs over the supported domain), net of the program's entrypoint and argument decoding;
the Black-Scholes and implied-volatility rows still include decoding six arguments.

<!-- generated:cu -->
| Function | Median CU | Max CU |
| --- | ---: | ---: |
| `Fixed::mul_down` | 277 | 290 |
| `Fixed::div_down` | 546 | 644 |
| `Fixed::sqrt_down` | 2281 | 2383 |
| `exp` | 316 | 337 |
| `ln` | 457 | 463 |
| `norm_cdf` | 1324 | 1384 |
| `norm_pdf` | 573 | 596 |
| `wide::mul_shr` | 340 | 355 |
| `price` | 9252 | 9577 |
| `price`, near the money | 11094 | 11475 |
| `price_and_greeks` | 14241 | 14641 |
| `price_and_greeks`, near the money | 16095 | 16540 |
| `implied_vol` | 27123 | 34706 |
| `implied_vol`, near the money | 32674 | 41954 |
| `implied_vol`, worst case (every step taken) | 41883 | 42828 |

`implied_vol` takes at most 3 steps.
<!-- /generated:cu -->

`price_and_greeks` computes the shared intermediates once and costs about a third of pricing
and computing each greek separately (the `Baseline` row of `docs/benchmarks.md`). Nothing in the pricer divides or takes a square root.

### Testing and Benchmarks

```shell
make test        # every test, including the million-case property tests
make bench       # build bench-program for SBF, run it in LiteSVM, regenerate docs/benchmarks.md
                 # (fails when a row is over its limit in bench/budget.md)
make precision   # measured exp and ln errors; regenerates docs/precision.md
make readme      # regenerate this README's example and tables
make pre-push    # fmt, clippy, lint controls, tests and docs, as required before a push
```

`OPTFIX_PROPTEST_CASES=<n>` lowers the property test case count. Every CU row has committed
limits in `bench/budget.md`; `make test` and `make bench` fail on a row over its limit, and a
PR that changes a row's cost updates its limit in the same diff. The README's example and
tables are generated: `make test` fails while they are stale.

### Documentation


- [`docs/PLAN.md`](https://github.com/joaquinbejar/optfix/blob/main/docs/PLAN.md): design, decisions and their measurements.
- [`docs/precision.md`](https://github.com/joaquinbejar/optfix/blob/main/docs/precision.md): measured errors against 256-bit and mpmath
  references and against OptionStratLib.
- [`docs/benchmarks.md`](https://github.com/joaquinbejar/optfix/blob/main/docs/benchmarks.md): every CU row, and how to reproduce it.
- [`CHANGELOG.md`](https://github.com/joaquinbejar/optfix/blob/main/CHANGELOG.md).
- API documentation: [docs.rs/optfix](https://docs.rs/optfix).

### Used by

Programs on Solana devnet that price with optfix:

| Program | Devnet program id |
|---------|-------------------|
| [quotebook](https://github.com/joaquinbejar/quotebook) | [`3Aq1AwFiK9ibNBLxvYQ18KLT9WbLwENQa3ZZViDW6mbo`](https://explorer.solana.com/address/3Aq1AwFiK9ibNBLxvYQ18KLT9WbLwENQa3ZZViDW6mbo?cluster=devnet) |
| [varswap](https://github.com/joaquinbejar/varswap) | [`BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9`](https://explorer.solana.com/address/BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9?cluster=devnet) |
| [metalbook](https://github.com/joaquinbejar/metalbook) | [`FPPuou5kgSp6tbwQRwKPBNhrMyf1Xb8n2ib3Yqpbsuwg`](https://explorer.solana.com/address/FPPuou5kgSp6tbwQRwKPBNhrMyf1Xb8n2ib3Yqpbsuwg?cluster=devnet) |

## Contribution and Contact

We welcome contributions to this project! If you would like to contribute, please follow these steps:

1. Fork the repository.
2. Create a new branch for your feature or bug fix.
3. Follow [`rules/global_rules.md`](https://github.com/joaquinbejar/optfix/blob/main/rules/global_rules.md) and run `make pre-push`.
4. Make your changes and ensure that the project still builds and all tests pass.
5. Commit your changes and push your branch to your forked repository.
6. Submit a pull request to the main repository.

If you have any questions, issues, or would like to provide feedback, please feel free to contact the project maintainer:


### **Contact Information**

- **Author**: Joaquín Béjar García <https://joaquinbejar.capitaldelta.co/>
- **Email**: jb@taunais.com
- **Telegram**: [@joaquin_bejar](https://t.me/joaquin_bejar)
- **Repository**: <https://github.com/joaquinbejar/optfix>
- **Documentation**: <https://github.com/joaquinbejar/optfix/tree/main/docs>

We appreciate your interest and look forward to your contributions!

## ✍️ License

Licensed under **MIT** license

<!-- related-projects:start -->
## Related projects

Repositories by the same author that this project depends on, and repositories that depend on it.

### Depends on

No runtime dependency by the same author. Used in development only:

| Repository | Description |
|------------|-------------|
| [OptionStratLib](https://github.com/joaquinbejar/OptionStratLib) · [crates.io](https://crates.io/crates/optionstratlib) | Dev-dependency only: the numerical oracle the property tests check optfix against. |

### Used by

| Repository | Description |
|------------|-------------|
| [quotebook](https://github.com/joaquinbejar/quotebook) | RFQ on Solana: maker quotes signed off-chain with ed25519, verified and settled atomically on-chain by a Pinocchio program. |
| [varswap](https://github.com/joaquinbejar/varswap) | Realized variance accumulated on-chain from Pyth prices, and variance swaps settled against it, on Solana. |
| [metalbook](https://github.com/joaquinbejar/metalbook) | Cash-settled European options on gold and silver (XAU, XAG) on Solana, settled in USDC from Pyth. |

<!-- related-projects:end -->
