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

# Build bench-program for SBF into target/deploy. cargo-build-sbf's exit status is kept (no
# pipe), and scripts/check-sbf-log.sh fails the build on a stack frame above 4 KiB, which
# cargo-build-sbf only warns about.
.PHONY: program
program:
	@mkdir -p target
	@cargo build-sbf --manifest-path bench-program/Cargo.toml --arch $(SBF_ARCH) \
		> target/build-sbf.log 2>&1; status=$$?; cat target/build-sbf.log; \
		if [ $$status -ne 0 ]; then echo "program: cargo-build-sbf failed ($$status)"; exit $$status; fi
	@sh scripts/check-sbf-log.sh target/build-sbf.log

# Failing control for `program`: the log check must reject a frame overflow in both wordings,
# and a failing cargo-build-sbf must fail the target.
.PHONY: program-control
program-control:
	@mkdir -p target
	@printf '%s\n' 'Error: Function entrypoint overflows the maximum allowed frame space by accessing an offset 64 bytes greater than the maximum of 4096.' \
		> target/sbf-control.log
	@if sh scripts/check-sbf-log.sh target/sbf-control.log > /dev/null; then \
		echo "program-control: the log check accepted a frame overflow"; exit 1; fi
	@printf '%s\n' 'Error: Function _ZN4main Stack offset of 4160 exceeded max offset of 4096 by 64 bytes' \
		> target/sbf-control.log
	@if sh scripts/check-sbf-log.sh target/sbf-control.log > /dev/null; then \
		echo "program-control: the log check accepted the older wording"; exit 1; fi
	@if $(MAKE) --no-print-directory program SBF_ARCH=no-such-arch > /dev/null 2>&1; then \
		echo "program-control: make program succeeded with a failing cargo-build-sbf"; exit 1; fi
	@echo "program-control: a frame overflow and a failing cargo-build-sbf both fail make program"

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
	cargo test -p optfix --release --features bench-internals --test precision -- --nocapture

# Regenerate README.md's example and tables from the constants and docs/benchmarks.md
# (`make test` fails while it is stale)
.PHONY: readme
readme:
	OPTFIX_README_WRITE=1 cargo test -p optfix --test readme

# Package optfix as crates.io would and build the package; never uploads
.PHONY: publish-dry-run
publish-dry-run:
	cargo publish -p optfix --dry-run

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

# The generated tables are reproducible: regenerating them (python3, standard library only) and
# formatting with rustfmt must give the committed files byte for byte.
KERNEL_TABLES ?= optfix/src/kernel/tables.rs
IV_TABLES ?= optfix/src/iv_tables.rs
.PHONY: tables-check
tables-check:
	@mkdir -p target
	@python3 tools/gen_tables.py > target/tables.rs && rustfmt +stable --edition 2021 target/tables.rs
	@cmp target/tables.rs $(KERNEL_TABLES) || \
		{ echo "tables-check: $(KERNEL_TABLES) is not what tools/gen_tables.py writes"; exit 1; }
	@python3 tools/gen_iv_tables.py > target/iv_tables.rs && rustfmt +stable --edition 2021 target/iv_tables.rs
	@cmp target/iv_tables.rs $(IV_TABLES) || \
		{ echo "tables-check: $(IV_TABLES) is not what tools/gen_iv_tables.py writes"; exit 1; }
	@echo "tables-check: both generated table files regenerate byte for byte"

# Failing control for `tables-check`: run against a copy of iv_tables.rs with one entry
# changed by one unit, it must fail.
.PHONY: tables-control
tables-control:
	@mkdir -p target
	@python3 -c 'import re, sys; t = open(sys.argv[1]).read(); \
		print(re.sub(r"(\n    \[-?)(\d+)", lambda m: m.group(1) + str(int(m.group(2)) + 1), t, count=1), end="")' \
		$(IV_TABLES) > target/iv_tables.control.rs
	@if cmp -s target/iv_tables.control.rs $(IV_TABLES); then \
		echo "tables-control: the control copy is unchanged"; exit 1; fi
	@if $(MAKE) --no-print-directory tables-check IV_TABLES=target/iv_tables.control.rs > /dev/null 2>&1; then \
		echo "tables-control: tables-check accepted a changed iv_tables.rs"; exit 1; fi
	@echo "tables-control: tables-check rejects a changed generated file"

# Rustdoc with every warning denied
.PHONY: doc
doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features

# Pre-push checks: must pass with zero warnings
.PHONY: pre-push
pre-push: fmt-check lint lint-control tables-control tables-check program-control program test doc

.PHONY: clean
clean:
	cargo clean
