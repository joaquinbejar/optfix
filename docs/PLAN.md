# optfix: plan

Status: draft for review. Nothing here is implemented yet.

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
risk-free rate, and implied volatility.

### Non-goals (v0.1)

- American options, barriers, binary payoffs, stochastic or local volatility.
- Dividends or a continuous carry `q` (left as an open question: metals have a lease rate, see
  section 6).
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
pub struct Spot(Fixed);        // > 0. Price of the underlying in quote units.
pub struct Strike(Fixed);      // > 0. Same unit as Spot.
pub struct Price(Fixed);       // >= 0. Option premium, same unit as Spot.
pub struct Volatility(Fixed);  // annualized sigma, in (0, MAX_VOL]; MAX_VOL = 10.0 (1000%).
pub struct Rate(Fixed);        // continuously compounded annual rate, in [-1.0, 1.0].
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

pub struct MarketInputs { pub spot: Spot, pub vol: Volatility, pub rate: Rate }

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
with explicit rounding. metalbook, for instance, uses the upper bound for a writer's margin
requirement and the lower bound for what a long is credited. Primitive operations (`mul`,
`div`, `sqrt`, `from_seconds`, `to_scaled`) are directed exactly, as above.

## 3. Account layout and instructions

optfix itself has no accounts. `bench-program/` is a Pinocchio program with no state, one
instruction per public function, used only for CU measurement:

| Disc. | Instruction | Data | Accounts |
| --- | --- | --- | --- |
| 0 | `Noop` | none | none (baseline, subtracted from every row) |
| 1 | `MulDown` | 2 × `Fixed` | none |
| 2 | `Exp` | `Fixed` | none |
| 3 | `Ln` | `Fixed` | none |
| ... | one per public function | its arguments, little-endian | none |

Each handler decodes its arguments with `bytemuck`, calls optfix, and writes the result with
`set_return_data`, so the LiteSVM test also checks the on-chain result equals the host result
bit for bit. There are no account-validation rules because there are no accounts; the program
is never deployed to a public cluster.

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
compiler-builtins `__multi3`), so limb width matters: milestone 2 benchmarks `MulDown` with
2 × `u128` limbs and with 4 × `u64` limbs and keeps the cheaper one. No `ruint` or
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
`max(K e^{−rT} − S, 0)` for a put, with delta 0 or ±1. The out-of-the-money side is priced
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
   for `x <= −5`) verified by proptest, and their CU rows are in the benchmarks doc.
5. **Black-Scholes price and greeks.** `price`, `price_and_greeks`, `intrinsic`, degenerate
   cases. *Done when* proptest against OptionStratLib passes over the grid `S/K in [0.2, 5]`,
   `σ in [0.01, 3]`, `T in [1 day, 5 years]`, `r in [-0.05, 0.2]`, both kinds, with the
   documented bounds, and `price_and_greeks` costs less than `price` plus four separate greek
   calls (measured).
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

## 6. Open questions

1. Precision target: what is "good enough" for metalbook margin and marks? Proposal: price
   absolute error below `10^-9 × S`, delta below `10^-9`. This drives polynomial degrees and
   so CU.
2. CU budget: is there a target for `price_and_greeks` (for example 15,000 CU) that should
   fail the bench if exceeded?
3. Decision weights for milestone 3: CU only once errors meet the target, or a combined score?
4. Carry rate `q`: gold and silver have a lease rate and futures-implied carry. Add
   Black-Scholes-Merton `q` in v0.1 (cheap now, breaking later) or leave it to v0.2?
   OptionStratLib already has Garman-Kohlhagen greeks, which would serve as the oracle.
5. Year basis: 365 days (proposed) or 365.25?
6. Should OptionStratLib be a git or crates.io dev-dependency? A crates.io pin keeps the
   oracle stable while you change OptionStratLib; note the 0.22 release hold.
7. Pinocchio 0.12 came out today. Start `bench-program` on 0.12 (new repo, no migration
   later) or on 0.11 to match matchbook? Proposal: 0.12.
8. Resolved in review: `Fixed::raw`/`from_raw` yes; newtypes only via `try_from_fixed`.
