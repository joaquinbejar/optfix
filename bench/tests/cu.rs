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
use optfix::{
    price_and_greeks, EuropeanOption, Fixed, Greeks, MarketInputs, OptionKind, Priced,
    PricingError, Rate, Rounding, Spot, Strike, Volatility, Years,
};
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
    Exp = 4,
    Ln = 5,
    NormCdf = 6,
    NormPdf = 7,
    MulShr = 8,
    Price = 9,
    PriceAndGreeks = 10,
    Baseline = 11,
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

/// `2^64`, the raw value of 1.0, as an f64 (exact).
const ONE: f64 = 18_446_744_073_709_551_616.0;

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
        let v = self.log_uniform();
        Fixed::from_raw(if self.next() & 1 == 0 {
            v.raw()
        } else {
            -v.raw()
        })
    }

    /// `10^(−6 + 15 u)` with `u` uniform in `[0, 1)` at `2^-53` resolution.
    fn log_uniform(&mut self) -> Fixed {
        let u = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        Fixed::from_raw((10f64.powf(-6.0 + 15.0 * u) * ONE) as i128)
    }

    /// Uniform in `[−42, 43]`: the `exp` arguments of pricing (discount and forward factors
    /// up to `e^±18`, `e^{−d²/2}`).
    fn exp_arg(&mut self) -> Fixed {
        let u = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        Fixed::from_raw(((-42.0 + 85.0 * u) * ONE) as i128)
    }

    /// Uniform in `[−9.25, 9.25]`: the range where `N(x)` is computed rather than saturated.
    fn norm_arg(&mut self) -> Fixed {
        let u = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        Fixed::from_raw(((-9.25 + 18.5 * u) * ONE) as i128)
    }

    /// Log-uniform in `[10^-6, 10^9]`: prices, strikes and their ratios.
    fn ln_arg(&mut self) -> Fixed {
        self.log_uniform()
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
    /// `wide::mul_shr` on the raw bits of two values, shift 64, rounding down.
    MulShr,
}

/// The shift `MulShr` is measured with: a Q64.64 product.
const MUL_SHR_SHIFT: u32 = 64;

/// `x`'s 128 bits as a `u128`, and back.
fn bits(x: Fixed) -> u128 {
    u128::from_le_bytes(x.raw().to_le_bytes())
}

/// Inputs of a row.
#[derive(Clone, Copy)]
enum Domain {
    /// Pairs from [`Rng::in_domain`]; unary rows take the absolute value of the first.
    Prices,
    /// `exp` arguments: uniform in `[−42, 43]`.
    ExpArg,
    /// `ln` arguments: log-uniform in `[10^-6, 10^9]`.
    LnArg,
    /// Normal CDF and PDF arguments: uniform in `[−9.25, 9.25]`.
    NormArg,
    /// Pairs from [`Rng::any`], read as `u128`.
    Wide,
}

/// One measured instruction and the host function its result must equal.
struct Row {
    ix: Ix,
    name: &'static str,
    op: Op,
    domain: Domain,
}

const ROWS: [Row; 8] = [
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
        ix: Ix::Exp,
        name: "Exp",
        op: Op::Unary(optfix::exp),
        domain: Domain::ExpArg,
    },
    Row {
        ix: Ix::Ln,
        name: "Ln",
        op: Op::Unary(optfix::ln),
        domain: Domain::LnArg,
    },
    Row {
        ix: Ix::NormCdf,
        name: "NormCdf",
        op: Op::Unary(optfix::norm_cdf),
        domain: Domain::NormArg,
    },
    Row {
        ix: Ix::NormPdf,
        name: "NormPdf",
        op: Op::Unary(optfix::norm_pdf),
        domain: Domain::NormArg,
    },
    Row {
        ix: Ix::MulShr,
        name: "MulShr",
        op: Op::MulShr,
        domain: Domain::Wide,
    },
];

fn apply(op: Op, a: Fixed, b: Fixed) -> Result<Fixed, PricingError> {
    match op {
        Op::Unary(f) => f(a),
        Op::Binary(f) => f(a, b),
        Op::MulShr => optfix::wide::mul_shr(bits(a), bits(b), MUL_SHR_SHIFT, Rounding::Down)
            .map(|v| Fixed::from_raw(i128::from_le_bytes(v.to_le_bytes()))),
    }
}

fn data(row: &Row, a: Fixed, b: Fixed) -> Vec<u8> {
    let mut d = vec![row.ix as u8];
    d.extend_from_slice(&a.to_bytes().0);
    match row.op {
        Op::Binary(_) => d.extend_from_slice(&b.to_bytes().0),
        Op::MulShr => {
            d.extend_from_slice(&b.to_bytes().0);
            d.extend_from_slice(&MUL_SHR_SHIFT.to_le_bytes());
            d.push(0);
        }
        Op::Unary(_) => {}
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
        let (cu, data) = self.run_raw(data);
        let mut out = [0u8; RESULT_LEN];
        assert!(
            data.len() <= RESULT_LEN,
            "return data too long: {}",
            data.len()
        );
        out[..data.len()].copy_from_slice(&data);
        (cu, out)
    }

    /// Runs one instruction; returns (CU consumed, return data).
    fn run_raw(&mut self, data: &[u8]) -> (u64, Vec<u8>) {
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
        (meta.compute_units_consumed, meta.return_data.data)
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
    bsm: Vec<Vec<u64>>,
}

/// The Black-Scholes rows: instruction, name, host function.
type BsmFn = fn(&EuropeanOption, &MarketInputs) -> Result<Priced, PricingError>;

fn price_only(o: &EuropeanOption, m: &MarketInputs) -> Result<Priced, PricingError> {
    Ok(Priced {
        price: optfix::price(o, m)?,
        greeks: Greeks {
            delta: Fixed::ZERO,
            gamma: Fixed::ZERO,
            vega: Fixed::ZERO,
            theta: Fixed::ZERO,
        },
    })
}

const BSM_ROWS: [(Ix, &str, BsmFn); 3] = [
    (Ix::Price, "Price", price_only),
    (Ix::PriceAndGreeks, "PriceAndGreeks", price_and_greeks),
    (
        Ix::Baseline,
        "Baseline (price, then each greek)",
        optfix::baseline::separately,
    ),
];

/// One Black-Scholes input over the supported domain: `S` log-uniform in `[10^-6, 10^9]`,
/// `S/K` log-uniform in `[0.05, 20]`, `T` uniform in `(0, 30]`, `σ` log-uniform in `[0.005, 5]`,
/// `r` and `q` uniform in `[−0.1, 0.5]`, call or put.
fn bsm_input(rng: &mut Rng) -> (EuropeanOption, MarketInputs) {
    let mut u = || (rng.next() >> 11) as f64 / (1u64 << 53) as f64;
    let s = 10f64.powf(-6.0 + 15.0 * u());
    let k = (s * 10f64.powf((0.05f64).log10() + (400f64).log10() * u())).clamp(1.01e-6, 0.99e9);
    let k = k.clamp(s / 19.9, s * 19.9);
    let t = 30.0 * u().max(1e-9);
    let sigma = 10f64.powf((0.005f64).log10() + 3.0 * u()).min(5.0);
    let (r, q) = (-0.1 + 0.6 * u(), -0.1 + 0.6 * u());
    let fx = |v: f64| Fixed::from_raw((v * ONE) as i128);
    let call = u() < 0.5;
    (
        EuropeanOption {
            kind: if call {
                OptionKind::Call
            } else {
                OptionKind::Put
            },
            strike: Strike::try_new(fx(k)).unwrap(),
            expiry: Years::try_new(fx(t)).unwrap(),
        },
        MarketInputs {
            spot: Spot::try_new(fx(s)).unwrap(),
            vol: Volatility::try_new(fx(sigma)).unwrap(),
            rate: Rate::try_new(fx(r)).unwrap(),
            carry: Rate::try_new(fx(q)).unwrap(),
        },
    )
}

/// Instruction data of a Black-Scholes row: tag, kind byte, six 16-byte values.
fn bsm_data(ix: Ix, o: &EuropeanOption, m: &MarketInputs) -> Vec<u8> {
    let mut d = vec![ix as u8, o.kind as u8];
    for v in [
        m.spot.get(),
        o.strike.get(),
        o.expiry.get(),
        m.vol.get(),
        m.rate.get(),
        m.carry.get(),
    ] {
        d.extend_from_slice(&v.to_bytes().0);
    }
    d
}

/// Expected return data: `Price` as one value, the others as status plus five values.
fn bsm_expected(ix: Ix, r: Result<Priced, PricingError>) -> Vec<u8> {
    if let Ix::Price = ix {
        return encode_result(r.map(|p| p.price.get())).to_vec();
    }
    let mut out = vec![0u8; 81];
    match r {
        Ok(p) => {
            for (i, v) in [
                p.price.get(),
                p.greeks.delta,
                p.greeks.gamma,
                p.greeks.vega,
                p.greeks.theta,
            ]
            .iter()
            .enumerate()
            {
                out[1 + 16 * i..17 + 16 * i].copy_from_slice(&v.raw().to_le_bytes());
            }
        }
        Err(e) => out[0] = 1 + e as u8,
    }
    out
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
                        Domain::ExpArg => (rng.exp_arg(), Fixed::ZERO),
                        Domain::LnArg => (rng.ln_arg(), Fixed::ZERO),
                        Domain::NormArg => (rng.norm_arg(), Fixed::ZERO),
                        Domain::Wide => (rng.any(), rng.any()),
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
    let mut rng = Rng(SEED ^ 0xB5);
    let bsm_inputs: Vec<_> = (0..RANDOM_INPUTS).map(|_| bsm_input(&mut rng)).collect();
    let bsm = BSM_ROWS
        .iter()
        .map(|&(ix, name, f)| {
            bsm_inputs
                .iter()
                .map(|(o, m)| {
                    let (cu, out) = vm.run_raw(&bsm_data(ix, o, m));
                    assert_eq!(
                        out,
                        bsm_expected(ix, f(o, m)),
                        "{name} on-chain differs from host at {o:?} {m:?}"
                    );
                    cu
                })
                .collect()
        })
        .collect();
    Measurement { noop, rows, bsm }
}

fn stats(v: &[u64]) -> (u64, u64, u64) {
    let mut s = v.to_vec();
    s.sort_unstable();
    (s[0], s[s.len() / 2], s[s.len() - 1])
}

/// Milestone 3 record: `make bench` at commit `6adf034` reproduces it; the decimal candidates
/// and the atanh form no longer exist.
const COMPARISON: &str = "
## Representation comparison (milestone 3)

Measured at commit `6adf034`, where `make bench` reproduces it, with the inputs above: each \
candidate of `docs/PLAN.md` 4.1 on the same arguments and build profile. \"Decimal, binary \
core\" converts in and out of the 64-bit-digit kernel that Q64.64 uses; \"decimal, native\" \
runs the same reduction and polynomials as decimal `Fixed` multiplies. Net CU, `Noop` \
(164 CU) subtracted:

| Representation | `exp` median | `exp` max | `ln` median | `ln` max | `exp` + `ln` median |
| --- | ---: | ---: | ---: | ---: | ---: |
| Q64.64 | 337 | 352 | 460 | 465 | 797 |
| Q64.64, `ln` atanh form | | | 614 | 655 | |
| Decimal `10^18`, binary core | 462 | 479 | 470 | 529 | 932 |
| Decimal `10^18`, native | 7,989 | 8,122 | 9,051 | 9,186 | 17,040 |

Q64.64 is 15% cheaper than the best decimal form, outside the 10% band in which decimal \
would have won for its exact token conversions (`docs/PLAN.md`, decision log); it is also \
the more accurate (`docs/precision.md`).
";

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
         {RANDOM_INPUTS} values or pairs from seed `{SEED:#x}`: for `MulDown`, `DivDown` and \
         `SqrtDown` magnitudes log-uniform in `[10^-6, 10^9]` with random signs (`sqrt` takes \
         the absolute value); `exp` arguments uniform in `[−42, 43]`; `ln` arguments \
         log-uniform in `[10^-6, 10^9]`; `norm_cdf` and `norm_pdf` arguments uniform in \
         `[−9.25, 9.25]`; `MulShr` (`wide::mul_shr`, shift 64, rounding down) two `u128` \
         with log-uniform magnitudes over the whole range; the Black-Scholes rows `S` \
         log-uniform in `[10^-6, 10^9]`, `S/K` log-uniform in `[0.05, 20]`, `T` uniform in \
         `(0, 30]`, `σ` log-uniform in `[0.005, 5]`, `r` and `q` uniform in `[−0.1, 0.5]`, \
         calls and puts. Every on-chain result equals the host result bit for bit, and two runs in fresh VMs give identical \
         numbers. Release profile: `lto = \"fat\"`, `codegen-units = 1`; a program built with \
         another profile will measure differently."
    )
    .unwrap();
    writeln!(s).unwrap();
    writeln!(
        s,
        "\"Net\" is the median minus the `Noop` median ({noop_med} CU), which covers the \
         entrypoint, decoding one `FixedBytes` argument and return data. `MulDown` and \
         `DivDown` decode two arguments, `MulShr` two plus a shift and a rounding, and the \
         Black-Scholes rows a kind byte and six, so \
         their net figures still include the extra decoding; the other rows decode one, like \
         `Noop`."
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
    for ((_, name, _), cu) in BSM_ROWS.iter().zip(&m.bsm) {
        let (lo, med, hi) = stats(cu);
        let net = med.saturating_sub(noop_med);
        writeln!(s, "| `{name}` | {lo} | {med} | {hi} | {net} |").unwrap();
    }
    let agave = std::env::var("OPTFIX_AGAVE").unwrap_or_else(|_| "unknown".into());
    writeln!(s).unwrap();
    s.push_str(COMPARISON);
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
    assert_eq!(
        first.bsm, second.bsm,
        "Black-Scholes CU differs between runs"
    );
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
