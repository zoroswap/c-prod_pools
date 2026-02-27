mod test_utils;

use anyhow::Result;
use c_prod_pool::pool_ops::{
    compile_custom_tx_script, compile_storage_fuzz_tx_script, compute_expected_lp,
    get_lp_local_library, get_math_library, get_pool_library, isqrt,
};
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

    let mut setup = setup_lightweight_environment().await?;

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
                setup.account.id(),
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

        assert_eq!(
            stack[0],
            expected,
            "Mismatch at iteration {}: reserve_in={}, reserve_out={}, amount_in={}",
            i + 1,
            reserve_in.as_int(),
            reserve_out.as_int(),
            amount_in.as_int(),
        );

        // let tx_request = TransactionRequestBuilder::new()
        //     .custom_script(script)
        //     .build()?;

        // let tx_result = setup
        //     .clients
        //     .client
        //     .execute_transaction(setup.account.id(), tx_request)
        //     .await?;
    }

    println!("All {iterations} fuzz iterations passed.");
    Ok(())
}

#[tokio::test]
async fn sqrt_u32_fuzz_test() -> Result<()> {
    use rand::Rng;

    let min_n: u32 = 0;
    let max_n: u32 = u32::MAX;
    let iterations: usize = 100;

    let mut setup = setup_lightweight_environment().await?;
    let math_library = get_math_library()?;
    let mut rng = rand::rng();

    let edge_cases: Vec<u32> = vec![0, 1, 2, 3, 4, 9, 15, 16, 255, 65535, u32::MAX - 1, u32::MAX];

    for (i, n) in edge_cases
        .into_iter()
        .chain((0..iterations).map(|_| rng.random_range(min_n..=max_n)))
        .enumerate()
    {
        let source = format!(
            "use zoro::math\n\
             begin\n\
                 push.{n}\n\
                 exec.math::sqrt_u32\n\
             end"
        );

        let script = compile_custom_tx_script(&math_library, &source)?;

        let stack = setup
            .clients
            .client
            .execute_program(
                setup.account.id(),
                script.clone(),
                AdviceInputs::default(),
                BTreeSet::new(),
            )
            .await?;

        let got = stack[0].as_int();
        let expected = isqrt(n as u128) as u64;

        println!("[{}] n={} => got={}, expected={}", i + 1, n, got, expected,);

        assert_eq!(
            got,
            expected,
            "Mismatch at iteration {}: n={}, got={}, expected={}",
            i + 1,
            n,
            got,
            expected,
        );
    }

    println!("All sqrt_u32 fuzz iterations passed.");
    Ok(())
}

#[tokio::test]
async fn sqrt_felt_fuzz_test() -> Result<()> {
    use rand::Rng;

    let min_n: u64 = 0;
    let max_n: u64 = u64::MAX >> 1;
    let iterations: usize = 100;

    let mut setup = setup_lightweight_environment().await?;
    let math_library = get_math_library()?;
    let mut rng = rand::rng();

    let edge_cases: Vec<u64> = vec![
        0,
        1,
        2,
        3,
        4,
        9,
        15,
        16,
        255,
        65535,
        u32::MAX as u64 - 1,
        u32::MAX as u64,
        u32::MAX as u64 + 1,
        1_000_000_000_000,
        u64::MAX >> 1,
    ];

    for (i, n) in edge_cases
        .into_iter()
        .chain((0..iterations).map(|_| rng.random_range(min_n..=max_n)))
        .enumerate()
    {
        let source = format!(
            "use zoro::math\n\
             use miden::core::sys\n\
             begin\n\
                 push.{n}\n\
                 exec.math::sqrt\n\
                 exec.sys::truncate_stack\n\
             end"
        );

        let script = compile_custom_tx_script(&math_library, &source)?;

        // let tx_request = TransactionRequestBuilder::new()
        //     .custom_script(script.clone())
        //     .build()?;

        // let tx_result = setup
        //     .clients
        //     .client
        //     .execute_transaction(setup.account.id(), tx_request)
        //     .await?;

        let stack = setup
            .clients
            .client
            .execute_program(
                setup.account.id(),
                script.clone(),
                AdviceInputs::default(),
                BTreeSet::new(),
            )
            .await?;

        let got = stack[0].as_int();
        let expected = isqrt(n as u128) as u64;

        println!("[{}] n={} => got={}, expected={}", i + 1, n, got, expected);

        assert_eq!(
            got,
            expected,
            "Mismatch at iteration {}: n={}, got={}, expected={}",
            i + 1,
            n,
            got,
            expected,
        );
    }

    println!("All sqrt felt fuzz iterations passed.");
    Ok(())
}

#[tokio::test]
async fn get_lp_amount_out_fuzz_test() -> Result<()> {
    use rand::Rng;

    let min_reserve: u64 = 1_000;
    let max_reserve: u64 = 1_000_000_000;
    let min_amount: u64 = 1;
    let max_amount: u64 = 100_000_000;
    let iterations: usize = 100;

    let mut setup = setup_lightweight_environment().await?;
    let lp_local_library = get_lp_local_library()?;
    let mut rng = rand::rng();

    let edge_cases: Vec<(u64, u64, u64, u64, u64)> = vec![
        (0, 100, 100, 0, 0),
        (0, 1_000_000, 1_000_000, 0, 0),
        (1000, 100, 100, 1000, 1000),
        (10000, 500, 500, 50000, 50000),
    ];

    for (i, (total_supply, amount_0, amount_1, reserve_0, reserve_1)) in edge_cases
        .into_iter()
        .chain((0..iterations).map(|_| {
            let total_supply = if rng.random_range(0..2) == 0 {
                0
            } else {
                rng.random_range(min_reserve..=max_reserve)
            };
            let amount_0 = rng.random_range(min_amount..=max_amount);
            let amount_1 = rng.random_range(min_amount..=max_amount);
            let reserve_0 = if total_supply == 0 {
                0
            } else {
                rng.random_range(min_reserve..=max_reserve)
            };
            let reserve_1 = if total_supply == 0 {
                0
            } else {
                rng.random_range(min_reserve..=max_reserve)
            };
            (total_supply, amount_0, amount_1, reserve_0, reserve_1)
        }))
        .enumerate()
    {
        if total_supply > 0 && (reserve_0 == 0 || reserve_1 == 0) {
            continue;
        }

        let source = format!(
            "use zoro::lp_local\n\
             use miden::core::sys\n\
             begin\n\
                 push.{reserve_1}.{reserve_0}.{amount_1}.{amount_0}.{total_supply}\n\
                 call.lp_local::get_lp_amount_out\n\
                 exec.sys::truncate_stack\n\
             end"
        );

        let script = compile_custom_tx_script(&lp_local_library, &source)?;

        let stack = setup
            .clients
            .client
            .execute_program(
                setup.account.id(),
                script.clone(),
                AdviceInputs::default(),
                BTreeSet::new(),
            )
            .await?;

        let got = stack[0].as_int();
        let expected = compute_expected_lp(amount_0, amount_1, reserve_0, reserve_1, total_supply);

        if got == expected {
            println!(
                "[{}] ts={} a0={} a1={} r0={} r1={} => got={}, expected={}",
                i + 1,
                total_supply,
                amount_0,
                amount_1,
                reserve_0,
                reserve_1,
                got,
                expected,
            );
        };
        assert_eq!(
            got,
            expected,
            "Mismatch at iteration {}: total_supply={}, amount_0={}, amount_1={}, reserve_0={}, reserve_1={}, got={}, expected={}",
            i + 1,
            total_supply,
            amount_0,
            amount_1,
            reserve_0,
            reserve_1,
            got,
            expected,
        );
    }

    println!("All get_lp_amount_out fuzz iterations passed.");
    Ok(())
}

#[tokio::test]
async fn add_to_storage_item_fuzz_test() -> Result<()> {
    use rand::Rng;

    let iterations: usize = 5;
    let min_inc: u64 = 1;
    let max_inc: u64 = 1_000_000_000;

    let mut setup = setup_storage_fuzz_environment().await?;
    let mut rng = rand::rng();

    let edge_cases: Vec<u64> = vec![0, 1];
    let mut accumulated: u64 = 0;

    for (i, inc) in edge_cases
        .into_iter()
        .chain((0..iterations).map(|_| rng.random_range(min_inc..=max_inc)))
        .enumerate()
    {
        let source = format!(
            "use zoro::storage_fuzz_dummy\n\
             #use zoro::storage_utils\n\
             use miden::core::sys\n\

             const VALUE_SLOT = word(\"zoro::storage_fuzz_dummy::value_slot\")\n
             const MAP_SLOT = word(\"zoro::storage_fuzz_dummy::map_slot\")\n
             begin\n\
                 push.{inc}\n\
                 push.VALUE_SLOT[0..2]\n\
                 call.storage_fuzz_dummy::add_to_storage_item drop\n 
                 call.storage_fuzz_dummy::get_value\n
                 exec.sys::truncate_stack\n\
             end"
        );

        let script = compile_storage_fuzz_tx_script(&source)?;

        let stack = setup
            .clients
            .client
            .execute_program(
                setup.dummy_account.id(),
                script.clone(),
                AdviceInputs::default(),
                BTreeSet::new(),
            )
            .await?;

        let got = stack[0].as_int();
        accumulated = accumulated.saturating_add(inc);
        let expected = accumulated;

        println!(
            "[{}] inc={} => got={}, expected={}",
            i + 1,
            inc,
            got,
            expected,
        );

        assert_eq!(
            got,
            expected,
            "Mismatch at iteration {}: inc={}, got={}, expected={}",
            i + 1,
            inc,
            got,
            expected,
        );
    }

    println!("All add_to_storage_item fuzz iterations passed.");
    Ok(())
}

#[tokio::test]
async fn add_to_map_item_fuzz_test() -> Result<()> {
    use rand::Rng;

    let iterations: usize = 5;
    let min_inc: u64 = 1;
    let max_inc: u64 = 1_000_000_000;

    let mut setup = setup_storage_fuzz_environment().await?;
    let mut rng = rand::rng();

    for i in 0..iterations {
        let key_0 = rng.random_range(0u64..=u64::MAX);
        let key_1 = rng.random_range(0u64..=u64::MAX);
        let key_2 = rng.random_range(0u64..=u64::MAX);
        let key_3 = rng.random_range(0u64..=u64::MAX);
        let increment_by = rng.random_range(min_inc..=max_inc);

        let add_source = format!(
            "use zoro::storage_fuzz_dummy\n\
             use zoro::storage_utils\n\
             use miden::core::sys\n\
             begin\n\
                 push.storage_fuzz_dummy::MAP_SLOT[0..2]\n\
                 push.{key_3}.{key_2}.{key_1}.{key_0}\n\
                 push.{increment_by}\n\
                 swap.7\n\
                 call.storage_utils::add_to_map_item\n\
                 dropw\n\
                 exec.sys::truncate_stack\n\
             end"
        );

        let add_script = compile_storage_fuzz_tx_script(&add_source)?;

        setup
            .clients
            .client
            .execute_program(
                setup.dummy_account.id(),
                add_script.clone(),
                AdviceInputs::default(),
                BTreeSet::new(),
            )
            .await?;

        let get_source = format!(
            "use zoro::storage_fuzz_dummy\n\
             use miden::protocol::active_account\n\
             use miden::core::sys\n\
             begin\n\
                 push.storage_fuzz_dummy::MAP_SLOT[0..2]\n\
                 push.{key_3}.{key_2}.{key_1}.{key_0}\n\
                 exec.active_account::get_map_item\n\
                 drop drop drop\n\
                 exec.sys::truncate_stack\n\
             end"
        );

        let get_script = compile_storage_fuzz_tx_script(&get_source)?;

        let stack = setup
            .clients
            .client
            .execute_program(
                setup.dummy_account.id(),
                get_script.clone(),
                AdviceInputs::default(),
                BTreeSet::new(),
            )
            .await?;

        let got = stack[0].as_int();
        let expected = increment_by;

        println!(
            "[{}] key=({}, {}, {}, {}), inc={} => got={}, expected={}",
            i + 1,
            key_0,
            key_1,
            key_2,
            key_3,
            increment_by,
            got,
            expected,
        );

        assert_eq!(
            got,
            expected,
            "Mismatch at iteration {}: inc={}, got={}, expected={}",
            i + 1,
            increment_by,
            got,
            expected,
        );
    }

    println!("All add_to_map_item fuzz iterations passed.");
    Ok(())
}

#[tokio::test]
async fn set_map_item_fuzz_test() -> Result<()> {
    use rand::Rng;

    let iterations: usize = 5;
    let min_val: u64 = 0;
    let max_val: u64 = 1_000_000_000;

    let mut setup = setup_storage_fuzz_environment().await?;
    let mut rng = rand::rng();

    for i in 0..iterations {
        let key_0 = rng.random_range(0u64..=u64::MAX);
        let key_1 = rng.random_range(0u64..=u64::MAX);
        let key_2 = rng.random_range(0u64..=u64::MAX);
        let key_3 = rng.random_range(0u64..=u64::MAX);
        let value = rng.random_range(min_val..=max_val);

        let set_source = format!(
            "use zoro::storage_fuzz_dummy\n\
             use zoro::storage_utils\n\
             use miden::core::sys\n\
             begin\n\
                 push.storage_fuzz_dummy::MAP_SLOT[0..2]\n\
                 push.{key_3}.{key_2}.{key_1}.{key_0}\n\
                 push.{value}\n\
                 swap.7\n\
                 exec.storage_utils::set_map_item\n\
                 dropw\n\
                 exec.sys::truncate_stack\n\
             end"
        );

        let set_script = compile_storage_fuzz_tx_script(&set_source)?;

        setup
            .clients
            .client
            .execute_program(
                setup.dummy_account.id(),
                set_script.clone(),
                AdviceInputs::default(),
                BTreeSet::new(),
            )
            .await?;

        let get_source = format!(
            "use zoro::storage_fuzz_dummy\n\
             use miden::protocol::active_account\n\
             use miden::core::sys\n\
             begin\n\
                 push.storage_fuzz_dummy::MAP_SLOT[0..2]\n\
                 push.{key_3}.{key_2}.{key_1}.{key_0}\n\
                 exec.active_account::get_map_item\n\
                 drop drop drop\n\
                 exec.sys::truncate_stack\n\
             end"
        );

        let get_script = compile_storage_fuzz_tx_script(&get_source)?;

        let stack = setup
            .clients
            .client
            .execute_program(
                setup.dummy_account.id(),
                get_script.clone(),
                AdviceInputs::default(),
                BTreeSet::new(),
            )
            .await?;

        let got = stack[0].as_int();
        let expected = value;

        println!(
            "[{}] key=({}, {}, {}, {}), value={} => got={}, expected={}",
            i + 1,
            key_0,
            key_1,
            key_2,
            key_3,
            value,
            got,
            expected,
        );

        assert_eq!(
            got,
            expected,
            "Mismatch at iteration {}: value={}, got={}, expected={}",
            i + 1,
            value,
            got,
            expected,
        );
    }

    println!("All set_map_item fuzz iterations passed.");
    Ok(())
}
