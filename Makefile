# Makefile for the optfix workspace. Targets mirror matchbook's; `pre-push` follows
# rules/global_rules.md (fmt check, clippy -D warnings, tests, docs -D warnings).

.PHONY: all
all: fmt-check lint test

# Build the workspace
.PHONY: build
build:
	cargo build --workspace

# Run every test, including the million-case property tests.
# OPTFIX_PROPTEST_CASES=<n> lowers the case count for a quick run.
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

# Rustdoc with every warning denied
.PHONY: doc
doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features

# Pre-push checks: must pass with zero warnings
.PHONY: pre-push
pre-push: fmt-check lint test doc

.PHONY: clean
clean:
	cargo clean
