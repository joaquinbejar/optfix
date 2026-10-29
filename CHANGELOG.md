# Changelog

All notable changes to `optfix` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows
[Semantic Versioning](https://semver.org/).

## [0.1.0] - unreleased

First release: milestones 1 to 6 of `docs/PLAN.md`.

### Added

- `Fixed`, binary Q64.64 (`raw / 2^64` in an `i128`), with checked arithmetic and directed
  rounding (`mul_down`/`mul_up`, `div_down`/`div_up`, `sqrt_down`/`sqrt_up`, `Rounding` for
  `from_ratio`, `from_scaled` and `to_scaled`), its 256-bit intermediates, and `FixedBytes`
  for alignment-1 account storage.
- Range-checked quantity newtypes `Spot`, `Strike`, `Volatility`, `Rate`, `Years` (with
  `Years::from_seconds`, 365-day years) and `Price`, over the supported domain of
  `docs/PLAN.md`.
- `exp` and `ln` (3 and 2 ulps), `norm_cdf` and `norm_pdf` (2 ulps; `norm_cdf` relative to
  `2^-56` on `[−8, −5]`).
- `wide::mul_shr`: a full 256-bit product shifted right with exact directed rounding.
- Black-Scholes-Merton with a continuous carry `q`: `intrinsic`, `price`, `price_and_greeks`
  (delta, gamma, vega, theta from one evaluation), `price_error_bound`, `Price::upper` and
  `Price::lower`, with published envelopes (`PRICE_ERR_REL`, `DELTA_ERR`, `GAMMA_ERR_REL`,
  `VEGA_ERR_REL`, `THETA_ERR_REL` and the absolute parts).
- `implied_vol` with `IvInputs`: a table-based initial guess and at most three
  third-order steps; answers within `4 E / vega + 2^-40 σ` where vega is at least `2^28 E`,
  and returns `IllConditioned` below that.
- `PricingError` (`#[repr(u8)]`): `Overflow`, `DomainError`, `NoConvergence`,
  `ArbitrageBound`, `IllConditioned`.
- Measured CU costs for every function (`docs/benchmarks.md`) and measured errors
  (`docs/precision.md`).
