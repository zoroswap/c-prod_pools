use anyhow::{Result, anyhow};
use c_prod_pool::common::{
    CachedFaucet, CachedTestState, Faucet, FaucetConfig, MidenClients, create_basic_account,
    deploy_c_prod_pool, deploy_simple_faucets_from_config, deploy_storage_fuzz_dummy, fund_wallet,
    instantiate_simple_client, load_faucets_config, load_test_state, save_test_state,
    try_import_account,
};
use c_prod_pool::utils::fetch_vault_for_account_from_chain;
use miden_client::{
    Felt, Word,
    account::{Account, AccountId},
    asset::FungibleAsset,
    keystore::FilesystemKeyStore,
    note::{NoteTag, NoteType},
    rpc::Endpoint,
    transaction::{OutputNote, TransactionRequestBuilder},
};
use std::{env, fs, path::PathBuf};

const DEFAULT_FUND_AMOUNT: u64 = 100_000;
// use url::Url;
// use zoro_miden_client::{MidenClient, create_basic_account, wait_for_note};
// use zoroswap::{
//     Config, PoolBalances, config::LiquidityPoolConfig, fetch_pool_state_from_chain,
//     fetch_vault_for_account_from_chain, instantiate_client, oracle_sse::PriceMetadata,
// };

/// Common state returned by [`setup_test_environment`] for E2E tests.
pub struct TestSetup {
    /// Parsed application config (endpoints, pool account ID, oracle URL, etc.).
    // pub config: Config,
    /// Miden client connected to the node and ready to submit transactions.
    pub clients: MidenClients,
    /// A freshly created basic account that acts as the test user.
    pub user: Account,
    pub pool: Account,
    // The first two liquidity pools from the config, used as swap pair.
    // pub pools: Vec<LiquidityPoolConfig>,
    pub faucets: Vec<Faucet>,
}

impl TestSetup {
    /// Funds the user wallet with the given amount from every configured faucet,
    /// regardless of current balance.
    pub async fn fund_user_wallet(&mut self, amount: u64) -> Result<()> {
        self.clients.client.sync_state().await?;
        for asset in self.faucets.iter() {
            fund_wallet(
                &mut self.clients,
                &self.user,
                &asset.config,
                &asset.faucet.id().clone(),
                amount,
            )
            .await?;
        }
        Ok(())
    }

    /// Funds the user wallet only for faucets where the on-chain balance is below `amount`.
    /// Skips funding for any faucet that already has sufficient balance.
    pub async fn maybe_fund_user_wallet(&mut self, amount: u64) -> Result<()> {
        self.clients.client.sync_state().await?;
        let vault =
            fetch_vault_for_account_from_chain(&self.clients.rpc_api, &self.user.id()).await?;

        for asset in self.faucets.iter() {
            let faucet_id = asset.faucet.id();
            let current = vault.get_balance(faucet_id).unwrap_or(0);
            if current >= amount {
                println!(
                    "{}: balance {} >= {}, skipping funding",
                    asset.config.symbol, current, amount
                );
                continue;
            }
            let needed = amount - current;
            println!(
                "{}: balance {} < {}, funding {} more",
                asset.config.symbol, current, amount, needed
            );
            fund_wallet(
                &mut self.clients,
                &self.user,
                &asset.config,
                &faucet_id,
                needed,
            )
            .await?;
        }
        Ok(())
    }
}

/// Try to restore faucets and user from a cached test state file.
/// Returns the reconstructed `(Vec<Faucet>, Account)` on success.
async fn try_restore_from_cache(
    clients: &mut MidenClients,
    state: &CachedTestState,
) -> Result<(Vec<Faucet>, Account)> {
    let mut faucets = Vec::with_capacity(state.faucets.len());
    for s_faucet in &state.faucets {
        let id = AccountId::from_hex(&s_faucet.account_id_hex)
            .map_err(|e| anyhow!("Bad cached faucet id '{}': {e}", s_faucet.account_id_hex))?;
        let account = try_import_account(clients, id).await?;
        faucets.push(Faucet {
            faucet: account,
            config: FaucetConfig {
                symbol: s_faucet.symbol.clone(),
                decimals: s_faucet.decimals,
                max_supply: s_faucet.max_supply,
            },
        });
        println!(
            "Restored faucet {} ({})",
            s_faucet.symbol, s_faucet.account_id_hex
        );
    }

    let user_id = AccountId::from_hex(&state.user_account_id_hex)
        .map_err(|e| anyhow!("Bad cached user id '{}': {e}", state.user_account_id_hex))?;
    let user = try_import_account(clients, user_id).await?;
    println!("Restored user account ({})", state.user_account_id_hex);

    Ok((faucets, user))
}

/// Deploy fresh faucets and create a new user account.
async fn deploy_fresh(
    clients: &mut MidenClients,
    keystore: &FilesystemKeyStore,
) -> Result<(Vec<Faucet>, Account)> {
    let client = &mut clients.client;
    let faucets = deploy_simple_faucets_from_config(client, keystore).await?;

    println!("\nCreating user account...");
    let (user, _) = create_basic_account(client, keystore.clone()).await?;
    println!(
        "Created User Account => ID: {:?} {:?}",
        user.id().to_hex(),
        user.id()
    );
    client.sync_state().await?;

    Ok((faucets, user))
}

fn build_cached_state(faucets: &[Faucet], user: &Account) -> CachedTestState {
    CachedTestState {
        faucets: faucets
            .iter()
            .map(|f| CachedFaucet {
                account_id_hex: f.faucet.id().to_hex(),
                symbol: f.config.symbol.clone(),
                decimals: f.config.decimals,
                max_supply: f.config.max_supply,
            })
            .collect(),
        user_account_id_hex: user.id().to_hex(),
    }
}

/// Load config, create a Miden client, sync state, and create a fresh basic account.
/// Reuses previously deployed faucets and user when a `test_state.toml` cache exists.
/// Set `FRESH_SETUP=1` to force a clean deployment.
pub async fn setup_test_environment() -> Result<TestSetup> {
    dotenv::dotenv().ok();

    let endpoint_label =
        env::var("MIDEN_NODE_ENDPOINT").unwrap_or_else(|_| "localhost".to_string());
    let endpoint = match endpoint_label.as_str() {
        "testnet" => Endpoint::testnet(),
        "devnet" => Endpoint::devnet(),
        _ => Endpoint::localhost(),
    };

    let base_dir = PathBuf::from("tmp").join(&endpoint_label);
    fs::create_dir_all(&base_dir)?;

    let keystore_path = base_dir.join("keystore");
    let store_path = base_dir.join("store.sqlite3");
    let state_path = base_dir.join("test_state.toml");

    let force_fresh = env::var("CLEAN_TEST").map_or(false, |v| v == "1");

    if force_fresh && store_path.exists() {
        println!(
            "CLEAN_TEST=1 — removing old store at {}",
            store_path.display()
        );
        fs::remove_file(&store_path)?;
    }

    let keystore_str = keystore_path.to_str().unwrap();
    let store_str = store_path.to_str().unwrap();

    let mut clients = instantiate_simple_client(keystore_str, store_str, &endpoint).await?;
    let keystore = FilesystemKeyStore::new(keystore_path.clone())?;

    let mut is_user_fresh = force_fresh;
    let (faucets, user) = if force_fresh {
        println!("CLEAN_TEST=1 — deploying fresh faucets and user.");
        deploy_fresh(&mut clients, &keystore).await?
    } else {
        match load_test_state(&state_path) {
            Some(cached) => match try_restore_from_cache(&mut clients, &cached).await {
                Ok(result) => {
                    println!("Reusing cached faucets and user from previous run.");
                    result
                }
                Err(e) => {
                    println!("Cache restore failed ({e}), deploying fresh...");
                    is_user_fresh = true;
                    deploy_fresh(&mut clients, &keystore).await?
                }
            },
            None => {
                println!("No cached test state found, deploying fresh...");
                is_user_fresh = true;
                deploy_fresh(&mut clients, &keystore).await?
            }
        }
    };

    save_test_state(&state_path, &build_cached_state(&faucets, &user))?;

    let token0_id = faucets[0].faucet.id();
    let token1_id = faucets[1].faucet.id();
    let (c_prod_pool, _) = deploy_c_prod_pool(
        &mut clients.client,
        keystore.clone(),
        &token0_id,
        &token1_id,
    )
    .await?;
    println!(
        "Created C Prod Pool Account => ID: {:?} {:?}",
        c_prod_pool.id().to_bech32(endpoint.to_network_id()),
        c_prod_pool.id().to_hex()
    );

    let mut setup = TestSetup {
        clients,
        user,
        pool: c_prod_pool,
        faucets,
    };

    if is_user_fresh {
        println!("Funding user wallet...");
        setup.maybe_fund_user_wallet(DEFAULT_FUND_AMOUNT).await?;
    }

    Ok(setup)
}

pub fn expected_amount_out(reserve_in: Felt, reserve_out: Felt, amount_in: Felt) -> Felt {
    let fee_adjusted = amount_in.as_int() as u128 * 997;
    let numerator = reserve_out.as_int() as u128 * fee_adjusted;
    let denominator = reserve_in.as_int() as u128 * 1000 + fee_adjusted;
    Felt::new((numerator / denominator) as u64)
}

/// Minimal test harness: client + one basic account. No faucets, pool, or funding.
pub struct LightweightTestSetup {
    pub clients: MidenClients,
    pub account: Account,
}

pub async fn setup_lightweight_environment() -> Result<LightweightTestSetup> {
    dotenv::dotenv().ok();

    let endpoint_label =
        env::var("MIDEN_NODE_ENDPOINT").unwrap_or_else(|_| "localhost".to_string());
    let endpoint = match endpoint_label.as_str() {
        "testnet" => Endpoint::testnet(),
        "devnet" => Endpoint::devnet(),
        _ => Endpoint::localhost(),
    };

    let base_dir = PathBuf::from("tmp").join(&endpoint_label);
    fs::create_dir_all(&base_dir)?;

    let keystore_path = base_dir.join("keystore");
    let store_path = base_dir.join("store.sqlite3");

    let keystore_str = keystore_path.to_str().unwrap();
    let store_str = store_path.to_str().unwrap();

    let mut clients = instantiate_simple_client(keystore_str, store_str, &endpoint).await?;
    let keystore = FilesystemKeyStore::new(keystore_path.clone())?;

    let (account, _) = create_basic_account(&mut clients.client, keystore).await?;
    println!(
        "Lightweight setup: account {:?}",
        account.id().to_hex()
    );

    Ok(LightweightTestSetup { clients, account })
}

/// Minimal test harness for storage_utils fuzz tests: client + dummy account with value and map slots.
pub struct StorageFuzzTestSetup {
    pub clients: MidenClients,
    pub dummy_account: Account,
}

pub async fn setup_storage_fuzz_environment() -> Result<StorageFuzzTestSetup> {
    dotenv::dotenv().ok();

    let endpoint_label =
        env::var("MIDEN_NODE_ENDPOINT").unwrap_or_else(|_| "localhost".to_string());
    let endpoint = match endpoint_label.as_str() {
        "testnet" => Endpoint::testnet(),
        "devnet" => Endpoint::devnet(),
        _ => Endpoint::localhost(),
    };

    let base_dir = PathBuf::from("tmp").join(&endpoint_label);
    fs::create_dir_all(&base_dir)?;

    let keystore_path = base_dir.join("keystore");
    let store_path = base_dir.join("store.sqlite3");

    let keystore_str = keystore_path.to_str().unwrap();
    let store_str = store_path.to_str().unwrap();

    let mut clients = instantiate_simple_client(keystore_str, store_str, &endpoint).await?;
    let keystore = FilesystemKeyStore::new(keystore_path.clone())?;

    let (dummy_account, _) =
        deploy_storage_fuzz_dummy(&mut clients.client, keystore).await?;
    println!(
        "Storage fuzz setup: dummy account {:?}",
        dummy_account.id().to_hex()
    );

    Ok(StorageFuzzTestSetup {
        clients,
        dummy_account,
    })
}
