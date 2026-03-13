mod test_utils;

use anyhow::Result;
use miden_client::{
    Felt, Word,
    account::{AccountId, StorageSlotName},
    asset::FungibleAsset,
    crypto::FeltRng,
    note::{NoteAttachment, NoteTag, NoteType, build_p2id_recipient, create_p2id_note},
    store::{AccountRecord, AccountRecordData},
    transaction::{AdviceInputs, OutputNote, TransactionRequestBuilder},
};
use miden_protocol::crypto::rand::Randomizable;
use xyk_pool::pool_ops::{
    build_lp_local_deposit_note, build_xyk_swap_exact_tokens_for_tokens_note,
    build_xyk_swap_tokens_for_exact_tokens_note, compile_custom_tx_script,
    compile_lp_local_fuzz_tx_script, compile_storage_fuzz_tx_script, compute_expected_lp,
    compute_expected_withdraw, get_combined_pool_library, get_lp_local_library, get_math_library,
    get_pool_library, isqrt,
};
use xyk_pool::utils::{fetch_vault_for_account_from_chain, slot_name};

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
            "use zoro::xyk_pool\n\
             use miden::core::sys\n\
             begin\n\
                 push.{reserve_out}.{reserve_in}.{amount_in}\n\
                 call.xyk_pool::get_amount_out_u64\n\
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

        let expected = expected_amount_out(amount_in, reserve_in, reserve_out);

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
async fn quote_fuzz_test() -> Result<()> {
    use rand::Rng;

    let min_reserve: u64 = 1_000_000_000;
    let max_reserve: u64 = 100_000_000_000;
    let min_amount: u64 = 100_000;
    let max_amount: u64 = 100_000;
    let iterations: usize = 50;

    let mut setup = setup_lightweight_environment().await?;
    let pool_library = get_pool_library()?;
    let mut rng = rand::rng();

    for i in 0..iterations {
        let reserve_a = Felt::new(rng.random_range(min_reserve..=max_reserve));
        let reserve_b = Felt::new(rng.random_range(min_reserve..=max_reserve));
        let amount_a = Felt::new(rng.random_range(min_amount..=max_amount));

        let source = format!(
            "use zoro::xyk_pool\n\
             use miden::core::sys\n\
             begin\n\
                 push.{reserve_b}.{reserve_a}.{amount_a}\n\
                 call.xyk_pool::quote\n\
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

        let expected = expected_quote(amount_a, reserve_a, reserve_b);
        println!(
            "[{}/{}] amount_a={}, reserve_a={}, reserve_b={} => got={}, expected={}",
            i + 1,
            iterations,
            amount_a.as_int(),
            reserve_a.as_int(),
            reserve_b.as_int(),
            stack[0].as_int(),
            expected.as_int(),
        );
        assert_eq!(
            stack[0],
            expected,
            "Mismatch at iteration {}: amount_a={}, reserve_a={}, reserve_b={}",
            i + 1,
            amount_a.as_int(),
            reserve_a.as_int(),
            reserve_b.as_int(),
        );
    }

    println!("All {iterations} quote fuzz iterations passed.");
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

#[tokio::test]
async fn get_amount_in_u64_fuzz_test() -> Result<()> {
    use rand::Rng;

    let min_reserve: u64 = 1_000_000_000;
    let max_reserve: u64 = 100_000_000_000;
    let min_amount_out: u64 = 100_000;
    let max_amount_out: u64 = 100_000;
    let iterations: usize = 50;

    let mut setup = setup_lightweight_environment().await?;

    let pool_library = get_pool_library()?;
    let mut rng = rand::rng();

    for i in 0..iterations {
        let reserve_in = Felt::new(rng.random_range(min_reserve..=max_reserve));
        let reserve_out = Felt::new(rng.random_range(min_reserve..=max_reserve));
        let amount_out = Felt::new(rng.random_range(min_amount_out..=max_amount_out));

        let source = format!(
            "use zoro::xyk_pool\n\
             use miden::core::sys\n\
             begin\n\
                 push.{reserve_out}.{reserve_in}.{amount_out}\n\
                 call.xyk_pool::get_amount_in_u64\n\
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

        let expected = expected_amount_in(amount_out, reserve_in, reserve_out);

        println!(
            "[{}/{}] reserve_in={}, reserve_out={}, amount_out={} => got={}, expected={}",
            i + 1,
            iterations,
            reserve_in.as_int(),
            reserve_out.as_int(),
            amount_out.as_int(),
            stack[0].as_int(),
            expected.as_int(),
        );

        assert_eq!(
            stack[0],
            expected,
            "Mismatch at iteration {}: reserve_in={}, reserve_out={}, amount_out={}",
            i + 1,
            reserve_in.as_int(),
            reserve_out.as_int(),
            amount_out.as_int(),
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
async fn lp_local_asset_getters_test() -> Result<()> {
    let mut setup = setup_lp_local_test_environment().await?;
    let lp_local_library = get_lp_local_library()?;
    let mut rng = rand::rng();

    let pool_id = setup.contract.id();
    let token0_id = setup.faucets[0].faucet.id();
    let token1_id = setup.faucets[1].faucet.id();

    let getters_source = format!(
        "use zoro::lp_local\n\
         use miden::core::sys\n\
         begin\n\
             push.{}.{}\n\
             call.lp_local::get_asset_index\n\
             push.{}.{}\n\
             call.lp_local::get_asset_index\n\
             exec.sys::truncate_stack\n\
         end",
        token1_id.suffix().as_int(),
        token1_id.prefix().as_u64(),
        token0_id.suffix().as_int(),
        token0_id.prefix().as_u64()
    );

    let script = compile_custom_tx_script(&lp_local_library, &getters_source)?;

    let expected = vec![0, 1];
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

    let got: Vec<u64> = stack[0..2].iter().map(|x| x.as_int()).collect();

    assert_eq!(
        got, expected,
        "lp_local_asset_getters_test mismatch: got={:?}, expected={:?}",
        got, expected
    );

    Ok(())
}

#[tokio::test]
async fn lp_local_reserve_by_asset_id_test() -> Result<()> {
    use rand::Rng;

    let mut setup = setup_lp_local_test_environment().await?;
    let lp_local_library = get_lp_local_library()?;
    let mut rng = rand::rng();

    let token0_id = setup.faucets[0].faucet.id();
    let token1_id = setup.faucets[1].faucet.id();

    let iterations = 100u32;
    let min_reserve = 50000000u64;
    let max_reserve = 500000000000u64;
    let max_add = 200000u64;
    let max_sub = 200000u64;

    for i in 0..iterations {
        let start_reserves_0 = rng.random_range(min_reserve..=max_reserve);
        let start_reserves_1 = rng.random_range(min_reserve..=max_reserve);
        let add_to_token0_amount = rng.random_range(1..=max_add);
        let sub_from_token1_amount = rng.random_range(1..=max_sub.min(start_reserves_1));

        let source = format!(
            "use zoro::lp_local\n\
             use miden::core::sys\n\
             begin\n\
                 push.{start_reserves_1}.{start_reserves_0}\n\
                 call.lp_local::set_reserves\n\
                 push.{t0_suffix}.{t0_prefix}.{add_to_token0_amount}\n\
                 call.lp_local::add_to_reserve_by_asset_id\n\
                 push.{t1_suffix}.{t1_prefix}.{sub_from_token1_amount}\n\
                 call.lp_local::sub_from_reserve_by_asset_id\n\
                 push.{t1_suffix}.{t1_prefix}\n\
                 call.lp_local::get_reserve_by_asset_id\n\
                 push.{t0_suffix}.{t0_prefix}\n\
                 call.lp_local::get_reserve_by_asset_id\n\
                 exec.sys::truncate_stack\n\
             end",
            t0_prefix = token0_id.prefix().as_u64(),
            t0_suffix = token0_id.suffix().as_int(),
            t1_prefix = token1_id.prefix().as_u64(),
            t1_suffix = token1_id.suffix().as_int(),
        );

        let script = compile_custom_tx_script(&lp_local_library, &source)?;

        let stack = setup
            .clients
            .client
            .execute_program(
                setup.contract.id(),
                script,
                AdviceInputs::default(),
                BTreeSet::new(),
            )
            .await?;

        let reserve_0 = stack[0].as_int();
        let reserve_1 = stack[1].as_int();
        let expected_reserve_0 = start_reserves_0 + add_to_token0_amount;
        let expected_reserve_1 = start_reserves_1 - sub_from_token1_amount;
        println!(
            "iteration {}: start_reserves_0={}, start_reserves_1={}, add_to_token0_amount={}, sub_from_token1_amount={}, got (reserve_0={}, reserve_1={}), expected (reserve_0={}, reserve_1={})",
            i + 1,
            start_reserves_0,
            start_reserves_1,
            add_to_token0_amount,
            sub_from_token1_amount,
            reserve_0,
            reserve_1,
            expected_reserve_0,
            expected_reserve_1
        );
        assert_eq!(
            (reserve_0, reserve_1),
            (expected_reserve_0, expected_reserve_1),
            "reserve_by_asset_id: expected (reserve_0={}, reserve_1={}), got (reserve_0={}, reserve_1={})",
            expected_reserve_0,
            expected_reserve_1,
            reserve_0,
            reserve_1,
        );
    }

    Ok(())
}

#[tokio::test]
async fn lp_local_reserve_by_asset_id_unknown_asset_fails_test() -> Result<()> {
    let mut setup = setup_lp_local_test_environment().await?;
    let lp_local_library = get_lp_local_library()?;

    // Non-pool asset id (0, 0) so get_reserve_by_asset_id hits ERR_UNKNOWN_ASSET.
    let source = "use zoro::lp_local\n\
         use miden::core::sys\n\
         begin\n\
             push.100.200\n\
             call.lp_local::set_reserves\n\
             push.0.0\n\
             call.lp_local::get_reserve_by_asset_id\n\
             exec.sys::truncate_stack\n\
         end";

    let script = compile_custom_tx_script(&lp_local_library, &source)?;

    let result = setup
        .clients
        .client
        .execute_program(
            setup.contract.id(),
            script,
            AdviceInputs::default(),
            BTreeSet::new(),
        )
        .await;

    assert!(
        result.is_err(),
        "get_reserve_by_asset_id with non-pool asset id should fail with ERR_UNKNOWN_ASSET"
    );

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
    use miden_client::note::NoteTag;
    use xyk_pool::common::wait_for_note;

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
async fn lp_deposit_withdraw_happy_path_test() -> Result<()> {
    use xyk_pool::common::get_return_note_serial;
    use xyk_pool::pool_ops::{build_lp_local_withdraw_note, compute_expected_withdraw};

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

    let withdraw_note_serial_num = setup.clients.client.rng().draw_word();
    // ── Step 2: Build and submit the withdraw note ──
    // Return note params are placeholders; withdraw currently doesn't create the output note.
    let return_note_tag = NoteTag::with_account_target(setup.user.id());
    let return_note_type = NoteType::Public;

    let withdraw_note = build_lp_local_withdraw_note(
        setup.contract.id(),
        &lp_lib,
        withdraw_amount,
        setup.user.id(),
        return_note_tag.into(),
        return_note_type.into(),
        withdraw_note_serial_num,
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

    let return_note_serial_num = get_return_note_serial(withdraw_note_serial_num, setup.user.id());
    let return_note_recipient =
        build_p2id_recipient(setup.user.id(), return_note_serial_num).unwrap();
    println!("-=-=-=-=-=-=-=-=-=-=-=user_id={:?}", setup.user.id());
    println!(
        "-=-=-=-=-=-=-=-=-=-=-=return_note_recipient={:?}",
        return_note_recipient.digest()
    );
    let consume_req = TransactionRequestBuilder::new()
        .input_notes([(withdraw_note.clone(), None)])
        // .expected_output_recipients(vec![return_note_recipient])
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

    // withdraw curuser_keyrently only runs simulate_withdraw (burn/create_note commented out),
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

    let user_key = Word::new([
        Felt::new(0),
        Felt::new(0),
        setup.user.id().suffix(),
        setup.user.id().prefix().into(),
    ]);
    let user_deposit_after = storage_after_withdraw.get_map_item(
        &slot_name("zoro::lp_local::user_deposits_mapping"),
        user_key,
    )?;
    println!(
        "User deposit after withdraw: {}",
        user_deposit_after[0].as_int()
    );

    let user_deposit_before = storage_after_deposit.get_map_item(
        &slot_name("zoro::lp_local::user_deposits_mapping"),
        user_key,
    )?;
    assert_eq!(
        user_deposit_after[0].as_int(),
        user_deposit_before[0].as_int() - withdraw_amount,
        "user deposit should decrease by withdraw_amount"
    );

    println!("lp_deposit_withdraw_happy_path_test finished successfully");
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

#[tokio::test]
async fn swap_tokens_for_exact_tokens_happy_path_test() -> Result<()> {
    use xyk_pool::pool_ops::get_amount_in;
    use xyk_pool::utils::fetch_vault_for_account_from_chain;

    let deposit_amount: u64 = 10_000_000;
    let swap_amount_out: u64 = 100_000;

    let mut setup = setup_combined_pool_test_environment().await?;
    setup.maybe_fund_user_wallet(deposit_amount * 2).await?;

    let xyk_pool_lib = get_combined_pool_library()?;
    let token0_id = setup.faucets[0].faucet.id();
    let token1_id = setup.faucets[1].faucet.id();

    // ── Step 1: Deposit to seed the pool ──
    println!("\n=== DEPOSIT PHASE ===");
    let depositor = setup.user.id().clone();
    let pool_state =
        lp_local_deposit(&mut setup, deposit_amount, deposit_amount, depositor).await?;
    let (ts_d, r0_d, r1_d, pool_balance0_d, pool_balance1_d) = (
        pool_state.total_supply,
        pool_state.reserve0,
        pool_state.reserve1,
        pool_state.pool_balance0,
        pool_state.pool_balance1,
    );
    println!("After deposit:");
    println!("  total_supply  = {}", ts_d);
    println!("  reserve0      = {}", r0_d);
    println!("  reserve1      = {}", r1_d);
    println!("  pool_balance0 = {}", pool_balance0_d);
    println!("  pool_balance1 = {}", pool_balance1_d);

    // ── Read user balances before swap ──
    let user_vault_before =
        fetch_vault_for_account_from_chain(&setup.clients.rpc_api, &setup.user.id()).await?;
    let user_balance0_before = user_vault_before.get_balance(token0_id).unwrap_or(0);
    let user_balance1_before = user_vault_before.get_balance(token1_id).unwrap_or(0);
    println!("\nUser balances BEFORE swap:");
    println!("  token0 = {}", user_balance0_before);
    println!("  token1 = {}", user_balance1_before);

    // ── Step 2: Swap token0 → token1 ──
    println!("\n=== SWAP PHASE ===");
    println!("Swapping token0 for {} token1", swap_amount_out);

    let expected_in = get_amount_in(swap_amount_out, r0_d, r1_d);
    println!("Expected amount_in (Rust): {}", expected_in);

    // let return_note_tag = NoteTag::with_account_target(setup.user.id());
    let return_note_type = NoteType::Public;
    // let return_note_serial_num = Word::from_random_bytes(&[0; 32]).unwrap();
    // let return_note_recipient =
    // build_p2id_recipient(setup.user.id(), return_note_serial_num).unwrap();
    let return_note = create_p2id_note(
        setup.contract.id(),
        setup.user.id(),
        vec![
            FungibleAsset::new(token1_id.clone(), swap_amount_out)?.into(),
            FungibleAsset::new(token0_id.clone(), 10)?.into(),
        ],
        return_note_type.into(),
        NoteAttachment::default(),
        setup.clients.client.rng(),
    )?;

    let swap_max_input_asset = FungibleAsset::new(token0_id.clone(), expected_in + 10)?;
    let swap_output_asset = FungibleAsset::new(token1_id.clone(), swap_amount_out)?;
    let swap_note = build_xyk_swap_tokens_for_exact_tokens_note(
        setup.contract.id(),
        &xyk_pool_lib,
        swap_max_input_asset,
        swap_output_asset,
        0,
        setup.user.id(),
        return_note.metadata().tag().into(),
        return_note_type.into(),
        return_note.recipient().digest(),
    )?;

    let create_swap_req = TransactionRequestBuilder::new()
        .own_output_notes([OutputNote::Full(swap_note.clone())])
        .build()?;
    let _tx_id = setup
        .clients
        .client
        .submit_new_transaction(setup.user.id(), create_swap_req)
        .await?;
    setup.clients.client.sync_state().await?;

    let consume_swap_req = TransactionRequestBuilder::new()
        .input_notes([(swap_note.clone(), None)])
        .expected_output_recipients(vec![return_note.recipient().clone()])
        .expected_future_notes(vec![(
            return_note.clone().into(),
            return_note.metadata().tag().into(),
        )])
        .build()?;
    let _consume_id = setup
        .clients
        .client
        .submit_new_transaction(setup.contract.id(), consume_swap_req)
        .await?;
    setup.clients.client.sync_state().await?;
    println!("---------------------------Consumed swap note---------------------------");

    tokio::time::sleep(Duration::from_secs(1)).await;
    let user_consume_return_note_request =
        TransactionRequestBuilder::new().build_consume_notes(vec![return_note.clone()])?;
    let _user_consume_return_note_id = setup
        .clients
        .client
        .submit_new_transaction(setup.user.id(), user_consume_return_note_request)
        .await?;
    setup.clients.client.sync_state().await?;
    tokio::time::sleep(Duration::from_secs(10)).await;

    // ── Read pool state after swap ──
    let acc_after_swap = setup
        .clients
        .client
        .get_account(setup.contract.id().clone())
        .await?
        .unwrap();
    let acc_after_swap = match acc_after_swap.account_data() {
        AccountRecordData::Full(account) => account,
        AccountRecordData::Partial(_) => return Err(anyhow::anyhow!("Account not found")),
    };
    let storage_after_swap = acc_after_swap.storage();
    let total_supply_after_swap =
        storage_after_swap.get_item(&slot_name("zoro::lp_local::total_supply"))?;
    let reserve_after_swap = storage_after_swap.get_item(&slot_name("zoro::lp_local::reserve"))?;
    let vault_after_swap = acc_after_swap.vault();

    let ts_s = total_supply_after_swap[0].as_int();
    let r0_s = reserve_after_swap[1].as_int();
    let r1_s = reserve_after_swap[0].as_int();
    let pool_balance0_s = vault_after_swap.get_balance(token0_id)?;
    let pool_balance1_s = vault_after_swap.get_balance(token1_id)?;

    println!("\nAfter swap:");
    println!("  total_supply  = {} (was {})", ts_s, ts_d);
    println!("  reserve0      = {} (was {})", r0_s, r0_d);
    println!("  reserve1      = {} (was {})", r1_s, r1_d);
    println!(
        "  pool_balance0 = {} (was {})",
        pool_balance0_s, pool_balance0_d
    );
    println!(
        "  pool_balance1 = {} (was {})",
        pool_balance1_s, pool_balance1_d
    );

    // ── Read user balances after swap ──
    let user_vault_after =
        fetch_vault_for_account_from_chain(&setup.clients.rpc_api, &setup.user.id()).await?;
    let user_balance0_after = user_vault_after.get_balance(token0_id).unwrap_or(0);
    let user_balance1_after = user_vault_after.get_balance(token1_id).unwrap_or(0);
    println!("\nUser balances AFTER swap:");
    println!(
        "  token0 = {} (was {})",
        user_balance0_after, user_balance0_before
    );
    println!(
        "  token1 = {} (was {})",
        user_balance1_after, user_balance1_before
    );

    println!("\n=== SUMMARY ===");
    println!("Expected swap input:  {} token0", expected_in);
    println!("Swap amount out: {} token1", swap_amount_out);
    println!(
        "Reserve delta: r0 {} → {}, r1 {} → {}",
        r0_d, r0_s, r1_d, r1_s
    );

    assert_eq!(
        r0_d + expected_in,
        r0_s,
        "reserve0 should increase by expected_in"
    );
    assert_eq!(
        r1_d - swap_amount_out,
        r1_s,
        "reserve1 should decrease by swap_amount_out"
    );

    println!("\nswap_happy_path_test finished");
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

#[tokio::test]
async fn swap_exact_tokens_for_tokens_happy_path_test() -> Result<()> {
    use xyk_pool::pool_ops::get_amount_out;
    use xyk_pool::utils::fetch_vault_for_account_from_chain;

    let deposit_amount: u64 = 10_000_000;
    let swap_amount_in: u64 = 100_000;

    let mut setup = setup_combined_pool_test_environment().await?;
    setup.maybe_fund_user_wallet(deposit_amount * 2).await?;

    let xyk_pool_lib = get_combined_pool_library()?;
    let token0_id = setup.faucets[0].faucet.id();
    let token1_id = setup.faucets[1].faucet.id();

    // ── Step 1: Deposit to seed the pool ──
    println!("\n=== DEPOSIT PHASE ===");
    let depositor = setup.user.id().clone();
    let pool_state =
        lp_local_deposit(&mut setup, deposit_amount, deposit_amount, depositor).await?;
    let (ts_d, r0_d, r1_d, pool_balance0_d, pool_balance1_d) = (
        pool_state.total_supply,
        pool_state.reserve0,
        pool_state.reserve1,
        pool_state.pool_balance0,
        pool_state.pool_balance1,
    );
    println!("After deposit:");
    println!("  total_supply  = {}", ts_d);
    println!("  reserve0      = {}", r0_d);
    println!("  reserve1      = {}", r1_d);
    println!("  pool_balance0 = {}", pool_balance0_d);
    println!("  pool_balance1 = {}", pool_balance1_d);

    // ── Read user balances before swap ──
    let user_vault_before =
        fetch_vault_for_account_from_chain(&setup.clients.rpc_api, &setup.user.id()).await?;
    let user_balance0_before = user_vault_before.get_balance(token0_id).unwrap_or(0);
    let user_balance1_before = user_vault_before.get_balance(token1_id).unwrap_or(0);
    println!("\nUser balances BEFORE swap:");
    println!("  token0 = {}", user_balance0_before);
    println!("  token1 = {}", user_balance1_before);

    // ── Step 2: Swap token0 → token1 ──
    println!("\n=== SWAP PHASE ===");
    println!(
        "Swapping {} of token0 for token1 (min_out=0)",
        swap_amount_in
    );

    let expected_out = get_amount_out(swap_amount_in, r0_d, r1_d);
    println!("Expected amount_out (Rust): {}", expected_out);

    // let return_note_tag = NoteTag::with_account_target(setup.user.id());
    let return_note_type = NoteType::Public;
    // let return_note_serial_num = Word::from_random_bytes(&[0; 32]).unwrap();
    // let return_note_recipient =
    // build_p2id_recipient(setup.user.id(), return_note_serial_num).unwrap();
    let return_note = create_p2id_note(
        setup.contract.id(),
        setup.user.id(),
        vec![FungibleAsset::new(token1_id.clone(), expected_out)?.into()],
        return_note_type.into(),
        NoteAttachment::default(),
        setup.clients.client.rng(),
    )?;

    let swap_input_asset = FungibleAsset::new(token0_id.clone(), swap_amount_in)?;
    let swap_min_output_asset = FungibleAsset::new(token1_id.clone(), expected_out - 5)?;
    let swap_note = build_xyk_swap_exact_tokens_for_tokens_note(
        setup.contract.id(),
        &xyk_pool_lib,
        swap_input_asset,
        swap_min_output_asset,
        0,
        setup.user.id(),
        return_note.metadata().tag().into(),
        return_note_type.into(),
        return_note.recipient().digest(),
    )?;

    let create_swap_req = TransactionRequestBuilder::new()
        .own_output_notes([OutputNote::Full(swap_note.clone())])
        .build()?;
    let _tx_id = setup
        .clients
        .client
        .submit_new_transaction(setup.user.id(), create_swap_req)
        .await?;
    setup.clients.client.sync_state().await?;

    let consume_swap_req = TransactionRequestBuilder::new()
        .input_notes([(swap_note.clone(), None)])
        .expected_output_recipients(vec![return_note.recipient().clone()])
        .expected_future_notes(vec![(
            return_note.clone().into(),
            return_note.metadata().tag().into(),
        )])
        .build()?;
    let _consume_id = setup
        .clients
        .client
        .submit_new_transaction(setup.contract.id(), consume_swap_req)
        .await?;
    setup.clients.client.sync_state().await?;
    println!("---------------------------Consumed swap note---------------------------");

    tokio::time::sleep(Duration::from_secs(1)).await;
    let user_consume_return_note_request =
        TransactionRequestBuilder::new().build_consume_notes(vec![return_note.clone()])?;
    let _user_consume_return_note_id = setup
        .clients
        .client
        .submit_new_transaction(setup.user.id(), user_consume_return_note_request)
        .await?;
    setup.clients.client.sync_state().await?;
    tokio::time::sleep(Duration::from_secs(10)).await;

    // ── Read pool state after swap ──
    let acc_after_swap = setup
        .clients
        .client
        .get_account(setup.contract.id().clone())
        .await?
        .unwrap();
    let acc_after_swap = match acc_after_swap.account_data() {
        AccountRecordData::Full(account) => account,
        AccountRecordData::Partial(_) => return Err(anyhow::anyhow!("Account not found")),
    };
    let storage_after_swap = acc_after_swap.storage();
    let total_supply_after_swap =
        storage_after_swap.get_item(&slot_name("zoro::lp_local::total_supply"))?;
    let reserve_after_swap = storage_after_swap.get_item(&slot_name("zoro::lp_local::reserve"))?;
    let vault_after_swap = acc_after_swap.vault();

    let ts_s = total_supply_after_swap[0].as_int();
    let r0_s = reserve_after_swap[1].as_int();
    let r1_s = reserve_after_swap[0].as_int();
    let pool_balance0_s = vault_after_swap.get_balance(token0_id)?;
    let pool_balance1_s = vault_after_swap.get_balance(token1_id)?;

    println!("\nAfter swap:");
    println!("  total_supply  = {} (was {})", ts_s, ts_d);
    println!("  reserve0      = {} (was {})", r0_s, r0_d);
    println!("  reserve1      = {} (was {})", r1_s, r1_d);
    println!(
        "  pool_balance0 = {} (was {})",
        pool_balance0_s, pool_balance0_d
    );
    println!(
        "  pool_balance1 = {} (was {})",
        pool_balance1_s, pool_balance1_d
    );

    // ── Read user balances after swap ──
    let user_vault_after =
        fetch_vault_for_account_from_chain(&setup.clients.rpc_api, &setup.user.id()).await?;
    let user_balance0_after = user_vault_after.get_balance(token0_id).unwrap_or(0);
    let user_balance1_after = user_vault_after.get_balance(token1_id).unwrap_or(0);
    println!("\nUser balances AFTER swap:");
    println!(
        "  token0 = {} (was {})",
        user_balance0_after, user_balance0_before
    );
    println!(
        "  token1 = {} (was {})",
        user_balance1_after, user_balance1_before
    );

    println!("\n=== SUMMARY ===");
    println!("Swap input:  {} token0", swap_amount_in);
    println!("Expected out: {} token1 (Rust calc)", expected_out);
    println!(
        "Reserve delta: r0 {} → {}, r1 {} → {}",
        r0_d, r0_s, r1_d, r1_s
    );

    assert_eq!(
        r0_d + swap_amount_in,
        r0_s,
        "reserve0 should increase by swap_amount_in"
    );
    assert_eq!(
        r1_d - expected_out,
        r1_s,
        "reserve1 should decrease by expected_out"
    );

    println!("\nswap_happy_path_test finished");
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}
