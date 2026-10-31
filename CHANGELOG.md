# Changelog

All notable changes to `optfix` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows
[Semantic Versioning](https://semver.org/).

## [0.2.0] - unreleased

Milestones 8 to 14 of `docs/PLAN.md` (section 8).

### Added

- `price_strip` and `price_and_greeks_strip` with `StripLeg`: one expiry and market shared
  by a strip of strikes; each leg is bit-identical to `price` / `price_and_greeks`, and each
  extra leg costs about half a `price` call (two thirds of a `price_and_greeks` call). On an error, the earlier legs are written and the rest untouched.
- Black-76: `FuturesInputs` (`as_market()`: `S = F`, `q = r`), `price_black76` and
  `price_and_greeks_black76`, bit-identical to the Black-Scholes-Merton functions with `q = r`.
- `price_cash_or_nothing` and `price_asset_or_nothing`, within
  `BINARY_ERR_REL × notional + BINARY_ERR_ABS` (`2^-48`, 64 ulps; the notional is the payout or
  `S`). `T = 0` pays strictly in the money.
- CI on GitHub Actions with pinned toolchains, and a CU budget per bench row (`bench/budget.md`)
  enforced by `make test` and `make bench`.

### Changed

- `PricingError` is `#[non_exhaustive]` (the reason for 0.2.0): a `match` needs a wildcard arm.
  The `#[repr(u8)]` discriminants are unchanged, so `e as u8` mappings keep working.
- Division and square root run on native 64-bit digits: `div_down` about 3 times and `sqrt_down`
  about 4.8 times cheaper on SBF, with results bit-identical to 0.1.0 (checked against the
  published 0.1.0 kernel and outputs).
- Every 0.1.0 result is reproduced bit for bit (`tests/data/frozen_0_1.csv`); `price` and
  `price_and_greeks` cost slightly less than in 0.1.0.

## [0.1.0] - 2026-10-30

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
