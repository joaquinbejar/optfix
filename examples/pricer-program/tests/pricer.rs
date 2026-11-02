//! The example program in LiteSVM (`docs/PLAN-improvements.md`, milestone 18): its return data
//! equals the host's `price_and_greeks` and `price_error_bound` bit for bit, optfix errors and
//! malformed data fail the transaction with the documented program errors, and its compute
//! units stay under [`CU_LIMIT`]. Needs `make program` (which builds this program for SBF).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::panic_in_result_fn,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    missing_docs
)]

use std::path::PathBuf;

use litesvm::LiteSVM;
use optfix::{
    EuropeanOption, Fixed, MarketInputs, OptionKind, PricingError, Rate, Rounding, Spot, Strike,
    Volatility, Years,
};
use optfix_example_pricer::{DATA_LEN, OPTFIX_ERROR_BASE, RETURN_LEN};
use solana_address::Address;
use solana_instruction::error::InstructionError;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;

/// The most compute units one pricing may take (entrypoint, decoding and `price_and_greeks`
/// plus the envelope; `PriceAndGreeks` alone is 14,203 net in `docs/benchmarks.md`).
const CU_LIMIT: u64 = 16_000;

const PROGRAM_ID: Address = Address::new_from_array([9u8; 32]);

fn program() -> (LiteSVM, Keypair) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/deploy/optfix_example_pricer.so");
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; run `make program` first", path.display()));
    let mut svm = LiteSVM::new();
    svm.add_program(PROGRAM_ID, &bytes).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    (svm, payer)
}

/// Runs one instruction: `Ok((CU, return data))` or the instruction error.
fn run(
    svm: &mut LiteSVM,
    payer: &Keypair,
    data: &[u8],
) -> Result<(u64, Vec<u8>), InstructionError> {
    let ix = Instruction::new_with_bytes(PROGRAM_ID, data, vec![]);
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&payer.pubkey()),
        &[payer],
        svm.latest_blockhash(),
    );
    let out = svm.send_transaction(tx);
    svm.expire_blockhash();
    match out {
        Ok(meta) => Ok((meta.compute_units_consumed, meta.return_data.data)),
        Err(e) => match e.err {
            solana_transaction::TransactionError::InstructionError(0, ie) => Err(ie),
            other => panic!("transaction error {other:?}"),
        },
    }
}

fn fx(s: &str) -> Fixed {
    Fixed::from_decimal_str(s, Rounding::Down).unwrap()
}

/// The instruction data for raw inputs.
fn data(kind: u8, values: [Fixed; 6]) -> Vec<u8> {
    let mut d = vec![kind];
    for v in values {
        d.extend_from_slice(&v.to_bytes().0);
    }
    assert_eq!(d.len(), DATA_LEN);
    d
}

/// The host's expected return data.
fn expected(kind: u8, v: [Fixed; 6]) -> Result<Vec<u8>, PricingError> {
    let o = EuropeanOption {
        kind: if kind == 0 {
            OptionKind::Call
        } else {
            OptionKind::Put
        },
        strike: Strike::try_new(v[1])?,
        expiry: Years::try_new(v[2])?,
    };
    let m = MarketInputs {
        spot: Spot::try_new(v[0])?,
        vol: Volatility::try_new(v[3])?,
        rate: Rate::try_new(v[4])?,
        carry: Rate::try_new(v[5])?,
    };
    let p = optfix::price_and_greeks(&o, &m)?;
    let e = optfix::price_error_bound(&o, &m)?;
    let mut out = Vec::new();
    for x in [
        p.price.get(),
        p.greeks.delta,
        p.greeks.gamma,
        p.greeks.vega,
        p.greeks.theta,
        p.greeks.rho,
        e,
    ] {
        out.extend_from_slice(&x.to_bytes().0);
    }
    assert_eq!(out.len(), RETURN_LEN);
    Ok(out)
}

#[test]
fn test_prices_match_the_host_within_the_cu_limit() {
    let (mut svm, payer) = program();
    let cases = [
        (0u8, ["100", "105", "0.5", "0.3", "0.04", "0"]),
        (1, ["100", "95", "1", "0.2", "0.03", "0.01"]),
        (0, ["2350.75", "2400", "0.0833", "0.65", "0.05", "0"]),
        (1, ["0.000001", "0.00000105", "30", "5", "-0.1", "0.5"]),
        (0, ["1000000000", "60000000", "0", "0.005", "0.5", "-0.1"]),
    ];
    let mut worst = 0;
    for (kind, values) in cases {
        let v = values.map(fx);
        let (cu, got) = run(&mut svm, &payer, &data(kind, v)).unwrap();
        assert_eq!(got, expected(kind, v).unwrap(), "{values:?}");
        worst = worst.max(cu);
    }
    println!(
        "example pricer: at most {worst} CU over {} cases",
        cases.len()
    );
    assert!(worst <= CU_LIMIT, "{worst} CU over {CU_LIMIT}");
}

#[test]
fn test_errors_fail_with_the_documented_codes() {
    let (mut svm, payer) = program();
    // S/K below 0.05: optfix's DomainError.
    let v = ["100", "10000", "1", "0.2", "0", "0"].map(fx);
    let want = InstructionError::Custom(OPTFIX_ERROR_BASE + PricingError::DomainError as u32);
    assert_eq!(run(&mut svm, &payer, &data(0, v)), Err(want.clone()));
    // A volatility above the range: the quantity check, also DomainError.
    let v = ["100", "100", "1", "6", "0", "0"].map(fx);
    assert_eq!(run(&mut svm, &payer, &data(1, v)), Err(want));
    // Malformed: a short buffer and an unknown kind byte.
    let good = data(0, ["100", "100", "1", "0.2", "0", "0"].map(fx));
    assert_eq!(
        run(&mut svm, &payer, &good[..DATA_LEN - 1]),
        Err(InstructionError::InvalidInstructionData)
    );
    let mut bad_kind = good;
    bad_kind[0] = 2;
    assert_eq!(
        run(&mut svm, &payer, &bad_kind),
        Err(InstructionError::InvalidInstructionData)
    );
}
