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

# Failing control: clippy must reject each panicking macro in lint-control's library code
# (compiled only under --cfg lint_control, in its own target dir so the cfg does not leak).
# No -D warnings here, so it also fails if [workspace.lints] stops denying the lint.
LINT_CONTROL_MACROS = assert assert_eq assert_ne debug_assert debug_assert_eq debug_assert_ne panic unreachable
.PHONY: lint-control
lint-control:
	@out="$$(RUSTFLAGS='--cfg lint_control' cargo clippy -p lint-control --lib \
		--target-dir target/lint-control 2>&1)"; \
	if [ $$? -eq 0 ]; then echo "lint-control: clippy accepted panicking macros in lib code"; exit 1; fi; \
	for m in $(LINT_CONTROL_MACROS); do \
		echo "$$out" | grep -q "^error: use of a disallowed macro \`[a-z]*::$$m\`" \
			|| { echo "$$out"; echo "lint-control: $$m! was not rejected"; exit 1; }; \
	done; \
	echo "lint-control: clippy rejects $(LINT_CONTROL_MACROS)"

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
