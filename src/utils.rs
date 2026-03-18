use miden_client::ClientError;
use miden_client::account::component::BasicWallet;
use miden_client::account::{
    AccountBuilder, AccountComponent, AccountStorageMode, AccountType, StorageSlot,
};
use miden_client::auth::{AuthFalcon512Rpo, AuthSecretKey, NoAuth};
use miden_client::note::{NoteError, WellKnownNote};
use miden_protocol::account::StorageSlotName;

use std::{fs, path::PathBuf, sync::Arc};

use anyhow::{Result, anyhow};
use miden_client::assembly::{
    Assembler, DefaultSourceManager, Module, ModuleKind, Path as AssemblyPath,
};
use miden_client::{
    Felt, Word,
    account::{Account, AccountId},
    asset::AssetVault,
    rpc::{GrpcClient, NodeRpcClient, domain::account::FetchedAccount},
    store::AccountRecordData,
};

use crate::pool_ops::{
    compile_xyk_register_note_script, get_combined_pool_library, get_lp_local_library,
};

pub fn slot_name(name: &str) -> StorageSlotName {
    StorageSlotName::new(name).expect("valid slot name")
}

pub fn create_library(
    assembler: Assembler,
    library_path: &str,
    source_code: &str,
) -> Result<miden_client::assembly::Library, Box<dyn std::error::Error>> {
    println!("creating library: {:?}", library_path);
    let source_manager = Arc::new(DefaultSourceManager::default());
    // println!("parsing library: {:?}", library_path);
    let module = Module::parser(ModuleKind::Library).parse_str(
        AssemblyPath::new(library_path),
        source_code,
        source_manager.clone(),
    )?;
    // println!("assembling library: {:?}", library_path);
    let library = assembler.assemble_library([module])?;
    Ok(library)
}

pub async fn fetch_vault_for_account_from_chain(
    rpc_api: &Arc<GrpcClient>,
    account_id: &AccountId,
) -> Result<AssetVault> {
    let fetched = rpc_api
        .get_account_details(*account_id)
        .await
        .map_err(|e| anyhow!("Failed to fetch account details from node: {e}"))?;
    let account = match fetched {
        FetchedAccount::Public(account, _) => account,
        FetchedAccount::Private(_, _) => {
            return Err(anyhow!("Pool account is private, cannot read state"));
        }
    };

    Ok(account.vault().clone())
}

pub fn extract_full_account(data: &AccountRecordData) -> Result<&Account> {
    match data {
        AccountRecordData::Full(account) => Ok(account),
        AccountRecordData::Partial(_) => Err(anyhow!("Expected full account data, got partial")),
    }
}

pub fn read_masm_to_string(kind: &str, name: &str) -> Result<String> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "asm", kind, &format!("{name}.masm")]
        .iter()
        .collect();
    fs::read_to_string(&path).map_err(|e| anyhow!("Failed to read {path:?}: {e}"))
}

pub fn get_p2id_root_hash() -> Word {
    println!("P2ID script root: {:?}", WellKnownNote::P2ID.script_root());
    WellKnownNote::P2ID.script_root()
}

pub fn get_register_note_root_hash() -> Word {
    let note_script = compile_xyk_register_note_script().unwrap();
    note_script.root()
}

pub fn get_pool_account_code_commitment() -> Word {
    let lp_local_library = get_lp_local_library()
        .map_err(|e| ClientError::NoteError(NoteError::other(e.to_string())))
        .unwrap();
    let xyk_pool_library = get_combined_pool_library()
        .map_err(|e| ClientError::NoteError(NoteError::other(e.to_string())))
        .unwrap();
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

    let lp_local_component = AccountComponent::new(
        lp_local_library,
        vec![
            assets_mapping_slot,
            reserve_slot,
            total_supply_slot,
            user_deposits_slot,
            registry_id_slot,
            register_note_root,
        ],
    )
    .map_err(|e| ClientError::NoteError(NoteError::other(e.to_string())))
    .unwrap()
    .with_supports_all_types();

    let xyk_pool_component = AccountComponent::new(xyk_pool_library, vec![])
        .map_err(|e| ClientError::NoteError(NoteError::other(e.to_string())))
        .unwrap()
        .with_supports_all_types();

    let init_seed = [0_u8; 32];
    let contract = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountImmutableCode)
        .storage_mode(AccountStorageMode::Network)
        .with_component(lp_local_component)
        .with_component(xyk_pool_component)
        .with_auth_component(NoAuth)
        .with_component(BasicWallet)
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
    let a0_pfx: u64 = a0_pfx.into();
    let a0_sfx: u64 = a0_sfx.into();
    let a1_pfx: u64 = a1_pfx.into();
    let a1_sfx: u64 = a1_sfx.into();
    if (a0_pfx, a0_sfx) < (a1_pfx, a1_sfx) {
        Ok((
            Felt::new(a0_pfx),
            Felt::new(a0_sfx),
            Felt::new(a1_pfx),
            Felt::new(a1_sfx),
        ))
    } else if (a0_pfx, a0_sfx) > (a1_pfx, a1_sfx) {
        Ok((
            Felt::new(a1_pfx),
            Felt::new(a1_sfx),
            Felt::new(a0_pfx),
            Felt::new(a0_sfx),
        ))
    } else {
        Err(anyhow!("Both assets are the same"))
    }
}
