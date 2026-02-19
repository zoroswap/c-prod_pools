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

use serde::Deserialize;

use crate::utils::{create_library, fetch_vault_for_account_from_chain, slot_name};

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
        .sqlite_store("store.sqlite3".into())
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
    Ok((account, key_pair))
}

pub async fn deploy_c_prod_pool(
    client: &mut MidenClient,
    keystore: FilesystemKeyStore,
) -> Result<(Account, AuthSecretKey), ClientError> {
    let sync_summary = client.sync_state().await?;
    println!("\nLatest block: {}", sync_summary.block_num);
    println!("\n[STEP 1] Create c_prod_pool account");

    // Load the MASM file for the counter contract
    let manifest_dir: &str = env!("CARGO_MANIFEST_DIR");

    let c_prod_pool_code_path: PathBuf = [manifest_dir, "masm", "accounts", "c_prod_pool.masm"]
        .iter()
        .collect();
    let pool_code = fs::read_to_string(&c_prod_pool_code_path)
        .unwrap_or_else(|err| panic!("unable to read from {c_prod_pool_code_path:?}: {err}"));

    let assembler = TransactionKernel::assembler().with_warnings_as_errors(true);

    // let fees_mapping = StorageSlot::with_map(n("zoroswap::fees"), fees_mapping);
    let reserves = StorageSlot::with_empty_value(slot_name("zoro::c_prod_pool::reserve"));
    // let user_deposits_mapping = StorageSlot::with_empty_map(n("zoroswap::user_deposits"));

    // Compile the account code into a Library, then create AccountComponent
    let c_prod_pool_library = create_library(assembler.clone(), "zoro::c_prod_pool", &pool_code)
        .map_err(|e| anyhow!("Failed to create pool library: {e}"))
        .unwrap();
    let c_prod_pool_component =
        AccountComponent::new(c_prod_pool_library, vec![reserves])?.with_supports_all_types();

    // Init seed for the pool contract
    let mut init_seed = [0_u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let key_pair = AuthSecretKey::new_falcon512_rpo_with_rng(client.rng());

    // Build the new `Account` with the component
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
    // println!(
    //     "contract id: {:?}",
    //     c_prod_pool_contract.id().to_bech32())
    // );

    keystore.add_key(&key_pair).unwrap();
    client
        .add_account(&c_prod_pool_contract.clone(), false)
        .await?;
    client.sync_state().await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    /*
        println!("\n[STEP 2] Mint tokens from our faucet to two_pools_account");

        let (lp_account, _) = create_basic_account(&mut client, keystore.clone()).await?;

        let amount = 1000000;
        for pool in config.liquidity_pools.iter() {
            println!("liq pool: {:?}", pool.name);
            println!("Importing the faucet account to client");
            client.import_account_by_id(pool.faucet_id).await?;
            let amount_raw: u64 = amount * 10u64.pow(pool.decimals as u32);
            let fungible_asset = FungibleAsset::new(pool.faucet_id, amount_raw)?;
            let transaction_request = TransactionRequestBuilder::new().build_mint_fungible_asset(
                fungible_asset,
                lp_account.id(),
                NoteType::Public,
                client.rng(),
            )?;
            println!("tx request built");
            let _tx_id = client
                .submit_new_transaction(pool.faucet_id, transaction_request)
                .await?;
            println!("Minted note of {} tokens for liq pool.", amount_raw);
            client.sync_state().await?;
        }

        loop {
            // Resync to get the latest data
            client.sync_state().await?;

            let consumable_notes = client.get_consumable_notes(Some(lp_account.id())).await?;
            let notes: Vec<Note> = consumable_notes
                .iter()
                .filter_map(|(rec, _)| {
                    let metadata = rec.metadata()?;
                    Some(Note::new(
                        rec.details().assets().clone(),
                        metadata.clone(),
                        rec.details().recipient().clone(),
                    ))
                })
                .collect();

            if notes.len() == config.liquidity_pools.len() {
                println!("Found consumable notes for lp account. Consuming them now...");
                let transaction_request =
                    TransactionRequestBuilder::new().build_consume_notes(notes)?;
                let _tx_id = client
                    .submit_new_transaction(lp_account.id(), transaction_request)
                    .await?;

                println!("All of liq pool's P2ID notes consumed successfully.");
                break;
            } else {
                println!(
                    "Currently, liq pool has {} consumable P2ID notes. Waiting...",
                    notes.len()
                );
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }

        // Re-sync so minted notes become visible
        client.sync_state().await?;

        let pool_contract_tag = NoteTag::with_account_target(pool_contract.id());

        // Retrieve updated contract data to see the state
        let account = client
            .get_account(pool_contract.id())
            .await?
            .ok_or(anyhow!("Account {:?} not found.", pool_contract.id()))?;
        println!("pool contract storage: {:?}", {
            use miden_client::store::AccountRecordData;
            match account.account_data() {
                AccountRecordData::Full(acc) => format!("{:?}", acc.storage()),
                AccountRecordData::Partial(_) => "partial account".to_string(),
            }
        });
        println!("\n[STEP 3] Make DEPOSIT notes for each liq pool");

        for pool in config.liquidity_pools.iter() {
            println!("liq pool: {:?}", pool.name);
            // println!("Importing the lp account to client");
            //client.import_account_by_id(lp_account.id()).await?;
            let amount_in: u64 = amount * 10u64.pow(pool.decimals as u32);
            let max_slippage = 0.005; // 0.5 %
            let min_lp_amount_out = (amount_in as f64) * (1.0 - max_slippage);
            let min_lp_amount_out = min_lp_amount_out as u64;
            let asset_in = FungibleAsset::new(pool.faucet_id, amount_in)?;
            //let asset_out: FungibleAsset = FungibleAsset::new(pool1.faucet_id, min_amount_out)?;
            // let requested_asset_word: Word = asset_out.into();
            let p2id_tag = NoteTag::with_account_target(lp_account.id());
            let deadline = (Utc::now().timestamp_millis() as u64) + 120000;
            let inputs = vec![
                Felt::new(0),
                Felt::new(min_lp_amount_out), // min_lp_amount_out
                Felt::new(deadline),          // deadline
                p2id_tag.into(),              // p2id tag
                Felt::new(0),
                Felt::new(0),
                lp_account.id().suffix(),
                lp_account.id().prefix().into(),
            ];
            let deposit_serial_num = client.rng().draw_word();
            println!(
                "Made an deposit note for {amount_in} {} expecting  at least {min_lp_amount_out} lp amount out.",
                pool.symbol
            );
            let deposit_note = create_deposit_note(
                inputs,
                vec![asset_in.into()],
                lp_account.id(),
                deposit_serial_num,
                pool_contract_tag,
                NoteType::Public,
            )?;

            let note_req = TransactionRequestBuilder::new()
                .own_output_notes(vec![OutputNote::Full(deposit_note.clone())])
                .build()
                .unwrap();

            println!("tx request built");
            let _tx_id = client
                .submit_new_transaction(lp_account.id(), note_req)
                .await?;
            println!("Minted note of {} tokens for liq pool.", amount_in);
            client.sync_state().await?;
        }

        // Consume DEPOSIT notes by POOL CONTRACT
        let failed_notes = Vec::new();
        loop {
            ////////    !!!!!!!!!!!!!!!!!!!!!!!!!

            // Resync to get the latest data
            match fetch_new_notes_by_tag(&mut client, &pool_contract_tag).await {
                Ok(notes) => {
                    let valid_notes: Vec<&Note> = notes
                        .iter()
                        .filter(|n| !failed_notes.contains(&n.id()))
                        .collect();

                    let number_of_notes = valid_notes.len();
                    if number_of_notes == config.liquidity_pools.len() {
                        println!(
                            "Found consumable DEPOSIT notes for pool contract account. Consuming them now..."
                        );

                        let in_amount: u64 = amount * 10u64.pow(8);
                        let args: Word = [
                            Felt::new(in_amount),
                            Felt::new(in_amount),
                            Felt::new(in_amount),
                            Felt::new(in_amount),
                        ]
                        .into();
                        let consume_req = TransactionRequestBuilder::new()
                            .input_notes(
                                valid_notes
                                    .iter()
                                    .map(|deposit_note| ((*deposit_note).clone(), Some(args)))
                                    .collect::<Vec<_>>(),
                            )
                            .build()
                            .map_err(|e| {
                                anyhow::anyhow!("Failed to build batch transaction request: {}", e)
                            })?;
                        let _tx_id = client
                            .submit_new_transaction(pool_contract.id(), consume_req)
                            .await?;

                        println!("All of liq pool's DEPOSIT notes consumed successfully.");
                        break;
                    } else {
                        println!(
                            "Currently, pool contract has {} consumable DEPOSIT notes. Waiting...",
                            number_of_notes
                        );
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                }
                Err(e) => {
                    println!("Error in listening for zoro swap notes: {}", e);
                }
            };
        }

        println!("\n[STEP 3] Set initial states of the two_pools_account");

        for pool in config.liquidity_pools.iter() {
            let (balances_pool, settings_pool) =
                fetch_pool_state_from_chain(&mut client, pool_contract.id(), pool.faucet_id).await?;
            let vault = fetch_vault_for_account_from_chain(&mut client, pool_contract.id()).await?;
            let total_supply =
                fetch_lp_total_supply_from_chain(&mut client, pool_contract.id(), pool.faucet_id)
                    .await?;
            println!(
                "Liquidity {} ({})",
                pool.name,
                pool.faucet_id.to_bech32(config.network_id.clone())
            );
            println!("Balances {:?}", balances_pool,);
            println!("Settings {:?}", settings_pool);
            println!("pool vault: {vault:?}");
            println!("pool lp total supply: {total_supply}");
        }
        println!(
            "\n------\n New pool created: {:?}\n-----\n",
            pool_contract.id().to_bech32(endpoint.to_network_id())
        );
    */
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

        let transaction_request = TransactionRequestBuilder::new().build()?;
        let _tx_id = client
            .submit_new_transaction(account.id(), transaction_request)
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
