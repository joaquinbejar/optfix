# CU budget

Enforced by `make bench` and `make test` (`bench/tests/cu.rs`, checker in `bench/src/lib.rs`): a
measured row over either limit, a measured row without a budget, a budget row that is no longer
measured, a duplicate row or a malformed row fails. Limits are net CU (the row's median or max
minus the `Noop` median, as in `docs/benchmarks.md` and the README), except `Noop`'s own, which
are gross. Edit by hand only: the bench never rewrites this file. Rules (`docs/PLAN.md`,
milestone 8, and `docs/PLAN-improvements.md`, milestone 15): a new or deliberately dearer row
gets its measurement plus 5%, rounded up to a multiple of 10 while that is at most 1,000 and to a
multiple of 100 above, in the same diff; a cheaper row is tightened the same way.

| Row | Median limit | Max limit |
| --- | ---: | ---: |
| `Noop` | 200 | 200 |
| `MulDown` | 290 | 310 |
| `DivDown` | 580 | 680 |
| `SqrtDown` | 2400 | 2500 |
| `Exp` | 340 | 360 |
| `Ln` | 480 | 490 |
| `NormCdf` | 1400 | 1500 |
| `NormPdf` | 610 | 630 |
| `MulShr` | 340 | 350 |
| `Price` | 9700 | 10100 |
| `PriceAndGreeks` | 15300 | 15700 |
| `Baseline (price, then each greek)` | 48900 | 50600 |
| `Price, near the money` | 11700 | 12100 |
| `PriceBlack76` | 9800 | 10100 |
| `PriceAndGreeksBlack76` | 15600 | 16000 |
| `CashOrNothing` | 7700 | 8000 |
| `AssetOrNothing` | 7300 | 7600 |
| `PriceAndGreeks, near the money` | 17200 | 17700 |
| `ImpliedVol` | 28500 | 34600 |
| `ImpliedVol, all iterations` | 28500 | 34600 |
| `ImpliedVol, near the money` | 34400 | 35300 |
| `ImpliedVol, all iterations, near the money` | 34400 | 35300 |
| `PriceStrip, 1 leg` | 10000 | 10300 |
| `PriceStrip, 2 legs` | 15200 | 15700 |
| `PriceStrip, 8 legs` | 45500 | 50100 |
| `PriceStrip, 16 legs` | 85100 | 92500 |
| `PriceAndGreeksStrip, 1 leg` | 15700 | 16100 |
| `PriceAndGreeksStrip, 2 legs` | 26600 | 27200 |
| `PriceAndGreeksStrip, 8 legs` | 90500 | 95800 |
| `PriceAndGreeksStrip, 16 legs` | 176100 | 184300 |
| `Digest, 1 leg` | 160 | 160 |
| `Digest, 2 legs` | 300 | 300 |
| `Digest, 8 legs` | 1200 | 1200 |
| `Digest, 16 legs` | 2200 | 2200 |
