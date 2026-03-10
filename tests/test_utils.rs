use anyhow::{Result, anyhow};
use c_prod_pool::common::{
    CachedFaucet, CachedTestState, Faucet, FaucetConfig, MidenClients, create_basic_account,
    deploy_c_prod_pool, deploy_combined_pool, deploy_lp_local_fuzz_dummy, deploy_lp_local_pool,
    deploy_simple_faucets_from_config, deploy_storage_fuzz_dummy, fund_wallet,
    instantiate_simple_client, load_test_state, save_test_state, try_import_account,
};
use c_prod_pool::utils::fetch_vault_for_account_from_chain;
use miden_client::{
    Felt,
    account::{Account, AccountId},
    keystore::FilesystemKeyStore,
    rpc::Endpoint,
};
use std::{env, fs, path::PathBuf};

const DEFAULT_FUND_AMOUNT: u64 = 1_000_000_000_000;

pub struct TestSetup {
    pub clients: MidenClients,
    pub user: Account,
    pub contract: Account,
    pub faucets: Vec<Faucet>,
}

impl TestSetup {
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

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn resolve_endpoint() -> (String, Endpoint) {
    let label = env::var("MIDEN_NODE_ENDPOINT").unwrap_or_else(|_| "localhost".to_string());
    let endpoint = match label.as_str() {
        "testnet" => Endpoint::testnet(),
        "devnet" => Endpoint::devnet(),
        _ => Endpoint::localhost(),
    };
    (label, endpoint)
}

fn maybe_clean_store(base_dir: &PathBuf) {
    let force_fresh = env::var("CLEAN_TEST").map_or(false, |v| v == "1");
    if force_fresh {
        let store_path = base_dir.join("store.sqlite3");
        if store_path.exists() {
            println!(
                "CLEAN_TEST=1 — removing old store at {}",
                store_path.display()
            );
            fs::remove_file(&store_path).ok();
        }
    }
}

async fn init_clients(
    base_dir: &PathBuf,
    endpoint: &Endpoint,
) -> Result<(MidenClients, FilesystemKeyStore)> {
    maybe_clean_store(base_dir);
    let keystore_path = base_dir.join("keystore");
    let store_path = base_dir.join("store.sqlite3");
    let clients = instantiate_simple_client(
        keystore_path.to_str().unwrap(),
        store_path.to_str().unwrap(),
        endpoint,
    )
    .await?;
    let keystore = FilesystemKeyStore::new(keystore_path)?;
    Ok((clients, keystore))
}

// ---------------------------------------------------------------------------
// Cache helpers (for setups that deploy faucets + user)
// ---------------------------------------------------------------------------

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

/// Resolves faucets + user from cache or deploys fresh, saves cache.
async fn resolve_faucets_and_user(
    clients: &mut MidenClients,
    keystore: &FilesystemKeyStore,
    base_dir: &PathBuf,
) -> Result<(Vec<Faucet>, Account, bool)> {
    let state_path = base_dir.join("test_state.toml");
    let force_fresh = env::var("CLEAN_TEST").map_or(false, |v| v == "1");
    let mut is_user_fresh = force_fresh;
    let (faucets, user) = if force_fresh {
        println!("CLEAN_TEST=1 — deploying fresh faucets and user.");

        deploy_fresh(clients, keystore).await?
    } else {
        match load_test_state(&state_path) {
            Some(cached) => match try_restore_from_cache(clients, &cached).await {
                Ok(result) => {
                    println!("Reusing cached faucets and user from previous run.");
                    result
                }
                Err(e) => {
                    println!("Cache restore failed ({e}), deploying fresh...");
                    is_user_fresh = true;
                    deploy_fresh(clients, keystore).await?
                }
            },
            None => {
                println!("No cached test state found, deploying fresh...");
                is_user_fresh = true;
                deploy_fresh(clients, keystore).await?
            }
        }
    };

    save_test_state(&state_path, &build_cached_state(&faucets, &user))?;
    Ok((faucets, user, is_user_fresh))
}

// ---------------------------------------------------------------------------
// Setup functions — all return TestSetup
// ---------------------------------------------------------------------------

pub fn expected_amount_out(reserve_in: Felt, reserve_out: Felt, amount_in: Felt) -> Felt {
    let fee_adjusted = amount_in.as_int() as u128 * 997;
    let numerator = reserve_out.as_int() as u128 * fee_adjusted;
    let denominator = reserve_in.as_int() as u128 * 1000 + fee_adjusted;
    Felt::new((numerator / denominator) as u64)
}

pub fn expected_amount_in(amount_out: Felt, reserve_in: Felt, reserve_out: Felt) -> Felt {
    let amount_out_scaled = amount_out.as_int() as u128 * 1000;
    let numerator = reserve_in.as_int() as u128 * amount_out_scaled;
    let denominator = (reserve_out.as_int() as u128 - amount_out.as_int() as u128) * 997;
    Felt::new((numerator / denominator) as u64)
}

/// Minimal setup: client + one basic account. No faucets, no contract deployment.
/// `user` and `contract` point to the same basic account.
pub async fn setup_lightweight_environment() -> Result<TestSetup> {
    dotenv::dotenv().ok();
    let (label, endpoint) = resolve_endpoint();
    let base_dir = PathBuf::from("tmp").join(&label);
    fs::create_dir_all(&base_dir)?;

    let (mut clients, keystore) = init_clients(&base_dir, &endpoint).await?;
    let (account, _) = create_basic_account(&mut clients.client, keystore).await?;
    println!("Lightweight setup: account {:?}", account.id().to_hex());

    Ok(TestSetup {
        clients,
        user: account.clone(),
        contract: account,
        faucets: vec![],
    })
}

/// Full c_prod_pool setup: faucets, user, pool contract, funding.
pub async fn setup_test_environment() -> Result<TestSetup> {
    dotenv::dotenv().ok();
    let (label, endpoint) = resolve_endpoint();
    let base_dir = PathBuf::from("tmp").join(&label);
    fs::create_dir_all(&base_dir)?;

    let (mut clients, keystore) = init_clients(&base_dir, &endpoint).await?;
    let (faucets, user, is_user_fresh) =
        resolve_faucets_and_user(&mut clients, &keystore, &base_dir).await?;

    let token0_id = faucets[0].faucet.id();
    let token1_id = faucets[1].faucet.id();
    let (pool, _) = deploy_c_prod_pool(
        &mut clients.client,
        keystore.clone(),
        &token0_id,
        &token1_id,
    )
    .await?;
    println!(
        "Created C Prod Pool Account => ID: {:?} {:?}",
        pool.id().to_bech32(endpoint.to_network_id()),
        pool.id().to_hex()
    );

    let mut setup = TestSetup {
        clients,
        user,
        contract: pool,
        faucets,
    };

    if is_user_fresh {
        println!("Funding user wallet...");
        setup.fund_user_wallet(DEFAULT_FUND_AMOUNT).await?;
    }

    Ok(setup)
}

/// Storage utils fuzz setup: client + storage_fuzz_dummy contract.
pub async fn setup_storage_fuzz_environment(
    initial_value: u64,
    initial_map_value: u64,
) -> Result<TestSetup> {
    dotenv::dotenv().ok();
    let (label, endpoint) = resolve_endpoint();
    let base_dir = PathBuf::from("tmp").join(&label);
    fs::create_dir_all(&base_dir)?;

    let (mut clients, keystore) = init_clients(&base_dir, &endpoint).await?;
    let (dummy, _) = deploy_storage_fuzz_dummy(
        &mut clients.client,
        keystore,
        initial_value,
        initial_map_value,
    )
    .await?;
    println!(
        "Storage fuzz setup: dummy account {:?}",
        dummy.id().to_hex()
    );

    Ok(TestSetup {
        clients,
        user: dummy.clone(),
        contract: dummy,
        faucets: vec![],
    })
}

/// LP local mint/burn fuzz setup: client + lp_local_fuzz_dummy contract.
pub async fn setup_lp_local_fuzz_environment() -> Result<TestSetup> {
    dotenv::dotenv().ok();
    let (label, endpoint) = resolve_endpoint();
    let base_dir = PathBuf::from("tmp").join(&label);
    fs::create_dir_all(&base_dir)?;

    let (mut clients, keystore) = init_clients(&base_dir, &endpoint).await?;
    let (dummy, _) = deploy_lp_local_fuzz_dummy(&mut clients.client, keystore).await?;
    println!(
        "LP local fuzz setup: dummy account {:?}",
        dummy.id().to_hex()
    );

    Ok(TestSetup {
        clients,
        user: dummy.clone(),
        contract: dummy,
        faucets: vec![],
    })
}

/// LP local deposit E2E setup: faucets, user, lp_local pool contract, funding.
pub async fn setup_lp_local_test_environment() -> Result<TestSetup> {
    dotenv::dotenv().ok();
    let (label, endpoint) = resolve_endpoint();
    let base_dir = PathBuf::from("tmp").join(&label);
    fs::create_dir_all(&base_dir)?;

    let (mut clients, keystore) = init_clients(&base_dir, &endpoint).await?;
    let (faucets, user, is_user_fresh) =
        resolve_faucets_and_user(&mut clients, &keystore, &base_dir).await?;

    let token0_id = faucets[0].faucet.id();
    let token1_id = faucets[1].faucet.id();
    let (lp_local_pool, _) = deploy_lp_local_pool(
        &mut clients.client,
        keystore.clone(),
        &token0_id,
        &token1_id,
    )
    .await?;

    let mut setup = TestSetup {
        clients,
        user,
        contract: lp_local_pool,
        faucets,
    };

    if is_user_fresh {
        setup.fund_user_wallet(DEFAULT_FUND_AMOUNT).await?;
    }

    Ok(setup)
}

/// Combined pool E2E setup: faucets, user, combined (lp_local + c_prod_pool) contract, funding.
pub async fn setup_combined_pool_test_environment() -> Result<TestSetup> {
    dotenv::dotenv().ok();
    let (label, endpoint) = resolve_endpoint();
    let base_dir = PathBuf::from("tmp").join(&label);
    fs::create_dir_all(&base_dir)?;

    let (mut clients, keystore) = init_clients(&base_dir, &endpoint).await?;
    let (faucets, user, is_user_fresh) =
        resolve_faucets_and_user(&mut clients, &keystore, &base_dir).await?;

    let token0_id = faucets[0].faucet.id();
    let token1_id = faucets[1].faucet.id();
    let (combined_pool, _) = deploy_combined_pool(
        &mut clients.client,
        keystore.clone(),
        &token0_id,
        &token1_id,
    )
    .await?;

    let mut setup = TestSetup {
        clients,
        user,
        contract: combined_pool,
        faucets,
    };

    if is_user_fresh {
        setup.fund_user_wallet(DEFAULT_FUND_AMOUNT).await?;
    }

    Ok(setup)
}
