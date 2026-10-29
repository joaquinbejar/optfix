//! Compute-unit benchmarks for optfix.
//!
//! The harness lives in `tests/cu.rs`: it loads the SBF build of `optfix-bench-program` into
//! LiteSVM, runs every instruction on deterministic inputs, checks each on-chain result against
//! the host result bit for bit, and prints the CU table (`make bench` writes it to
//! `docs/benchmarks.md`). Build the program first with `make program`.
