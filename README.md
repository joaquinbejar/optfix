# optfix

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Fixed-point option pricing for Solana programs: `no_std`, allocation-free and measured in compute units.

Black-Scholes prices, greeks and implied volatility in integer arithmetic, with `ln`, `exp` and the normal CDF built to fit a program's compute budget. Every function ships with its error bound against an `f64` reference and its cost in CU, measured in LiteSVM.

**Status:** design phase, no code yet.
