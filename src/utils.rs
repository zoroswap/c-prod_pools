use miden_protocol::account::StorageSlotName;

use std::{fs, path::PathBuf, sync::Arc};

use anyhow::{Result, anyhow};
use miden_client::assembly::{
    Assembler, DefaultSourceManager, Module, ModuleKind, Path as AssemblyPath,
};
use miden_client::{
    account::{Account, AccountId},
    asset::AssetVault,
    note::Note,
    rpc::{GrpcClient, NodeRpcClient, domain::account::FetchedAccount},
    store::AccountRecordData,
};

use tracing::{debug, info};

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
        .get_account_details(account_id.clone())
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
