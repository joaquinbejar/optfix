# optfix: plan

Status: approved by Joaquin on 2026-10-29. Changes after approval are recorded as decisions.

## 1. Goal and non-goals

### Goal

A fixed-point option pricing library that runs unchanged inside a Solana program and in
off-chain Rust:

- `no_std`, no allocator, no floating point, no `unsafe`, no Solana dependency.
- Every arithmetic step is checked; every rounding is explicit and its direction documented.
- Every public function has a documented maximum error against an `f64` reference, enforced by
  property tests that use OptionStratLib as the oracle.
- Every public function has a measured compute-unit cost on SBF, reported in
  `docs/benchmarks.md` like matchbook's.

Scope of v0.1: `exp`, `ln`, `sqrt`, the standard normal CDF `N(x)` and PDF `n(x)`, Black-Scholes
price and greeks (delta, gamma, vega, theta) for European calls and puts with a continuous
risk-free rate and a continuous carry `q` (Black-Scholes-Merton; oracle: OptionStratLib's
Garman-Kohlhagen), and implied volatility.

### Non-goals (v0.1)

- American options, barriers, binary payoffs, stochastic or local volatility.
- Discrete dividends (a continuous carry `q` is in scope, see below).
- Volatility surfaces or smile interpolation (consumers pass one `Volatility` per series).
- A Solana program that anyone deploys: `bench-program/` exists only to measure CU.
- Arbitrary precision or `f64` interop in the public API (`f64` appears only in tests and an
  optional `std`-gated conversion helper for off-chain callers).

## 2. Domain model

The representation (`Raw` below) is decided by milestone 3. Until then the sketches use `Raw =
i128` and a `SCALE` that is either `10^18` (decimal) or `2^64` (binary Q64.64). The public API
does not expose `SCALE` arithmetic: callers convert at the edges with `Fixed::from_ratio`,
`from_int` and `to_scaled(decimals)`.

```rust
/// Signed fixed-point number. Internal representation: `raw / SCALE`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug, Pod, Zeroable)]
#[repr(transparent)]
pub struct Fixed(i128);

/// Direction of a rounding. Every operation that can lose precision takes one, or has a
/// `_down` / `_up` suffix. "Down" means toward negative infinity, "up" toward positive
/// infinity; never toward zero, so sign does not flip the meaning.
pub enum Rounding { Down, Up }

impl Fixed {
    pub const ZERO: Self; pub const ONE: Self; pub const MAX: Self; pub const MIN: Self;
    pub const fn from_int(n: i64) -> Self;                                  // exact
    pub fn from_ratio(num: i128, den: i128, r: Rounding) -> Result<Self, PricingError>;
    pub fn to_scaled(self, decimals: u8, r: Rounding) -> Result<i128, PricingError>; // e.g. USDC 6
    pub fn checked_add(self, o: Self) -> Result<Self, PricingError>;        // exact or Overflow
    pub fn checked_sub(self, o: Self) -> Result<Self, PricingError>;
    pub fn checked_neg(self) -> Result<Self, PricingError>;                 // MIN has no negation
    pub fn mul_down(self, o: Self) -> Result<Self, PricingError>;           // 256-bit intermediate
    pub fn mul_up(self, o: Self) -> Result<Self, PricingError>;
    pub fn div_down(self, o: Self) -> Result<Self, PricingError>;           // o == 0 -> DomainError
    pub fn div_up(self, o: Self) -> Result<Self, PricingError>;
    pub fn sqrt_down(self) -> Result<Self, PricingError>;                   // self < 0 -> DomainError
    pub fn sqrt_up(self) -> Result<Self, PricingError>;
}
```

`Fixed` deliberately does not implement `Add`, `Mul`, etc.: an operator cannot carry a rounding
direction or return an error, and a panic on overflow is not acceptable on-chain.

Invariants of `Fixed`:

- `mul_down(a, b) <= exact(a * b) <= mul_up(a, b)` and `mul_up - mul_down <= 1 ulp`; same for
  `div_*` and `sqrt_*`. Property tests check both inequalities against a 256-bit exact product.
- No operation wraps. Overflow returns `PricingError::Overflow`.

### Quantity newtypes

Each wraps a `Fixed` and is constructed only through a validating constructor, so the type
proves its own invariant. There are no `From<Fixed>` impls between them: a `Volatility` can
never be passed as a `Strike`, and the compiler rejects `bs_price(vol, spot, ...)`.

```rust
pub struct Spot(Fixed);        // price of the underlying in quote units; range: domain table
pub struct Strike(Fixed);      // same unit as Spot; range: domain table
pub struct Price(Fixed);       // >= 0. Option premium, same unit as Spot.
pub struct Volatility(Fixed);  // annualized sigma; range: domain table
pub struct Rate(Fixed);        // continuously compounded annual rate; range: domain table
pub struct Years(Fixed);       // time to expiry, in [0, MAX_YEARS]; MAX_YEARS = 30.

impl Years {
    /// `seconds / 31_536_000` (365-day year) with the caller's rounding: the library does
    /// not know which side is conservative (rounding down understates time value, which is
    /// not conservative for a writer's margin). Negative seconds -> DomainError (expired
    /// options are priced by `intrinsic`, not by Black-Scholes).
    pub fn from_seconds(seconds: i64, r: Rounding) -> Result<Self, PricingError>;
}

pub enum OptionKind { Call, Put }   // #[repr(u8)], 0 = Call, 1 = Put

pub struct EuropeanOption {
    pub kind: OptionKind,
    pub strike: Strike,
    pub expiry: Years,     // time remaining, not an absolute timestamp
}

pub struct MarketInputs { pub spot: Spot, pub vol: Volatility, pub rate: Rate, pub carry: Rate }
// carry: continuous yield q (Black-Scholes-Merton); the lease rate for XAU/XAG, 0 for none.

pub struct Greeks {
    pub delta: Fixed,   // dV/dS, in [-1, 1]
    pub gamma: Fixed,   // d2V/dS2, >= 0, per unit of spot
    pub vega:  Fixed,   // dV/dsigma per 1.00 of vol (divide by 100 for "per vol point")
    pub theta: Fixed,   // dV/dt in calendar time per year (= -dV/dT), usually <= 0
}

pub struct Priced { pub price: Price, pub greeks: Greeks }

pub enum PricingError {
    /// An intermediate result does not fit the representation.
    Overflow,
    /// Input outside the function's domain (ln of a non-positive number, zero divisor,
    /// negative time, a newtype constructor's range check).
    DomainError,
    /// Implied volatility did not reach the tolerance within the iteration cap.
    NoConvergence,
    /// Target price outside the no-arbitrage bounds, so no implied volatility exists.
    ArbitrageBound,
}
```

`i128` is 16-aligned on x86_64 hosts since Rust 1.77 but account data is only guaranteed
8-aligned, so milestone 1 checks `align_of::<Fixed>()` on SBF and on the host and, if they
differ, provides a `FixedBytes([u8; 16])` storage form that downstream accounts use.
Measured in milestone 1: `align_of::<i128>()` is 8 on `sbpf-solana-solana` and
`sbpfv3-solana-solana` (platform-tools v1.57, rustc 1.95.0) and 16 on `aarch64-apple-darwin`
(rustc 1.99.1), so `FixedBytes` exists (alignment 1, little-endian).

`Fixed` exposes `raw()` / `from_raw()` for zero-copy storage (every `i128` is a valid
`Fixed`). The newtypes do not: reading one from storage goes through `try_from_fixed`, which
re-runs the range check.

`PricingError` is `#[repr(u8)]` so a program can map it into its own error codes.

### Public functions (v0.1)

```rust
pub fn exp(x: Fixed) -> Result<Fixed, PricingError>;
pub fn ln(x: Fixed) -> Result<Fixed, PricingError>;
pub fn norm_cdf(x: Fixed) -> Result<Fixed, PricingError>;     // N(x)
pub fn norm_pdf(x: Fixed) -> Result<Fixed, PricingError>;     // n(x)
pub fn intrinsic(opt: &EuropeanOption, spot: Spot) -> Price;
pub fn price(opt: &EuropeanOption, m: &MarketInputs) -> Result<Price, PricingError>;
pub fn price_and_greeks(opt: &EuropeanOption, m: &MarketInputs) -> Result<Priced, PricingError>;
pub fn implied_vol(opt: &EuropeanOption, spot: Spot, rate: Rate, target: Price)
    -> Result<Volatility, PricingError>;
```

`price_and_greeks` computes `d1`, `d2`, `N(d1)`, `N(d2)`, `n(d1)`, `sqrt(T)` and the discount
factor once and derives all four greeks from them. `price` is a thin wrapper that skips the
greeks.

### Rounding contract for compound functions

A library cannot know which side of a trade is "the protocol". So compound functions
(`price`, greeks, `implied_vol`) return the internal result rounded to nearest and publish a
bound `ERR_PRICE` etc. Callers that need a conservative value use
`Price::upper(err)` / `Price::lower(err)` helpers, which add or subtract the documented bound
with explicit rounding. (metalbook's collateral never depends on a price, so these helpers
serve marks and quoting there.) Primitive operations (`mul`,
`div`, `sqrt`, `from_seconds`, `to_scaled`) are directed exactly, as above.

## 3. Account layout and instructions

optfix itself has no accounts. `bench-program/` is a Pinocchio program with no state, one
instruction per public function, used only for CU measurement:

| Disc. | Instruction | Data | Accounts |
| --- | --- | --- | --- |
| 0 | `Noop` | none | none (baseline, subtracted from every row) |
| 1 | `MulDown` | 2 × `Fixed` | none |
| 2 | `DivDown` | 2 × `Fixed` | none |
| 3 | `SqrtDown` | `Fixed` | none |
| ... | one per public function (`Exp`, `Ln` next) | its arguments, little-endian | none |

Each handler decodes its arguments with `bytemuck`, calls optfix, and writes the result with
`set_return_data`, so the LiteSVM test also checks the on-chain result equals the host result
bit for bit. There are no account-validation rules because there are no accounts; the program
is never deployed to a public cluster.

### Supported domain and error envelope

The table is the single source of the ranges: `Spot` and `Strike` constructors check
`[10^-6, 10^9]`, the others their own row, and the two-input `S/K` constraint is checked by
`price`, `price_and_greeks` and `implied_vol`, which return `DomainError` (tested). Constructors
reject anything outside the **supported domain**, and the supported domain is
exactly what the tests cover (codex review: an envelope measured on a smaller grid is not a
bound for a wider accepted domain). Accepted 2026-10-29:

| Input | Supported range |
| --- | --- |
| `Spot`, `Strike` | `[10^-6, 10^9]` each, `S/K` in `[0.05, 20]` |
| `Volatility` | `[0.005, 5]` |
| `Rate` | `[-0.1, 0.5]` |
| `Years` | `[0, 30]`; the degenerate branch of 4.5 applies when `σ√T` is below its threshold (which covers `T = 0` and very small `T`) |

Error envelopes are input-dependent formulas, not one constant: for price,
`|err| <= ERR_REL × max(S, K) + ERR_ABS`, with both constants fixed explicitly in milestone 5
(`ERR_ABS` matters: at extreme moneyness, low `σ` and long `T` prices are about `10^-30 × S`),
including approximation, fixed-point propagation,
degenerate branches and input conversion. Tests sample every range, tiny `T`, the cutoff
transitions, the range extremes, and `S` and `K` scaled together.

## 4. Key design decisions

### 4.1 Representation: decimal `i128 × 10^18` vs binary Q64.64 (decided by measurement)

| | Decimal, scale `10^18` | Binary Q64.64 |
| --- | --- | --- |
| Integer range | about ±1.7 × 10^20 | about ±9.2 × 10^18 |
| Resolution | 10^-18 | 5.4 × 10^-20 |
| Token amounts and prices | exact conversion from 6/9 decimals | inexact, needs a rounding |
| `mul` / `div` | 256-bit product then divide by 10^18 (costly division) | 256-bit product then shift by 64 |
| `exp` range reduction `2^k` | multiply by a power of two (a division by 10^18 later) | a shift |
| `ln` range reduction to [1, 2) | find k with a comparison loop or a 10^18-aware log2 | leading-zero count |

Recommendation on paper: Q64.64 is probably cheaper and decimal is more convenient at the
edges. Per the brief, the decision is not taken here. Milestone 3 implements `exp` and `ln`
in both, behind a private `Repr` trait, and the representation that wins on
`CU × max error` for the two functions (weights in the open questions) becomes `Fixed`. The
loser is deleted, not kept behind a feature: one representation, one set of error bounds.

256-bit intermediates are implemented in-crate with only the operations needed (`mul_wide`,
`div_rem` by a `u128`, shifts). `u128 × u128` is not native on SBF (it lowers to
compiler-builtins `__multi3`), so the kernel matters: milestone 2 benchmarked the milestone 1
kernel (2 × `u128` limbs) against a 4 × `u64` kernel with a short-division path and kept the
latter (decision log). No `ruint` or
`primitive-types`: they pull in more than we need and their CU cost is unknown.

### 4.2 `exp`

`exp(x) = 2^k × exp(r)` with `k = round(x / ln 2)` and `r = x − k ln 2`, `|r| <= ln2 / 2`, then a
minimax polynomial (or a Padé `[n/n]`) for `exp(r)`. Coefficients are generated offline with
Sollya or a Remez script in `tools/` and committed as integer constants with the script that
made them, so they are reproducible. Overflow when `x` exceeds `ln(MAX)`; underflow returns
`ZERO` (not an error) below `ln(ulp)`.

### 4.3 `ln`

Range reduction `x = 2^k × m`, `m in [1, 2)`, then `ln(x) = k ln 2 + p(m)`. Options for `p`:
a minimax polynomial in `m − 1`, or in `s = (m − 1)/(m + 1)` (atanh series, odd and faster to
converge, costs one division). Milestone 3 measures both on the winning representation.

### 4.4 Normal CDF

`N(x) = erfc(−x / √2) / 2`. `erfc` uses a rational approximation in the style of W. J. Cody
(1969), split into ranges (`|x| < 0.5`, `0.5 to 4`, `> 4`) so the relative error stays small in
the tails, where an absolute-error approximation such as Abramowitz-Stegun 7.1.26
(`|ε| < 1.5 × 10^-7` absolute) is useless for deep out-of-the-money options. The cutoff
follows the fixed-point ulp, not `f64`: `N(x) < 10^-18` already at `x ≈ −8.8` (`≈ −9.1` for
Q64.64), so for `x < −X_CUT` return `ZERO` and for `x > X_CUT`, `ONE`, with `X_CUT` about 9
fixed by milestone 3's representation. This also bounds the tail range. Relative error is
only meaningful where `N(x)` spans many ulps (at `x = −8`, `N ≈ 6 × 10^-16` is about 600 ulps
at `10^-18`), so relative bounds are stated on `[−8, −5]` only, absolute bounds everywhere. The tail range uses
`exp(−x²/2)` from 4.2, so `N` inherits its cost.

Rejected: Abramowitz-Stegun (bad tails), Hart 5666 (fine, but Cody is better documented with
published error bounds), Marsaglia (series, iteration count varies with `x`).

### 4.5 Black-Scholes

`d1 = (ln(S/K) + (r + σ²/2)T) / (σ√T)`, `d2 = d1 − σ√T`. Degenerate cases handled before the
formula: `T == 0` returns intrinsic value with delta 0 or ±1, gamma and vega 0; `σ√T` below a
threshold likewise returns forward intrinsic: `max(S − K e^{−rT}, 0)` for a call,
`max(K e^{−rT} − S, 0)` for a put, with delta 0 or ±1. The out-of-the-money side, by forward moneyness (`S` vs `K e^{−rT}`, not `S` vs `K`, so that
`S − K e^{−rT}` has a fixed sign), is priced
directly and the in-the-money side via put-call parity (`C = P + S − K e^{−rT}`), a sum of
positive terms. Fixed-point errors are absolute, so the ITM formula's cancellation would not
amplify them; parity is used because it reuses the OTM evaluation, which has small values and
the tail-accurate `N`.

### 4.6 Implied volatility

1. Check bounds: `max(0, S − K e^{−rT}) < target < S` for calls (puts analogous), else
   `ArbitrageBound`. Strict inequalities: at the bounds, vol is 0 or infinite.
2. Initial guess: Brenner-Subrahmanyam `σ0 = √(2π/T) × C/S` near the money; Li (2005) rational
   approximation away from it. Milestone 6 compares both on a grid of moneyness and picks the
   switch point by measured iterations.
3. Up to 3 Halley iterations (Newton with the volga correction, which `price_and_greeks`
   almost gives for free), iterating in normalized price space (price over forward, or its
   log for deep OTM) rather than raw price, which converges far better for deep OTM and short
   `T`. Stop at tolerance; otherwise `NoConvergence`. A fixed cap keeps the CU bound fixed;
   milestone 6 measures the `NoConvergence` rate over the milestone 5 grid.

Possible later upgrade: Jäckel, "Let's be rational" (2015), which reaches machine precision in
two iterations everywhere. Not in v0.1: it relies on several rational approximations and
careful `f64` behavior that would need its own fixed-point error analysis.

### 4.7 Testing approach

- Unit tests for edge cases (zero, one, MIN/MAX, domain boundaries).
- Proptest for every public function: random inputs across the documented domain, compare
  against OptionStratLib (dev-dependency only) for pricing and greeks, and against `f64` /
  `libm` for `exp`, `ln`, `erfc`. Each test asserts the documented bound; the bound is a
  `pub const` next to the function, so docs, tests and README tables share one number.
- Exact-arithmetic tests for `mul_*`, `div_*`, `sqrt_*` against a 256-bit reference.
- OptionStratLib's API takes `Positive` inputs and returns `rust_decimal::Decimal`: `black_scholes(&Options)`, greeks `delta`/`gamma`/`vega`/`theta` in
  `optionstratlib-pricing/src/greeks/equations.rs`, `implied_volatility(Positive, &mut Options,
  max_iterations)`, `big_n` (CDF) and `n` (PDF). Its own approximation error, not its number
  type, limits what it can validate, so milestone 4 first measures OptionStratLib's `big_n`
  against an `mpmath` table committed under `tools/`, and optfix bounds are never claimed
  tighter than the oracle's measured error. `big_n` is `f64` grade, not `Decimal` grade: it
  converts to `f64` and calls `statrs::Normal::cdf` (`kernels.rs:385`), so the oracle is about
  `10^-16` relative and worse in the tails. `exp`, `ln`, `erfc` are checked against `libm` f64
  and the same `mpmath` tables.
- Unit mismatches with the oracle: OptionStratLib's `theta` is per day (divided by 365,
  `equations.rs:1373`) and `vega` is per 1% (divided by 100, `equations.rs:1525`). Tests
  rescale the oracle to optfix units (theta per year, vega per 1.00) explicitly, never by
  widening tolerances.

### 4.8 Workspace layout

Mirrors matchbook:

```
optfix/            no_std library, the only crate published to crates.io
bench-program/     Pinocchio program, one instruction per public function (publish = false)
bench/             LiteSVM tests that run bench-program and print the CU table (publish = false)
tools/             offline coefficient generation (Sollya or Python + mpmath), not a crate
docs/              PLAN.md, benchmarks.md, precision.md
```

Shared with matchbook: `edition = "2021"`, stable toolchain via `rust-toolchain.toml`, the same
`[workspace.lints]` (`unsafe_code`, `missing_docs`, `unwrap_used`, `expect_used`, `panic` denied;
`indexing_slicing` warn; plus `clippy::float_arithmetic = "deny"` in `optfix/`), release profile
with `overflow-checks = true`, and the same Makefile targets (`all`, `program`, `test`, `bench`,
`lint`, `fmt`, `fmt-check`, `pre-push`).

## 5. Milestones

Each milestone is one feature branch (`feat/m<N>-<name>`) and one PR.

1. **`Fixed` and property tests.** `Fixed`, `Rounding`, `U256`, the newtypes with their
   constructors, `Years::from_seconds`, `PricingError`, using provisional `SCALE = 10^18`.
   *Done when* `make pre-push` passes, proptests show `mul_down <= exact <= mul_up` with a gap
   of at most one ulp for `mul`, `div`, `sqrt`, and no operation panics over 1,000,000 random
   cases including extremes.
2. **CU bench harness with a trivial op.** `bench-program` with `Noop` and `MulDown` (both
   limb layouts), `bench` crate running it in LiteSVM, `docs/benchmarks.md` generated from the
   test output, `make bench`. First step: confirm LiteSVM 0.18 loads the sBPF v3 output of
   `cargo build-sbf` 4.4, else pin `--arch` in the Makefile. *Done when* `make bench` prints a
   table with the rows, the on-chain result of
   `MulDown` equals the host result for 1,000 random inputs, and the CU numbers are stable
   across runs.
3. **`ln` and `exp` in both representations, then the decision.** Both representations behind
   a private trait, both bench rows, both error tables. *Done when* `docs/benchmarks.md` and
   `docs/precision.md` contain CU and max error for `exp` and `ln` in both representations, a
   short decision record is added to this plan, the losing representation is deleted, and
   milestone 1's tests pass on the winner.
4. **`N(x)` and `n(x)`.** *Done when* both meet published bounds (absolute everywhere, relative
   on exactly `[−8, −5]`, plus saturation tests beyond the cutoff) verified by proptest, and their CU rows are in the benchmarks doc.
5. **Black-Scholes price and greeks.** `price`, `price_and_greeks`, `intrinsic`, degenerate
   cases. *Done when* proptest against OptionStratLib passes over the whole supported
   domain table (every range, extremes, tiny `T`, `S` and `K` scaled together), both kinds,
   with the documented envelopes, and `price_and_greeks` costs less than a benchmark-only baseline that
   computes `price` and each greek separately (not public API).
6. **Implied volatility.** *Done when* round-trip `implied_vol(price(σ)) ≈ σ` holds within the
   documented bound wherever vega exceeds a documented threshold, `ArbitrageBound` and
   `NoConvergence` are each covered by tests, the `NoConvergence` rate over the milestone 5 grid is
   reported, and the worst-case CU (all iterations) is in the table.
7. **README with error and CU tables, v0.1 on crates.io.** *Done when* README tables are
   generated from the same constants and bench output, `cargo publish --dry-run` passes, a
   CHANGELOG entry and signed tag exist, and (after your go-ahead) `optfix 0.1.0` is published.
   The name is free on crates.io as of 2026-10-28.

## Verified facts (checked 2026-10-28)

- crates.io: `pinocchio` 0.12.0 (published 2026-10-28; matchbook is on 0.11), `litesvm` 0.18.1,
  `mollusk-svm` 0.16.4, `bytemuck` 1.25.2, `proptest` 1.12.0, `solana-address` 2.9.0. Source:
  `https://crates.io/api/v1/crates/<name>`. Dependency requirements are written `MAJOR.MINOR`.
- `cargo build-sbf` 4.4.0 locally (platform-tools v1.57, rustc 1.95.0) defaults to `--arch v3`;
  latest platform-tools is v1.58 (GitHub releases API). sBPF v1, v2 and v3 deployment features
  are active on mainnet (feature accounts read via `getAccountInfo`); v0 to v2 deployment is
  not disabled yet.
- Crate name `optfix` is unclaimed on crates.io.
- OptionStratLib local workspace is 0.22.0 and is under a release hold; market-maker-rs pins
  `optionstratlib` 0.23. The oracle pin must be chosen with that in mind (question 6).
- Checked 2026-10-29 on the optfix machine: Agave CLI 4.4.0 (official installer,
  `release.anza.xyz/v4.4.0`; GitHub flags the v4.4.0 release as a prerelease) ships
  `cargo-build-sbf` 4.5.0 with platform-tools v1.57 and Rust 1.95.0, default `--arch v3`.
  LiteSVM 0.18.1 loads that sBPF v3 output, so the Makefile keeps `SBF_ARCH ?= v3`.

## Dependencies (latest stable on crates.io, checked 2026-10-29)

Rule from `rules/global_rules.md`: every dependency at the latest stable version, written
`MAJOR.MINOR`, re-checked via `https://crates.io/api/v1/crates/<name>` (`max_stable_version`)
whenever it is added or touched.

| Crate | Latest stable | Requirement |
| --- | --- | --- |
| `pinocchio` | 0.12.0 (0.11.2 used) | `0.11`, decided exception below |
| `bytemuck` | 1.25.2 | `1.25` |
| `proptest` (dev) | 1.12.0 | `1.12` |
| `litesvm` (dev, `bench/`) | 0.18.1 | `0.18` |
| `solana-address` (dev, `bench/`) | 2.9.0 (2.7.0 used) | `2.7`, approved exception below |
| `solana-instruction` (dev, `bench/`) | 4.0.0 (3.5.1 used) | `3.5`, approved exception below |
| `solana-keypair` (dev, `bench/`) | 4.0.0 (3.1.2 used) | `3.1`, approved exception below |
| `solana-signer` (dev, `bench/`) | 4.0.0 (3.0.1 used) | `3.0`, approved exception below |
| `solana-transaction` (dev, `bench/`) | 6.0.0 (4.3.0 used) | `4.3`, approved exception below |
| `mollusk-svm` (dev, optional) | 0.16.4 | not used |
| `optionstratlib` (dev, oracle) | 0.23.0 | `0.23` |
| `libm` (dev, reference) | 0.2.16 | `0.2` |

optfix needs only `pinocchio` itself (in `bench-program/`); by decision (Joaquin,
2026-10-29) it uses the same Pinocchio 0.11 generation as the other three repos and matchbook,
the documented exception to the latest-stable rule (see the other plans for the
`pinocchio-token` conflict that forces it). OptionStratLib 0.23.0 is
published on crates.io, so the oracle is a crates.io dev-dependency, not a git or path one.

`bench/` exception (approved by Joaquin, 2026-10-29): LiteSVM 0.18.1, the latest, pins older
generations of the Solana client crates (`solana-address ~2.7`, `solana-instruction ~3.5`,
`solana-keypair ^3.1`, `solana-signer ^3.0`, `solana-transaction ^4.1`), and Mollusk 0.16.4 is
on the same generation, so `bench/` uses those versions. They are dev-dependencies of a
`publish = false` crate; `optfix` and `bench-program` are unaffected. In addition, `Cargo.lock`
is seeded from LiteSVM's own v0.18.1 lock: LiteSVM's `^4.3` requirements now select the Agave
4.4.0 crates published 2026-10-28, which change APIs LiteSVM 0.18.1 calls and pin each other with
`=`, so the build fails unless the whole family stays at 4.3.2. Revisit when LiteSVM releases
against Agave 4.4.

## 6. Open questions

1. Decided (2026-10-29): price error `<= 10^-9 × max(S, K) + ERR_ABS`, delta below `10^-9`;
   supported domain table accepted as proposed.
2. CU budget: is there a target for `price_and_greeks` (for example 15,000 CU) that should
   fail the bench if exceeded?
3. Decision weights for milestone 3: CU only once errors meet the target, or a combined score?
4. Decided (2026-10-29): continuous carry `q` in v0.1.
5. Year basis: 365 days (proposed) or 365.25?
6. Decided (latest-stable rule): OptionStratLib 0.23 from crates.io as the oracle.
7. Decided: Pinocchio 0.11, like the other three repos (exception recorded above).
8. Decided (2026-10-29): `Fixed::raw`/`from_raw` yes; newtypes only via their validating
   constructor, named `try_new` as `rules/global_rules.md` requires (the plan's `try_from_fixed`).

## 7. Decision log

Milestone 1 (2026-10-29):

- `FixedBytes([u8; 16])` added: SBF and host alignments of `i128` differ (section 2).
- Quantity constructors are `try_new(Fixed) -> Result<Self, PricingError>` (the plan's
  `try_from_fixed`), with `MIN` / `MAX` constants equal to the domain table; there is no `Default`
  on the newtypes. `Price` accepts `[0, Fixed::MAX]`. The carry `q` uses `Rate` and its range.
- 256-bit division is Knuth's algorithm D with 64-bit digits (divisor normalized, two quotient
  digits, at most two corrections each); `sqrt` is integer Newton from a power of two above the
  root. Every limb operation is `checked_*`, as the rules require; milestone 2 measures what that
  costs on SBF.
- One `#[allow(clippy::arithmetic_side_effects)]` in production code: `Fixed::from_int`, a `const
  fn` returning `Self`, where `|n × 10^18| < 2^123` cannot overflow (proof in the comment, extremes
  tested).
- Internal invariants that the math excludes (algorithm D's correction bound, the Newton cap)
  return `PricingError::Overflow` rather than adding an `InvariantViolated` variant.

Milestone 2 (2026-10-29):

- **`U256` kernel: 4 × `u64` limbs.** Two complete kernels were compared, not two layouts of
  one algorithm. The milestone 1 kernel (2 × `u128` limbs) runs algorithm D for every divisor;
  the milestone 2 kernel (4 × `u64` limbs) adds a short-division path for one-digit divisors,
  which the `10^18` scale always takes. The gains below belong to the whole kernel and cannot be
  attributed to the limb layout alone; a 2 × `u128` kernel with the same short-division path was
  not measured. Measured at commit `cc48b4b` (`make bench` there reproduces it), 1,000 in-domain
  inputs, median CU with the `Noop` baseline of 164 CU included:

  | Operation | 2 × `u128` kernel (M1) | 4 × `u64` kernel (M2) | Change |
  | --- | ---: | ---: | ---: |
  | `mul_down`, divisor opaque | 1,895 | 1,702 | −10% |
  | `mul_down`, divisor constant | 1,889 | 1,696 | −10% |
  | `div_down` | 1,761 | 1,701 | −3% |
  | `sqrt_down` | 8,309 | 8,044 | −3% |

  The 4 × `u64` kernel is cheaper at the median for every operation and is kept. Its `sqrt_down`
  worst case is higher (14,040 against 11,809 CU), from the two-digit division in large Newton
  steps; milestone 5 can revisit if `sqrt` dominates the pricing cost. The milestone 1 kernel is
  deleted.
- **Checked arithmetic costs about 6% on `mul_down`.** The two-`u128`-limb kernel with
  `wrapping_*` limb operations measured 1,774 CU against 1,895 checked (−121 CU). The rules'
  `checked_*` requirement stays; the cost is small next to the division.
- **Release profile: `codegen-units = 1`, `lto = "fat"`.** With the default 16 codegen units the
  same production `mul_down` measured 1,710 CU or 1,384 CU depending on how the code was
  partitioned (inlining across codegen units). One codegen unit with fat LTO, as Solana programs
  are usually built, gives stable numbers: `MulDown` 1,383, `DivDown` 1,700, `SqrtDown` 8,043 CU
  (`docs/benchmarks.md`). A program built with another profile will measure differently.
- A constant divisor (`mul_down` divides by `10^18`) changed nothing measurable (−6 CU): the
  short division path already makes it one 64-bit digit.
- `bench-program` is `cdylib` only (a `lib` crate type rules out LTO in `cargo build-sbf`); the
  bench restates the instruction tags and return-data layout instead of linking the program, so
  the on-chain check does not reuse the program's encoder.
- `DivDown` and `SqrtDown` rows were added to the benchmark beyond the planned `MulDown`, because
  the layout decision applies to every `U256` operation.
