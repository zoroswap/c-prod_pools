use anyhow::{Context, Result, anyhow};
use miden_client::{
    ClientError, DebugMode, Felt, Word,
    account::{
        Account, AccountBuilder, AccountId, AccountStorageMode, AccountType, StorageMap,
        StorageSlot, StorageSlotName,
    },
    asset::{AssetVault, FungibleAsset, TokenSymbol},
    auth::{AuthFalcon512Rpo, AuthSecretKey},
    builder::ClientBuilder,
    keystore::FilesystemKeyStore,
    note::{
        Note, NoteAssets, NoteError, NoteMetadata, NoteRecipient, NoteScreener, NoteTag, NoteType,
    },
    rpc::{GrpcClient, NodeRpcClient, domain::account::FetchedAccount},
    store::TransactionFilter,
    sync::StateSync,
    transaction::{OutputNote, TransactionRequestBuilder},
};
use miden_standards::account::{faucets::BasicFungibleFaucet, wallets::BasicWallet};

use rand::RngCore;

use miden_client_sqlite_store::{ClientBuilderSqliteExt, SqliteStore};
use miden_protocol::{account::AccountComponent, transaction::TransactionKernel};

use miden_standards::code_builder::CodeBuilder;
use miden_standards::note::utils::build_p2id_recipient;
use rusqlite::Connection;
use std::sync::Arc;
use std::{fs, path::PathBuf, time::Duration};
use tracing::{debug, info, warn};

use serde::{Deserialize, Serialize};

use crate::utils::{
    create_library, extract_full_account, fetch_vault_for_account_from_chain, slot_name,
};

//use crate::{Config, order::OrderType};
//use zoro_miden_client::{MidenClient, create_library};

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
    let transaction_request = TransactionRequestBuilder::new().build()?;
    let _tx_id = client
        .submit_new_transaction(account.id(), transaction_request)
        .await?;

    Ok((account, key_pair))
}

/// Deploys a constant-product pool account configured for the given token pair.
///
/// Storage slots:
///   - `reserve`:   [reserve0, reserve1, total_lp, 0]  (initially empty)
///   - `config`:    [token0_prefix, token0_suffix, token1_prefix, token1_suffix]
///   - `lp_shares`: StorageMap (initially empty)
pub async fn deploy_c_prod_pool(
    client: &mut MidenClient,
    keystore: FilesystemKeyStore,
    token0_id: &AccountId,
    token1_id: &AccountId,
) -> Result<(Account, AuthSecretKey), ClientError> {
    let sync_summary = client.sync_state().await?;
    println!("\nLatest block: {}", sync_summary.block_num);
    println!("\n[STEP 1] Create c_prod_pool account");

    let manifest_dir: &str = env!("CARGO_MANIFEST_DIR");

    let c_prod_pool_code_path: PathBuf = [manifest_dir, "masm", "accounts", "c_prod_pool.masm"]
        .iter()
        .collect();
    let pool_code = fs::read_to_string(&c_prod_pool_code_path)
        .unwrap_or_else(|err| panic!("unable to read from {c_prod_pool_code_path:?}: {err}"));

    let assembler = TransactionKernel::assembler(); //.with_warnings_as_errors(true);

    let reserves = StorageSlot::with_empty_value(slot_name("zoro::c_prod_pool::reserve"));

    let config_word: Word = [
        token0_id.prefix().as_felt(),
        token0_id.suffix(),
        token1_id.prefix().as_felt(),
        token1_id.suffix(),
    ]
    .into();
    let config = StorageSlot::with_value(slot_name("zoro::c_prod_pool::config"), config_word);

    let lp_shares = StorageSlot::with_empty_map(slot_name("zoro::c_prod_pool::lp_shares"));

    let c_prod_pool_library = create_library(assembler.clone(), "zoro::c_prod_pool", &pool_code)
        .map_err(|e| anyhow!("Failed to create pool library: {e:?}"))
        .unwrap();
    let c_prod_pool_component =
        AccountComponent::new(c_prod_pool_library, vec![reserves, config, lp_shares])?
            .with_supports_all_types();

    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let key_pair = AuthSecretKey::new_falcon512_rpo_with_rng(client.rng());

    let c_prod_pool_contract = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_component(c_prod_pool_component.clone())
        .with_auth_component(AuthFalcon512Rpo::new(key_pair.public_key().to_commitment()))
        .with_component(BasicWallet)
        .build()?;

    println!(
        "pool contract commitment hash: {:?}",
        c_prod_pool_contract.commitment().to_hex()
    );
    println!(
        "pool config: token0={}, token1={}",
        token0_id.to_hex(),
        token1_id.to_hex(),
    );

    keystore.add_key(&key_pair).unwrap();
    client
        .add_account(&c_prod_pool_contract.clone(), false)
        .await?;
    client.sync_state().await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    Ok((c_prod_pool_contract, key_pair))
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
    let transaction_request = TransactionRequestBuilder::new().build()?;
    let _tx_id = client
        .submit_new_transaction(faucet_account.id(), transaction_request)
        .await?;

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
    println!("User successfully consumed swap into its wallet");

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
