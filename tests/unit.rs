mod test_utils;

use anyhow::Result;
use c_prod_pool::pool_ops::{
    build_lp_local_deposit_note, compile_custom_tx_script, compile_lp_local_fuzz_tx_script,
    compile_storage_fuzz_tx_script, compute_expected_lp, compute_expected_withdraw,
    get_lp_local_library, get_math_library, get_pool_library, isqrt,
};
use c_prod_pool::utils::{fetch_vault_for_account_from_chain, slot_name};
use miden_client::{
    Felt, Word,
    account::StorageSlotName,
    asset::FungibleAsset,
    note::{NoteTag, NoteType, build_p2id_recipient},
    store::{AccountRecord, AccountRecordData},
    transaction::{AdviceInputs, OutputNote, TransactionRequestBuilder},
};
use miden_protocol::crypto::rand::Randomizable;

use std::{collections::BTreeSet, time::Duration};
use test_utils::*;

use miden_client::rpc::NodeRpcClient;

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
                setup.contract.id(),
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
        //     .execute_transaction(setup.contract.id(), tx_request)
        //     .await?;
    }

    println!("All {iterations} fuzz iterations passed.");
    tokio::time::sleep(Duration::from_secs(1)).await;
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
                setup.contract.id(),
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
    tokio::time::sleep(Duration::from_secs(1)).await;
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
        //     .execute_transaction(setup.contract.id(), tx_request)
        //     .await?;

        let stack = setup
            .clients
            .client
            .execute_program(
                setup.contract.id(),
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
    tokio::time::sleep(Duration::from_secs(1)).await;
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
                setup.contract.id(),
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
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

#[tokio::test]
async fn simulate_withdraw_fuzz_test() -> Result<()> {
    use rand::Rng;

    let min_reserve: u64 = 1_000;
    let max_reserve: u64 = 1_000_000_000;
    let min_amount: u64 = 1;
    let max_amount: u64 = 100_000_000;
    let iterations: usize = 100;

    let mut setup = setup_lightweight_environment().await?;
    let lp_local_library = get_lp_local_library()?;
    let mut rng = rand::rng();

    let edge_cases: Vec<(u64, u64, u64, u64)> = vec![
        (100, 100, 100, 0),
        (1_000_000, 1_000_000, 1_000_000, 0),
        (1000, 100, 100, 1000),
        (10000, 500, 500, 50000),
    ];

    for (i, (total_supply, lp_amount, reserve_0, reserve_1)) in edge_cases
        .into_iter()
        .chain((0..iterations).map(|_| {
            let total_supply = if rng.random_range(0..2) == 0 {
                0
            } else {
                rng.random_range(min_reserve..=max_reserve)
            };
            let lp_amount = rng.random_range(min_amount..=max_amount);
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
            (total_supply, lp_amount, reserve_0, reserve_1)
        }))
        .enumerate()
    {
        if total_supply == 0 || (total_supply > 0 && (reserve_0 == 0 || reserve_1 == 0)) {
            continue;
        }
        let expected = compute_expected_withdraw(total_supply, lp_amount, reserve_0, reserve_1);

        let source = format!(
            "use zoro::lp_local\n\
             use miden::core::sys\n\
             begin\n\
                 push.{reserve_1}.{reserve_0}.{lp_amount}.{total_supply}\n\
                 call.lp_local::simulate_withdraw\n\
                 exec.sys::truncate_stack\n\
             end"
        );

        let script = compile_custom_tx_script(&lp_local_library, &source)?;

        let stack = setup
            .clients
            .client
            .execute_program(
                setup.contract.id(),
                script.clone(),
                AdviceInputs::default(),
                BTreeSet::new(),
            )
            .await?;

        let got = (stack[0].as_int(), stack[1].as_int());

        println!(
            "[{}] ts={} lp={} ar0={} r1={} => got=({},{}), expected=({},{})",
            i + 1,
            total_supply,
            lp_amount,
            reserve_0,
            reserve_1,
            got.0,
            got.1,
            expected.0,
            expected.1,
        );

        assert_eq!(
            got,
            expected,
            "Mismatch at iteration {}: total_supply={}, lp_amount={}, reserve_0={}, reserve_1={}, got=({},{}), expected=({},{})",
            i + 1,
            total_supply,
            lp_amount,
            reserve_0,
            reserve_1,
            got.0,
            got.1,
            expected.0,
            expected.1,
        );
    }

    println!("All get_lp_amount_out fuzz iterations passed.");
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

#[tokio::test]
async fn add_to_storage_item_fuzz_test() -> Result<()> {
    use rand::Rng;

    let iterations: usize = 5;
    let min_inc: u64 = 1;
    let max_inc: u64 = 1_000_000_000;

    let initial_value = 1;
    let initial_map_value = 1;
    let mut setup = setup_storage_fuzz_environment(initial_value, initial_map_value).await?;
    let mut rng = rand::rng();

    let edge_cases: Vec<u64> = vec![0, 1];
    let mut accumulated: u64 = initial_value;

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
                 call.storage_fuzz_dummy::add_to_storage_item\n 
                 call.storage_fuzz_dummy::get_value\n
                 exec.sys::truncate_stack\n\
             end"
        );
        let script = compile_storage_fuzz_tx_script(&source)?;

        let stack = setup
            .clients
            .client
            .execute_program(
                setup.contract.id(),
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

        let tx_request = TransactionRequestBuilder::new()
            .custom_script(script.clone())
            .build()?;

        let tx_result = setup
            .clients
            .client
            .submit_new_transaction(setup.contract.id(), tx_request)
            .await?;

        setup.clients.client.sync_state().await?;

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
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

#[tokio::test]
async fn add_sub_storage_item_fuzz_test() -> Result<()> {
    use rand::Rng;

    let iterations: usize = 10;
    let min_amount: u64 = 1;
    let max_add: u64 = 1_000_000;
    let max_sub: u64 = 1_000_000;

    let initial_value = 1_000_000_000;
    let mut setup = setup_storage_fuzz_environment(initial_value.clone(), 10).await?;
    let mut rng = rand::rng();
    let mut accumulated: u64 = initial_value;

    for i in 0..iterations {
        let op_add = accumulated == 0 || rng.random_range(0..2) == 0;

        let (source, expected, amount, op_name) = if op_add {
            let amount = rng.random_range(min_amount..=max_add);
            let new_acc = accumulated.saturating_add(amount);
            (
                format!(
                    "use zoro::storage_fuzz_dummy\n\
                     use miden::core::sys\n\
                     const VALUE_SLOT = word(\"zoro::storage_fuzz_dummy::value_slot\")\n\
                     const MAP_SLOT = word(\"zoro::storage_fuzz_dummy::map_slot\")\n\
                     begin\n\
                         push.{amount}\n\
                         push.VALUE_SLOT[0..2]\n\
                         call.storage_fuzz_dummy::add_to_storage_item\n\
                         exec.sys::truncate_stack\n\
                     end"
                ),
                new_acc,
                amount,
                "add",
            )
        } else {
            let amount = rng.random_range(min_amount..=accumulated.min(max_sub));
            let new_acc = accumulated - amount;
            (
                format!(
                    "use zoro::storage_fuzz_dummy\n\
                     use miden::core::sys\n\
                     const VALUE_SLOT = word(\"zoro::storage_fuzz_dummy::value_slot\")\n\
                     const MAP_SLOT = word(\"zoro::storage_fuzz_dummy::map_slot\")\n\
                     begin\n\
                         push.{amount}\n\
                         push.VALUE_SLOT[0..2]\n\
                         call.storage_fuzz_dummy::sub_from_storage_item\n\
                         exec.sys::truncate_stack\n\
                     end"
                ),
                new_acc,
                amount,
                "sub",
            )
        };

        accumulated = expected;

        let script = compile_storage_fuzz_tx_script(&source)?;

        let stack = setup
            .clients
            .client
            .execute_program(
                setup.contract.id(),
                script.clone(),
                AdviceInputs::default(),
                BTreeSet::new(),
            )
            .await?;

        let got = stack[0].as_int();

        println!(
            "[{}/{}] {} {} => got={}, expected={}",
            i + 1,
            iterations,
            op_name,
            amount,
            got,
            expected,
        );

        let tx_request = TransactionRequestBuilder::new()
            .custom_script(script)
            .build()?;

        setup
            .clients
            .client
            .submit_new_transaction(setup.contract.id(), tx_request)
            .await?;

        setup.clients.client.sync_state().await?;

        assert_eq!(
            got,
            expected,
            "Mismatch at iteration {}: got={}, expected={}",
            i + 1,
            got,
            expected,
        );
    }

    println!(
        "All add_sub_storage_item fuzz iterations ({} add+sub) passed.",
        iterations
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

#[tokio::test]
async fn add_to_map_item_fuzz_test() -> Result<()> {
    use rand::Rng;

    let iterations: usize = 5;
    let min_inc: u64 = 1;
    let max_inc: u64 = 1_000_000_000;

    let mut setup = setup_storage_fuzz_environment(1, 1).await?;
    let mut rng = rand::rng();

    let mut accumulated: u64 = 0;

    let key_0 = 0;
    let key_1 = 0;
    let key_2 = rng.random_range(0..=u32::MAX as u64);
    let key_3 = rng.random_range(0..=u32::MAX as u64);

    for i in 0..iterations {
        let increment_by = rng.random_range(min_inc..=max_inc);

        let add_source = format!(
            "use zoro::storage_fuzz_dummy\n\
             use miden::core::sys\n\

             const VALUE_SLOT = word(\"zoro::storage_fuzz_dummy::value_slot\")\n
             const MAP_SLOT = word(\"zoro::storage_fuzz_dummy::map_slot\")\n
             begin\n\
                 push.{increment_by}\n\
                 push.{key_0}.{key_1}.{key_2}.{key_3}\n\
                 push.MAP_SLOT[0..2]\n\
                 call.storage_fuzz_dummy::add_to_map_item\n\
                 exec.sys::truncate_stack\n\
             end"
        );

        let add_script = compile_storage_fuzz_tx_script(&add_source)?;

        let stack = setup
            .clients
            .client
            .execute_program(
                setup.contract.id(),
                add_script.clone(),
                AdviceInputs::default(),
                BTreeSet::new(),
            )
            .await?;

        let got = stack[0].as_int();
        accumulated = accumulated.saturating_add(increment_by);
        let expected = accumulated;

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

        let tx_request = TransactionRequestBuilder::new()
            .custom_script(add_script.clone())
            .build()?;

        let tx_id = setup
            .clients
            .client
            .submit_new_transaction(setup.contract.id(), tx_request)
            .await?;

        setup.clients.client.sync_state().await?;

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
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

#[tokio::test]
async fn sub_from_map_item_fuzz_test() -> Result<()> {
    use rand::Rng;

    let iterations: usize = 5;
    let min_inc: u64 = 1;
    let max_inc: u64 = 1_000_000;

    let initial_map_value = 1_000_000_000;
    let mut setup = setup_storage_fuzz_environment(1, initial_map_value.clone()).await?;
    let mut rng = rand::rng();

    let mut accumulated: u64 = initial_map_value;

    let key_0 = 0;
    let key_1 = 0;
    let key_2 = 0;
    let key_3 = 0;

    for i in 0..iterations {
        let sub_by = rng.random_range(min_inc..=max_inc);

        let sub_source = format!(
            "use zoro::storage_fuzz_dummy\n\
             use miden::core::sys\n\

             const VALUE_SLOT = word(\"zoro::storage_fuzz_dummy::value_slot\")\n
             const MAP_SLOT = word(\"zoro::storage_fuzz_dummy::map_slot\")\n
             begin\n\
                 push.{sub_by}\n\
                 push.{key_0}.{key_1}.{key_2}.{key_3}\n\
                 push.MAP_SLOT[0..2]\n\
                 call.storage_fuzz_dummy::sub_from_map_item\n\
                 exec.sys::truncate_stack\n\
             end"
        );

        let sub_script = compile_storage_fuzz_tx_script(&sub_source)?;

        let stack = setup
            .clients
            .client
            .execute_program(
                setup.contract.id(),
                sub_script.clone(),
                AdviceInputs::default(),
                BTreeSet::new(),
            )
            .await?;

        let got = stack[0].as_int();
        accumulated = accumulated.saturating_sub(sub_by);
        let expected = accumulated;

        println!(
            "[{}] key=({}, {}, {}, {}), sub_by={} => got={}, expected={}",
            i + 1,
            key_0,
            key_1,
            key_2,
            key_3,
            sub_by,
            got,
            expected,
        );

        let tx_request = TransactionRequestBuilder::new()
            .custom_script(sub_script.clone())
            .build()?;

        let tx_id = setup
            .clients
            .client
            .submit_new_transaction(setup.contract.id(), tx_request)
            .await?;

        setup.clients.client.sync_state().await?;

        assert_eq!(
            got,
            expected,
            "Mismatch at iteration {}: sub_by={}, got={}, expected={}",
            i + 1,
            sub_by,
            got,
            expected,
        );
    }

    println!("All add_to_map_item fuzz iterations passed.");
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

#[tokio::test]
async fn sub_from_storage_item_test() -> Result<()> {
    let initial_value = 100;
    let mut setup = setup_storage_fuzz_environment(initial_value.clone(), 10).await?;

    let sub_by = 30;
    let source = format!(
        "use zoro::storage_fuzz_dummy\n\
         use miden::core::sys\n\

         const VALUE_SLOT = word(\"zoro::storage_fuzz_dummy::value_slot\")\n
         const MAP_SLOT = word(\"zoro::storage_fuzz_dummy::map_slot\")\n
         begin\n\
             push.{sub_by}\n\
             push.VALUE_SLOT[0..2]\n\
             call.storage_fuzz_dummy::sub_from_storage_item\n\
             call.storage_fuzz_dummy::get_value\n\
             exec.sys::truncate_stack\n\
         end"
    );

    let sub_script = compile_storage_fuzz_tx_script(&source)?;

    let stack = setup
        .clients
        .client
        .execute_program(
            setup.contract.id(),
            sub_script.clone(),
            AdviceInputs::default(),
            BTreeSet::new(),
        )
        .await?;

    let got = stack[0].as_int();

    let expected = initial_value - sub_by; // 100 - 30

    println!(
        "sub_from_storage_item: add 100, sub 30 => got={}, expected={}",
        got, expected
    );

    assert_eq!(
        got, expected,
        "sub_from_storage_item mismatch: got={}, expected={}",
        got, expected,
    );

    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

#[tokio::test]
async fn sub_from_storage_item_underflow_test() -> Result<()> {
    let initial_value = 100;
    let mut setup = setup_storage_fuzz_environment(initial_value.clone(), 10).await?;

    let sub_by = 3000;
    let source = format!(
        "use zoro::storage_fuzz_dummy\n\
         use miden::core::sys\n\

         const VALUE_SLOT = word(\"zoro::storage_fuzz_dummy::value_slot\")\n
         const MAP_SLOT = word(\"zoro::storage_fuzz_dummy::map_slot\")\n
         begin\n\
             push.{sub_by}\n\
             push.VALUE_SLOT[0..2]\n\
             call.storage_fuzz_dummy::sub_from_storage_item\n\
             exec.sys::truncate_stack\n\
         end"
    );

    let sub_fail_script = compile_storage_fuzz_tx_script(&source)?;

    let result = setup
        .clients
        .client
        .execute_program(
            setup.contract.id(),
            sub_fail_script,
            AdviceInputs::default(),
            BTreeSet::new(),
        )
        .await;

    assert!(
        result.is_err(),
        "sub_from_storage_item should fail with underflow when subtracting 3000 from 100, got Ok"
    );

    println!(
        "sub_from_storage_item underflow: 50 - 100 correctly failed with: {:?}",
        result.unwrap_err()
    );

    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

#[tokio::test]
async fn lp_mint_fuzz_test() -> Result<()> {
    use rand::Rng;

    let iterations: usize = 10;
    let min_amount: u64 = 1;
    let max_amount: u64 = 1_000_000;

    let mut setup = setup_lp_local_fuzz_environment().await?;
    let mut rng = rand::rng();

    let prefix = setup.contract.id().prefix().as_felt();
    let suffix = setup.contract.id().suffix();

    let mut expected_total_supply: u64 = 0;
    let mut expected_user_balance: u64 = 0;

    for i in 0..iterations {
        let amount = rng.random_range(min_amount..=max_amount);

        let source = format!(
            "use zoro::lp_local\n\
             use miden::core::sys\n\
             begin\n\
                 push.{suffix}.{prefix}.{amount}\n\
                 call.lp_local::mint\n\
                 push.{suffix}.{prefix}\n\
                 call.lp_local::get_user_deposit\n\
                 call.lp_local::total_supply\n\
                 exec.sys::truncate_stack\n\
             end"
        );

        let script = compile_lp_local_fuzz_tx_script(&source)?;

        let stack = setup
            .clients
            .client
            .execute_program(
                setup.contract.id(),
                script.clone(),
                AdviceInputs::default(),
                BTreeSet::new(),
            )
            .await?;

        expected_total_supply = expected_total_supply.saturating_add(amount);
        expected_user_balance = expected_user_balance.saturating_add(amount);

        let total_supply = stack[0].as_int();
        let user_deposit = stack[1].as_int();

        println!(
            "[{}/{}] mint amount={} => total_supply={}, user_deposit={} (expected {} {})",
            i + 1,
            iterations,
            amount,
            total_supply,
            user_deposit,
            expected_total_supply,
            expected_user_balance,
        );

        let tx_request = TransactionRequestBuilder::new()
            .custom_script(script)
            .build()?;
        setup
            .clients
            .client
            .submit_new_transaction(setup.contract.id(), tx_request)
            .await?;
        setup.clients.client.sync_state().await?;

        assert_eq!(
            total_supply,
            expected_total_supply as u64,
            "total_supply mismatch at iteration {}",
            i + 1
        );
        assert_eq!(
            user_deposit,
            expected_user_balance as u64,
            "user_deposit mismatch at iteration {}",
            i + 1
        );
    }

    println!("All lp_mint fuzz iterations passed.");
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

#[tokio::test]
async fn lp_burn_fuzz_test() -> Result<()> {
    use rand::Rng;

    let iterations: usize = 10;
    let min_burn: u64 = 1;
    let max_burn: u64 = 100_000;
    let initial_mint: u64 = 1_000_000_000;

    let mut setup = setup_lp_local_fuzz_environment().await?;
    let mut rng = rand::rng();

    let prefix = setup.contract.id().prefix().as_felt();
    let suffix = setup.contract.id().suffix();

    // Initial mint
    let mint_source = format!(
        "use zoro::lp_local\n\
         use miden::core::sys\n\
         begin\n\
             push.{suffix}.{prefix}.{initial_mint}\n\
             call.lp_local::mint\n\
             exec.sys::truncate_stack\n\
         end"
    );
    let mint_script = compile_lp_local_fuzz_tx_script(&mint_source)?;
    let tx_request = TransactionRequestBuilder::new()
        .custom_script(mint_script)
        .build()?;
    setup
        .clients
        .client
        .submit_new_transaction(setup.contract.id(), tx_request)
        .await?;
    setup.clients.client.sync_state().await?;

    let acc_after = setup
        .clients
        .client
        .get_account(setup.contract.id().clone())
        .await?
        .unwrap();
    let acc_after = match acc_after.account_data() {
        AccountRecordData::Full(account) => account,
        AccountRecordData::Partial(_) => return Err(anyhow::anyhow!("Account not found")),
    };

    let acc_after_storage = acc_after.storage();
    let usr_key = Word::new([Felt::new(0), Felt::new(0), suffix, prefix]);
    let usr_depo = acc_after_storage
        .get_map_item(&slot_name("zoro::lp_local::user_deposits_mapping"), usr_key)?;
    println!("usr_depo: after mint {:?}", usr_depo);

    let mut expected_total_supply: u64 = initial_mint;
    let mut expected_user_balance: u64 = initial_mint;

    for i in 0..iterations {
        if expected_user_balance == 0 {
            break;
        }
        let burn_amount = rng.random_range(min_burn..=max_burn.min(expected_user_balance));
        if burn_amount == 0 {
            continue;
        }

        let source = format!(
            "use zoro::lp_local\n\
             use miden::core::sys\n\
             begin\n\
                 push.{suffix}.{prefix}.{burn_amount}\n\
                 call.lp_local::burn\n\
                 push.{suffix}.{prefix}\n\
                 call.lp_local::get_user_deposit\n\
                 call.lp_local::total_supply\n\
                 exec.sys::truncate_stack\n\
             end"
        );

        let script = compile_lp_local_fuzz_tx_script(&source)?;

        let stack = setup
            .clients
            .client
            .execute_program(
                setup.contract.id(),
                script.clone(),
                AdviceInputs::default(),
                BTreeSet::new(),
            )
            .await?;

        expected_total_supply = expected_total_supply.saturating_sub(burn_amount);
        expected_user_balance = expected_user_balance.saturating_sub(burn_amount);

        let total_supply = stack[0].as_int();
        let user_deposit = stack[1].as_int();

        println!(
            "[{}/{}] burn amount={} => total_supply={}, user_deposit={} (expected {} {})",
            i + 1,
            iterations,
            burn_amount,
            total_supply,
            user_deposit,
            expected_total_supply,
            expected_user_balance,
        );

        let tx_request = TransactionRequestBuilder::new()
            .custom_script(script)
            .build()?;
        setup
            .clients
            .client
            .submit_new_transaction(setup.contract.id(), tx_request)
            .await?;
        setup.clients.client.sync_state().await?;

        assert_eq!(
            total_supply,
            expected_total_supply as u64,
            "total_supply mismatch at iteration {}",
            i + 1
        );
        assert_eq!(
            user_deposit,
            expected_user_balance as u64,
            "user_deposit mismatch at iteration {}",
            i + 1
        );
    }

    println!("All lp_burn fuzz iterations passed.");
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

#[tokio::test]
async fn deposit_happy_path_test() -> Result<()> {
    use miden_client::note::NoteTag;

    let mut setup = setup_lp_local_test_environment().await?;
    // setup.maybe_fund_user_wallet(100_000_000_000).await?;

    let lp_lib = get_lp_local_library()?;
    let token0_id = setup.faucets[0].faucet.id();
    let token1_id = setup.faucets[1].faucet.id();
    let amount0 = 1000_000u64;
    let amount1 = 1000_000u64;
    let token0_asset = FungibleAsset::new(token0_id.clone(), amount0)?;
    let token1_asset = FungibleAsset::new(token1_id.clone(), amount1)?;

    let deposit_note = build_lp_local_deposit_note(
        setup.contract.id(),
        &lp_lib,
        token0_asset,
        token1_asset,
        setup.user.id(),
        setup.user.id(),
    )?;

    let pool_tag = NoteTag::with_account_target(setup.contract.id());
    setup.clients.client.add_note_tag(pool_tag).await?;

    let create_req = TransactionRequestBuilder::new()
        .own_output_notes([OutputNote::Full(deposit_note.clone())])
        .build()?;
    let _tx_id = setup
        .clients
        .client
        .submit_new_transaction(setup.user.id(), create_req)
        .await?;
    setup.clients.client.sync_state().await?;

    // wait_for_note(&mut setup.clients.client, &deposit_note).await?;
    let consume_req = TransactionRequestBuilder::new()
        // .input_notes([(deposit_note.clone(), None), (deposit_note_2.clone(), None)])
        .input_notes([(deposit_note.clone(), None)])
        .build()?;

    let _consume_id = setup
        .clients
        .client
        .submit_new_transaction(setup.contract.id(), consume_req)
        .await?;
    setup.clients.client.sync_state().await?;

    let deposit_note_2 = build_lp_local_deposit_note(
        setup.contract.id(),
        &lp_lib,
        token0_asset,
        token1_asset,
        setup.user.id(),
        setup.user.id(),
    )?;

    let create_req_2 = TransactionRequestBuilder::new()
        .own_output_notes([OutputNote::Full(deposit_note_2.clone())])
        .build()?;
    let _tx_id = setup
        .clients
        .client
        .submit_new_transaction(setup.user.id(), create_req_2)
        .await?;
    setup.clients.client.sync_state().await?;

    // wait_for_note(&mut setup.clients.client, &deposit_note).await?;
    let consume_req_2 = TransactionRequestBuilder::new()
        // .input_notes([(deposit_note.clone(), None), (deposit_note_2.clone(), None)])
        .input_notes([(deposit_note_2.clone(), None)])
        .build()?;

    let _consume_id_2 = setup
        .clients
        .client
        .submit_new_transaction(setup.contract.id(), consume_req_2)
        .await?;
    setup.clients.client.sync_state().await?;

    // read the storage items: total supply, reserve0, reserve1, user_deposits_mapping value for the user
    let acc_after = setup
        .clients
        .client
        .get_account(setup.contract.id().clone())
        .await?
        .unwrap();
    let acc_after = match acc_after.account_data() {
        AccountRecordData::Full(account) => account,
        AccountRecordData::Partial(_) => return Err(anyhow::anyhow!("Account not found")),
    };

    let acc_after_storage = acc_after.storage();
    let total_supply = acc_after_storage.get_item(&slot_name("zoro::lp_local::total_supply"))?;
    let reserve = acc_after_storage.get_item(&slot_name("zoro::lp_local::reserve"))?;
    let user_key = Word::new([
        Felt::new(0),
        Felt::new(0),
        setup.user.id().suffix(),
        setup.user.id().prefix().into(),
    ]);
    let user_deposit_balance = acc_after_storage.get_map_item(
        &slot_name("zoro::lp_local::user_deposits_mapping"),
        user_key,
    )?;

    println!(
        "total_supply={}\nreserve0={}\nreserve1={}\nuser_deposit_balance={}\n",
        total_supply[0].as_int(),
        reserve[0].as_int(),
        reserve[1].as_int(),
        user_deposit_balance[0].as_int(),
    );

    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

#[tokio::test]
async fn deposit_initial_underflow_test() -> Result<()> {
    use c_prod_pool::common::wait_for_note;
    use miden_client::note::NoteTag;

    let mut setup = setup_lp_local_test_environment().await?;
    setup.maybe_fund_user_wallet(1_000).await?;

    let lp_lib = get_lp_local_library()?;
    let token0_id = setup.faucets[0].faucet.id();
    let token1_id = setup.faucets[1].faucet.id();
    let amount0 = 10u64;
    let amount1 = 10u64;
    let token0_asset = FungibleAsset::new(token0_id.clone(), amount0)?;
    let token1_asset = FungibleAsset::new(token1_id.clone(), amount1)?;

    let deposit_note = build_lp_local_deposit_note(
        setup.contract.id(),
        &lp_lib,
        token0_asset,
        token1_asset,
        setup.user.id(),
        setup.user.id(),
    )?;

    let pool_tag = NoteTag::with_account_target(setup.contract.id());
    setup.clients.client.add_note_tag(pool_tag).await?;

    let create_req = TransactionRequestBuilder::new()
        .own_output_notes([OutputNote::Full(deposit_note.clone())])
        .build()?;
    let _tx_id = setup
        .clients
        .client
        .submit_new_transaction(setup.user.id(), create_req)
        .await?;
    setup.clients.client.sync_state().await?;

    let consume_req = TransactionRequestBuilder::new()
        .input_notes([(deposit_note, None)])
        .build()?;
    let result = setup
        .clients
        .client
        .submit_new_transaction(setup.contract.id(), consume_req)
        .await;

    assert!(
        result.is_err(),
        "deposit with amount0=10 amount1=10 should fail (sqrt(100)-100 underflows)"
    );
    println!(
        "deposit_initial_underflow_test: correctly failed with {:?}",
        result.unwrap_err()
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

#[tokio::test]
async fn lp_withdraw_happy_path_test() -> Result<()> {
    use c_prod_pool::pool_ops::{build_lp_local_withdraw_note, compute_expected_withdraw};

    let deposit_amount: u64 = 10_000_000;
    let withdraw_amount: u64 = 1_000_000;

    let mut setup = setup_lp_local_test_environment().await?;
    setup.maybe_fund_user_wallet(deposit_amount).await?;

    let lp_lib = get_lp_local_library()?;
    let token0_id = setup.faucets[0].faucet.id();
    let token1_id = setup.faucets[1].faucet.id();
    let token0_asset = FungibleAsset::new(token0_id.clone(), deposit_amount)?;
    let token1_asset = FungibleAsset::new(token1_id.clone(), deposit_amount)?;

    // ── Step 1: Deposit to seed the pool with reserves and LP supply ──
    let deposit_note = build_lp_local_deposit_note(
        setup.contract.id(),
        &lp_lib,
        token0_asset,
        token1_asset,
        setup.user.id(),
        setup.user.id(),
    )?;

    let pool_tag = NoteTag::with_account_target(setup.contract.id());
    setup.clients.client.add_note_tag(pool_tag).await?;

    let create_req = TransactionRequestBuilder::new()
        .own_output_notes([OutputNote::Full(deposit_note.clone())])
        .build()?;
    let _tx_id = setup
        .clients
        .client
        .submit_new_transaction(setup.user.id(), create_req)
        .await?;
    setup.clients.client.sync_state().await?;

    let consume_req = TransactionRequestBuilder::new()
        .input_notes([(deposit_note.clone(), None)])
        .build()?;
    let _consume_id = setup
        .clients
        .client
        .submit_new_transaction(setup.contract.id(), consume_req)
        .await?;
    setup.clients.client.sync_state().await?;

    // ── Read storage after deposit ──
    let acc_after_deposit = setup
        .clients
        .client
        .get_account(setup.contract.id().clone())
        .await?
        .unwrap();
    let acc_after_deposit = match acc_after_deposit.account_data() {
        AccountRecordData::Full(account) => account,
        AccountRecordData::Partial(_) => return Err(anyhow::anyhow!("Account not found")),
    };
    let storage_after_deposit = acc_after_deposit.storage();
    let total_supply_after_deposit =
        storage_after_deposit.get_item(&slot_name("zoro::lp_local::total_supply"))?;
    let reserve_after_deposit =
        storage_after_deposit.get_item(&slot_name("zoro::lp_local::reserve"))?;
    let vault_after_deposit = acc_after_deposit.vault();
    println!(
        "after deposit token0 balance={}, token1 balance={}",
        vault_after_deposit.get_balance(token0_id)?,
        vault_after_deposit.get_balance(token1_id)?
    );

    let ts = total_supply_after_deposit[0].as_int();
    let r0 = reserve_after_deposit[0].as_int();
    let r1 = reserve_after_deposit[1].as_int();
    println!(
        "After deposit: total_supply={}, reserve0={}, reserve1={}",
        ts, r0, r1
    );
    assert!(ts > 0, "total supply should be > 0 after deposit");
    assert!(r0 > 0, "reserve0 should be > 0 after deposit");
    assert!(r1 > 0, "reserve1 should be > 0 after deposit");

    // ── Step 2: Build and submit the withdraw note ──
    // Return note params are placeholders; withdraw currently doesn't create the output note.
    let return_note_tag = NoteTag::with_account_target(setup.user.id());
    let return_note_type = NoteType::Public;
    let return_note_serial_num = Word::from_random_bytes(&[0; 32]).unwrap();
    let return_note_recipient =
        build_p2id_recipient(setup.user.id(), return_note_serial_num).unwrap();
    let withdraw_note = build_lp_local_withdraw_note(
        setup.contract.id(),
        &lp_lib,
        withdraw_amount,
        setup.user.id(),
        return_note_tag.into(),
        return_note_type.into(),
        return_note_recipient.digest(),
    )?;

    let create_req = TransactionRequestBuilder::new()
        .own_output_notes([OutputNote::Full(withdraw_note.clone())])
        .build()?;
    let _tx_id = setup
        .clients
        .client
        .submit_new_transaction(setup.user.id(), create_req)
        .await?;
    setup.clients.client.sync_state().await?;

    let consume_req = TransactionRequestBuilder::new()
        .input_notes([(withdraw_note.clone(), None)])
        .expected_output_recipients(vec![return_note_recipient])
        .build()?;
    let _consume_id = setup
        .clients
        .client
        .submit_new_transaction(setup.contract.id(), consume_req)
        .await?;
    setup.clients.client.sync_state().await?;

    // ── Read storage after withdraw ──
    let acc_after_withdraw = setup
        .clients
        .client
        .get_account(setup.contract.id().clone())
        .await?
        .unwrap();
    let acc_after_withdraw = match acc_after_withdraw.account_data() {
        AccountRecordData::Full(account) => account,
        AccountRecordData::Partial(_) => return Err(anyhow::anyhow!("Account not found")),
    };
    let storage_after_withdraw = acc_after_withdraw.storage();
    let total_supply_after_withdraw =
        storage_after_withdraw.get_item(&slot_name("zoro::lp_local::total_supply"))?;
    let reserve_after_withdraw =
        storage_after_withdraw.get_item(&slot_name("zoro::lp_local::reserve"))?;

    let ts_after = total_supply_after_withdraw[0].as_int();
    let r0_after = reserve_after_withdraw[0].as_int();
    let r1_after = reserve_after_withdraw[1].as_int();

    let (expected_amount0_out, expected_amount1_out) =
        compute_expected_withdraw(ts, withdraw_amount, r0, r1);

    println!(
        "After withdraw: total_supply={}, reserve0={}, reserve1={}",
        ts_after, r0_after, r1_after
    );
    println!(
        "Expected withdraw outputs: amount0={}, amount1={}",
        expected_amount0_out, expected_amount1_out
    );

    // withdraw currently only runs simulate_withdraw (burn/create_note commented out),
    // so state should remain unchanged
    assert_eq!(
        ts_after,
        ts - withdraw_amount,
        "total supply should decrease by withdraw_amount"
    );
    assert_eq!(
        r0_after,
        r0 - expected_amount0_out,
        "reserve0 should decrease by expected_amount0_out"
    );
    assert_eq!(
        r1_after,
        r1 - expected_amount1_out,
        "reserve1 should decrease by expected_amount1_out"
    );

    let user_key_after = Word::new([
        Felt::new(0),
        Felt::new(0),
        setup.user.id().suffix(),
        setup.user.id().prefix().into(),
    ]);
    let user_deposit_after = storage_after_withdraw.get_map_item(
        &slot_name("zoro::lp_local::user_deposits_mapping"),
        user_key_after,
    )?;
    println!(
        "User deposit after withdraw: {}",
        user_deposit_after[0].as_int()
    );

    let user_key_before = Word::new([
        Felt::new(0),
        Felt::new(0),
        setup.user.id().suffix(),
        setup.user.id().prefix().into(),
    ]);
    let user_deposit_before = storage_after_deposit.get_map_item(
        &slot_name("zoro::lp_local::user_deposits_mapping"),
        user_key_before,
    )?;
    assert_eq!(
        user_deposit_after[0].as_int(),
        user_deposit_before[0].as_int() - withdraw_amount,
        "user deposit should decrease by withdraw_amount"
    );

    println!("lp_withdraw_happy_path_test finished successfully");
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}
