# Changelog

All notable changes to `optfix` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows
[Semantic Versioning](https://semver.org/).

## [0.3.1] - 2026-11-07

Documentation and packaging only: no change to the code, the public API or any result (every
output is bit-identical to 0.3.0, and conformance generations 1 to 3 replay unchanged).

### Changed

- README: the npm badge and install line for `optfix-wasm`; the `optfix-wasm` README links its
  npm and crates.io pages.
- CI: `npm pack --dry-run` of the `optfix-wasm` package, and the downstream check pinned to
  varswap's optfix 0.3 build.

## [0.3.0] - 2026-11-02

Milestones 15 to 20 of `docs/PLAN-improvements.md`. The breaking changes are the two below,
both in milestone 19; every other result is bit-identical to 0.2 except `implied_vol`'s, which
may move within its round-trip envelope.

### Changed (breaking)

- `Greeks` gains `rho` (`dV/dr` per 1.00 of the rate): `K T Dr N(d2)` for a call and
  `−K T Dr N(−d2)` for a put; `price_and_greeks_black76` holds `F` fixed, so its rho is
  `−T × price`. Every other output is bit-identical to 0.2 (the frozen 0.1 fixture, the q = 0
  fixture and conformance generation 1 still match). Within `RHO_ERR_REL × T × max(S, K) +
  RHO_ERR_ABS` (`2^-44`, 2,048 ulps; measured `4.6 × 10^-18` over 1,000,000 points). It costs
  about 300 CU in `price_and_greeks` (14,203 to 14,497 net median) and as much per strip leg.
- `Greeks` and `Priced` are `#[non_exhaustive]`, so later fields are not breaking. Outside the
  crate they are no longer built with struct literals: start from `Greeks::ZERO` or
  `Priced::ZERO` (new) and assign fields, for example to fill the `out` slice of
  `price_and_greeks_strip`. Reading fields is unchanged.

### Changed

- `implied_vol` takes at most two steps (`IV_MAX_ITERATIONS` is 2, was 3). Its initial-guess
  table near the money is twice as fine in each direction (`tools/gen_iv_tables.py`), so the
  guess is within about 1.25% of the root wherever it answers (3.8% before) and two steps always
  suffice. Every target answers or fails exactly as before: per-point outcomes are identical over
  the 3,000,000-point grid and the mpmath and threshold fixtures. Answers may move within the
  round-trip envelope; the worst case drops from 42,951 to 33,585 CU net (about 22%), and the
  median is unchanged. Programs embedding optfix grow by about 16 KB (the table).

### Added

- `Fixed::from_decimal_str` (exact decimal text, any number of digits, explicit rounding) and
  `Display` for `Fixed` (the exact value, at most 64 fractional digits).
- `Fixed::from_bps`, `Rate::from_bps` and `Volatility::from_bps`: basis points with an explicit
  rounding (one basis point is not a binary fraction).
- `optfix-wasm` (new crate, also on npm): WebAssembly bindings for JavaScript, every value an
  exact `bigint` or decimal text, bit-identical to the Rust crate; out-of-range arguments throw
  `DomainError` instead of wrapping.
- Conformance vectors (`conformance/vectors-g*.json`, about 20,000 calls per generation with
  their exact results): generation 1 (0.2's results), 2 (rho, `from_bps`) and 3 (the new
  `implied_vol` bits). CI replays every generation natively, through the wasm package on Node.js,
  and a subset on SBF; older generations replay under the agreed changes.
- `examples/pricer-program`: a minimal Pinocchio program to copy, tested in LiteSVM (bit-for-bit
  equal to the host, at most 16,000 CU per pricing), and "Using optfix On Chain" in the README.
- Coverage-guided fuzzing (`fuzz/`, `make fuzz-smoke` in CI, a nightly run) against shared,
  independent contracts (price bounds and parity, exact discounts, the implied-volatility round
  trip and its error classification).
- CI: a downstream guard (varswap and metalbook built against the PR's optfix, failing on any
  CU growth), `cargo-semver-checks`, the wasm job and the fuzz smoke, all required; rows under
  1,000 CU get a 10-CU budget rounding.
- `make iv-profile`: the implied-volatility profile (seed error, steps, CU per step) behind the
  `iv-profile` feature (bench only, not public API).

### Fixed

- The decimal parser reports malformed text as `DomainError` before any `Overflow` (found by the
  decimal fuzz target), and its digit arithmetic is checked rather than wrapping.

## [0.2.1] - 2026-11-01

### Fixed

- CU regression from 0.2.0: `U256::shr` gained a second caller (the `sqrt_down` seed) and stopped
  being inlined into `wide::mul_shr`, so `mul_shr` cost 340 CU net instead of 317 and its callers
  paid the difference. `shr` is now `#[inline(always)]`: `MulShr` is back to 317, and `SqrtDown`
  drops from 2281 to 2218. Results are unchanged.

## [0.2.0] - 2026-10-31

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
