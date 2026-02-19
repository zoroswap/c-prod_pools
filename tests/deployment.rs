mod test_utils;

use anyhow::Result;
use chrono::Utc;
use miden_client::crypto::FeltRng;
use miden_client::{
    Felt, Word,
    asset::FungibleAsset,
    note::{NoteTag, NoteType},
    transaction::{OutputNote, TransactionRequestBuilder},
};
use std::time::Duration;
use test_utils::*;

#[tokio::test]
async fn smoke_test() -> Result<()> {
    let mut setup = setup_test_environment().await?;
    setup.fund_user_wallet(100_000).await?;
    println!("Setup: {:?}", setup.user.id());
    Ok(())
}
