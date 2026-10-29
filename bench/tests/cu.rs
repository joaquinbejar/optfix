//! Runs `optfix-bench-program` in LiteSVM and measures every instruction.
//!
//! - Every on-chain result equals the host result bit for bit, success and error alike.
//! - CU numbers are deterministic: the measurement runs twice in fresh VMs and must match.
//! - With `OPTFIX_BENCH_OUT=<path>` the table is written there (`make bench` uses
//!   `docs/benchmarks.md`); it is always printed.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::float_arithmetic,
    clippy::disallowed_macros,
    missing_docs
)]

use std::fmt::Write as _;
use std::path::PathBuf;

use litesvm::LiteSVM;
use optfix::repr::{Binary as Q64, Decimal, Repr};
use optfix::{Fixed, PricingError};
use solana_address::Address;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;

/// Instruction tags and return-data layout of `bench-program` (`bench-program/src/lib.rs`),
/// restated here so the check does not reuse the program's own encoder.
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
enum Ix {
    Noop = 0,
    MulDown = 1,
    DivDown = 2,
    SqrtDown = 3,
    ExpBinary = 4,
    LnBinary = 5,
    ExpDecimal = 6,
    LnDecimal = 7,
}

/// Status byte plus one 16-byte little-endian value.
const RESULT_LEN: usize = 17;

/// Expected return data: status `0` and the value, or `1 +` the error discriminant and zero.
fn encode_result(r: Result<Fixed, PricingError>) -> [u8; RESULT_LEN] {
    let mut out = [0u8; RESULT_LEN];
    match r {
        Ok(v) => out[1..].copy_from_slice(&v.raw().to_le_bytes()),
        Err(e) => out[0] = 1 + e as u8,
    }
    out
}

/// Random inputs per instruction.
const RANDOM_INPUTS: usize = 1_000;
/// Seed of the input generator.
const SEED: u64 = 0x0F7F_1C5B_E7C4_0002;

const PROGRAM_ID: Address = Address::new_from_array([7u8; 32]);

fn program_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/deploy/optfix_bench_program.so")
}

/// splitmix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value with magnitude log-uniform in `[10^-6, 10^9]` and a random sign: the range of
    /// prices and strikes in the supported domain.
    fn in_domain(&mut self) -> Fixed {
        // raw in [10^12, 10^27]: a decimal exponent, then a mantissa in [1, 10).
        let exp = 12 + (self.next() % 15) as u32;
        let mantissa = 1_000_000 + (self.next() % 9_000_000) as i128;
        let raw = mantissa * 10i128.pow(exp) / 1_000_000;
        Fixed::from_raw(if self.next() & 1 == 0 { raw } else { -raw })
    }

    /// Uniform in `[−42, 43]` with `one` as the raw value of 1: the `exp` arguments of
    /// pricing (discount and forward factors up to `e^±18`, `e^{−d²/2}`).
    fn exp_arg(&mut self, one: i128) -> Fixed {
        let span = 85 * one;
        let u = ((u128::from(self.next()) << 64) | u128::from(self.next())) % span as u128;
        Fixed::from_raw(u as i128 - 42 * one)
    }

    /// Log-uniform in `[10^-6, 10^9]` with `one` as the raw value of 1: prices, strikes and
    /// their ratios.
    fn ln_arg(&mut self, one: i128) -> Fixed {
        // 10^(-6 + 15 u) with u uniform in [0, 1), at 2^-53 resolution.
        let u = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        let v = 10f64.powf(-6.0 + 15.0 * u);
        Fixed::from_raw((v * one as f64) as i128)
    }

    /// Any raw value, log-uniform magnitude, both signs: exercises overflow and edge paths.
    fn any(&mut self) -> Fixed {
        let x = (u128::from(self.next()) << 64) | u128::from(self.next());
        let shift = (self.next() % 128) as u32;
        Fixed::from_raw((x >> shift) as i128)
    }
}

type Unary = fn(Fixed) -> Result<Fixed, PricingError>;
type Binary = fn(Fixed, Fixed) -> Result<Fixed, PricingError>;

#[derive(Clone, Copy)]
enum Op {
    Unary(Unary),
    Binary(Binary),
}

/// Inputs of a row.
#[derive(Clone, Copy)]
enum Domain {
    /// Pairs from [`Rng::in_domain`]; unary rows take the absolute value of the first.
    Prices,
    /// `exp` arguments: uniform in `[−42, 43]`, in units of `one`.
    ExpArg { one: i128 },
    /// `ln` arguments: log-uniform in `[10^-6, 10^9]`, in units of `one`.
    LnArg { one: i128 },
}

/// One measured instruction and the host function its result must equal.
struct Row {
    ix: Ix,
    name: &'static str,
    op: Op,
    domain: Domain,
}

/// `R::exp` or `R::ln` on the raw value of a `Fixed`, as the bench program computes it.
fn raw_fn<R: Repr, const EXP: bool>(x: Fixed) -> Result<Fixed, PricingError> {
    let f = if EXP { R::exp } else { R::ln };
    f(x.raw()).map(Fixed::from_raw)
}

const ROWS: [Row; 7] = [
    Row {
        ix: Ix::MulDown,
        name: "MulDown",
        op: Op::Binary(Fixed::mul_down),
        domain: Domain::Prices,
    },
    Row {
        ix: Ix::DivDown,
        name: "DivDown",
        op: Op::Binary(Fixed::div_down),
        domain: Domain::Prices,
    },
    Row {
        ix: Ix::SqrtDown,
        name: "SqrtDown",
        op: Op::Unary(Fixed::sqrt_down),
        domain: Domain::Prices,
    },
    Row {
        ix: Ix::ExpBinary,
        name: "ExpBinary",
        op: Op::Unary(raw_fn::<Q64, true>),
        domain: Domain::ExpArg { one: Q64::ONE },
    },
    Row {
        ix: Ix::LnBinary,
        name: "LnBinary",
        op: Op::Unary(raw_fn::<Q64, false>),
        domain: Domain::LnArg { one: Q64::ONE },
    },
    Row {
        ix: Ix::ExpDecimal,
        name: "ExpDecimal",
        op: Op::Unary(raw_fn::<Decimal, true>),
        domain: Domain::ExpArg { one: Decimal::ONE },
    },
    Row {
        ix: Ix::LnDecimal,
        name: "LnDecimal",
        op: Op::Unary(raw_fn::<Decimal, false>),
        domain: Domain::LnArg { one: Decimal::ONE },
    },
];

fn apply(op: Op, a: Fixed, b: Fixed) -> Result<Fixed, PricingError> {
    match op {
        Op::Unary(f) => f(a),
        Op::Binary(f) => f(a, b),
    }
}

fn data(row: &Row, a: Fixed, b: Fixed) -> Vec<u8> {
    let mut d = vec![row.ix as u8];
    d.extend_from_slice(&a.to_bytes().0);
    if let Op::Binary(_) = row.op {
        d.extend_from_slice(&b.to_bytes().0);
    }
    d
}

struct Vm {
    svm: LiteSVM,
    payer: Keypair,
}

impl Vm {
    fn new() -> Self {
        let mut svm = LiteSVM::new();
        let bytes = std::fs::read(program_path()).unwrap_or_else(|e| {
            panic!(
                "{}: {e}; run `make program` first",
                program_path().display()
            )
        });
        svm.add_program(PROGRAM_ID, &bytes)
            .expect("LiteSVM rejected the program ELF");
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
        Self { svm, payer }
    }

    /// Runs one instruction; returns (CU consumed, return data padded to `RESULT_LEN`).
    fn run(&mut self, data: &[u8]) -> (u64, [u8; RESULT_LEN]) {
        let ix = Instruction::new_with_bytes(PROGRAM_ID, data, vec![]);
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&self.payer.pubkey()),
            &[&self.payer],
            self.svm.latest_blockhash(),
        );
        let meta = self
            .svm
            .send_transaction(tx)
            .unwrap_or_else(|e| panic!("transaction failed: {:?}\n{:#?}", e.err, e.meta.logs));
        // A fresh blockhash keeps otherwise identical transactions distinct.
        self.svm.expire_blockhash();
        let mut out = [0u8; RESULT_LEN];
        let n = meta.return_data.data.len();
        assert!(n <= RESULT_LEN, "return data too long: {n}");
        out[..n].copy_from_slice(&meta.return_data.data);
        (meta.compute_units_consumed, out)
    }

    /// Runs `row` on `(a, b)` and checks the on-chain result against the host result.
    fn check(&mut self, row: &Row, a: Fixed, b: Fixed) -> u64 {
        let expected = encode_result(apply(row.op, a, b));
        let (cu, out) = self.run(&data(row, a, b));
        assert_eq!(
            out, expected,
            "{}({a:?}, {b:?}) on-chain differs from host",
            row.name
        );
        cu
    }
}

/// CU per instruction over the random in-domain inputs, in input order.
struct Measurement {
    noop: Vec<u64>,
    rows: Vec<Vec<u64>>,
}

fn measure() -> Measurement {
    let mut vm = Vm::new();
    let mut rng = Rng(SEED);
    let inputs: Vec<(Fixed, Fixed)> = (0..RANDOM_INPUTS)
        .map(|_| (rng.in_domain(), rng.in_domain()))
        .collect();

    let noop = inputs
        .iter()
        .enumerate()
        .map(|(i, (a, _))| {
            let mut d = vec![Ix::Noop as u8];
            d.extend_from_slice(&a.to_bytes().0);
            d.extend_from_slice(&(i as u64).to_le_bytes());
            let (cu, out) = vm.run(&d);
            assert_eq!(out, encode_result(Ok(*a)), "Noop echo");
            cu
        })
        .collect();

    let rows = ROWS
        .iter()
        .map(|row| {
            let mut rng = Rng(SEED);
            (0..RANDOM_INPUTS)
                .map(|i| {
                    let (a, b) = match row.domain {
                        // The shared pairs: `MulDown`, `DivDown` and `SqrtDown` keep the inputs
                        // of milestone 2.
                        Domain::Prices => inputs[i],
                        Domain::ExpArg { one } => (rng.exp_arg(one), Fixed::ZERO),
                        Domain::LnArg { one } => (rng.ln_arg(one), Fixed::ZERO),
                    };
                    // sqrt is measured on |a|: a negative input is a cheap DomainError.
                    let a = match (row.op, row.domain) {
                        (Op::Unary(_), Domain::Prices) => Fixed::from_raw(a.raw().abs()),
                        _ => a,
                    };
                    vm.check(row, a, b)
                })
                .collect()
        })
        .collect();
    Measurement { noop, rows }
}

fn stats(v: &[u64]) -> (u64, u64, u64) {
    let mut s = v.to_vec();
    s.sort_unstable();
    (s[0], s[s.len() / 2], s[s.len() - 1])
}

fn table(m: &Measurement) -> String {
    let (noop_lo, noop_med, noop_hi) = stats(&m.noop);
    let toolchain = std::env::var("OPTFIX_SBF_TOOLCHAIN").unwrap_or_else(|_| "unknown".into());
    let mut s = String::new();
    writeln!(s, "# Benchmarks").unwrap();
    writeln!(s).unwrap();
    writeln!(
        s,
        "Generated by `make bench` (`bench/tests/cu.rs`); do not edit by hand. Compute units \
         measured in LiteSVM 0.18 on the SBF build of `bench-program` ({toolchain}). Inputs: \
         {RANDOM_INPUTS} values or pairs from seed `{SEED:#x}`, magnitudes log-uniform in \
         `[10^-6, 10^9]`, random signs (`sqrt` takes the absolute value). Every on-chain \
         result equals the host result bit for bit, and two runs in fresh VMs give identical \
         numbers. Release profile: `lto = \"fat\"`, `codegen-units = 1`; a program built with \
         another profile will measure differently."
    )
    .unwrap();
    writeln!(s).unwrap();
    writeln!(
        s,
        "\"Net\" is the median minus the `Noop` median ({noop_med} CU), which covers the \
         entrypoint, decoding one `FixedBytes` argument and return data. `MulDown` and \
         `DivDown` decode two arguments, so their net figure still includes the second \
         decode; `SqrtDown` decodes one, like `Noop`."
    )
    .unwrap();
    writeln!(s).unwrap();
    writeln!(s, "| Instruction | Min | Median | Max | Net |").unwrap();
    writeln!(s, "| --- | ---: | ---: | ---: | ---: |").unwrap();
    writeln!(s, "| `Noop` | {noop_lo} | {noop_med} | {noop_hi} | 0 |").unwrap();
    for (row, cu) in ROWS.iter().zip(&m.rows) {
        let (lo, med, hi) = stats(cu);
        let net = med.saturating_sub(noop_med);
        writeln!(s, "| `{}` | {lo} | {med} | {hi} | {net} |", row.name).unwrap();
    }
    let agave = std::env::var("OPTFIX_AGAVE").unwrap_or_else(|_| "unknown".into());
    writeln!(s).unwrap();
    writeln!(s, "## Reproducing").unwrap();
    writeln!(s).unwrap();
    writeln!(
        s,
        "CU numbers depend on the toolchain and the build profile. This table was produced with:"
    )
    .unwrap();
    writeln!(s).unwrap();
    writeln!(s, "- Agave CLI: `{agave}`").unwrap();
    writeln!(s, "- SBF build: {toolchain}").unwrap();
    writeln!(s, "- LiteSVM 0.18.1, Agave crates 4.3.2 (`Cargo.lock`)").unwrap();
    writeln!(
        s,
        "- Release profile: `overflow-checks = true`, `lto = \"fat\"`, `codegen-units = 1`"
    )
    .unwrap();
    writeln!(s).unwrap();
    writeln!(s, "Run `make bench` to regenerate this file.").unwrap();
    s
}

#[test]
fn test_cu_table_is_deterministic_and_matches_host() {
    let first = measure();
    let second = measure();
    assert_eq!(first.noop, second.noop, "Noop CU differs between runs");
    assert_eq!(first.rows, second.rows, "CU differs between runs");
    let t = table(&first);
    println!("{t}");
    if let Ok(path) = std::env::var("OPTFIX_BENCH_OUT") {
        std::fs::write(&path, t).unwrap();
    }
}

#[test]
fn test_every_variant_matches_host_on_edge_inputs() {
    let mut vm = Vm::new();
    let mut rng = Rng(SEED ^ 0xED6E);
    let special = [
        Fixed::ZERO,
        Fixed::ONE,
        Fixed::MAX,
        Fixed::MIN,
        Fixed::from_raw(1),
        Fixed::from_raw(-1),
    ];
    let mut pairs: Vec<(Fixed, Fixed)> = special
        .iter()
        .flat_map(|&a| special.iter().map(move |&b| (a, b)))
        .collect();
    pairs.extend((0..RANDOM_INPUTS).map(|_| (rng.any(), rng.any())));
    for (a, b) in pairs {
        for row in &ROWS {
            vm.check(row, a, b);
        }
    }
}
