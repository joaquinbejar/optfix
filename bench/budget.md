# CU budget

Enforced by `make bench` and `make test` (`bench/tests/cu.rs`, checker in `bench/src/lib.rs`): a
measured row over either limit, a measured row without a budget, a budget row that is no longer
measured, a duplicate row or a malformed row fails. Limits are net CU (the row's median or max
minus the `Noop` median, as in `docs/benchmarks.md` and the README), except `Noop`'s own, which
are gross. Edit by hand only: the bench never rewrites this file. Rules (`docs/PLAN.md`,
milestone 8): a new or deliberately dearer row gets its measurement plus 5%, rounded up to a
multiple of 100, in the same diff; a cheaper row is tightened the same way.

| Row | Median limit | Max limit |
| --- | ---: | ---: |
| `Noop` | 200 | 200 |
| `MulDown` | 300 | 400 |
| `DivDown` | 600 | 700 |
| `SqrtDown` | 2400 | 2600 |
| `Exp` | 400 | 400 |
| `Ln` | 500 | 500 |
| `NormCdf` | 1400 | 1500 |
| `NormPdf` | 700 | 700 |
| `MulShr` | 400 | 400 |
| `Price` | 9700 | 10100 |
| `PriceAndGreeks` | 15000 | 15400 |
| `Baseline (price, then each greek)` | 41300 | 42700 |
| `Price, near the money` | 11700 | 12100 |
| `PriceAndGreeks, near the money` | 16900 | 17400 |
| `ImpliedVol` | 28500 | 36500 |
| `ImpliedVol, all iterations` | 36200 | 44300 |
| `ImpliedVol, near the money` | 34400 | 44100 |
| `ImpliedVol, all iterations, near the money` | 44000 | 45000 |
| `PriceStrip, 1 leg` | 10000 | 10300 |
| `PriceStrip, 2 legs` | 15200 | 15600 |
| `PriceStrip, 8 legs` | 45500 | 50000 |
| `PriceStrip, 16 legs` | 85000 | 92400 |
| `PriceAndGreeksStrip, 1 leg` | 15400 | 15800 |
| `PriceAndGreeksStrip, 2 legs` | 26000 | 26600 |
| `PriceAndGreeksStrip, 8 legs` | 88100 | 93400 |
| `PriceAndGreeksStrip, 16 legs` | 171200 | 179300 |
| `Digest, 1 leg` | 200 | 200 |
| `Digest, 2 legs` | 300 | 300 |
| `Digest, 8 legs` | 1100 | 1100 |
| `Digest, 16 legs` | 2100 | 2100 |
