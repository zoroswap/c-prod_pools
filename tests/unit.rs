mod test_utils;

use anyhow::Result;
use c_prod_pool::pool_ops::{compile_custom_tx_script, get_pool_library};
use miden_client::transaction::TransactionRequestBuilder;
use test_utils::*;

#[tokio::test]
async fn smoke_test() -> Result<()> {
    let mut setup = setup_test_environment().await?;
    setup.maybe_fund_user_wallet(10_000).await?;
    println!("Setup: {:?}", setup.user.id());
    Ok(())
}

#[tokio::test]
async fn get_amount_out_naive_test() -> Result<()> {
    let mut setup = setup_test_environment().await?;
    setup.maybe_fund_user_wallet(10_000).await?;

    let pool_library = get_pool_library()?;

    let reserve_in: u64 = 50_000;
    let reserve_out: u64 = 50_000;
    let amount_in: u64 = 1_000;

    let source = format!(
        "use zoro::c_prod_pool\n\
         use miden::core::sys\n\
         begin\n\
             push.{amount_in}.{reserve_out}.{reserve_in}\n\
             call.c_prod_pool::get_amount_out_naive\n\
             exec.sys::truncate_stack\n\
         end"
    );

    let script = compile_custom_tx_script(&pool_library, &source)?;

    let tx_request = TransactionRequestBuilder::new()
        .custom_script(script)
        .build()?;

    let tx_id = setup
        .clients
        .client
        .submit_new_transaction(setup.pool.id(), tx_request)
        .await?;

    println!("Transaction succeeded: {tx_id:?}");
    println!(
        "get_amount_out_naive(reserve_in={reserve_in}, reserve_out={reserve_out}, amount_in={amount_in}) executed successfully"
    );

    Ok(())
}
