use anyhow::{Context, Result, anyhow};
use c_prod_pool::common::{MidenClient, create_basic_account, instantiate_simple_client};
use miden_client::store::TransactionFilter;
use miden_client::{
    Felt, Word,
    account::{Account, AccountId},
    asset::FungibleAsset,
    keystore::FilesystemKeyStore,
    note::{NoteTag, NoteType},
    rpc::Endpoint,
    transaction::{OutputNote, TransactionRequestBuilder},
};
use std::{collections::HashMap, env, str::FromStr};
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
    pub client: MidenClient,
    /// A freshly created basic account that acts as the test user.
    pub user: Account,
    // pub pool: Account,
    // The first two liquidity pools from the config, used as swap pair.
    // pub pools: Vec<LiquidityPoolConfig>,
}

/// Load config, create a Miden client, sync state, and create a fresh basic account.
pub async fn setup_test_environment() -> Result<TestSetup> {
    dotenv::dotenv().ok();

    // let config = Config::from_config_file(
    //     "../../config.toml",
    //     "../../masm",
    //     "../../keystore",
    //     store_path,
    // )?;

    // assert!(
    //     config.liquidity_pools.len() > 1,
    //     "Less than 2 liquidity pools configured"
    // );

    let store_path = "../test_store.sqlite3";
    let keystore_path = "../keystore";
    let endpoint = env::var("MIDEN_NODE_ENDPOINT").unwrap_or_else(|_| "".to_string());
    let endpoint = match endpoint.as_str() {
        "testnet" => Endpoint::testnet(),
        "devnet" => Endpoint::devnet(),
        _ => Endpoint::localhost(),
    };

    let mut client = instantiate_simple_client(keystore_path, &endpoint).await?;
    let keystore = FilesystemKeyStore::new(keystore_path.into())?;

    println!("\nCreating user account...");
    let (user, _) = create_basic_account(&mut client, keystore.clone()).await?;
    println!(
        "Created User Account ⇒ ID: {:?}",
        user.id().to_bech32(endpoint.to_network_id())
    );
    client.sync_state().await?;

    // Ok(TestSetup { client, user, pool })
    Ok(TestSetup { client, user })
}
