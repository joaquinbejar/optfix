# optfix

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Fixed-point option pricing for Solana programs and off-chain Rust: `no_std`, no allocator, no
floating point, no `unsafe`, and no Solana dependency.

Black-Scholes-Merton prices, greeks and implied volatility of European options with a
continuous carry, in integer arithmetic, with `ln`, `exp` and the normal CDF built to fit a
program's compute budget. Every function has a published error bound, tested against
arbitrary-precision references, and a cost in compute units, measured in LiteSVM.

```toml
[dependencies]
optfix = "0.1"
```

## Example

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

## Numbers and rounding

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

## Error bounds

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

## Compute units

Generated from `docs/benchmarks.md` (`make bench`: LiteSVM, SBF release build, 1,000 random
inputs over the supported domain), net of the program's entrypoint and argument decoding;
the Black-Scholes and implied-volatility rows still include decoding six arguments.

<!-- generated:cu -->
| Function | Median CU | Max CU |
| --- | ---: | ---: |
| `Fixed::mul_down` | 277 | 290 |
| `Fixed::div_down` | 1597 | 1991 |
| `Fixed::sqrt_down` | 10927 | 13980 |
| `exp` | 316 | 337 |
| `ln` | 457 | 463 |
| `norm_cdf` | 1324 | 1384 |
| `norm_pdf` | 573 | 596 |
| `wide::mul_shr` | 317 | 333 |
| `price` | 9251 | 9577 |
| `price`, near the money | 11094 | 11475 |
| `price_and_greeks` | 14240 | 14641 |
| `price_and_greeks`, near the money | 16095 | 16540 |
| `implied_vol` | 27121 | 34706 |
| `implied_vol`, near the money | 32675 | 41954 |
| `implied_vol`, worst case (every step taken) | 41882 | 42828 |

`implied_vol` takes at most 3 steps.
<!-- /generated:cu -->

`price_and_greeks` computes the shared intermediates once and costs about a third of pricing
and computing each greek separately (the `Baseline` row of `docs/benchmarks.md`). Nothing in the pricer divides or takes a square root.

## Documentation

- [`docs/PLAN.md`](https://github.com/joaquinbejar/optfix/blob/main/docs/PLAN.md): design, decisions and their measurements.
- [`docs/precision.md`](https://github.com/joaquinbejar/optfix/blob/main/docs/precision.md): measured errors against 256-bit and mpmath
  references and against OptionStratLib.
- [`docs/benchmarks.md`](https://github.com/joaquinbejar/optfix/blob/main/docs/benchmarks.md): every CU row, and how to reproduce it.
- [`CHANGELOG.md`](https://github.com/joaquinbejar/optfix/blob/main/CHANGELOG.md).
- API documentation: [docs.rs/optfix](https://docs.rs/optfix).

## License

MIT.
