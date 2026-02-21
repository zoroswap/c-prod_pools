use anyhow::{Result, anyhow};
use miden_client::{
    Felt, Word,
    account::AccountId,
    assembly::Library,
    asset::FungibleAsset,
    crypto::FeltRng,
    note::{Note, NoteAssets, NoteMetadata, NoteRecipient, NoteTag, NoteType},
};
use miden_protocol::{
    FieldElement,
    note::{NoteInputs, NoteScript},
    transaction::{TransactionKernel, TransactionScript},
};
use std::{fs, path::PathBuf};

use crate::utils::create_library;

/// Compiles the pool MASM library from source.
pub fn get_pool_library() -> Result<Library> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "masm", "accounts", "c_prod_pool.masm"]
        .iter()
        .collect();
    let source = fs::read_to_string(&path)?;
    let assembler = TransactionKernel::assembler().with_warnings_as_errors(true);
    create_library(assembler, "zoro::c_prod_pool", &source)
        .map_err(|e| anyhow!("Failed to compile pool library: {e}"))
}

/// Compiles a transaction script from arbitrary MASM source, linked against the pool library.
pub fn compile_custom_tx_script(pool_library: &Library, source: &str) -> Result<TransactionScript> {
    let assembler = TransactionKernel::assembler()
        .with_warnings_as_errors(true)
        .with_static_library(pool_library.clone())
        .map_err(|e| anyhow!("Failed to add pool library to assembler: {e}"))?;
    let program = assembler
        .assemble_program(source)
        .map_err(|e| anyhow!("Failed to compile tx script: {e}"))?;
    Ok(TransactionScript::new(program))
}

/// Compiles a transaction script that calls the given pool procedure via `call`.
pub fn compile_pool_tx_script(
    pool_library: &Library,
    procedure_name: &str,
) -> Result<TransactionScript> {
    let source =
        format!("use zoro::c_prod_pool\nbegin\n    exec.c_prod_pool::{procedure_name}\nend");
    compile_custom_tx_script(pool_library, &source)
}

/// Compiles a note script that calls the given pool procedure via `call`.
pub fn compile_pool_note_script(
    pool_library: &Library,
    procedure_name: &str,
) -> Result<NoteScript> {
    let source =
        format!("use.zoro::c_prod_pool\nbegin\n    call.c_prod_pool::{procedure_name}\nend");
    let assembler = TransactionKernel::assembler()
        .with_warnings_as_errors(true)
        .with_static_library(pool_library.clone())
        .map_err(|e| anyhow!("Failed to add pool library to assembler: {e}"))?;
    let program = assembler
        .assemble_program(source)
        .map_err(|e| anyhow!("Failed to compile {procedure_name} note script: {e}"))?;
    Ok(NoteScript::new(program))
}

/// Builds a deposit note targeting the pool.
///
/// For first deposits (`current_total_lp == 0`), the sqrt hint is included in note inputs.
pub fn build_deposit_note(
    pool_id: AccountId,
    pool_library: &Library,
    token0_asset: FungibleAsset,
    token1_asset: FungibleAsset,
    min_lp_out: u64,
    current_total_lp: u64,
    sender: AccountId,
    rng: &mut impl FeltRng,
) -> Result<Note> {
    let script = compile_pool_note_script(pool_library, "deposit")?;

    let sqrt_hint = if current_total_lp == 0 {
        let product = token0_asset.amount() as u128 * token1_asset.amount() as u128;
        isqrt(product) as u64
    } else {
        0
    };

    let inputs = NoteInputs::new(vec![
        Felt::new(1),
        Felt::new(min_lp_out),
        Felt::new(sqrt_hint),
    ])?;

    let assets = NoteAssets::new(vec![token0_asset.into(), token1_asset.into()])?;

    let tag = NoteTag::with_account_target(pool_id);
    let metadata = NoteMetadata::new(sender, NoteType::Public, tag);

    let serial_num = rng.draw_word();
    let recipient = NoteRecipient::new(serial_num, script, inputs);
    Ok(Note::new(assets, metadata, recipient))
}

/// Builds a swap note targeting the pool.
pub fn build_swap_note(
    pool_id: AccountId,
    pool_library: &Library,
    input_asset: FungibleAsset,
    min_amount_out: u64,
    sender: AccountId,
    output_tag: Felt,
    output_note_type: Felt,
    output_recipient_digest: Word,
    rng: &mut impl FeltRng,
) -> Result<Note> {
    let script = compile_pool_note_script(pool_library, "swap")?;

    let inputs = NoteInputs::new(vec![
        Felt::new(0),
        Felt::new(min_amount_out),
        output_tag,
        Felt::ZERO,
        output_note_type,
        Felt::ZERO,
        output_recipient_digest[0],
        output_recipient_digest[1],
        output_recipient_digest[2],
        output_recipient_digest[3],
    ])?;

    let assets = NoteAssets::new(vec![input_asset.into()])?;

    let tag = NoteTag::with_account_target(pool_id);
    let metadata = NoteMetadata::new(sender, NoteType::Public, tag);

    let serial_num = rng.draw_word();
    let recipient = NoteRecipient::new(serial_num, script, inputs);
    Ok(Note::new(assets, metadata, recipient))
}

/// Builds a withdraw note targeting the pool (carries no assets).
pub fn build_withdraw_note(
    pool_id: AccountId,
    pool_library: &Library,
    lp_amount: u64,
    sender: AccountId,
    output_tag: Felt,
    output_note_type: Felt,
    output_recipient_digest: Word,
    rng: &mut impl FeltRng,
) -> Result<Note> {
    let script = compile_pool_note_script(pool_library, "withdraw")?;

    let inputs = NoteInputs::new(vec![
        Felt::new(2),
        Felt::new(lp_amount),
        output_tag,
        Felt::ZERO,
        output_note_type,
        Felt::ZERO,
        output_recipient_digest[0],
        output_recipient_digest[1],
        output_recipient_digest[2],
        output_recipient_digest[3],
    ])?;

    let assets = NoteAssets::new(vec![])?;

    let tag = NoteTag::with_account_target(pool_id);
    let metadata = NoteMetadata::new(sender, NoteType::Public, tag);

    let serial_num = rng.draw_word();
    let recipient = NoteRecipient::new(serial_num, script, inputs);
    Ok(Note::new(assets, metadata, recipient))
}

// ---------------------------------------------------------------------------
// Math helpers
// ---------------------------------------------------------------------------

/// Expected LP tokens for a deposit.
pub fn compute_expected_lp(
    amount0: u64,
    amount1: u64,
    reserve0: u64,
    reserve1: u64,
    total_lp: u64,
) -> u64 {
    if total_lp == 0 {
        let product = amount0 as u128 * amount1 as u128;
        isqrt(product) as u64 - 1000
    } else {
        let lp0 = (amount0 as u128 * total_lp as u128 / reserve0 as u128) as u64;
        let lp1 = (amount1 as u128 * total_lp as u128 / reserve1 as u128) as u64;
        lp0.min(lp1)
    }
}

/// Expected output amount for a swap (0.3% fee).
pub fn compute_swap_output(amount_in: u64, reserve_in: u64, reserve_out: u64) -> u64 {
    let fee_adjusted = amount_in as u128 * 997;
    let numerator = reserve_out as u128 * fee_adjusted;
    let denominator = reserve_in as u128 * 1000 + fee_adjusted;
    (numerator / denominator) as u64
}

/// Integer square root (Newton's method, floor).
pub fn isqrt(n: u128) -> u128 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_isqrt() {
        assert_eq!(isqrt(0), 0);
        assert_eq!(isqrt(1), 1);
        assert_eq!(isqrt(4), 2);
        assert_eq!(isqrt(9), 3);
        assert_eq!(isqrt(10), 3);
        assert_eq!(isqrt(10_000 * 50_000), 22360);
    }

    #[test]
    fn test_swap_output() {
        let out = compute_swap_output(1_000, 50_000, 50_000);
        assert!(out > 970 && out < 1000, "out={out}");
    }
}
