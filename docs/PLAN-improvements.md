# optfix improvement plan (toward 0.3.0)

Status: draft for codex review, 2026-11-02. Scope from the coordinator's go (bus message 43337,
SOLANA-PROJECTS) after the roadmap-v2 review: a downstream CU guard and `cargo-semver-checks`
in CI; fuzzing; `optfix-wasm` with exact conformance vectors and a Pinocchio example; the 0.3
API (breaking changes batched there); implied volatility seed work only after profiling, with
no Jäckel port. Milestone numbers continue `docs/PLAN.md` (milestones 1 to 14).

Process, as in 0.2: one branch and one PR per milestone (`feat/m<N>-<name>`, or `ci/` and
`docs/` prefixes where nothing ships), codex reviews every PR before merge, the six required
checks on `main` pass (plus any check a milestone adds and makes required), and the frozen 0.1
fixture (`optfix/tests/frozen_0_1.rs`) stays bit-exact unless a milestone below agrees a
change. Each merged milestone is reported to the coordinator. One release at the end (0.3.0,
milestone 21): the tooling milestones ship nothing, and batching avoids a 0.2.2 that 0.3 would
supersede within days.

## Context on main (0.2.1, 0dade38)

- Net CU (`docs/benchmarks.md`, median / max): `MulShr` 317 / 333, `Price` 9,233 / 9,553,
  `PriceAndGreeks` 14,203 / 14,597, `ImpliedVol` 27,132 / 34,724, its worst row (all
  iterations, near the money) 42,003 / 42,951; `PriceStrip, 16 legs` 80,902.
- The 0.2.0 regression that motivates milestone 15: `U256::shr` gained a second caller (the
  `sqrt_down` seed) and stopped being inlined into `wide::mul_shr` (+23 CU). Our own budget did
  not fail, because `MulShr`'s limit is 400 by the +5%/round-to-100 rule; varswap's `sample`
  rows paid +22 to +69 and only codex's manual downstream measurement found it.
- Downstream use: varswap calls `ln` and `wide::mul_shr` on chain (`varswap-core`, measured by
  `program/tests/cu.rs`, 17 rows); metalbook calls `price_and_greeks`, `price_error_bound`,
  `from_scaled`, `to_scaled` and `sqrt_down` off chain only (`metalbook-maker`), so CU does not
  apply there, but compilation and its tests do. quotebook does not depend on optfix. All three
  repos are public and none has CI.
- optfix has no text parser. Its input decoders are `FixedBytes` (total: every 16 bytes are a
  value), `from_ratio`, `from_scaled` / `to_scaled`, `Years::from_seconds` and the quantity
  `try_new` checks. Milestone 17 adds the first parser (decimal strings).
- The bench program is already Pinocchio (`bench-program`, pinocchio 0.11 like quotebook), but
  it is a measurement harness, not an example a user can copy.

## Scope and order

| # | Milestone | Item | Size | Ships in |
| --- | --- | --- | --- | --- |
| 15 | Downstream CU guard and semver checks | 1 | M | CI only |
| 16 | Fuzzing | 2 | M | CI only |
| 17 | Decimal strings and `optfix-wasm` with conformance vectors | 3 | M | 0.3.0 |
| 18 | Pinocchio example program | 3 | S | 0.3.0 (repo only) |
| 19 | 0.3 API: rho, `from_bps`, `#[non_exhaustive]` outputs | 4 | M | 0.3.0 |
| 20 | Implied volatility: profile, then seed or stop | 5 | S, then M if the gate passes | 0.3.0 |
| 21 | Release 0.3.0 | | S | 0.3.0 |

The guards come first so every later milestone is checked by them on its own PR.

## Milestone 15: downstream CU guard and `cargo-semver-checks`

(Built.) `tools/downstream.sh` (clone at the pin, requirement rewrite, `cargo tree` assertion,
varswap's `make bench` or metalbook's `metalbook-maker` tests, ELF SHA-256),
`tools/downstream_cu.py` (same rows, columns and text, then no numeric cell may grow),
`ci/downstream.env`, and the `downstream` and `semver` jobs in `ci.yml`. Measured locally on
0.2.1: one varswap run takes 90 to 160 seconds warm and metalbook-maker's tests 20; base and
head give identical tables. Local failing control: optfix at 0dade38 with `#[inline(always)]`
removed from `U256::shr` grows exactly the rows of the 0.2.0 regression (`sample` with
neighbours +44, `finalize (complete)` +22, `open_swap` +46, `settle_swap` +46 and +69) and the
compare step fails; cargo-semver-checks 0.51.0 on host Rust 1.99.1 passes on main and fails a
public `rho` field on `Greeks` at 0.2.1 ("1 major ... checks failed"). The small-row rule also
tightens `Digest, 1 leg` and `Digest, 2 legs` (net 146 and 267), which are under 1,000 too.
CI on the final workflow: `downstream` takes about 7.5 minutes and `semver` about 4, in
parallel with the other jobs. On #29 itself the base (dea7adf) and head builds produce the same
ELF SHA-256 and the compare reports no change. CI failing controls (scratch PRs on the M15
branch, closed unmerged): #30 (`#[inline(always)]` removed from `U256::shr`) fails `downstream`
on the same rows and amounts as the local control, base and head ELFs differing; #31 (a public
`rho` field on `Greeks` at 0.2.1) fails `semver` with "semver requires new major version: 1
major and 0 minor checks failed", while its `downstream` passes (varswap does not use `Greeks`,
metalbook only reads its fields).

**Downstream job** (`downstream` in `ci.yml`, a new required check once green on main):

- Pins in `ci/downstream.env`: varswap and metalbook commit SHAs, bumped deliberately in their
  own PR (a moving `main` would let an unrelated downstream change fail optfix's CI).
- Base against head, on the same runner. The job builds varswap's program twice: once with
  optfix from the PR's merge base, once from the PR head, and runs varswap's `cu` test with
  `VARSWAP_BENCH_OUT` for each. `tools/downstream_cu.py` compares the two tables row by row and
  fails if any CU column of any row grows; decreases are reported in the job summary. This
  does not depend on varswap's committed `docs/benchmarks.md` (measured with whatever optfix
  release varswap pinned), so an optfix improvement can never be masked or a stale table blamed.
- How optfix is substituted: the job rewrites the downstream's `optfix = "0.2"` requirement to
  `optfix = { path = "<checkout>/optfix" }` in its workspace `Cargo.toml` and refreshes only
  that entry of its lockfile (`cargo update -p optfix`). A `[patch.crates-io]` would silently
  fall back to crates.io once the path's version stops matching the requirement (0.3.0 against
  `"0.2"`), so the job also asserts with `cargo tree -i optfix` that the path source is in use.
- metalbook: `cargo test -p metalbook-maker --locked` against the PR head only (off chain, no
  CU). It catches API breaks and behaviour changes its tests pin.
- Breaking milestones (19, and 20 if it changes results): the downstream code will not compile
  or will see new bits. The PR then bumps `ci/downstream.env` to a downstream branch commit
  migrated to the new API (opened in that repo through the coordinator); the job stays required.
  If a downstream migration is not ready, the coordinator decides whether to wait or to merge
  with the job temporarily non-required (recorded in the PR).
- Comparable measurements: `tools/downstream_cu.py` first checks that the base and head tables
  have the same scenario rows and the same columns, in the same order, and fails on any
  difference (a renamed, added or missing row is not a CU result). Base and head build in
  separate checkouts with separate target directories; for each, the job records `cargo tree -i
  optfix` (the path source and the optfix commit it points at) and the SHA-256 of the program
  ELF it built, and the `cu` test is pointed at that ELF explicitly, so a run can never load the
  other build's artifact. Both are printed in the job summary.
- Toolchain: the same pinned Agave, platform-tools and host Rust as the other jobs (the
  composite action), so the CU numbers are comparable between base and head.
- Failing control (a scratch PR, closed unmerged, as in milestone 9): removing
  `#[inline(always)]` from `U256::shr` must fail the job on varswap's `sample` rows; a PR that
  only edits a doc comment must pass.

**Own budget.** The rule that let `MulShr` drift stays for big rows, but rows under 1,000 CU get
the measurement plus 5% rounded up to a multiple of 10 instead of 100 while that stays at or
under 1,000 (`MulDown`, `DivDown`, `Exp`, `Ln`, `NormPdf`, `MulShr`, `Digest, 1 leg`, `Digest, 2
legs`; `Noop`, gross, keeps 200). `MulShr` becomes 340 / 350 (317 and 333 plus 5%).
The checker's unit tests gain the small-row rounding.

**`cargo-semver-checks`** (`semver` job, required): `cargo semver-checks check-release -p
optfix` against the latest published version, installed at a pinned version with `--locked`
(the newest release whose rustdoc JSON support covers host Rust 1.99.1, checked when the
milestone is built). On main today it compares 0.2.1 with 0.2.1. From milestone 19 the
workspace version is 0.3.0, which permits breaking changes, so the job then guards only
against a breaking change slipping into a 0.3.x patch later. Failing control: a scratch PR
adding a public field to `Greeks` without a version bump fails the job.

*Done when* both jobs are green on main and required, both failing controls are recorded here,
and the CI wall time stays under the 30-minute threshold of milestone 9 (the downstream job runs
in parallel; if it alone exceeds 30 minutes, base and head run as two jobs and a third compares
their uploaded tables).

## Milestone 16: fuzzing

(Built.) `fuzz/` (own workspace and lockfile; `libfuzzer-sys` 0.4, `num-bigint` 0.5), five
targets, `make fuzz-smoke`, the required `fuzz-smoke` job and `.github/workflows/fuzz-nightly.yml`
(30 minutes per target, nightly and on demand). As built:

- One definition of the contracts: `optfix/tests/common/contract.rs` depends on `optfix`,
  `std` and `num-bigint` only, is included by `#[path]` in `fuzz/`, and is what
  `common::iv::round_trip` (`iv_round_trip.rs`, `make precision`) and `binary_props.rs`'s
  identity now call. `optfix/tests/contract.rs` tests it on stable: its exponential against
  the 256-bit reference of `common` (within `2^-100`), its bounds accepting optfix over 20,000
  grid points, and rejecting a shifted price.
- Bounds that need `Dr` or `Dq` use an exact reference (`e^x` at 192 fractional bits from the
  exact products `rT`, `qT`), compared in units of `2^-128` with a 2 raw unit margin. A first
  version in `f64` failed on a valid deep in-the-money call (`S = 1.6 × 10^8`, `q = −0.1`, `T =
  29.5`, so `S Dq ≈ 3.06 × 10^9`): `f64` holds only about `5 × 10^-7` there, against an
  envelope of `5.7 × 10^-7` relative to `max(S, K)`. Exact, the point is inside.
- The round trip's `DomainError` is now accepted only where `σ` is within its envelope of a
  `Volatility` range end (the third documented case); the grid had none anyway.
- Inputs are fixed little-endian fields (`fuzz/src/lib.rs`, `Bytes`), not `arbitrary`
  structures, so an entry can be written by hand. The pricing target takes half its inputs as
  any in-range raw quantities (then `DomainError` exactly outside `S/K` in `[0.05, 20]`) and
  half from a sampler weighted toward the edges.
- Findings: none in optfix. Two in the harness, both fixed before commit: the `f64` bound
  above, and a payout generator that exceeded `Spot::MAX` (where `DomainError` is correct).
- Corpora minimized with `cargo fuzz cmin`: about 21 KB over 705 files, plus the control entry
  `fuzz/corpus/arith/planted-control-mul-up` (`mul_up` of a raw value whose low 16 bits are
  `0xBEEF` by 3), kept as a regular entry. Locally, `mul_up` patched to round down on that
  pattern fails `make fuzz-smoke` on that entry during the corpus replay, before libFuzzer's
  `INITED`; unpatched, every target passes.
- 30-minute local runs (Apple M-series, all five in parallel, nightly-2026-10-11): no
  failure; runs and final coverage `scaled` 128,178,716 (735), `arith` 187,191,548 (973),
  `wide` 209,139,220 (487), `pricing` 8,554,611 (964), `iv` 147,710,779 (410). `pricing` is
  the slow one: each input evaluates the exact reference exponential several times.

Complements the property tests (uniform or structured sampling, a million cases) with
coverage-guided search at the boundaries the samplers rarely hit. `fuzz/` is a `cargo-fuzz`
crate outside the workspace, with its own lockfile and a pinned nightly (`cargo-fuzz` needs
`-Zsanitizer`); the library keeps building on stable. Inputs come from `arbitrary` and are
mapped onto the supported domains, so a finding is a real bug, not an out-of-domain call.
Debug assertions stay on, so any arithmetic overflow panics.

Targets and the invariant each checks (exact oracles use `num-bigint`, already a dev
dependency):

1. `scaled`: `from_scaled` / `to_scaled` for every `decimals` in `0..=38` and both roundings:
   the result brackets the exact rational, down and up differ by at most one unit, and
   `Overflow` exactly when the exact value does not fit.
2. `arith`: `mul_*`, `div_*`, `sqrt_*`, `from_ratio` against the exact value: down ≤ exact ≤
   up, at most one ulp apart, equal when exact, errors exactly when the oracle says so.
3. `wide`: `wide::mul_shr` for every shift and both roundings, and the 256-bit division kernel
   through `div_*`, against `num-bigint`.
4. `pricing`: no panic on any input, valid or not, and each family checked against its own
   bounds and envelope (the families' envelopes differ, `optfix/src/bsm.rs` 343 to 351):
   - Vanilla (`price`, `price_and_greeks`, strips, Black-76), with `E = price_error_bound`
     (`PRICE_ERR_REL × max(S, K) + PRICE_ERR_ABS`): a call within `[max(0, S Dq − K Dr) − E,
     S Dq + E]`, a put within `[max(0, K Dr − S Dq) − E, K Dr + E]`; `price_black76` and
     every field of `price_and_greeks_black76` except `rho` bit-identical to the BSM calls with
     `S = F`, `q = r` (from milestone 19, Black-76 `rho` is checked on its own against
     `−T × price` within its envelope); each strip
     leg bit-identical to the scalar call, and on an error the earlier legs written and the rest
     untouched.
   - Cash-or-nothing, with `Eb = BINARY_ERR_REL × payout + BINARY_ERR_ABS` (the undiscounted
     payout, independent of `S` and `K`): within `[0, payout Dr + Eb]`; for `T > 0` call + put
     equals `payout Dr` within `2 Eb` plus one rounding of `Dr`. Payouts drawn with extra weight
     on `0`, raw `1` and `Spot::MAX`; a payout above `Spot::MAX` must return `DomainError`.
   - Asset-or-nothing, with `Ea = BINARY_ERR_REL × S + BINARY_ERR_ABS`: within `[0, S Dq + Ea]`.
   - `T = 0` for both binaries: exactly the payoff, paid only strictly in the money (`payout`
     or `S`), zero at and out of the money, with extra weight on `S = K`.
   - Identity for `T > 0`: asset call minus `K` times the unit-payout cash call equals `price`
     of the call within `Ea + K × Eb(1) + E` plus two roundings, the bound
     `optfix/tests/binary_props.rs` (`test_identities`) already uses.
5. `iv`: the contract of the existing round trip oracle (`optfix/tests/common/iv.rs`,
   `round_trip`), included by `#[path]` so there is one definition. For a valid `σ` and
   `target = price(σ)`: `T = 0` must return `DomainError`; `Ok(iv)` must sit within the `σ`
   envelope and the price round trip envelope and must not be below the conditioning threshold;
   `IllConditioned` must not be above it; `ArbitrageBound` is accepted only where the
   out-of-the-money value rounded to zero (the target has no representable time value) and
   never at a well-conditioned point; `DomainError` only where the input fails a documented
   domain check, recomputed independently; `NoConvergence`, `Overflow` or any other result is a
   failure. Conditioning is judged by the oracle's `conditioned` with its uncertainty band
   (`GRID_LOWER`), inside which either answer is accepted.
6. `decimal` (added by milestone 17): parsing never panics on arbitrary bytes, and
   `parse(format(x)) == x` for every `Fixed`.

CI: a `fuzz-smoke` job (required) runs each target for 60 seconds from the committed corpus;
a scheduled nightly workflow runs each for 30 minutes and uploads any crash as an artifact.
The corpora are minimized (`cargo fuzz cmin`) before committing and kept small (target: under
1 MB in total). A crash becomes a regression test in `optfix/tests/` on stable (its input
inlined), fixed in the same PR. `make fuzz-smoke` runs the PR job locally.

*Done when* the six targets (five here, the sixth with milestone 17) run clean for 30 minutes
each, the smoke job is required, and the failing control is deterministic: a scratch PR plants
a bug (`mul_up` rounding down on inputs whose low raw bits match a fixed pattern) together with
a committed corpus entry that triggers it, and the smoke job fails on that entry during corpus
replay, before any random search; the same PR without the planted bug passes.

## Milestone 17: decimal strings and `optfix-wasm`

**Decimal strings in the core crate** (additive, `no_std`, no allocation):
`Fixed::from_decimal_str(&str, Rounding) -> Result<Fixed, PricingError>` (optional sign,
digits, optional fraction, no exponent; `DomainError` on malformed input, `Overflow` out of
range, the fraction rounded explicitly), and `Fixed::write_decimal(&self, &mut impl
fmt::Write)` plus `Display`, which prints the exact value (a Q64.64 number has at most 64
decimal places; trailing zeros dropped). Exactness checked against `num-bigint` in property
tests and by the `decimal` fuzz target.

**`optfix-wasm`** (new workspace crate, `wasm-bindgen`, pinned version and matching
`wasm-bindgen-cli`): the public pricing surface (`price`, `price_and_greeks`, Black-76,
binaries, strips, `implied_vol`, `exp`, `ln`, `norm_cdf`, `norm_pdf`, the `Fixed` constructors
and arithmetic). Values cross the boundary as raw Q64.64 `bigint`s or as decimal strings, never
as `number`; errors as the `PricingError` code (`e as u8`) and name. No floating point, so the
workspace lints apply unchanged.

**Conformance vectors** (`conformance/vectors.json`, generated by `tools/conformance`, a host
crate): about 2,000 cases per function, drawn from the existing oracle fixtures
(`bsm_fixture.csv`, `iv_fixture.csv`, `binary_fixture.csv`, `frozen_0_1.csv`) plus the edge
sets, each with raw inputs, the exact native output bits or error code. Three consumers must
reproduce them bit for bit: a native test, a `wasm-bindgen-test` run under Node in CI, and the
bench program on SBF for a subset (the LiteSVM harness already compares SBF with host). SDKs in other repos can use it as their test vector.

Versioning: each generation is an immutable file `conformance/vectors-g<N>.json` (`g1` in
milestone 17), carrying a format version, the optfix version and the generating commit; a
generation is never edited after its merge. Milestones 19 (rho) and 20 (if it changes
`implied_vol` bits) each add the next generation, although both are 0.3.0, and keep every
earlier one, which is still replayed
for every output those milestones did not agree to change (its `implied_vol` rows, after
milestone 20, against the round trip envelope instead of bits), as the frozen 0.1 fixture is.

Distribution (coordinator, msg 43356): `optfix-wasm` is published on crates.io with `optfix`,
and the `wasm-pack` package tarball (pinned `wasm-pack`) is attached to the GitHub release. No
npm publication for now; the coordinator will say if that changes.

*Done when* the three consumers match the vectors in CI, the README documents both crates, and
the CI time stays under the threshold.

## Milestone 18: Pinocchio example program

`examples/pricer-program/`: a minimal on-chain consumer to copy, separate from the bench
harness. One instruction reads a Pyth-free set of inputs from instruction data (`FixedBytes`
for each value, a kind byte), prices with `price_and_greeks`, and returns `Priced` through
`set_return_data`; errors map with `e as u8`. A LiteSVM test checks the result against the host
call and records its CU; the example builds in the `checks` job with the same SBF stack check as
the bench program. It is documented in the README ("Using optfix on chain"), together with the
legs-per-budget table from `docs/benchmarks.md` (strip cost per extra leg: about 4,760 CU for
prices, 9,770 with greeks).

*Done when* the example builds and its test passes in CI.

## Milestone 19: the 0.3 API

All breaking changes of the cycle land here, and the workspace version becomes 0.3.0 in this
PR.

- **Rho.** `Greeks` gains `rho` (per unit of `r`): BSM `ρ = K T Dr N(d2)` for a call and
  `−K T Dr N(−d2)` for a put; Black-76 (`F` fixed) `ρ = −T × price`, which is the one greek
  where Black-76 differs from BSM with `q = r`, so the strip and Black-76 functions compute their
  own rho and keep every other field bit-identical. A published `RHO_ERR_REL` / `GREEK_ERR_ABS`
  style envelope, derived like the others and checked against regenerated `mpmath` fixtures
  (`tools/bsm_fixture`, a new column). The cost lands in `PriceAndGreeks` (and the greeks
  strips): measured, and the budget raised in this diff if it grows.
- **`#[non_exhaustive]` on the output structs** `Greeks` and `Priced`, so later fields (epsilon,
  binary greeks) are not breaking. The input structs (`EuropeanOption`, `MarketInputs`,
  `FuturesInputs`, `IvInputs`, `StripLeg`) stay exhaustive: callers build them with literals,
  as the README does (the decision of section 8, "Version").
- **`from_bps`** (additive, here for the release notes): `Fixed::from_bps(bps: i64, Rounding)`,
  and `Rate::from_bps`, `Volatility::from_bps` wrapping `try_new`. One basis point is not exact
  in binary, hence the explicit rounding.
- **Anything else breaking found by milestones 15 to 18** is added here, not earlier.
- The downstream job pins migrated varswap and metalbook commits (milestone 15, "Breaking
  milestones"). metalbook reads `greeks.delta` and friends, so its migration is a version bump;
  varswap does not use `Greeks`.

*Done when* the fixtures with rho pass, the frozen 0.1 fixture still matches for every
non-rho output, `cargo-semver-checks` passes at 0.3.0, and the downstream job is green.

## Milestone 20: implied volatility, profile then seed

Profiling first, then a gate. Section 8 recorded that 0.1's solve answers after 1 step for
1,869 grid points, 2 steps for 766,980 and 3 for 8,472 (of 3,000,000 grid points), about 5,200
CU per step, from a table seed within 6% of the root.

1. **Profile** (always done; S). A `bench-internals` hook reports, per bench input and over the
   codex 3,000,000-point grid on the host: the seed's relative error, the steps taken, and CU
   split into setup and seed, each step, and the final conditioning check (LiteSVM rows for
   the split). The result is recorded in this file.
2. **Gate.** Proceed only if a prototype seed, measured on the bench inputs, cuts the
   `ImpliedVol` median by at least 10% or removes a step from the worst case (so that
   `IV_MAX_ITERATIONS` can drop from 3 to 2 with the grid still answering everywhere it does
   today). Candidates, both within the existing scheme: finer interpolation tables
   (`tools/gen_iv_tables.py`), and a stopping rule that accepts a step whose cubic error bound
   is already below `IV_TOLERANCE` instead of taking a confirming step. Not a Jäckel port: its
   rational approximations are float designs and would need a new error analysis.
3. **If the gate passes** (M): the change keeps `IV_ERR_REL` and `IV_VEGA_MIN_FACTOR`, and the
   `Ok`/`IllConditioned` classification over `iv_fixture.csv` and both `iv_threshold` fixtures
   (any reclassified point is listed in the PR and must sit within the documented threshold
   margin `δ`). Output bits may change, which is why this is 0.3 work: the frozen 0.1 fixture's
   `implied_vol` rows switch from bit equality to the round trip envelope (an agreed change),
   and the downstream job sees the new bits. **If it fails**, the milestone closes with the
   profile recorded and no change to `implied_vol`.

*Done when* the profile is recorded and either the gate's change is merged with the fixtures
above, or the gate's failure is recorded.

## Milestone 21: release 0.3.0

`CHANGELOG.md` [0.3.0], README (install `optfix = "0.3"`, the wasm crate, the example, rho),
publish dry run of both crates, then on the coordinator's go: merge, signed tag `v0.3.0`,
`cargo publish -p optfix` and `-p optfix-wasm`, GitHub release with the wasm package tarball attached (no npm).
Downstream bumps to `"0.3"` follow through the coordinator.

## Decisions from the coordinator

1. npm publication of `optfix-wasm`: not now; GitHub release asset only (msg 43356).
2. `optfix-wasm` on crates.io: yes, under the existing crates authorization (msg 43356).
3. Downstream pins for breaking milestones: coordinated through the coordinator (msg 43356).
