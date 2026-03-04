use crate::utils::create_library;
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
    crypto::rand::Randomizable,
    note::{NoteInputs, NoteScript},
    transaction::{TransactionKernel, TransactionScript},
};
use rand::{Rng, SeedableRng, rngs::StdRng};
use std::{fs, path::PathBuf};

/// Compiles the pool MASM library from source.
pub fn get_pool_library() -> Result<Library> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "asm", "accounts", "c_prod_pool.masm"]
        .iter()
        .collect();
    let source = fs::read_to_string(&path)?;
    let assembler = TransactionKernel::assembler().with_warnings_as_errors(true);
    create_library(assembler, "zoro::c_prod_pool", &source)
        .map_err(|e| anyhow!("Failed to compile pool library: {e:?}"))
}

/// Compiles the math MASM library (sqrt, safe_sub, safe_cast_u64_into_felt, etc.).
pub fn get_math_library() -> Result<Library> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "asm", "accounts", "math.masm"]
        .iter()
        .collect();
    let source = fs::read_to_string(&path)?;
    let assembler = TransactionKernel::assembler().with_warnings_as_errors(true);
    create_library(assembler, "zoro::math", &source)
        .map_err(|e| anyhow!("Failed to compile math library: {e:?}"))
}

/// Compiles the lp_local MASM library (get_lp_amount_out, deposit, withdraw, etc.).
/// Depends on math and storage_utils libraries.
pub fn get_lp_local_library() -> Result<Library> {
    let math_library = get_math_library()?;
    let storage_utils_library = get_storage_utils_library()?;
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "asm", "accounts", "lp_local.masm"]
        .iter()
        .collect();
    let source = fs::read_to_string(&path)?;
    let assembler = TransactionKernel::assembler()
        .with_warnings_as_errors(true)
        .with_static_library(math_library)
        .map_err(|e| anyhow!("Failed to add math library to assembler: {e:?}"))?
        .with_static_library(storage_utils_library)
        .map_err(|e| anyhow!("Failed to add storage_utils library to assembler: {e:?}"))?;
    create_library(assembler, "zoro::lp_local", &source)
        .map_err(|e| anyhow!("Failed to compile lp_local library: {e:?}"))
}

/// Generates the lp_local fuzz dummy library by reading lp_local.masm and transforming it:
/// - Makes mint and burn public for fuzz testing
/// - Adds get_user_deposit helper for verification
fn generate_lp_local_fuzz_dummy_source() -> Result<String> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "asm", "accounts", "lp_local.masm"]
        .iter()
        .collect();
    let source = fs::read_to_string(&path)?;

    let source = source.replace("proc mint#", "pub proc mint#");
    let source = source.replace("proc burn#", "pub proc burn#");

    Ok(source)
}

/// Compiles the lp_local fuzz dummy library (generated from lp_local.masm with public mint/burn).
pub fn get_lp_local_fuzz_dummy_library() -> Result<Library> {
    let math_library = get_math_library()?;
    let storage_utils_library = get_storage_utils_library()?;
    let source = generate_lp_local_fuzz_dummy_source()?;
    let assembler = TransactionKernel::assembler()
        .with_warnings_as_errors(true)
        .with_static_library(math_library)
        .map_err(|e| anyhow!("Failed to add math library to assembler: {e:?}"))?
        .with_static_library(storage_utils_library)
        .map_err(|e| anyhow!("Failed to add storage_utils library to assembler: {e:?}"))?;
    create_library(assembler, "zoro::lp_local", &source)
        .map_err(|e| anyhow!("Failed to compile lp_local_fuzz_dummy library: {e:?}"))
}

/// Compiles a transaction script for lp_local mint/burn fuzz tests.
pub fn compile_lp_local_fuzz_tx_script(source: &str) -> Result<TransactionScript> {
    let lp_local_fuzz_dummy_library = get_lp_local_fuzz_dummy_library()?;
    compile_custom_tx_script(&lp_local_fuzz_dummy_library, source)
}

/// Compiles the storage_utils MASM library (add_to_storage_item, add_to_map_item, set_map_item).
/// Depends on the math library.
pub fn get_storage_utils_library() -> Result<Library> {
    let math_library = get_math_library()?;
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "asm", "accounts", "storage_utils.masm"]
        .iter()
        .collect();
    let source = fs::read_to_string(&path)?;
    let assembler = TransactionKernel::assembler()
        .with_warnings_as_errors(true)
        .with_static_library(math_library)
        .unwrap_or_else(|e| panic!("Failed to add math library to assembler: {e:?}"));
    create_library(assembler, "zoro::storage_utils", &source)
        .map_err(|e| anyhow!("Failed to compile storage_utils library: {e:?}"))
}

/// Compiles the storage_fuzz_dummy MASM library (minimal dummy with slot constants).
pub fn get_storage_fuzz_dummy_library() -> Result<Library> {
    let storage_utils_library = get_storage_utils_library()?;
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "asm", "accounts", "storage_fuzz_dummy.masm"]
        .iter()
        .collect();
    let source = fs::read_to_string(&path)?;

    let assembler = TransactionKernel::assembler()
        .with_warnings_as_errors(true)
        .with_static_library(storage_utils_library)
        .unwrap_or_else(|e| panic!("Failed to add storage_utils library to assembler: {e:?}"));
    create_library(assembler, "zoro::storage_fuzz_dummy", &source)
        .map_err(|e| anyhow!("Failed to compile storage_fuzz_dummy library: {e:?}"))
}

/// Compiles a transaction script for storage fuzz tests.
/// Links both storage_utils and storage_fuzz_dummy libraries.
pub fn compile_storage_fuzz_tx_script(source: &str) -> Result<TransactionScript> {
    let storage_utils_library = get_storage_utils_library()?;
    let storage_fuzz_dummy_library = get_storage_fuzz_dummy_library()?;
    let assembler = TransactionKernel::assembler()
        .with_warnings_as_errors(true)
        .with_static_library(storage_utils_library)
        .map_err(|e| anyhow!("Failed to add storage_utils library: {e:?}"))?
        .with_static_library(storage_fuzz_dummy_library)
        .map_err(|e| anyhow!("Failed to add storage_fuzz_dummy library: {e:?}"))?;
    let program = assembler
        .assemble_program(source)
        .map_err(|e| anyhow!("Failed to compile storage fuzz script: {e:?}"))?;
    Ok(TransactionScript::new(program))
}

/// Compiles a transaction script from arbitrary MASM source, linked against the pool library.
pub fn compile_custom_tx_script(pool_library: &Library, source: &str) -> Result<TransactionScript> {
    let assembler = TransactionKernel::assembler()
        .with_warnings_as_errors(true)
        .with_static_library(pool_library.clone())
        .map_err(|e| anyhow!("Failed to add pool library to assembler: {e:?}"))?;
    let program = assembler
        .assemble_program(source)
        .map_err(|e| anyhow!("Failed to compile tx script: {e:?}"))?;
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

/// Compiles the lp_local deposit note script.
/// The script loads assets and user_id from the note via active_note::get_assets/get_inputs,
/// then calls lp_local::deposit with [ASSET0, ASSET1, user_id_prefix, user_id_suffix].
pub fn compile_lp_local_note_script(lp_local_library: &Library) -> Result<NoteScript> {
    let source = r#"use miden::protocol::active_note
use zoro::lp_local
use miden::core::sys
begin
    # Load assets from note (dest_ptr=0)
    push.0 exec.active_note::get_assets
    # => [num_assets, 0]
    swap drop
    # => [num_assets]
    push.2 eq assert
    # => []
    exec.active_note::get_sender
    # => [sender_prefix, sender_suffix]
    padw mem_loadw_be.0 padw mem_loadw_be.4
    # => [ASSET0, ASSET1, sender_prefix, sender_suffix ]
    call.lp_local::deposit
    exec.sys::truncate_stack
end"#;
    let assembler = TransactionKernel::assembler()
        .with_warnings_as_errors(true)
        .with_static_library(lp_local_library.clone())
        .map_err(|e| anyhow!("Failed to add lp_local library to assembler: {e:?}"))?;
    let program = assembler
        .assemble_program(source)
        .map_err(|e| anyhow!("Failed to compile lp_local deposit note script: {e:?}"))?;
    Ok(NoteScript::new(program))
}

/// Builds a deposit note targeting the lp_local pool.
/// Note inputs: [user_id_prefix, user_id_suffix].
pub fn build_lp_local_deposit_note(
    pool_id: AccountId,
    lp_local_library: &Library,
    token0_asset: FungibleAsset,
    token1_asset: FungibleAsset,
    user_id: AccountId,
    sender: AccountId,
) -> Result<Note> {
    let script = compile_lp_local_note_script(lp_local_library)?;

    let inputs = NoteInputs::new(vec![
        user_id.prefix().as_felt().into(),
        user_id.suffix().into(),
    ])?;

    let assets = NoteAssets::new(vec![token0_asset.into(), token1_asset.into()])?;

    let tag = NoteTag::with_account_target(pool_id);
    let metadata = NoteMetadata::new(sender, NoteType::Public, tag);

    let mut seed = [0; 32];
    // default from os to get initial seed
    let mut std_rng = StdRng::from_os_rng();
    std_rng.fill(&mut seed);
    // regenerate with a seed
    // TODO: maybe not needed?
    let mut rng = StdRng::from_seed(seed);
    let mut seed = [0u8; 32];
    rng.fill(&mut seed);
    let serial_num = Word::from_random_bytes(&seed)
        .ok_or(anyhow!("Error generating new word, no word was produced"))?;
    let recipient = NoteRecipient::new(serial_num, script, inputs);
    Ok(Note::new(assets, metadata, recipient))
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
        .map_err(|e| anyhow!("Failed to add pool library to assembler: {e:?}"))?;
    let program = assembler
        .assemble_program(source)
        .map_err(|e| anyhow!("Failed to compile {procedure_name} note script: {e:?}"))?;
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
        isqrt(product) as u64 - 100
    } else {
        let lp0 = (amount0 as u128 * total_lp as u128 / reserve0 as u128) as u64;
        let lp1 = (amount1 as u128 * total_lp as u128 / reserve1 as u128) as u64;
        lp0.min(lp1)
    }
}

/// Expected output amounts for a withdraw.
pub fn compute_expected_withdraw(
    total_supply: u64,
    lp_amount: u64,
    reserve_0: u64,
    reserve_1: u64,
) -> (u64, u64) {
    let amount_0 = lp_amount as u128 * reserve_0 as u128 / total_supply as u128;
    let amount_1 = lp_amount as u128 * reserve_1 as u128 / total_supply as u128;
    (amount_0 as u64, amount_1 as u64)
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

    // #[test]
    fn test_isqrt() {
        assert_eq!(isqrt(0), 0);
        assert_eq!(isqrt(1), 1);
        assert_eq!(isqrt(4), 2);
        assert_eq!(isqrt(9), 3);
        assert_eq!(isqrt(10), 3);
        assert_eq!(isqrt(10_000 * 50_000), 22360);
    }

    // #[test]
    fn test_swap_output() {
        let out = compute_swap_output(1_000, 50_000, 50_000);
        assert!(out > 970 && out < 1000, "out={out}");
    }

    // #[test]
    fn test_lp_local_deposit_note_script_compiles() {
        let lp_lib = get_lp_local_library().expect("lp_local library");
        let result = compile_lp_local_note_script(&lp_lib);
        assert!(
            result.is_ok(),
            "lp_local deposit note script: {:?}",
            result.err()
        );
    }

    // #[test]
    fn test_storage_fuzz_scripts_compile() {
        let add_source = "use zoro::storage_fuzz_dummy\n\
             #use zoro::storage_utils\n\
             use miden::core::sys\n\


             const VALUE_SLOT = word(\"zoro::storage_fuzz_dummy::value_slot\")\n\
             const MAP_SLOT = word(\"zoro::storage_fuzz_dummy::map_slot\")\n\
             begin\n\
                 push.42\n\
                 push.VALUE_SLOT[0..2]\n\
                 call.storage_fuzz_dummy::add_to_storage_item\n\
                 exec.sys::truncate_stack\n\
             end";
        let result = compile_storage_fuzz_tx_script(add_source);
        assert!(result.is_ok(), "compile error: {:?}", result.err());
    }
}
