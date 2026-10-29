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

The representation is binary Q64.64: `Fixed(i128)` with value `raw / 2^64`, decided by
measurement in milestone 3 (decision log). Resolution is `2^-64` (about `5.4 × 10^-20`) and the
range about `±9.2 × 10^18`. The public API does not expose `SCALE` arithmetic: callers convert
at the edges with `Fixed::from_ratio`,
`from_int` and `to_scaled(decimals)`.

```rust
/// Signed fixed-point number. Internal representation: `raw / 2^64` (Q64.64).
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
    pub fn from_scaled(amount: i128, decimals: u8, r: Rounding) -> Result<Self, PricingError>;
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

Token-amount boundary (Q64.64, milestone 3): `10^-d` is not a dyadic fraction, so amounts are
not exact in `Fixed`. `from_scaled(amount, d, r)` converts in and `to_scaled(d, r)` converts
out, each with an explicit direction that the caller chooses toward the protocol. For
`d <= 19` one ulp is below one token unit (`10^19 × 2^-64 ≈ 0.54`), so rounding down one way
and up the other returns the amount exactly (property-tested); `from_scaled` accepts
`d <= 38` and `to_scaled` `d <= 76`.

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

/// Inputs of `implied_vol`: `MarketInputs` without the volatility being solved for. Named
/// fields, because `rate` and `carry` share the `Rate` type and positional arguments could be
/// swapped without a compile error.
pub struct IvInputs { pub spot: Spot, pub rate: Rate, pub carry: Rate }

/// Bounds below use `Dq = e^{−qT}`; over the domain table `Dq` is in `[e^{−15}, e^{3}]`
/// (`q = 0.5, T = 30` and `q = −0.1, T = 30`), so `|delta|` can reach about 20.1.
pub struct Greeks {
    pub delta: Fixed,   // dV/dS: call in [0, Dq], put in [−Dq, 0]
    pub gamma: Fixed,   // d2V/dS2, >= 0, per unit of spot
    pub vega:  Fixed,   // dV/dsigma per 1.00 of vol (divide by 100 for "per vol point"),
                        // >= 0, at most S × Dq × √T / √(2π)
    pub theta: Fixed,   // dV/dt in calendar time per year (= -dV/dT); sign not fixed: deep
                        // in the money it is about q S Dq − r K e^{−rT} for a call and the
                        // negative of that for a put, either of which can be > 0
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
    /// Implied volatility: vega at the root below `IV_VEGA_MIN_FACTOR` times the price
    /// envelope, so the target does not determine `σ` (added in milestone 6).
    IllConditioned,
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
pub fn implied_vol(opt: &EuropeanOption, m: &IvInputs, target: Price)
    -> Result<Volatility, PricingError>;
```

`optfix::wide::mul_shr(a: u128, b: u128, shift: u32, r: Rounding) -> Result<u128, PricingError>`
(added 2026-10-29 for varswap): the full 256-bit product shifted right with exact directed
rounding (`MUL_SHR_MAX_ERROR_ULPS = 1`, the floor or the ceiling), `Overflow` when the rounded
result does not fit; bench row `MulShr` (tag 8).

`price_and_greeks` computes `d1`, `d2`, `N(d1)`, `N(d2)`, `n(d1)`, `sqrt(T)` and the two
discount factors `e^{−rT}` and `e^{−qT}` once and derives all four greeks from them. `price` is a thin wrapper that skips the
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
| 4 | `Exp` | `Fixed` | none |
| 5 | `Ln` | `Fixed` | none |
| 6 | `NormCdf` | `Fixed` | none |
| 7 | `NormPdf` | `Fixed` | none |
| 8 | `MulShr` | 2 × `u128`, `u32` shift, rounding byte | none |
| 9 | `Price` | kind byte, 6 × `Fixed` (S, K, T, σ, r, q) | none |
| 10 | `PriceAndGreeks` | as `Price`; returns 5 values | none |
| 11 | `Baseline` | as `PriceAndGreeks`: the price and each greek computed separately (bench-only) | none |
| 12 | `ImpliedVol` | kind byte, 6 × `Fixed` (S, K, T, target, r, q); returns `σ` | none |
| 13 | `ImpliedVolAllIterations` | as `ImpliedVol`, every step taken (bench-only worst case) | none |

Each handler decodes its arguments with `bytemuck`, calls optfix, and writes the result with
`set_return_data`, so the LiteSVM test also checks the on-chain result equals the host result
bit for bit. There are no account-validation rules because there are no accounts; the program
is never deployed to a public cluster.

### Supported domain and error envelope

The table is the single source of the ranges: `Spot` and `Strike` constructors check
`[10^-6, 10^9]`, the others their own row (bounds Q64.64 cannot hold exactly, `10^-6`, `0.005`
and `−0.1`, are rounded outward, so either rounding of a boundary constant is accepted), and the two-input `S/K` constraint is checked by
`price`, `price_and_greeks` and `implied_vol`, which return `DomainError` (tested). Constructors
reject anything outside the **supported domain**, and the supported domain is
exactly what the tests cover (codex review: an envelope measured on a smaller grid is not a
bound for a wider accepted domain). Accepted 2026-10-29:

| Input | Supported range |
| --- | --- |
| `Spot`, `Strike` | `[10^-6, 10^9]` each, `S/K` in `[0.05, 20]` |
| `Volatility` | `[0.005, 5]` |
| `Rate` | `[-0.1, 0.5]`, for both `rate` (`r`) and `carry` (`q`); so `e^{−qT}` is in `[e^{−15}, e^{3}]` and the forward factor `e^{(r−q)T}` in `[e^{−18}, e^{18}]` |
| `Years` | `[0, 30]`; the degenerate branch of 4.5 applies when `σ√T` is below its threshold (which covers `T = 0` and very small `T`) |

Range: `Fixed::MAX` is about `9.2 × 10^18` (Q64.64). The largest values the table produces
fit with room to spare: `S` and `K` up to `10^9`, the forward `S e^{(r−q)T}` up to
`10^9 × e^{18} ≈ 6.6 × 10^16`, prices and `S Dq` up to `10^9 × e^3 ≈ 2 × 10^10`; milestone 5
checks the greeks' intermediates (gamma's `1 / (S σ √T)`) against it explicitly.

Error envelopes are input-dependent formulas, not one constant: for price,
`|err| <= ERR_REL × max(S, K) + ERR_ABS`, fixed in milestone 5 at `PRICE_ERR_REL = 2^-48`
(`3.6 × 10^-15`, measured `2.4 × 10^-18`) and `PRICE_ERR_ABS` = 64 ulps, well inside the
decided `10^-9`; both constants were to be fixed explicitly in milestone 5
(`ERR_ABS` matters: at extreme moneyness, low `σ` and long `T` prices are about `10^-30 × S`),
including approximation, fixed-point propagation,
degenerate branches and input conversion. Tests sample every range, tiny `T`, the cutoff
transitions, the range extremes, and `S` and `K` scaled together.

## 4. Key design decisions

### 4.1 Representation: decimal `i128 × 10^18` vs binary Q64.64 (decided: Q64.64)

Decided in milestone 3 by the rule of question 3: Q64.64, at 797 CU for `exp` + `ln` against
932 for the best decimal form (decision log). The comparison as planned:

| | Decimal, scale `10^18` | Binary Q64.64 |
| --- | --- | --- |
| Integer range | about ±1.7 × 10^20 | about ±9.2 × 10^18 |
| Resolution | 10^-18 | 5.4 × 10^-20 |
| Token amounts and prices | exact conversion from 6/9 decimals | inexact, needs a rounding |
| `mul` / `div` | 256-bit product then divide by 10^18 (costly division) | 256-bit product then shift by 64 |
| `exp` range reduction `2^k` | multiply by a power of two (a division by 10^18 later) | a shift |
| `ln` range reduction to [1, 2) | find k with a comparison loop or a 10^18-aware log2 | leading-zero count |

Recommendation on paper was: Q64.64 is probably cheaper and decimal is more convenient at
the edges. Milestone 3 implemented `exp` and `ln` in both, behind a `Repr` trait, and the
representation that won under the rule of question 3 became `Fixed`. The loser is deleted,
not kept behind a feature: one representation, one set of error bounds.

256-bit intermediates are implemented in-crate with only the operations needed (`mul_wide`,
`div_rem` by a `u128`, shifts). `u128 × u128` is not native on SBF (it lowers to
compiler-builtins `__multi3`), so the kernel matters: milestone 2 benchmarked the milestone 1
kernel (2 × `u128` limbs) against a 4 × `u64` kernel with a short-division path and kept the
latter (decision log). No `ruint` or
`primitive-types`: they pull in more than we need and their CU cost is unknown.

### 4.2 `exp`

As built (milestone 3): `x = (64k + j) ln2/64 + r` with `n = 64k + j = round(64 x / ln 2)` and
`|r| <= ln2/128`, the step split in a high and a low part (Cody-Waite) so `x − n HI` is exact;
then `exp(x) = 2^k × 2^(j/64) × e^r` with a 64-entry table of `2^(j/64)` and
`e^r = 1 + r + r^2 p(r)`, `p` of degree 4. The polynomial runs on `u64` fractions with a
64 × 64 → 128 product from 32-bit halves; the high-order steps, whose error is scaled by a power
of `r`, use one native 32 × 32 multiply. Coefficients interpolate `(e^r − 1 − r) / r^2` at
Chebyshev nodes (within a small factor of minimax at this degree), generated by
`tools/gen_tables.py` with Python's standard `decimal` module and committed as integer
constants, so they are reproducible without Sollya. `Overflow` when `e^x` exceeds `MAX`
(`x` above about 43.668); results below half an ulp (`x` below about −45.05) are `ZERO`, not an
error. Error bound `EXP_MAX_ERROR_ULPS = 3` ulps of `max(e^x, 1)`; measured 1.97.

### 4.3 `ln`

Range reduction `x = 2^k × m`, `m in [1, 2)`, then `ln(x) = k ln 2 + p(m)`. Options for `p`:
a minimax polynomial in `m − 1`, or in `s = (m − 1)/(m + 1)` (atanh series, odd and faster to
converge, costs one division). Milestone 3 measured both on Q64.64, each with 129 buckets
`c_j = 1 + j/128`: the polynomial form `ln m = −ln(RINV_j) + ln(1 + z)`, `z = m × RINV_j − 1`
with a table of rounded reciprocals (so `|z| <= 2^-8`) and `ln(1 + z) = z − z^2 q(z)`, `q` of
degree 5, costs 460 CU; the atanh form `ln c_j + 2 atanh((m − c_j)/(m + c_j))` with a
32-bit-digit division costs 614 CU at the same precision. The polynomial form is kept. Error
bound `LN_MAX_ERROR_ULPS = 2` ulps absolute; measured 1.51, dominated by rounding `m` to 64
bits and the final rounding.

### 4.4 Normal CDF

Planned: `N(x) = erfc(−x / √2) / 2` with a rational approximation in the style of W. J. Cody
(1969), split into ranges, so the relative error stays small in the tails, where an
absolute-error approximation such as Abramowitz-Stegun 7.1.26 (`|ε| < 1.5 × 10^-7` absolute)
is useless for deep out-of-the-money options.

As built (milestone 4), without any division: for `a = |x|`, `N(−a) = e^(−a²/2) P(a)`, where
`P(a) = N(−a) e^(a²/2)` is a scaled Mills ratio, an entire function, approximated by a
degree-12 polynomial on each of 37 segments of width `1/4` covering `[0, 9.25)`, interpolated
at Chebyshev nodes (relative error about `2^-69` per segment). On every segment the
coefficients alternate in sign, so the table stores magnitudes and the Horner steps stay
positive; the constant term and the last step keep 128 bits. `e^(−a²/2)` is the `exp` kernel's
64-bit mantissa and exponent before rounding, so `N` keeps its relative precision in the tail
and rounds once. `N(a) = 1 − N(−a)` exactly. At `|x| >= 9.25` (`NORM_CDF_CUTOFF`), `N(−9.25)
≈ 1.1 × 10^-20` is below half an ulp, so `N` saturates to exactly `0` or `1`. `n(x)` is
`e^(−x²/2) / √(2π)` on the same mantissa. Bounds: absolute 2 ulps for both (measured 1.32 and
1.08), and on `[−8, −5]` a relative `2^-56` beyond the final rounding (measured: no excess at
all), which is what relative precision can mean at a resolution of `2^-64`: at `x = −8`,
`N ≈ 6 × 10^-16` is about 11,000 ulps, so half an ulp alone is a relative `4.4 × 10^-5`.

Rejected: Cody's rationals (one 128-bit division per call, about 135 CU even on 32-bit digits,
and the same relative precision), Abramowitz-Stegun (bad tails), Hart 5666 (also rational),
Marsaglia's series (iteration count varies with `x`; kept as the test reference).

### 4.5 Black-Scholes

Black-Scholes-Merton with continuous carry `q`. Write `Dr = e^{−rT}`, `Dq = e^{−qT}` and
`F = S e^{(r−q)T}` (so `S Dq = F Dr`):

- `d1 = (ln(S/K) + (r − q + σ²/2)T) / (σ√T)`, `d2 = d1 − σ√T`.
- `C = S Dq N(d1) − K Dr N(d2)`, `P = K Dr N(−d2) − S Dq N(−d1)`.
- Greeks: delta `Dq N(d1)` (call) and `−Dq N(−d1)` (put); gamma `Dq n(d1) / (S σ √T)`; vega
  `S Dq n(d1) √T`; theta (call) `−S Dq n(d1) σ / (2√T) − r K Dr N(d2) + q S Dq N(d1)`, (put)
  `−S Dq n(d1) σ / (2√T) + r K Dr N(−d2) − q S Dq N(−d1)`. Bounds as in the `Greeks` struct.

With `q = 0`, `Dq` is exactly `ONE` (`exp(0)` is exact), so every formula reduces to plain
Black-Scholes without an extra rounding.

Degenerate cases handled before the formula: `T == 0` returns intrinsic value (`Dq = Dr = 1`)
with delta 0 or ±1, gamma and vega 0, and theta from the forward intrinsic below at `T = 0`.
`σ√T` below a threshold likewise returns forward intrinsic: `max(S Dq − K Dr, 0)` for a call, `max(K Dr − S Dq, 0)` for a put, with delta 0 or
`±Dq`, gamma and vega 0, and theta the derivative of that intrinsic (`q S Dq − r K Dr` for an
in-the-money call, its negative for an in-the-money put, 0 out of the money). The
out-of-the-money side, by forward moneyness (`S Dq` vs `K Dr`, equivalently `F` vs `K`, not `S`
vs `K`, so that `S Dq − K Dr` has a fixed sign), is priced directly and the in-the-money side
via put-call parity (`C = P + S Dq − K Dr`), a sum of positive terms. Fixed-point errors are
absolute, so the ITM formula's cancellation would not amplify them; parity is used because it
reuses the OTM evaluation, which has small values and the tail-accurate `N`.

As built (milestone 5): nothing divides or takes a square root. `√T`, `σ√T`, `1/(σ√T)`,
`1/(S σ√T)` (gamma) and `σ/√T` (theta) are `exp` of sums of `ln σ`, `ln T / 2`, `ln S` and
`ln K`. `d1 = ln(S/K)/(σ√T) + (r − q)√T/σ + σ√T/2`, each term rounded once at its own scale:
forming the numerator at absolute resolution and then multiplying by `1/(σ√T)` (up to about
`8.6 × 10^11`) amplified its rounding near the money and near expiry (#11 review: delta off by
up to `2 × 10^7` envelopes, down to about 300 s at `σ = 0.005`). Away from the money
(`|S − K| × 256 >= K`) the first term is `(ln S − ln K) × exp(−ln σ√T)`, whose relative error
is below `2^-54`; near it, `(S − K) × W × ln(1 + u)/u` with the exact `S − K`,
`W = 1/(K σ√T)`, `u = (S − K)/K` and `ln(1 + u)/u` on the `ln` kernel's polynomial, so nothing
cancels and nothing tiny is divided by something tiny. Factors that can be small in Q64.64
(`W` at a large `K`, `√T` near expiry, `1/(S σ√T)` at a large `S`) go through `mul_exp`,
which scales `e^x` by a power of two into `[2^20, 2^21)`, multiplies and shifts back, so their
relative precision survives; `S/K` in `[0.05, 20]` is checked exactly as `20 S >= K` and
`20 K >= S`. `N(d1)` and `n(d1)` share one `exp`. The only degenerate case is `T = 0`: the
smallest positive `T` is `2^-64` years, so `σ√T >= 0.005 × 2^-32 ≈ 1.2 × 10^-12`, where every
value fits (the largest are theta, about `4.3 × 10^18`, and gamma, about `3.4 × 10^17`), and
the formula is evaluated as is. A `σ√T` threshold would only have added an approximation error
(up to `S Dq σ√T / √(2π)`). The envelopes are `PRICE_ERR_REL × max(S, K) + PRICE_ERR_ABS` for
the price (`2^-48`, 64 ulps), `DELTA_ERR` absolute (`2^-49`), and `REL × scale + 64 ulps` for
gamma, vega and theta with the scales `Dq / (S σ√T)`, `S Dq √T` and `S Dq σ/√T + |r| K Dr +
|q| S Dq`; `price_error_bound`, `Price::upper` and `Price::lower` widen a price by its
envelope.

### 4.6 Implied volatility

Inputs are `IvInputs { spot, rate, carry }`; `Dr`, `Dq` and `F` as in 4.5.

1. Check bounds: `max(0, S Dq − K Dr) < target < S Dq` for calls and
   `max(0, K Dr − S Dq) < target < K Dr` for puts, else `ArbitrageBound`. Strict inequalities:
   at the bounds, vol is 0 or infinite.
2. Initial guess: Brenner-Subrahmanyam `σ0 = √(2π/T) × C/(S Dq)` near the money (forward
   moneyness, `F` vs `K`); Li (2005) rational approximation away from it, applied to the
   forward-normalized price. Milestone 6 compares both on a grid of moneyness and picks the
   switch point by measured iterations.
3. Up to 3 Halley iterations (Newton with the volga correction, which `price_and_greeks`
   almost gives for free), iterating in normalized price space (price over the discounted
   forward `S Dq`, or its log for deep OTM) rather than raw price, which converges far better
   for deep OTM and short `T`. Stop at tolerance; otherwise `NoConvergence`. A fixed cap keeps the CU bound fixed;
   milestone 6 measures the `NoConvergence` rate over the milestone 5 grid.

As built (milestone 6), replacing steps 2 and 3 after the prototype (decision log):

- **Normalized form.** The out-of-the-money option by forward moneyness (the side `price`
  computes directly; an in-the-money target is moved there by subtracting the parity term,
  which `price` adds exactly) with `a = |ln(S Dq / K Dr)|`, `s = σ√T`, `y = p / bound`
  (`bound = S Dq` for a call, `K Dr` for a put) and `L = −ln y`.
- **Initial guess.** A bilinear interpolation of `ln s` in one of two generated tables
  (`tools/gen_iv_tables.py`, standard library only; `make tables-check` regenerates them byte
  for byte): near the money `ln s + L` over `(u = ln a + L, Z = ln L)`, `u <= 10`; deep out of
  the money `ln s − w` over `(w = ln a − (ln 2 + Z)/2, Z)`, where `w` is the leading asymptotic
  `ln s`. 1,838 `i32` entries; the guess is within 6% of the root over the normalized domain
  and within 3.9% on the milestone 5 grid. Brenner-Subrahmanyam is the `a → 0` limit of the
  first table; Li (2005) covers only `|x| <= 0.5` and `s <= 1`, while the domain reaches
  `a = 21` and `s = 27`.
- **Iteration.** Up to 3 Chebyshev (third-order) steps in `v = ln σ` on a log objective:
  `ln p(v) − ln p` below the inflection point `s = √(2a)`, `ln(bound − p(v)) − ln(bound − p)`
  above it (`bound − p = S Dq N(−d1) + K Dr N(d2)`, no cancellation). `g'' / g' = 1 + d1 d2 − g'`
  and `ln |g'| = ln(s S Dq n(d1) / value)` in closed form, so `g / g' = g e^(−ln |g'|)`: no
  division. Each evaluation uses `price`'s own operations (`d1_terms`), so the solve inverts
  the function `price` computes. Stop when a step is at most `2^-16`.
- **Conditioning.** Vega at the root (at the last evaluation when converged, the smallest over
  the evaluations otherwise) must be at least `IV_VEGA_MIN_FACTOR = 2^28` times the price
  envelope `E`, else `IllConditioned` (coordinator's requirement: no silent best effort). Where
  it answers, `|implied_vol(price(σ)) − σ| <= 4 E / vega + 2^-40 σ`. `T = 0` and a root outside
  the `Volatility` range are `DomainError`.

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
  `10^-16` relative and worse in the tails. Measured in milestone 4 (`docs/precision.md`):
  `big_n` is off by up to `2.5 × 10^-11` absolute (`statrs`'s `cdf`, confirmed against
  Python's `math.erfc` at `x = 0.7074`) and `5.3 × 10^-11` relative on `[−8, −5]`, and `n`
  returns zero beyond `|x|` of about 4.84 (an `exp` cutoff at −11.7), an absolute error up to
  `3.3 × 10^-6` there. Milestone 5 therefore compares prices and greeks within those oracle
  errors scaled by the inputs, not within optfix's own bounds, and checks `n`-based greeks
  beyond `|d1| = 4.8` against the 256-bit reference instead. `exp` and `ln` are checked against a 256-bit
  `num-bigint` reference, itself checked against Python's correctly rounded `decimal` at fixed
  points (`tools/reference.txt`); f64 `libm` is too coarse to see errors near `2^-64`.
  `erfc` follows the same scheme in milestone 4. Python's standard `decimal` replaces `mpmath`
  for the reference tables (decided 2026-10-29): it computes `exp` and `ln` correctly rounded
  and needs no install.
- Milestone 5: `price_and_greeks` is checked against a 256-bit BSM reference (the exact
  formulas in `optfix/tests/common/`) over the whole domain, by default on 100,000 cases per
  property (the reference makes them dearer than the primitives' million; 1,000,000 cases pass
  in a release build in 22 s), and against OptionStratLib within the oracle tolerances of
  `tests/common/oracle.rs`.
- Carry `q`: OptionStratLib's Garman-Kohlhagen is the oracle with its foreign rate set to `q`.
  Price, greeks and implied vol each have tests on three planned sets: `q > 0` (up to
  `q = 0.5, T = 30`, where `Dq = e^{−15}`), `q < 0` (down to `q = −0.1, T = 30`, where
  `Dq = e^{3}` and `|delta| > 1`; the delta bounds `[0, Dq]` and `[−Dq, 0]` are asserted), and a
  `q = 0` regression: the same inputs with `carry = 0` must match the oracle's plain
  Black-Scholes values within the same envelope, and a committed table of `q = 0` results is
  checked bit for bit so later carry changes cannot move them. Put-call parity
  `C − P = S Dq − K Dr` is checked within the sum of the two price envelopes, and the
  `ArbitrageBound` edges of 4.6 use the carry-adjusted bounds.
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
lint-control/      failing control for the lint set, `make lint-control` (publish = false)
tools/             offline generation (Python standard library only): kernel tables and
                   reference values, not a crate
docs/              PLAN.md, benchmarks.md (`make bench`), precision.md (`make precision`)
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
3. **`ln` and `exp` in both representations, then the decision.** (Done: Q64.64.) Both
   representations behind
   a private trait, both bench rows, both error tables. *Done when* `docs/benchmarks.md` and
   `docs/precision.md` contain CU and max error for `exp` and `ln` in both representations, a
   short decision record is added to this plan, the losing representation is deleted, and
   milestone 1's tests pass on the winner.
4. **`N(x)` and `n(x)`.** (Done.) *Done when* both meet published bounds (absolute everywhere, relative
   on exactly `[−8, −5]`, plus saturation tests beyond the cutoff) verified by proptest, and their CU rows are in the benchmarks doc.
5. **Black-Scholes price and greeks.** (Done.) `price`, `price_and_greeks`, `intrinsic`, degenerate
   cases. *Done when* proptest against OptionStratLib passes over the whole supported
   domain table (every range, extremes, tiny `T`, `S` and `K` scaled together, `q > 0`, `q < 0`
   and the `q = 0` regression of 4.7), both kinds,
   with the documented envelopes, and `price_and_greeks` costs less than a benchmark-only baseline that
   computes `price` and each greek separately (not public API).
   Note from the milestone 3 review (codex): binary `sqrt` costs about 36% more than decimal
   on matched samples (10,703 against 7,878 CU), so profile the whole `price_and_greeks` path
   before claiming any total saving from Q64.64; the division follow-up applies to `sqrt`.
6. **Implied volatility.** (Done.) *Done when* round-trip `implied_vol(price(σ)) ≈ σ` holds within the
   documented bound wherever vega exceeds a documented threshold, for `q > 0`, `q < 0` and
   `q = 0`, `ArbitrageBound` and
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
| `optionstratlib` (dev, oracle; `default-features = false`, `pricing`) | 0.23.0 | `0.23` |
| `num-bigint` (dev, `exp`/`ln`/normal reference) | 0.5.1 | `0.5` |
| `rust_decimal` (dev, oracle inputs) | 1.43.0 | `1.43` |
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

## Follow-ups (not in a milestone yet)

- **Cheaper U256 division.** `div_*`, `from_ratio`, `from_scaled` and `sqrt` still divide
  with `u128` operations, which lower to `__udivti3` (about 580 CU per call on SBF). A
  32-bit-digit 128/64 step measured about 135 CU in milestone 3, so a division kernel on
  native `u64` digits should cut `DivDown` (1,597 CU) and `SqrtDown` (10,703 CU) severalfold.
  `MulDown` no longer divides since the switch to Q64.64 (284 CU).

## 6. Open questions

1. Decided (2026-10-29): price error `<= 10^-9 × max(S, K) + ERR_ABS`, delta below `10^-9`;
   supported domain table accepted as proposed.
2. CU budget: is there a target for `price_and_greeks` (for example 15,000 CU) that should
   fail the bench if exceeded?
3. Decided (2026-10-29, coordinator): a gate first, then CU. Each representation must reach
   `exp` relative error and `ln` absolute error of at most `10^-15` over the supported range
   (errors reported in ulps of both formats); among those that pass, the lower median CU of
   `exp` + `ln` wins and the worst case is reported; within 10% decimal wins, for its exact
   token conversions. Both formats are measured with the same inputs and build profile, and
   decimal also in a variant that converts in and out of the binary kernel.
4. Decided (2026-10-29): continuous carry `q` in v0.1. Plan review 2 (codex) made it
   consistent: `d1` uses `r − q`; forward intrinsic, put-call parity, forward moneyness and the
   implied vol call ceiling use `S e^{−qT}` (4.5, 4.6); `implied_vol` takes `IvInputs` with
   `carry`; delta is bounded by `±e^{−qT}`, up to about 20.1 at `q = −0.1, T = 30`, not
   `[−1, 1]`; theta's sign is not fixed; tests cover `q > 0`, `q < 0` and a `q = 0`
   regression (4.7). The price envelope of question 1 is stated in `max(S, K)`, while carry
   prices scale with `S e^{−qT}` (up to about `20 S`); milestone 5 measured it over the carry
   range: beyond 64 ulps the error is at most `2.4 × 10^-18 × max(S, K)`, while measured
   against `max(S Dq, K Dr)` it reaches `2.9 × 10^-14` (where `S Dq` falls to `e^{−15} S`), so
   `max(S, K)` stays the scale (decided 2026-10-29, `docs/precision.md`).
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

Milestone 3 (2026-10-29):

- **Representation: binary Q64.64.** Measured at commit `6adf034` (`make bench` and
  `make precision` there reproduce it) on the same inputs and release profile, net median CU
  with the `Noop` baseline subtracted, worst case in parentheses, and the maximum error over
  1,000,000 samples per domain:

  | Candidate | `exp` CU | `ln` CU | Sum | `exp` max error | `ln` max error |
  | --- | ---: | ---: | ---: | ---: | ---: |
  | Q64.64 | 337 (352) | 460 (465) | 797 | 1.97 ulp `2^-64` (`1.1 × 10^-19`) | 1.51 ulp (`8.2 × 10^-20`) |
  | Decimal, binary core | 462 (479) | 470 (529) | 932 | 0.59 ulp `10^-18` (`5.9 × 10^-19`) | 0.60 ulp (`6.0 × 10^-19`) |
  | Decimal, native | 7,989 (8,122) | 9,051 (9,186) | 17,040 | 3.13 ulp `10^-18` | 3.03 ulp |

  All three pass the `10^-15` gate of question 3 by four orders of magnitude. Decimal's best
  form costs 17% more than Q64.64 (Q64.64 is 15% cheaper), outside the 10% band, so Q64.64
  wins; it is also the more accurate in absolute terms. "Decimal, binary core" reduces `exp`
  in decimal (Cody-Waite split of `ln2/64` at `10^-18`), converts the remainder with one
  multiply, folds `ln(10^18)` into `ln`, and leaves each function with one multiply by `10^18`:
  those multiplies are the whole difference. "Decimal, native" runs the same reduction and
  polynomials as decimal `Fixed` multiplies (each a 256-bit product divided by `10^18`) and
  shows why the binary core is used. The decimal candidates and the `Repr` trait are deleted.
- **`Fixed` switched to Q64.64; milestone 1's tests pass on it.** `mul_*` is now a 256-bit
  product and a shift: `MulDown` fell from 1,220 to 284 net CU. `div_*` still divides
  (1,597 CU, about the same). `sqrt` rose from 7,880 to 10,703 CU; its operand `raw × 2^64`
  has a few more bits than `raw × 10^18` for the same value, but the cause was not
  investigated here: milestone 5 revisits `sqrt` if it dominates pricing. `MAX` drops from about `1.7 × 10^20` to
  `9.2 × 10^18`, which still holds every value of the domain table (prices up to
  `S × e^3 ≈ 2 × 10^10`). `to_scaled` accepts `decimals` up to 76 (the excess factor
  `10^(decimals − 38)` must fit). Domain bounds that Q64.64 cannot hold exactly are rounded
  outward.
- **SBF costs that shaped the kernel** (sBPF v3, platform-tools v1.57, measured with a
  throwaway loop program): a `u64` multiply or divide is about 1 CU; `u64 × u64 → u128` through
  `__multi3` about 50 CU; `u128 / u64` through `__udivti3` about 580 CU; a variable `u128` shift
  a libcall of about 20 instructions. So the kernel never divides, and multiplies 64-bit digits
  from 32-bit halves (about 26 CU). LLVM recognizes the usual "ladder" form of that split and
  folds it back into `__multi3`; the carry form (`overflowing_add`) is not folded. Checked
  versus unchecked limb arithmetic made no difference, because LLVM removes checks it can prove
  never fail.
- **`ln`: table and polynomial, not atanh** (460 against 614 CU at equal precision, 4.3).
- **Polynomials: Chebyshev interpolation, not Taylor.** At degree 6 the Taylor remainder of
  `e^r` on `|r| <= ln2/128` is about `2^-65`, half an ulp; the interpolated coefficients make it
  negligible at the same cost.
- **Reference: Python `decimal`, not `mpmath`** (coordinator, 2026-10-29), and a 256-bit
  `num-bigint` series reference in the property tests, so CI needs no Python.
- **The same U256 findings apply to `div_*` and `sqrt`**, which still use `u128` division in
  algorithm D; a 32-bit-digit division (as in the measured atanh variant, about 135 CU per
  128/64 step) is a later optimization (Follow-ups), not part of this decision.

Milestone 4 (2026-10-29):

- **`N(x)` and `n(x)` without division** (4.4): `N(−a) = e^(−a²/2) P(a)`, `P` piecewise
  polynomial (37 segments × degree 12, 4.1 KB of coefficients). Net median CU over `x` uniform
  in `[−9.25, 9.25]`: `NormCdf` 1,312, `NormPdf` 575 (`docs/benchmarks.md`). Maximum error
  over 1,000,000 samples per domain: `N` 1.32 ulps, `n` 1.08 ulps (`docs/precision.md`).
- **64-bit `exp` mantissa, not 128-bit.** A 128-bit mantissa (table at `2^-128`, one product
  more) brings `N` and `n` to 1 ulp and `exp` itself from 1.97 to 0.52 ulps, but costs
  measured `NormCdf` 1,581 against 1,487, `NormPdf` 818 against 705 and `Exp` 459 against 315
  CU (same build, before the inlining below). Every one of those errors is near `10^-19`,
  against a price budget of `10^-9 × max(S, K)`, so the cheaper core is kept and the bounds
  are 2 ulps. Two precision fixes that cost almost nothing are kept: `x² / 2` is rounded, not
  truncated (it had added up to half an ulp), and the polynomial's constant term and last step
  keep 128 bits.
- **`exp` is unchanged**: the reduction is shared through `exp_parts` and `#[inline(always)]`
  on the reduction and the core keeps `Exp` at 315 CU, bit-identical to milestone 3.
- **Oracle limits recorded** (4.7): OptionStratLib's `big_n` is good to `2.5 × 10^-11`, its `n`
  flushes to zero beyond `|x| ≈ 4.84`. The oracle is a dev-dependency with
  `default-features = false, features = ["pricing"]`; `Cargo.lock` only gains crates (the
  Agave 4.3.2 pin is unchanged).
- **Test layout**: the 256-bit reference moved to `optfix/tests/common/`, checked against
  Python `decimal` (now with `N` and `n` rows, computed there by the erf series and the erfc
  continued fraction, a different method from the reference's positive series) in
  `tests/reference.rs`; `tests/precision.rs` writes `docs/precision.md`.

Milestone 5 (2026-10-29):

- **Pricer without `sqrt` or division** (4.5). Net median CU over the whole domain
  (`docs/benchmarks.md`): `Price` 9,247, `PriceAndGreeks` 14,242, and the bench-only baseline
  that computes the price and each greek separately 39,287, so `price_and_greeks` costs 36% of
  it (the milestone's criterion); near the money `Price` 11,092 and `PriceAndGreeks` 16,097.
  The `d1` restructure of the #11 review costs about 1,600 CU on `PriceAndGreeks` (12,629
  before) and buys the greek envelopes near expiry.
- **Profile of `price_and_greeks`** (calls executed on the path, times the component rows):
  4 `ln` (about 1,830 CU), 8 `exp` (2,520), `N(d1)` with `n(d1)` and `N(d2)` (about 2,800), 18
  `mul_down` (4,970), the rest decoding, checks, shifts and additions. Near the money
  (`|S − K| < K/256`) the moneyness term takes 9 `exp`, 20 `mul_down` and `ln(1 + u)/u`.
  `price` alone executes 4 `ln`, 5 `exp` (6 near the money) and 8 `mul_down` (10).
  Multiplication is the largest share (35%), the next optimization target.
- **The `sqrt` caveat** (milestone 3 review): pricing never calls `sqrt_down`. The five `exp`
  that replace `√T`, `σ√T` and the reciprocals cost about 1,600 CU; `sqrt_down` and three
  `div_down` would cost about 15,700. The binary `sqrt` being 36% dearer than the decimal one
  has no effect on `price_and_greeks`.
- **`U256::mul_wide` on the kernel's carry-form product**, bit-identical: `MulDown` measured
  250 CU against 284 on the milestone 4 build, and 276 after the rebase over `wide::mul_shr`
  (the same code; inlining around `U256` changed).
- **Degenerate branch only at `T = 0`** (4.5): with `T >= 2^-64` every value fits, so no
  `σ√T` threshold and no approximation error.
- **Envelopes** (`docs/precision.md`, 1,000,000-point survey; the properties pass at 1,000,000
  cases): price `2.4 × 10^-18 × max(S, K)` beyond 64 ulps against `PRICE_ERR_REL = 2^-48`;
  delta `5.8 × 10^-18` against `2^-49`; gamma `1.5 × 10^-13`, vega `2.1 × 10^-14`, theta
  `2.3 × 10^-14` of their scales against `2^-40`, `2^-43` and `2^-42`. Question 4: `max(S, K)`.
- **Oracle** (OptionStratLib 0.23, Garman-Kohlhagen with foreign rate `q`): on `q > 0`,
  `q < 0` and `q = 0` (the last also against plain `black_scholes`), every difference is at
  most 25% of the oracle tolerance (price `10^-9 × max(S, K)`, delta `10^-9`, other greeks
  `10^-9 × scale + 10^-18`, the last only where `|d1| <= 4.8`, 4.7).
- **`q = 0` regression**: 64 committed results (`optfix/tests/data/bsm_q0.txt`) checked bit for
  bit (regenerated with the `d1` restructure).
- **#11 review fixture**: codex's sweep (`tools/bsm_fixture/gen_sweep.py`: `d1` from −9 to 9
  at 26 expiries from `10^-12` s to a year, five volatilities, four spot scales, three
  rate/carry sets, plus 3,000 broad points; 19,106 rows) with mpmath expected values at 100
  digits (`expected.py`) is committed as `optfix/tests/data/bsm_fixture.csv`. `tests/bsm_fixture.rs`
  checks the 256-bit reference against mpmath (within one raw unit) and `price_and_greeks`
  against its envelopes on every row: zero violations, the worst at 15% of an envelope (theta),
  and codex's independent checker (`check_mpmath.py`) agrees. New properties pair small `T` (one
  raw unit to about 2^45) with near-the-money strikes and check continuity across the
  `|S − K| × 256 = K` switch; all pass at 1,000,000 cases in release.
- **Codex's 4,224-point boundary set** (#11 review), regenerated at `fa43a53` and run through
  its 100-digit mpmath checker: 0 violations and 0 errors, the worst ratio theta at 0.161
  (`tools/bsm_fixture/boundary-fixed-check.log`; the compressed outputs are bus attachment 215
  on message #36947, too large to commit).
- **Bench-only items** live behind the non-default `bench-internals` feature of `optfix`
  (`baseline::separately`), enabled only by `bench-program` and `bench`.

Milestone 6 (2026-10-29):

- **Design change from 4.6, from a prototype** (exact arithmetic on 4,000 grid points): from a
  10% guess, 3 Halley steps leave about 60% of cases needing a fourth; from a 3% guess every
  case converges in 3 evaluations. Li (2005) cannot give that over this domain, so the guess
  is two generated tables (4.6, as built). The step is Chebyshev's rather than Halley's: the
  same order, and no division. The coordinator approved the change with six requirements
  (bus message #37026): reproducible tables checked by `make`, dense tests across the table
  seam with a guess-error test, a typed `IllConditioned` below the threshold, the iteration
  cap tested and its distribution reported, a price round trip plus an independent mpmath
  reference at small `T` and near the money, and CU rows.
- **Round trip** (`docs/precision.md`, 1,000,000 points per carry set): every answer within
  1.1% of its envelope (`4 E / vega + 2^-40 σ`), worst relative `σ` error `6.1 × 10^-9` (near
  the threshold); `price(implied_vol(p))` within 0.62% of `5 E + 2^-40 σ vega`. `NoConvergence`
  0 on the grid (it remains only for a well-conditioned target that misses the cap, tested by
  lowering the cap); `IllConditioned` 10% to 12% of the points; targets equal to a bound (no time
  value) 53%, `T = 0` 10%.
- **Steps** over the 3,000,000 grid points: 1,869 answers after 1 step, 766,980 after 2 and
  8,472 after 3; the cap is never exceeded.
- **mpmath reference** (`tests/data/iv_fixture.csv`, `tools/iv_fixture/gen_iv_fixture.py`):
  20,754 exact targets on a codex-style boundary set (`d1` from −9 to 9 including ±1/4 and 0,
  26 expiries from `10^-12` s to a year), each with the exact implied volatility of the rounded
  target: 8,228 answered, worst 0.80% of the envelope; the rest rejected with a typed error.
- **CU** (`docs/benchmarks.md`, net of `Noop`): `ImpliedVol` median 21,905, max 29,428
  (25,590 and 34,780 near the money); with every step taken, the worst case, median 29,214 and
  max 34,957 (34,765 and 35,658 near the money). A step costs about 7,300 CU (the difference of
  the medians: 3 steps against mostly 2).
- **Error variant** `PricingError::IllConditioned = 4` (bench status 5).
