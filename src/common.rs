use std::sync::Arc;
use std::{fs, path::PathBuf, time::Duration};

use anyhow::{Context, Result, anyhow};
use miden_client::auth::NoAuth;
use miden_client::note::NoteTag;
use miden_client::{
    ClientError, Felt, Word,
    account::{
        Account, AccountBuilder, AccountId, AccountStorageMode, AccountType, StorageMap,
        StorageSlot,
    },
    asset::{FungibleAsset, TokenSymbol},
    auth::{AuthFalcon512Rpo, AuthSecretKey},
    builder::ClientBuilder,
    crypto::FeltRng,
    keystore::FilesystemKeyStore,
    note::{Note, NoteError, NoteType},
    rpc::GrpcClient,
    store::TransactionFilter,
    transaction::{OutputNote, TransactionRequestBuilder},
};
use miden_client_sqlite_store::ClientBuilderSqliteExt;
use miden_protocol::{FieldElement, account::AccountComponent, transaction::TransactionKernel};
use miden_standards::account::{faucets::BasicFungibleFaucet, wallets::BasicWallet};
use rand::RngCore;
use tracing::{debug, info, warn};

use serde::{Deserialize, Serialize};

use crate::{
    pool_ops::build_dummy_register_note,
    utils::{create_library, extract_full_account, fetch_vault_for_account_from_chain, slot_name},
};
use crate::{
    pool_ops::{
        get_combined_pool_library, get_lp_local_fuzz_dummy_library, get_lp_local_library,
        get_pool_library, get_registry_library, get_storage_utils_library,
    },
    utils::get_register_note_root_hash,
};

use miden_client::{Client, rpc::Endpoint};
pub type MidenClient = Client<FilesystemKeyStore>;
pub struct MidenClients {
    pub client: MidenClient,
    pub rpc_api: Arc<GrpcClient>,
    pub endpoint: Endpoint,
}

pub async fn instantiate_simple_client(
    keystore_path: &str,
    store_path: &str,
    endpoint: &Endpoint,
) -> Result<MidenClients, ClientError> {
    let timeout_ms = 30_000;
    let rpc_api = Arc::new(GrpcClient::new(endpoint, timeout_ms));
    let keystore = FilesystemKeyStore::new(keystore_path.into())
        .unwrap_or_else(|err| panic!("Failed to create keystore: {err:?}"))
        .into();
    println!("\nConnecting to endpoint: {}", endpoint);

    let mut client = ClientBuilder::new()
        .rpc(rpc_api.clone())
        .authenticator(keystore)
        .in_debug_mode(true.into())
        .sqlite_store(store_path.into())
        .build()
        .await?;

    println!("\nSyncing state...");
    let sync_summary = client.sync_state().await?;
    println!("\nLatest block: {}", sync_summary.block_num);

    Ok(MidenClients {
        client,
        rpc_api,
        endpoint: endpoint.clone(),
    })
}

/// Creates a basic regular account with updatable code.
///
/// # Arguments
/// * `client`: Miden client instance
/// * `keystore`: Keystore to store the account's authentication key
///
/// # Returns
/// Tuple of `(Account, AuthSecretKey)`
pub async fn create_basic_account(
    client: &mut MidenClient,
    keystore: FilesystemKeyStore,
) -> Result<(Account, AuthSecretKey), ClientError> {
    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);
    let key_pair = AuthSecretKey::new_falcon512_rpo_with_rng(client.rng());
    let builder = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_auth_component(AuthFalcon512Rpo::new(key_pair.public_key().to_commitment()))
        .with_component(BasicWallet);
    let account = builder.build().unwrap();
    client.add_account(&account, false).await?;
    keystore.add_key(&key_pair).unwrap();
    client.sync_state().await?;

    // dummy tx to get the new account into node
    touch_account(client, &account).await.unwrap();

    Ok((account, key_pair))
}

/// Deploys a constant-product pool account configured for the given token pair.
///
/// Storage slots:
///   - `reserve`:   [reserve0, reserve1, total_lp, 0]  (initially empty)
///   - `config`:    [token0_prefix, token0_suffix, token1_prefix, token1_suffix]
///   - `lp_shares`: StorageMap (initially empty)
pub async fn deploy_xyk_pool(
    client: &mut MidenClient,
    keystore: FilesystemKeyStore,
    token0_id: &AccountId,
    token1_id: &AccountId,
) -> Result<(Account, AuthSecretKey), ClientError> {
    let sync_summary = client.sync_state().await?;
    println!("\nLatest block: {}", sync_summary.block_num);
    println!("\n[STEP 1] Create xyk_pool account");

    // let pool_code = read_masm_to_string("accounts", "xyk_pool")
    //     .unwrap_or_else(|e| panic!("Failed to read xyk_pool code: {e:?}"));

    // let assembler = TransactionKernel::assembler(); //.with_warnings_as_errors(true);

    let reserves = StorageSlot::with_empty_value(slot_name("zoro::lp_local::reserve"));
    let pool_assets: Word = [
        token0_id.prefix().as_felt(),
        token0_id.suffix(),
        token1_id.prefix().as_felt(),
        token1_id.suffix(),
    ]
    .into();
    let assets_mapping =
        StorageSlot::with_value(slot_name("zoro::lp_local::assets_mapping"), pool_assets);

    // let xyk_pool_library = create_library(assembler.clone(), "zoro::xyk_pool", &pool_code)
    //     .map_err(|e| anyhow!("Failed to create pool library: {e:?}"))
    //     .unwrap();
    let xyk_pool_library = get_pool_library().unwrap();
    let xyk_pool_component =
        AccountComponent::new(xyk_pool_library, vec![reserves, assets_mapping])?
            .with_supports_all_types();

    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let key_pair = AuthSecretKey::new_falcon512_rpo_with_rng(client.rng());

    let xyk_pool_contract = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(xyk_pool_component.clone())
        .with_auth_component(AuthFalcon512Rpo::new(key_pair.public_key().to_commitment()))
        .with_component(BasicWallet)
        .build()?;

    println!(
        "pool contract commitment hash: {:?}",
        xyk_pool_contract.commitment().to_hex()
    );
    println!(
        "pool config: token0={}, token1={}",
        token0_id.to_hex(),
        token1_id.to_hex(),
    );

    keystore.add_key(&key_pair).unwrap();
    client
        .add_account(&xyk_pool_contract.clone(), false)
        .await?;
    client.sync_state().await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    Ok((xyk_pool_contract, key_pair))
}

/// Deploys an lp_local pool account for the given token pair.
///
/// Storage slots (must match lp_local.masm):
///   - `reserve_mapping`: [reserve0, reserve1, 0, 0] (initially empty)
///   - `total_supply`: [total_lp, 0, 0, 0] (initially empty)
///   - `user_deposits_mapping`: map slot (initially empty)
pub async fn deploy_lp_local_pool(
    client: &mut MidenClient,
    keystore: FilesystemKeyStore,
    token0_id: &AccountId,
    token1_id: &AccountId,
) -> Result<(Account, AuthSecretKey), ClientError> {
    let _ = (token0_id, token1_id);
    let lp_local_library = get_lp_local_library()
        .map_err(|e| ClientError::NoteError(NoteError::other(e.to_string())))?;

    let mut assets_mapping = StorageMap::new();
    assets_mapping.insert(
        Word::default(),
        [
            token1_id.suffix(),
            token1_id.prefix().as_felt(),
            token0_id.suffix(),
            token0_id.prefix().as_felt(),
        ]
        .into(),
    )?;
    let assets_mapping_slot =
        StorageSlot::with_map(slot_name("zoro::lp_local::assets_mapping"), assets_mapping);
    let reserve_slot = StorageSlot::with_empty_value(slot_name("zoro::lp_local::reserve"));
    let total_supply_slot =
        StorageSlot::with_empty_value(slot_name("zoro::lp_local::total_supply"));
    let mut user_deposits_mapping = StorageMap::new();
    user_deposits_mapping.insert(
        Word::new([Felt::new(0), Felt::new(0), Felt::new(0), Felt::new(1)]),
        Word::new([Felt::new(0), Felt::new(0), Felt::new(0), Felt::new(1)]),
    )?;
    let user_deposits_slot = StorageSlot::with_map(
        slot_name("zoro::lp_local::user_deposits_mapping"),
        user_deposits_mapping,
    );

    let lp_local_component = AccountComponent::new(
        lp_local_library,
        vec![
            assets_mapping_slot,
            reserve_slot,
            total_supply_slot,
            user_deposits_slot,
        ],
    )
    .map_err(|e| ClientError::NoteError(NoteError::other(e.to_string())))?
    .with_supports_all_types();

    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);
    let key_pair = AuthSecretKey::new_falcon512_rpo_with_rng(client.rng());

    let lp_local_contract = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(lp_local_component)
        .with_auth_component(AuthFalcon512Rpo::new(key_pair.public_key().to_commitment()))
        .with_component(BasicWallet)
        .build()
        .map_err(|e| anyhow!("Failed to build lp_local contract: {e:?}"))
        .unwrap();

    keystore.add_key(&key_pair).unwrap();
    client
        .add_account(&lp_local_contract.clone(), false)
        .await?;
    client.sync_state().await?;

    // let dummy_tx = TransactionRequestBuilder::new().build()?;
    // let _ = client
    //     .submit_new_transaction(lp_local_contract.id(), dummy_tx)
    //     .await?;
    // client.sync_state().await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    Ok((lp_local_contract, key_pair))
}

/// Deploys a combined pool account with both `lp_local` and `xyk_pool` components.
///
/// The `lp_local` component provides LP management (deposit, withdraw, mint, burn).
/// The `xyk_pool` component provides swap logic and references lp_local storage.
///
/// Storage slots (from lp_local):
///   - `assets_mapping`: map with token0/token1 IDs
///   - `reserve`: [reserve0, reserve1, 0, 0]
///   - `total_supply`: [total_lp, 0, 0, 0]
///   - `user_deposits_mapping`: map slot
pub async fn deploy_combined_pool(
    client: &mut MidenClient,
    keystore: FilesystemKeyStore,
    token0_id: &AccountId,
    token1_id: &AccountId,
    registry_id: &AccountId,
) -> Result<(Account, AuthSecretKey), ClientError> {
    let lp_local_library = get_lp_local_library()
        .map_err(|e| ClientError::NoteError(NoteError::other(e.to_string())))?;
    let xyk_pool_library = get_combined_pool_library()
        .map_err(|e| ClientError::NoteError(NoteError::other(e.to_string())))?;

    // lp_local storage slots
    let mut assets_mapping = StorageMap::new();
    assets_mapping.insert(
        Word::default(),
        [
            token1_id.suffix(),
            token1_id.prefix().as_felt(),
            token0_id.suffix(),
            token0_id.prefix().as_felt(),
        ]
        .into(),
    )?;
    let assets_mapping_slot =
        StorageSlot::with_map(slot_name("zoro::lp_local::assets_mapping"), assets_mapping);
    let reserve_slot = StorageSlot::with_empty_value(slot_name("zoro::lp_local::reserve"));
    let total_supply_slot =
        StorageSlot::with_empty_value(slot_name("zoro::lp_local::total_supply"));
    let registry_id_slot = StorageSlot::with_value(
        slot_name("zoro::lp_local::registry_id"),
        Word::new([
            registry_id.suffix().into(),
            registry_id.prefix().into(),
            Felt::ZERO,
            Felt::ZERO,
        ]),
    );

    println!(
        "REGISTRY SUFFIX {} PREFIX {} TAG {}",
        registry_id.suffix(),
        registry_id.prefix().as_felt(),
        NoteTag::with_account_target(*registry_id)
    );

    let register_note_root = StorageSlot::with_value(
        slot_name("zoro::lp_local::register_note_root"),
        get_register_note_root_hash(),
    );
    let mut user_deposits_mapping = StorageMap::new();
    user_deposits_mapping.insert(
        Word::new([Felt::new(0), Felt::new(0), Felt::new(0), Felt::new(1)]),
        Word::new([Felt::new(0), Felt::new(0), Felt::new(0), Felt::new(1)]),
    )?;
    let user_deposits_slot = StorageSlot::with_map(
        slot_name("zoro::lp_local::user_deposits_mapping"),
        user_deposits_mapping,
    );

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
    .map_err(|e| ClientError::NoteError(NoteError::other(e.to_string())))?
    .with_supports_all_types();

    let xyk_pool_component = AccountComponent::new(xyk_pool_library, vec![])
        .map_err(|e| ClientError::NoteError(NoteError::other(e.to_string())))?
        .with_supports_all_types();

    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);
    let key_pair = AuthSecretKey::new_falcon512_rpo_with_rng(client.rng());

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

    println!(
        "Combined pool deployed: lp_local + xyk_pool => ID: {:?}",
        contract.id().to_hex()
    );

    keystore.add_key(&key_pair).unwrap();
    client.add_account(&contract.clone(), false).await?;
    client.sync_state().await?;

    Ok((contract, key_pair))
}

/// Deploys an lp_local fuzz dummy account (generated from lp_local.masm with public mint/burn).
///
/// Storage slots (must match generated lp_local_fuzz_dummy):
///   - `reserve`: [0, 0, 0, 0]
///   - `total_supply`: [0, 0, 0, 0]
///   - `user_deposits_mapping`: empty map
pub async fn deploy_lp_local_fuzz_dummy(
    client: &mut MidenClient,
    keystore: FilesystemKeyStore,
) -> Result<(Account, AuthSecretKey), ClientError> {
    let lp_local_fuzz_dummy_library = get_lp_local_fuzz_dummy_library()
        .map_err(|e| ClientError::NoteError(NoteError::other(e.to_string())))?;

    let reserve_slot = StorageSlot::with_empty_value(slot_name("zoro::lp_local::reserve"));
    let total_supply_slot =
        StorageSlot::with_empty_value(slot_name("zoro::lp_local::total_supply"));
    let assets_mapping_slot =
        StorageSlot::with_empty_map(slot_name("zoro::lp_local::assets_mapping"));
    let mut user_deposits_mapping = StorageMap::new();
    user_deposits_mapping.insert(
        Word::new([Felt::new(0), Felt::new(0), Felt::new(0), Felt::new(1)]),
        Word::new([Felt::new(0), Felt::new(0), Felt::new(0), Felt::new(1)]),
    )?;
    let user_deposits_slot = StorageSlot::with_map(
        slot_name("zoro::lp_local::user_deposits_mapping"),
        user_deposits_mapping,
    );

    let component = AccountComponent::new(
        lp_local_fuzz_dummy_library,
        vec![
            assets_mapping_slot,
            reserve_slot,
            total_supply_slot,
            user_deposits_slot,
        ],
    )
    .map_err(|e| ClientError::NoteError(NoteError::other(e.to_string())))?
    .with_supports_all_types();

    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);
    let key_pair = AuthSecretKey::new_falcon512_rpo_with_rng(client.rng());

    let contract = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(component)
        .with_auth_component(AuthFalcon512Rpo::new(key_pair.public_key().to_commitment()))
        .with_component(BasicWallet)
        .build()?;

    keystore.add_key(&key_pair).unwrap();
    client.add_account(&contract.clone(), false).await?;
    client.sync_state().await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    Ok((contract, key_pair))
}

/// Deploys a dummy account for storage_utils fuzz tests.
///
/// Storage slots (must match constants in storage_fuzz_dummy.masm):
///   - `value_slot`: value slot (initially empty)
///   - `map_slot`: map slot (initially empty)
pub async fn deploy_storage_fuzz_dummy(
    client: &mut MidenClient,
    keystore: FilesystemKeyStore,
    initial_value: u64,
    initial_map_value: u64,
) -> Result<(Account, AuthSecretKey), ClientError> {
    let manifest_dir: &str = env!("CARGO_MANIFEST_DIR");
    let dummy_code_path: PathBuf = [manifest_dir, "asm", "accounts", "storage_fuzz_dummy.masm"]
        .iter()
        .collect();
    let dummy_code = fs::read_to_string(&dummy_code_path)
        .unwrap_or_else(|err| panic!("unable to read from {dummy_code_path:?}: {err}"));

    let storage_utils_library = get_storage_utils_library()
        .unwrap_or_else(|e| panic!("Failed to get storage_utils library: {e:?}"));
    // let math_library =
    //     get_math_library().unwrap_or_else(|e| panic!("Failed to get math library: {e:?}"));
    let assembler = TransactionKernel::assembler()
        .with_warnings_as_errors(true)
        .with_static_library(storage_utils_library)
        .unwrap_or_else(|e| panic!("Failed to add math library: {e:?}"));

    let dummy_library = create_library(assembler.clone(), "zoro::storage_fuzz_dummy", &dummy_code)
        .unwrap_or_else(|e| panic!("Failed to create storage_fuzz_dummy library: {e:?}"));

    let value_slot = StorageSlot::with_value(
        slot_name("zoro::storage_fuzz_dummy::value_slot"),
        Word::new([
            Felt::new(initial_value),
            Felt::new(0),
            Felt::new(0),
            Felt::new(0),
        ]),
    );

    let key = Word::new([Felt::new(0), Felt::new(0), Felt::new(0), Felt::new(0)]);
    let val = Word::new([
        Felt::new(initial_map_value),
        Felt::new(0),
        Felt::new(0),
        Felt::new(0),
    ]);
    let mut mapping_instance = StorageMap::new();
    mapping_instance.insert(key, val)?;
    let map_slot = StorageSlot::with_map(
        slot_name("zoro::storage_fuzz_dummy::map_slot"),
        mapping_instance,
    );
    let lp_total_supply_slot = StorageSlot::with_value(
        slot_name("zoro::storage_fuzz_dummy::lp_total_supply"),
        Word::new([Felt::new(0), Felt::new(0), Felt::new(0), Felt::new(0)]),
    );
    let lp_user_deposits_mapping = StorageSlot::with_empty_map(slot_name(
        "zoro::storage_fuzz_dummy::lp_user_deposits_mapping",
    ));

    let dummy_component = AccountComponent::new(
        dummy_library,
        vec![
            value_slot,
            map_slot,
            lp_total_supply_slot,
            lp_user_deposits_mapping,
        ],
    )?
    .with_supports_all_types();

    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);
    let key_pair = AuthSecretKey::new_falcon512_rpo_with_rng(client.rng());

    let dummy_contract = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(dummy_component)
        .with_auth_component(AuthFalcon512Rpo::new(key_pair.public_key().to_commitment()))
        .with_component(BasicWallet)
        .build()?;

    keystore.add_key(&key_pair).unwrap();
    client.add_account(&dummy_contract.clone(), false).await?;
    client.sync_state().await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    Ok((dummy_contract, key_pair))
}

/// Deploys a registry account pre-seeded with an accepted pool code hash.
///
/// Storage slots (must match constants in registry.masm):
///   - `accepted_code_hashes_mapping`: map with pool_code_hash → [1, 0, 0, 0]
///   - `pools_mapping`: empty map
///   - `assets_to_pool_mapping`: empty map
pub async fn deploy_registry(
    client: &mut MidenClient,
    keystore: FilesystemKeyStore,
    accepted_pool_code_hash: Word,
) -> Result<(Account, AuthSecretKey), ClientError> {
    let registry_library = get_registry_library()
        .map_err(|e| ClientError::NoteError(NoteError::other(e.to_string())))?;

    let mut accepted_hashes_map = StorageMap::new();

    println!("account pool code hash {:?}", accepted_pool_code_hash);

    accepted_hashes_map.insert(
        accepted_pool_code_hash,
        Word::new([Felt::new(1), Felt::new(0), Felt::new(0), Felt::new(0)]),
    )?;
    let accepted_hashes_slot = StorageSlot::with_map(
        slot_name("zoro::registry::accepted_code_hashes_mapping"),
        accepted_hashes_map,
    );

    let pools_mapping_slot =
        StorageSlot::with_empty_map(slot_name("zoro::registry::pools_mapping"));

    let assets_to_pool_mapping_slot =
        StorageSlot::with_empty_map(slot_name("zoro::registry::assets_to_pool_mapping"));

    let registry_component = AccountComponent::new(
        registry_library,
        vec![
            pools_mapping_slot,
            assets_to_pool_mapping_slot,
            accepted_hashes_slot,
        ],
    )
    .map_err(|e| ClientError::NoteError(NoteError::other(e.to_string())))?
    .with_supports_all_types();

    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);
    let key_pair = AuthSecretKey::new_falcon512_rpo_with_rng(client.rng());

    let registry = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountImmutableCode)
        .storage_mode(AccountStorageMode::Network)
        .with_component(registry_component)
        // .with_auth_component(AuthFalcon512Rpo::new(key_pair.public_key().to_commitment()))
        .with_auth_component(NoAuth)
        .with_component(BasicWallet)
        .build()
        .map_err(|e| anyhow!("Failed to build registry contract: {e:?}"))
        .unwrap();

    println!(
        "Registry deployed => ID: {:?}, accepted code hash: {:?}",
        registry.id().to_hex(),
        accepted_pool_code_hash,
    );

    keystore.add_key(&key_pair).unwrap();

    println!("add key");

    client.add_account(&registry, false).await?;

    println!("add account");

    client.sync_state().await?;
    println!("sync");

    let _ = touch_account(client, &registry).await;
    // println!("touch account");

    // println!("Dummy register note ...");

    // let dummy_register = build_dummy_register_note(&registry.id(), client.rng().draw_word());
    // let init_note_tx = TransactionRequestBuilder::new()
    //     .own_output_notes([OutputNote::Full(dummy_register)])
    //     .build()?;

    // println!("Dummy register note BUILT ");

    // client
    //     .submit_new_transaction(registry.id(), init_note_tx)
    //     .await?;

    // println!("Dummy register note sent");

    tokio::time::sleep(Duration::from_secs(2)).await;

    Ok((registry, key_pair))
}

#[derive(Deserialize, Debug)]
pub struct FaucetConfig {
    pub symbol: String,
    pub max_supply: u64,
    pub decimals: u8,
}
#[derive(Deserialize, Debug)]
pub struct FaucetsConfig {
    pub faucets: Vec<FaucetConfig>,
}
#[derive(Debug)]
pub struct Faucet {
    pub faucet: Account,
    pub config: FaucetConfig,
}

/// Load faucets config from `faucets.toml` in the manifest directory.
pub fn load_faucets_config() -> Result<FaucetsConfig> {
    let manifest_dir: &str = env!("CARGO_MANIFEST_DIR");
    let path: PathBuf = [manifest_dir, "faucets.toml"].iter().collect();
    let s = fs::read_to_string(&path).map_err(|e| anyhow!("Error reading {path:?}: {e}"))?;
    toml::from_str(&s).map_err(Into::into)
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CachedFaucet {
    pub account_id_hex: String,
    pub symbol: String,
    pub decimals: u8,
    pub max_supply: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CachedTestState {
    pub faucets: Vec<CachedFaucet>,
    pub user_account_id_hex: String,
}

pub fn save_test_state(path: &PathBuf, state: &CachedTestState) -> Result<()> {
    let toml_str =
        toml::to_string_pretty(state).map_err(|e| anyhow!("Failed to serialize state: {e}"))?;
    fs::write(path, toml_str).map_err(|e| anyhow!("Failed to write {path:?}: {e}"))?;
    println!("Saved test state to {path:?}");
    Ok(())
}

/// Returns `None` if the file is missing or corrupt (logged as warning).
pub fn load_test_state(path: &PathBuf) -> Option<CachedTestState> {
    let content = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return None,
    };
    match toml::from_str(&content) {
        Ok(state) => {
            println!("Loaded cached test state from {path:?}");
            Some(state)
        }
        Err(e) => {
            warn!("Corrupt test state at {path:?}: {e}");
            None
        }
    }
}

/// Imports a public account from the network by its ID.
/// Returns the full `Account` fetched via RPC.
pub async fn try_import_account(clients: &mut MidenClients, id: AccountId) -> Result<Account> {
    clients.client.import_account_by_id(id.clone()).await?;

    let record = clients
        .client
        .get_account(id)
        .await?
        .ok_or(anyhow!("No account found on chain for account_id {}", id))?;
    let account = extract_full_account(record.account_data())?.clone();

    Ok(account)
}

/// Deploys a single simple fungible faucet. Does not read any config files.
/// Returns the deployed faucet account.
pub async fn deploy_simple_faucet(
    client: &mut MidenClient,
    keystore: &FilesystemKeyStore,
    symbol: &str,
    decimals: u8,
    max_supply: u64,
) -> Result<Account> {
    let symbol =
        TokenSymbol::new(symbol).map_err(|e| anyhow!("Failed to create token symbol: {e:?}"))?;
    let max_supply = Felt::new(max_supply);

    let key_pair = AuthSecretKey::new_falcon512_rpo_with_rng(client.rng());
    let mut init_seed = [0u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let builder = AccountBuilder::new(init_seed)
        .account_type(AccountType::FungibleFaucet)
        .storage_mode(AccountStorageMode::Public)
        .with_auth_component(AuthFalcon512Rpo::new(key_pair.public_key().to_commitment()))
        .with_component(
            BasicFungibleFaucet::new(symbol, decimals, max_supply)
                .map_err(|e| anyhow!("Failed to create BasicFungibleFaucet: {e:?}"))?,
        );

    let faucet_account = builder
        .build()
        .map_err(|e| anyhow!("Failed to build faucet account: {e:?}"))?;

    client.add_account(&faucet_account, true).await?;
    keystore
        .add_key(&key_pair)
        .map_err(|e| anyhow!("Failed to add key to keystore: {e:?}"))?;

    client.sync_state().await?;
    // dummy tx to get the new account into node
    touch_account(client, &faucet_account).await.unwrap();

    Ok(faucet_account)
}

/// Reads `faucets.toml` from the manifest directory and deploys each configured faucet.
/// Returns the deployed faucet accounts in config order.
pub async fn deploy_simple_faucets_from_config(
    client: &mut MidenClient,
    keystore: &FilesystemKeyStore,
) -> Result<Vec<Faucet>> {
    let sync_summary = client.sync_state().await?;
    println!("Latest block: {}", sync_summary.block_num);

    let manifest_dir: &str = env!("CARGO_MANIFEST_DIR");
    let faucet_config_path: PathBuf = [manifest_dir, "faucets.toml"].iter().collect();
    let faucet_config = fs::read_to_string(&faucet_config_path)
        .map_err(|e| anyhow!("Error opening {faucet_config_path:?}: {e}"))?;

    let faucet_config: FaucetsConfig = toml::from_str(&faucet_config)?;

    let mut accounts = Vec::with_capacity(faucet_config.faucets.len());
    for faucet in faucet_config.faucets {
        println!("Deploying faucet {}.", faucet.symbol);
        let account = deploy_simple_faucet(
            client,
            keystore,
            &faucet.symbol,
            faucet.decimals,
            faucet.max_supply,
        )
        .await?;

        println!(
            "Faucet {} successfully deployed -> ID {:?}",
            faucet.symbol,
            // account.id().to_bech32(clients.endpoint.to_network_id()),
            account.id().to_hex(),
        );

        accounts.push(Faucet {
            faucet: account,
            config: faucet,
        });
    }

    println!("All faucets deployed successfully.");
    Ok(accounts)
}

pub async fn fund_wallet(
    clients: &mut MidenClients,
    account: &Account,
    asset: &FaucetConfig,
    asset_id: &AccountId,
    amount: u64,
) -> Result<()> {
    let client = &mut clients.client;
    let amount: u64 = if amount > 0 {
        amount
    } else {
        5 * 10u64.pow(asset.decimals as u32 - 2)
    }; // 0.05
    let fungible_asset = FungibleAsset::new(asset_id.clone(), amount)?;
    client.import_account_by_id(asset_id.clone()).await?;
    let transaction_request = TransactionRequestBuilder::new().build_mint_fungible_asset(
        fungible_asset,
        account.id(),
        NoteType::Public,
        client.rng(),
    )?;
    let tx_id = client
        .submit_new_transaction(asset_id.clone(), transaction_request)
        .await?;
    println!("Minted {amount} {} for the user.", asset.symbol);
    client.sync_state().await?;

    let transaction = client
        .get_transactions(TransactionFilter::Ids(vec![tx_id]))
        .await?
        .pop()
        .with_context(|| "failed to find transaction {tx_id:?} after submission")
        .unwrap();
    let minted_note = match transaction.details.output_notes.get_note(0) {
        OutputNote::Full(n) => n.clone(),
        _ => panic!("Expected OutputNote::Full, got something else"),
    };

    wait_for_note(client, &minted_note).await?;

    let consume_req = TransactionRequestBuilder::new()
        .input_notes([(minted_note, None)])
        .build()
        .unwrap();

    let _tx_id = client
        .submit_new_transaction(account.id(), consume_req)
        .await?;
    client.sync_state().await?;
    let new_balance_user = fetch_vault_for_account_from_chain(&clients.rpc_api, asset_id).await?;
    println!("New account vault: {:?}", new_balance_user);
    println!("User successfully consumed p2id note into its wallet");

    Ok(())
}

/// Waits for a specific note to become consumable.
///
/// # Arguments
/// * `client`: Miden client instance
/// * `_account_id`: Account ID (unused but kept for API compatibility)
/// * `expected`: The note to wait for
pub async fn wait_for_note(client: &mut MidenClient, expected: &Note) -> Result<(), ClientError> {
    loop {
        client.sync_state().await?;
        let notes = client.get_consumable_notes(None).await?;
        let found = notes.iter().any(|(rec, _)| rec.id() == expected.id());
        if found {
            info!("Note found {}", expected.id().to_hex());
            break;
        }
        debug!("Note {} not found. Waiting...", expected.id().to_hex());
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    Ok(())
}

pub fn get_return_note_serial(input_note_serial: Word, user_id: AccountId) -> Word {
    // let user_id_word = Word::new([
    //     Felt::ZERO,
    //     Felt::ZERO,
    //     user_id.prefix().as_felt(),
    //     user_id.suffix(),
    // ]);
    // let mut serial_reversed = input_note_serial.clone();
    // serial_reversed.reverse();

    // let mut user_id_word_reversed = user_id_word.clone();
    // user_id_word_reversed.reverse();
    // // Rpo256::merge(&[user_id_word, serial_reversed])
    // Rpo256::merge(&[user_id_word_reversed, input_note_serial])
    [
        input_note_serial[3] + Felt::new(1),
        input_note_serial[2],
        input_note_serial[1],
        input_note_serial[0],
        // input_note_serial[0],
        // input_note_serial[1],
        // input_note_serial[2],
        // input_note_serial[3] + Felt::new(1),
    ]
    .into()
}

pub async fn touch_account(client: &mut MidenClient, account: &Account) -> Result<()> {
    let transaction_request = TransactionRequestBuilder::new().build()?;
    let _tx_id = client
        .submit_new_transaction(account.id(), transaction_request)
        .await?;
    client.sync_state().await?;
    Ok(())
}
