# optfix phase 2 plan (toward 0.4.0): SVI smiles

Status: draft for codex review, 2026-11-03. Scope from the coordinator's phase 2 go (bus message
51618, SOLANA-PROJECTS): (1) raw SVI slice evaluation in fixed point with documented error
envelopes, and host-side no-arbitrage checks (butterfly and calendar); (2) a `price_strip`
variant whose legs take their volatility from an SVI slice; (3) WASM exposure and conformance
vectors; (4) release 0.4.0 (crates.io, npm, GitHub) at the end. Devnet only: nothing in this
phase deploys anything, and every on-chain measurement runs in LiteSVM as before. Semver and
the existing guarantees are kept. Milestone numbers continue `docs/PLAN-improvements.md`
(milestones 15 to 21).

Process, as in 0.3: one branch and one PR per milestone (`feat/m<N>-<name>`), codex reviews
every PR before merge, every required check on `main` passes, and each merged milestone is
reported to the coordinator (milestone reports only). One release at the end (milestone 26).

## Guarantees this phase keeps

- **Additive only.** No public item changes signature or behaviour; `cargo-semver-checks`
  stays green against 0.3.0. The version is 0.4.0 because the coordinator asked for it, not
  because anything breaks.
- **Bit-identical existing results.** Every existing function returns the same bits: the
  frozen 0.1 fixture, the mpmath fixtures and conformance generations 1 to 3 replay with no new
  agreed change.
- **CU of existing rows.** Milestone 24 refactors the strip's shared state. The existing rows of
  `docs/benchmarks.md` must stay byte-identical; if LLVM moves one anyway, the row is re-agreed
  under the usual rule (+5%, rounded up) and the cause is written in the PR. The downstream job
  (varswap on chain, metalbook off chain) must stay green with no CU growth.
- **Crate rules.** `no_std`, no allocator, no floats, no panics, checked arithmetic, explicit
  rounding, a public error bound for every new numeric function, and proptests against an
  independent reference.

## Background: raw SVI

A raw SVI slice (Gatheral 2004) gives the total implied variance at log-forward-moneyness `k`:

```
w(k) = a + b (ρ (k − m) + sqrt((k − m)² + σ²)),   k = ln(K / F),   F = S e^{(r − q) T}
```

with `b >= 0`, `|ρ| < 1`, `σ > 0` and `a + b σ sqrt(1 − ρ²) >= 0` (the minimum of `w` is not
negative). The implied volatility of strike `K` at expiry `T > 0` is `sqrt(w(k) / T)`.

**The root.** Forming `x² + σ²` (`x = k − m`) directly loses precision when both are tiny: at
`σ = 10^-6`, `σ² = 10^-12` carries one ulp of `5.4 × 10^-20`, so the root has a relative error
near `3 × 10^-8`. Scaling always (`M × sqrt(1 + (μ / M)²)`, with `M = max(|x|, σ)` and
`μ = min(|x|, σ)`) fixes that but is worse for large `M`, where its error grows with `M`. The
two forms are complementary, so the root switches at `M = 1`:

- `M >= 1`: `sqrt_down(x ⊗ x + σ ⊗ σ)`. The radicand is at most 2 ulps low and the root at least
  1, so the result is at most `2 / (2 × 1) + 1 = 2` ulps low.
- `M < 1`: `q = μ ⊘ M`, then `M ⊗ sqrt_down(1 + q ⊗ q)`. `q ⊗ q` is at most 3 ulps low, the root
  at most `3 / 2 + 1 = 2.5`, and the product at most `2.5 M + 1 < 3.5`.

**The envelope this gives.** Every step rounds down and `b >= 0`, so the computed value never
exceeds the true `w`, and the error does not depend on `k`, `σ` or `a` (`a` and `x` are exact):

```
0 <= w(k) − computed <= (3.5 + 1) b + 1 = 4.5 b + 1   ulps
```

The cancellation in the wings (`ρ` near `∓1`, `ρ x + s` much smaller than its terms) needs no
special handling in this absolute form; it only means that the error relative to `w` grows
where `w` is tiny.

**Evidence** (a Q64.64 emulation in Python integers against mpmath at 80 digits, 75,000 points
per stratum, before any Rust). Maximum absolute error of `w`:

| Stratum | Direct | Always scaled | Switch at `M = 1` |
| --- | ---: | ---: | ---: |
| Uniform over the domain | `1.5 × 10^-17` | `2.9 × 10^-17` | `1.2 × 10^-18` |
| Near `k = m`, `σ <= 10^-3` | `4.5 × 10^-13` | `1.1 × 10^-18` | `1.1 × 10^-18` |
| Wings, `1 − |ρ| <= 10^-2`, cancelling | `4.4 × 10^-17` | `2.8 × 10^-17` | `1.6 × 10^-18` |
| Around `M = 1` and the domain edges | `4.7 × 10^-13` | `8.6 × 10^-18` | `2.0 × 10^-18` |

The switching form stayed within `4.5 b + 1` ulps and was never above the exact value at any
point. Milestone 22 repeats this on the Rust code with its own fixture.

## Scope and order

| # | Milestone | Item | Size | Ships in |
| --- | --- | --- | --- | --- |
| 22 | SVI slice: parameters, `w(k)`, implied volatility, envelopes | 1 | M | 0.4.0 |
| 23 | No-arbitrage checks: wings, butterfly, calendar (host side) | 1 | M | 0.4.0 |
| 24 | `price_strip_svi` | 2 | M | 0.4.0 |
| 25 | `optfix-wasm` exposure and conformance generation 4 | 3 | S | 0.4.0 |
| 26 | Release 0.4.0 | 4 | S | 0.4.0 |

## Milestone 22: SVI slice

New module `optfix/src/svi.rs`, re-exported from the crate root.

**API** (names to be confirmed in review):

- `SviParams`, with private fields `a, b, rho, m, sigma: Fixed`, built only by
  `SviParams::try_new(a, b, rho, m, sigma) -> Result<Self, PricingError>`, with getters. Its
  invariants carry the validation, as the quantity newtypes do. `Copy`, `Eq` and `Hash`, like
  the inputs.
- `SviParams::total_variance(&self, k: Fixed) -> Result<Fixed, PricingError>`: `w(k)`, rounded
  once per operation in a fixed order, with the envelope below. This is the path used on chain.
- `SviParams::total_variance_bounds(&self, k: Fixed) -> Result<(Fixed, Fixed), PricingError>`:
  a rigorous enclosure `lo <= w(k) <= hi`: `lo` is `total_variance` and `hi` adds the envelope
  below, since the error is one-sided and derived rather than measured. Milestone 23 builds on
  it.
- `SviParams::min_total_variance(&self)`: `a + b σ sqrt(1 − ρ²)`, rounded down.
- `SviMarket { spot: Spot, rate: Rate, carry: Rate }`: an exhaustive input struct, like
  `MarketInputs` without the volatility.
- `log_forward_moneyness(expiry: Years, market: &SviMarket, strike: Strike)`:
  `k = ln K − ln S − (r ⊗ T − q ⊗ T)`, in exactly the operation order the strip uses, so that a
  caller reproduces a leg's `k` bit for bit.
- `svi_vol(params: &SviParams, expiry: Years, market: &SviMarket, strike: Strike) ->
  Result<Volatility, PricingError>`: `sqrt(w(k) ⊘ T)` with both roundings down, through
  `Volatility::try_new`.

**Domain.** The parameter ranges bound the envelope and rule out overflow; they are wide
enough for any slice whose volatilities fall in `Volatility`'s `[0.005, 5]` up to 30 years
(`w <= 750`):

- `|a| <= 1000`, `0 <= b <= 10`, `|ρ| < 1` (strict, in raw units), `|m| <= 10`,
  `10^-6 <= σ <= 10`, and `min_total_variance() >= 0`. The last check uses the rounded-down
  value, so a slice whose true minimum lies within a few ulps above 0 can be refused; this is
  documented.
- `total_variance` takes `|k| <= 32`. Every strip leg is inside it: the moneyness check gives
  `|ln(K/S)| <= ln 20 ≈ 3.0` and `|(r − q) T| <= 0.6 × 30 = 18`, so `|k| <= 21.1`.
- `svi_vol` refuses `T = 0` (`DomainError`: the total variance has no rate to spread over) and
  any result outside `Volatility`'s range (`DomainError` from `try_new`), negative `w` included.
  At a very small `T`, a quotient `w / T` that does not fit `Fixed` is `Overflow`.

**Envelopes.**

- `w`: `0 <= w(k) − total_variance(k) <= E_w`, with `E_w = SVI_W_ERR_REL ⊗↑ b + SVI_W_ERR_ABS`,
  `SVI_W_ERR_REL` 5 ulps and `SVI_W_ERR_ABS` 1 ulp (the derivation above, `4.5 b + 1`, rounded up
  to a representable factor). Derived, then checked against the fixture.
- Volatility: `0 <= σ_imp − svi_vol <= (E_w / T + 1 ulp) / (2 σ) + 1 ulp`, where `σ` is the
  computed volatility. The radicand is low by `E_w / T` plus one ulp from the division, a root
  is concave so the computed `σ` gives a valid slope, and the last ulp is the root's own floor.
  Note the division's ulp is amplified by `1 / (2 σ)`: up to 100 ulps at `σ = 0.005`. Published as
  a function `svi_vol_error_bound(...)`, as `price_error_bound` is for prices, because it depends
  on the inputs.
- The price of a leg is `price` at the computed `σ`, so its envelope is `price_error_bound`
  around that value; the error against the price at the true `σ_imp` adds at most
  `vega × (σ_imp − σ)`. The docs state this.

**Reference and tests.**

- `tools/svi_fixture/`: an mpmath reference at 80 digits for `w(k)` and `σ_imp`, over random
  parameters from the domain (log-uniform `σ` and `b`, uniform `ρ`, `m`, `a` subject to the
  minimum constraint) and `k` over the whole `[−32, 32]`, with extra strata near `k = m`, in the
  far wings at `|ρ| > 0.99`, around the `M = 1` switch and at the domain edges.
  `check_mpmath.py` is an independent checker, as for the pricer.
- Proptests:
  - the one-sided envelope against the fixture: the mpmath value inside `[lo, hi]`, never below
    `total_variance`;
  - `hi >= min_total_variance()`: the computed `w` stays above the slice's minimum, up to the
    envelope;
  - reflection symmetry: the slice `(a, b, −ρ, −m, σ)` at `−k` equals `(a, b, ρ, m, σ)` at `k`,
    bit for bit, since every step is symmetric in the sign of `x`;
  - `svi_vol` against `sqrt(w / T)` from the fixture.
- Fuzz target `svi`: arbitrary parameters and `k`. It checks `try_new` agreement with the
  domain, the same result from `svi_vol` and from its documented steps, and no panic. It joins
  `make fuzz-smoke`.
- `docs/precision.md`: measured `w` and `σ_imp` errors over 1,000,000 points, as for the other
  rows.

**CU.** Rows `SviTotalVariance` and `SviVol` in `bench-program` and `docs/benchmarks.md`, with
budget limits set by the usual rule from the first measurement. Estimate from today's rows:
`total_variance` about 3,300 CU net for `M >= 1` and 3,900 for `M < 1` (the `sqrt` at about 2,200
dominates), `svi_vol` up to about 7,700 (two `ln`, the variance, a division and a second `sqrt`).
If `SqrtDown` dominates as
expected, a cheaper exact square root (same bits: floor and ceiling are exactly defined, so a
faster integer root changes no result) is measured in this milestone and adopted only if
`SqrtDown`'s own row and every downstream row do not grow.

*Done when* the fixture, its independent checker and the proptests pass, the fuzz smoke covers
`svi`, `docs/precision.md` and `docs/benchmarks.md` have the new rows, and every existing
result and CU row is unchanged.

## Milestone 23: no-arbitrage checks (host side)

Validation is for the host (an off-chain publisher or keeper checking a slice before it is
used). It is `no_std` like the rest of the crate but is not benchmarked or budgeted for SBF.

- **Wings.** `check_wings(&SviParams)`: `b (1 + |ρ|) < 2`, strictly, evaluated rounding up, so a
  pass is certain. The slopes of `w` at `±∞` are `b (1 ± ρ)`. Lee's moment bound allows 2, but
  the slice is free of butterfly arbitrage only if `d+(k) → −∞` as `k → +∞` (and `d−(k) → +∞`
  as `k → −∞`), and a slope of exactly 2 gives `d+ → 0`: mpmath at `b (1 + ρ) = 2` gives 0.0 at
  `k = 10^2`, `10^4` and `10^6`, against −0.36, −3.6 and −36 at a slope of 1.9.
- **Butterfly (Durrleman, in the form of Gatheral and Jacquier).** With those limits, the density
  is non-negative if and only if
  `g(k) = (1 − k w' / (2 w))² − (w'² / 4) (1 / w + 1 / 4) + w'' / 2 >= 0` and `w > 0` for all `k`,
  with `w' = b (ρ + x / s)`, `w'' = b σ² / s³`, `s = sqrt(x² + σ²)`, `x = k − m`.
  `check_butterfly(&SviParams, grid: SviGrid) -> Result<ButterflyReport, PricingError>` evaluates
  `g` at every node of a uniform grid `SviGrid { k_min, k_max, nodes }` in interval arithmetic
  with the directed primitives (`mul_down`/`mul_up`, `div_down`/`div_up`,
  `sqrt_down`/`sqrt_up`), starting from `total_variance_bounds`. A node passes only if the lower bound of `g` is
  `>= 0` and the lower bound of `w` is `> 0`.
- **Calendar.** `check_calendar(earlier: (&SviParams, Years), later: (&SviParams, Years),
  grid) -> Result<CalendarReport, PricingError>`: `T_earlier < T_later`, and at every node the
  lower bound of `w_later` is at least the upper bound of `w_earlier`. Both slices are in their
  own forward's log-moneyness, the standard condition under deterministic rates and carry. The
  report documents that assumption.
- **Reports.** `ButterflyReport` and `CalendarReport` are `#[non_exhaustive]` outputs. Each
  holds `ok: bool`, the first failing node's `k` and its bound (`g`'s lower bound, or
  `w_later_lo − w_earlier_hi`), and the minimum bound over the grid. An `Err` means the
  evaluation itself failed (a grid outside `|k| <= 32`, zero nodes, overflow), never a
  violation; no allocation is needed.

**Semantics, stated in the docs:** the checks are conservative at the nodes (no false pass: an
interval that straddles 0 fails) and say nothing between nodes. They are not a certificate for
the whole real line; the wing check covers `|k| → ∞`. There is no built-in default grid: the
caller passes it, and the docs recommend the range of `k` the slice will be used on plus a
margin, with the spacing tied to `σ` (at least four nodes per `σ` around `m`, where `w''`
peaks).

**Tests.**

- A known butterfly arbitrage: the slice of Gatheral and Jacquier (2014), example 3.1 (Vogt:
  `a = −0.0410`, `b = 0.1331`, `ρ = 0.3060`, `m = 0.3586`, `σ = 0.4153`, `T = 1`) must fail,
  at the nodes where the mpmath `g` is negative by more than the interval width. A first
  mpmath pass (50 digits, nodes every 0.01 on `[−2, 2]`) gives `g < 0` for `k` in
  `[0.65, 1.25]`, with a minimum of about `−0.033`. The same slice passes the wing condition
  (`b (1 + |ρ|) ≈ 0.174`) and has a positive minimum variance (`≈ 0.0116`), so only the
  butterfly check catches it. The milestone regenerates this in its fixture rather than
  copying the numbers.
- Known clean slices (fits from published examples, plus symmetric `ρ = 0` slices with small
  `b`) must pass.
- Calendar: crossing slices fail at the crossing; a slice against itself shifted up by `a + ε`
  passes.
- Proptests against an mpmath `g` table: every node where mpmath `g` is above the interval
  width passes, and every node where it is below minus the width fails. A node inside the band
  may go either way, and the band's width is measured and reported.

*Done when* those tests pass and `docs/precision.md` reports the interval widths of `w` and `g`
on the fixture.

## Milestone 24: `price_strip_svi`

```
pub fn price_strip_svi(
    expiry: Years,
    market: &SviMarket,
    slice: &SviParams,
    legs: &[StripLeg],
    out: &mut [Price],
) -> Result<(), PricingError>
```

**Contract:** `out[i]` equals `price(&leg_i_option, &MarketInputs { vol: svi_vol(slice, expiry,
market, K_i)?, .. })` bit for bit. That is the same guarantee `price_strip` gives against
`price`, with the leg's own volatility, so every existing price envelope applies to each leg
unchanged. The partial-output contract is `price_strip`'s: legs are priced in order, the first
failing leg's error is returned, `out[..i]` holds the earlier prices and `out[i..]` is
unchanged. A length mismatch and `T = 0` return `DomainError` before anything is written. A
leg whose SVI volatility is outside `[0.005, 5]` fails with `svi_vol`'s error for that strike.

**Implementation.** `Shared` splits into what depends on `σ` and what does not. The strike- and
volatility-independent part is computed once: `Dr`, `Dq`, `S Dq`, `ln S`, `½ ln T`,
`r ⊗ T − q ⊗ T`. The leg's `ln K` serves both `k` and `d1`. Per leg, the volatility-dependent
values (`ln σ`, `ln σ√T`, `σ√T`, the drift term, `1/(σ√T)`) are recomputed from the leg's
`σ` in `price`'s rounding order, which is what keeps the bit-for-bit contract. Existing
`price_strip` and `price_and_greeks_strip` keep their code path and CU (see the guarantees
above). A faster contract (`σ√T = sqrt(w)` directly, skipping the leg's `σ`) would save about
one `sqrt` and one `ln` per leg but lose the bit-for-bit statement against `price`. It is not
proposed; noted for the review.

**Estimate.** About 13,000 CU net per extra leg at worst: 4,767 for today's strip leg, plus about
1,700 for the leg's volatility terms, plus up to about 6,700 for `k`, `w` and `sqrt(w / T)`. That is
against about 16,900 for `svi_vol` then `price` per strike, so roughly 22% cheaper, and about
15 legs fit the default 200,000 CU. Measured rows: `PriceStripSvi, 1 / 2 / 8 / 16 legs`. The
README's leg projections gain an SVI line.

**Tests.** A proptest of the contract against `svi_vol` plus `price` per leg, bit for bit, over
random slices, markets and legs. Partial output on a forced failing leg (an out-of-range
volatility in the middle of the strip). The SBF bench checks host equals chain for every leg.
Fuzz: the `pricing` target gains the SVI strip with the same contract.

**Out of scope:** a greeks variant (`price_and_greeks_strip_svi`). Its greeks would be BSM
greeks at the leg's volatility (sticky strike), which is a modelling choice to agree first. It
can follow in a later minor release, since the outputs are `#[non_exhaustive]` and the function
is additive.

*Done when* the contract proptest, the partial-output tests, the SBF host-equals-chain check
and the fuzz smoke pass, the new rows are budgeted, and every existing row and downstream row is
unchanged (or re-agreed with its cause written down).

## Milestone 25: WASM and conformance generation 4

- `optfix-wasm`:
  - an `SviParams` class (constructor throws `OptfixError` `DomainError` like the Rust
    `try_new`);
  - `totalVariance`, `totalVarianceBounds` (an object `{ lo, hi }`), `minTotalVariance`,
    `logForwardMoneyness`, `sviVol`, `sviVolErrorBound` and `priceStripSvi`;
  - `checkWings`, `checkButterfly` and `checkCalendar`, returning plain objects mirroring the
    reports.
  
  Every value stays an exact `bigint` or decimal text. README section with an example.
- Conformance generation 4 (`conformance/vectors-g4.json`): every function of generations 1 to
  3 again (as each generation does) plus the SVI functions, including failing cases (domain
  edges, an out-of-range leg volatility partway through a strip, the Vogt slice's butterfly
  report). Generations 1 to 3 replay unchanged with no new agreed change. Replayed natively,
  through wasm on Node.js, and the SVI subset on SBF through the bench program's instructions.
- The npm package is checked by the existing `npm pack --dry-run` step.

*Done when* all four generations replay on the three targets.

## Milestone 26: release 0.4.0

`CHANGELOG.md` [0.4.0] (Added only), README (`optfix = "0.4"`, an SVI section with the error
envelopes and the CU rows, the wasm README), `cargo publish --dry-run` for both crates, and
`cargo-semver-checks` against 0.3.0. After the merge: signed tag `v0.4.0`, `cargo publish -p
optfix` then `-p optfix-wasm`, and the GitHub release with the `wasm-pack` tarball attached.
npm: the login is on the coordinator's host, so the coordinator publishes the release tarball
there (`npm publish optfix-wasm-0.4.0.tgz --access public`). npm stages the publish until the
owner approves it, and the release notes gain the npm link once `npm view optfix-wasm version`
shows 0.4.0. Downstream bumps to `"0.4"` go through the coordinator (both are optional:
nothing breaks).

## Open points for review

1. **Contract of `price_strip_svi`:** bit-for-bit equal to `price` at `svi_vol` (proposed),
   versus a cheaper direct `σ√T = sqrt(w)` path with its own envelope.
2. **Parameter domain:** the ranges in milestone 22, and refusing slices whose rounded-down
   minimum variance is negative.
3. **Validation semantics:** conservative interval checks at caller-chosen nodes plus the exact
   wing condition, documented as not a certificate between nodes.
4. **Version:** 0.4.0 although the phase is additive (the coordinator's instruction).
5. **`svi_vol` rounding:** down for both the division and the root, fixed and documented,
   rather than a `Rounding` argument. A caller who needs a conservative volatility widens it by
   `svi_vol_error_bound`.
