mod test_utils;

use anyhow::Result;
use c_prod_pool::pool_ops::{compile_custom_tx_script, get_pool_library};
use miden_client::{
    Felt,
    transaction::{AdviceInputs, TransactionRequestBuilder},
};
use std::collections::BTreeSet;
use test_utils::*;

#[tokio::test]
async fn smoke_test() -> Result<()> {
    let mut setup = setup_test_environment().await?;
    setup.maybe_fund_user_wallet(10_000).await?;
    println!("Setup: {:?}", setup.user.id());
    Ok(())
}

#[tokio::test]
async fn get_amount_out_u64_fuzz_test() -> Result<()> {
    use rand::Rng;

    let min_reserve: u64 = 1_000_000_000;
    let max_reserve: u64 = 100_000_000_000;
    let min_amount_in: u64 = 100_000;
    let max_amount_in: u64 = 100_000;
    let iterations: usize = 50;

    let mut setup = setup_test_environment().await?;
    setup.maybe_fund_user_wallet(10_000).await?;

    let pool_library = get_pool_library()?;
    let mut rng = rand::rng();

    for i in 0..iterations {
        let reserve_in = Felt::new(rng.random_range(min_reserve..=max_reserve));
        let reserve_out = Felt::new(rng.random_range(min_reserve..=max_reserve));
        let amount_in = Felt::new(rng.random_range(min_amount_in..=max_amount_in));

        let source = format!(
            "use zoro::c_prod_pool\n\
             use miden::core::sys\n\
             begin\n\
                 push.{amount_in}.{reserve_out}.{reserve_in}\n\
                 call.c_prod_pool::get_amount_out_u64\n\
                 exec.sys::truncate_stack\n\
             end"
        );

        let script = compile_custom_tx_script(&pool_library, &source)?;

        let stack = setup
            .clients
            .client
            .execute_program(
                setup.pool.id(),
                script.clone(),
                AdviceInputs::default(),
                BTreeSet::new(),
            )
            .await?;

        let expected = expected_amount_out(reserve_in, reserve_out, amount_in);

        println!(
            "[{}/{}] reserve_in={}, reserve_out={}, amount_in={} => got={}, expected={}",
            i + 1,
            iterations,
            reserve_in.as_int(),
            reserve_out.as_int(),
            amount_in.as_int(),
            stack[0].as_int(),
            expected.as_int(),
        );
        let tx_request = TransactionRequestBuilder::new()
            .custom_script(script)
            .build()?;

        let tx_result = setup
            .clients
            .client
            .execute_transaction(setup.pool.id(), tx_request)
            .await?;

        assert_eq!(
            stack[0],
            expected,
            "Mismatch at iteration {}: reserve_in={}, reserve_out={}, amount_in={}",
            i + 1,
            reserve_in.as_int(),
            reserve_out.as_int(),
            amount_in.as_int(),
        );
    }

    println!("All {iterations} fuzz iterations passed.");
    Ok(())
}
