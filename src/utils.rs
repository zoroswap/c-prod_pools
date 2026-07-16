use std::{fs, path::PathBuf, sync::Arc};

use anyhow::{Result, anyhow};
use miden_client::account::component::BasicWallet;
use miden_client::account::{
    AccountBuilder, AccountComponent, AccountType, StorageSlot,
};
use miden_client::auth::{AuthSchemeId, AuthSecretKey, AuthSingleSig, NoAuth};
use miden_client::note::{NoteError, StandardNote};
use miden_client::{
    AccountError, ClientError, Felt, Word,
    account::AccountId,
    assembly::Library,
    asset::{AssetCallbackFlag, AssetVault, AssetVaultKey},
    rpc::{GrpcClient, NodeRpcClient},
};
use miden_protocol::account::StorageSlotName;
use miden_protocol::account::component::AccountComponentMetadata;

use crate::pool_ops::{
    compile_xyk_register_note_script, get_combined_pool_library, get_lp_local_library,
};

pub fn slot_name(name: &str) -> StorageSlotName {
    StorageSlotName::new(name).expect("valid slot name")
}

pub fn auth_single_sig_component(key_pair: &AuthSecretKey) -> AccountComponent {
    AuthSingleSig::new(
        key_pair.public_key().to_commitment(),
        AuthSchemeId::Falcon512Poseidon2,
    )
    .into()
}

pub fn zoro_component(
    library: Arc<Library>,
    slots: Vec<StorageSlot>,
    name: &str,
) -> Result<AccountComponent, AccountError> {
    AccountComponent::new(
        library.as_ref().clone(),
        slots,
        AccountComponentMetadata::new(name),
    )
}

pub fn vault_fungible_balance(vault: &AssetVault, faucet_id: AccountId) -> Result<u64> {
    let key = AssetVaultKey::new_fungible(faucet_id, AssetCallbackFlag::Disabled);
    Ok(vault.get_balance(key)?.as_u64())
}

pub async fn fetch_vault_for_account_from_chain(
    rpc_api: &Arc<GrpcClient>,
    account_id: &AccountId,
) -> Result<AssetVault> {
    let account = rpc_api
        .get_account_details(*account_id)
        .await
        .map_err(|e| anyhow!("Failed to fetch account details from node: {e}"))?
        .ok_or_else(|| anyhow!("Pool account is private or missing, cannot read state"))?;

    Ok(account.vault().clone())
}

pub fn read_masm_to_string(kind: &str, name: &str) -> Result<String> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "asm", kind, &format!("{name}.masm")]
        .iter()
        .collect();
    fs::read_to_string(&path).map_err(|e| anyhow!("Failed to read {path:?}: {e}"))
}

pub fn get_p2id_root_hash() -> Word {
    let root = StandardNote::P2ID.script_root();
    println!("P2ID script root: {:?}", root);
    root.into()
}

pub fn get_register_note_root_hash() -> Word {
    let note_script = compile_xyk_register_note_script().unwrap();
    note_script.root().into()
}

pub fn get_pool_account_code_commitment() -> Word {
    let lp_local_library = get_lp_local_library().unwrap();
    let xyk_pool_library = get_combined_pool_library().unwrap();
    let assets_mapping_slot =
        StorageSlot::with_empty_map(slot_name("zoro::lp_local::assets_mapping"));
    let reserve_slot = StorageSlot::with_empty_value(slot_name("zoro::lp_local::reserve"));
    let total_supply_slot =
        StorageSlot::with_empty_value(slot_name("zoro::lp_local::total_supply"));
    let registry_id_slot = StorageSlot::with_empty_value(slot_name("zoro::lp_local::registry_id"));
    let register_note_root = StorageSlot::with_value(
        slot_name("zoro::lp_local::register_note_root"),
        get_register_note_root_hash(),
    );
    let user_deposits_slot =
        StorageSlot::with_empty_map(slot_name("zoro::lp_local::user_deposits_mapping"));

    let lp_local_component = zoro_component(
        lp_local_library,
        vec![
            assets_mapping_slot,
            reserve_slot,
            total_supply_slot,
            user_deposits_slot,
            registry_id_slot,
            register_note_root,
        ],
        "zoro::lp_local",
    )
    .map_err(|e| ClientError::AccountError(e))
    .unwrap();

    let xyk_pool_component = zoro_component(xyk_pool_library, vec![], "zoro::xyk_pool")
        .map_err(|e| ClientError::AccountError(e))
        .unwrap();

    let init_seed = [0_u8; 32];
    let contract = AccountBuilder::new(init_seed)
        .account_type(AccountType::Public)
        .with_component(lp_local_component)
        .with_component(xyk_pool_component)
        .with_auth_component(NoAuth)
        .with_component(BasicWallet)
        // Keep this identical to `deploy_combined_pool`; schema-commitment builds produce
        // a different account code commitment than the pool deployed with `build()`.
        .build()
        .map_err(|e| anyhow!("Failed to build combined pool contract: {e:?}"))
        .unwrap();

    contract.code().commitment()
}

pub fn order_assets_as_felts(
    a0_pfx: Felt,
    a0_sfx: Felt,
    a1_pfx: Felt,
    a1_sfx: Felt,
) -> Result<(Felt, Felt, Felt, Felt)> {
    let a0_pfx = a0_pfx.as_canonical_u64();
    let a0_sfx = a0_sfx.as_canonical_u64();
    let a1_pfx = a1_pfx.as_canonical_u64();
    let a1_sfx = a1_sfx.as_canonical_u64();
    if (a0_pfx, a0_sfx) < (a1_pfx, a1_sfx) {
        Ok((
            Felt::new(a0_pfx)?,
            Felt::new(a0_sfx)?,
            Felt::new(a1_pfx)?,
            Felt::new(a1_sfx)?,
        ))
    } else if (a0_pfx, a0_sfx) > (a1_pfx, a1_sfx) {
        Ok((
            Felt::new(a1_pfx)?,
            Felt::new(a1_sfx)?,
            Felt::new(a0_pfx)?,
            Felt::new(a0_sfx)?,
        ))
    } else {
        Err(anyhow!("Both assets are the same"))
    }
}
