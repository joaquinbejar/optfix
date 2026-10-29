# Makefile for the optfix workspace. Targets mirror matchbook's; `pre-push` follows
# rules/global_rules.md (fmt check, clippy -D warnings, tests, docs -D warnings).

.PHONY: all
all: fmt-check lint test

# sBPF target version for the bench program (cargo-build-sbf's default is v3).
SBF_ARCH ?= v3

# Build the workspace
.PHONY: build
build:
	cargo build --workspace

# Build bench-program for SBF into target/deploy
.PHONY: program
program:
	cargo build-sbf --manifest-path bench-program/Cargo.toml --arch $(SBF_ARCH)

# Run bench-program in LiteSVM and regenerate docs/benchmarks.md
.PHONY: bench
bench: program
	OPTFIX_SBF_TOOLCHAIN="$$(cargo build-sbf --version 2>/dev/null | paste -sd ',' - | sed 's/,/, /g'), sBPF $(SBF_ARCH)" \
	OPTFIX_AGAVE="$$(solana --version 2>/dev/null)" \
	OPTFIX_BENCH_OUT=$(CURDIR)/docs/benchmarks.md \
	cargo test -p optfix-bench --release --test cu -- --nocapture

# Maximum exp and ln errors over a deterministic sample; regenerates docs/precision.md
PRECISION_SAMPLES ?= 200000
.PHONY: precision
precision:
	OPTFIX_PRECISION_SAMPLES=$(PRECISION_SAMPLES) OPTFIX_PRECISION_OUT=$(CURDIR)/docs/precision.md \
	cargo test -p optfix --release --test exp_ln_props test_precision_report -- --nocapture

# Run every test, including the million-case property tests and the LiteSVM bench
# (which needs `make program` first). OPTFIX_PROPTEST_CASES=<n> lowers the case count.
.PHONY: test
test:
	cargo test --workspace --all-features

# Format the code
.PHONY: fmt
fmt:
	cargo +stable fmt --all

# Check formatting
.PHONY: fmt-check
fmt-check:
	cargo +stable fmt --all --check

# Clippy with every warning denied
.PHONY: lint
lint:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

# Failing control: clippy must reject every banned macro in lint-control's library code, on
# each probe line with the exact definition path (see lint-control/check.sh).
.PHONY: lint-control
lint-control:
	@sh lint-control/check.sh

# Rustdoc with every warning denied
.PHONY: doc
doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features

# Pre-push checks: must pass with zero warnings
.PHONY: pre-push
pre-push: fmt-check lint lint-control program test doc

.PHONY: clean
clean:
	cargo clean
