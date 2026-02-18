use anyhow::Result;
use miden_client::{
    ClientError, Felt, Word,
    account::AccountId,
    note::{
        Note, NoteAssets, NoteError, NoteMetadata, NoteRecipient, NoteScreener, NoteTag, NoteType,
    },
    sync::StateSync,
};
use miden_client::{
    DebugMode,
    account::{Account, AccountBuilder, AccountStorageMode, AccountType},
    auth::{AuthFalcon512Rpo, AuthSecretKey},
    builder::ClientBuilder,
    keystore::FilesystemKeyStore,
    rpc::GrpcClient,
};
use miden_standards::account::wallets::BasicWallet;

use rand::RngCore;

use miden_client_sqlite_store::{ClientBuilderSqliteExt, SqliteStore};
use miden_protocol::transaction::TransactionKernel;
use miden_standards::code_builder::CodeBuilder;
use miden_standards::note::utils::build_p2id_recipient;
use rusqlite::Connection;
use std::sync::Arc;
use std::{fs, path::PathBuf};
use tracing::{debug, info, warn};

//use crate::{Config, order::OrderType};
//use zoro_miden_client::{MidenClient, create_library};

use miden_client::{Client, rpc::Endpoint};
pub type MidenClient = Client<FilesystemKeyStore>;

pub async fn instantiate_simple_client(
    keystore_path: &str,
    endpoint: &Endpoint,
) -> Result<MidenClient, ClientError> {
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

    Ok(client)
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
