// TODOs
// 1) in the algo, ensure that ATAs are already created, this saves some ixs
// 2) validate the pool update is correct
// 3) implement calculate amount out for a given amount for both pools for profit search
// 4) take volume into account when calculating profit and best size (flash loans might be an
//    option)
use raydium_amm::math::{Calculator, CheckedCeilDiv, SwapDirection, U128};
use solana_client::nonblocking::rpc_client::RpcClient;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;

use log::{error, info, warn};
use solana_entry::entry::Entry;
use solana_program::instruction::CompiledInstruction;
use solana_sdk::message::VersionedMessage;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::transaction::VersionedTransaction;
use tokio::sync::mpsc;

use crate::raydium::{self, download_raydium_json, ParsedAmmInstruction};
use raydium_library::amm::{self, openbook, AmmKeys, CalculateResult};

pub const WHIRLPOOL: &str = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc";
pub const RAYDIUM_CP: &str = "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C";
pub const RAYDIUM_AMM: &str = "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8";
pub const RAYDIUM_LIQUIDITY_POOL_V4_PUBKEY: &str =
    "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8";

#[derive(Debug, Default)]
pub struct PoolsState {
    pub raydium_cp_count: u64,
    pub raydium_amm_count: u64,
    pub orca_count: u64,
    pub orca_token_to_pool: HashMap<Pubkey, Arc<OrcaPool>>,
    // program_id to pool
    pub raydium_pools: HashMap<Pubkey, Arc<RwLock<RaydiumAmmPool>>>,
    // mint to program_id vec
    pub raydium_pools_by_mint: HashMap<Pubkey, Vec<Pubkey>>,
    pub raydium_pool_ids: Vec<Pubkey>,
}

#[derive(Debug, Default)]
pub struct OrcaPool {}

// TODO there might be sol-token or token-sol both versions, gotta be able to handle both
#[derive(Debug, Clone)]
pub struct RaydiumAmmPool {
    pub token: Pubkey,
    pub amm_keys: AmmKeys,
    pub state: CalculateResult,
}

pub fn calculate_price(state: &CalculateResult) -> f64 {
    let pc_amount = state.pool_pc_vault_amount as f64;
    let coin_amount = state.pool_coin_vault_amount as f64;

    if coin_amount == 0.0 {
        return 0.0; // Avoid division by zero
    }

    pc_amount / coin_amount
}

impl PoolsState {
    pub fn reduce_orca_tx(&mut self, _tx: VersionedTransaction) {
        // TODO: Implement Orca transaction processing
    }

    pub async fn reduce_raydium_amm_tx(
        &mut self,
        tx: Arc<VersionedTransaction>,
    ) {
        let raydium_amm_program_id = Pubkey::from_str(RAYDIUM_AMM)
            .expect("Failed to parse Raydium AMM program ID");

        for (idx, instruction) in tx.message.instructions().iter().enumerate()
        {
            let program_id = tx.message.static_account_keys()
                [instruction.program_id_index as usize];

            if program_id == raydium_amm_program_id {
                match raydium::parse_amm_instruction(&instruction.data) {
                    Ok(parsed_instruction) => {
                        self.process_raydium_instruction(
                            parsed_instruction,
                            instruction,
                            &tx.message,
                        )
                        .await;
                    }
                    Err(e) => {
                        warn!("Error parsing instruction {}: {:?}", idx, e);
                    }
                }
            }
        }
    }

    pub fn reduce_raydium_cp_tx(&mut self, _tx: VersionedTransaction) {
        panic!("Not implemented yet");
    }

    async fn process_raydium_instruction(
        &mut self,
        parsed_instruction: ParsedAmmInstruction,
        instruction: &CompiledInstruction,
        message: &VersionedMessage,
    ) {
        let amm_id_index = 1; // Amm account index
        let pool_coin_token_account_index = 5; // Pool Coin Token Account index
        let pool_pc_token_account_index = 6; // Pool Pc Token Account index

        let amm_id = message.static_account_keys()
            [instruction.accounts[amm_id_index] as usize];
        let pool_coin_vault = message.static_account_keys()
            [instruction.accounts[pool_coin_token_account_index] as usize];
        let pool_pc_vault = message.static_account_keys()
            [instruction.accounts[pool_pc_token_account_index] as usize];

        match parsed_instruction {
            ParsedAmmInstruction::SwapBaseOut(swap_instruction) => {
                println!(
                    "Swap Base Out for AMM {}: {:?}",
                    amm_id, swap_instruction
                );
                self.update_pool_state_swap(
                    &amm_id,
                    &pool_coin_vault,
                    &pool_pc_vault,
                    swap_instruction.max_amount_in,
                    swap_instruction.amount_out,
                    false,
                )
                .await;
            }
            ParsedAmmInstruction::SwapBaseIn(swap_instruction) => {
                println!(
                    "Swap Base In for AMM {}: {:?}",
                    amm_id, swap_instruction
                );
                self.update_pool_state_swap(
                    &amm_id,
                    &pool_coin_vault,
                    &pool_pc_vault,
                    swap_instruction.amount_in,
                    swap_instruction.minimum_amount_out,
                    true,
                )
                .await;
            }
            // Handle other instruction types...
            _ => {
                warn!("Unhandled instruction type: {:?}", parsed_instruction)
            }
        }
    }

    // this might work but also might not work
    // code is from raydium
    async fn update_pool_state_swap(
        &mut self,
        amm_id: &Pubkey,
        pool_coin_vault: &Pubkey,
        pool_pc_vault: &Pubkey,
        amount_specified: u64,
        other_amount_threshold: u64,
        is_swap_base_in: bool,
    ) {
        if let Some(pool) = self.raydium_pools.get(amm_id) {
            info!(
                "Initial price: {}",
                calculate_price(&pool.read().await.state)
            );
            let mut pool = pool.write().await;
            // double check here
            let is_coin_to_pc = pool.amm_keys.amm_coin_vault
                == *pool_coin_vault
                && pool.amm_keys.amm_pc_vault == *pool_pc_vault;

            let swap_direction = if is_coin_to_pc {
                SwapDirection::Coin2PC
            } else {
                SwapDirection::PC2Coin
            };

            let (pc_amount, coin_amount) = if is_swap_base_in {
                let swap_fee = U128::from(amount_specified)
                    .checked_mul(pool.state.swap_fee_numerator.into())
                    .unwrap()
                    .checked_ceil_div(pool.state.swap_fee_denominator.into())
                    .unwrap()
                    .0;
                let swap_in_after_deduct_fee = U128::from(amount_specified)
                    .checked_sub(swap_fee)
                    .unwrap();
                let swap_amount_out = Calculator::swap_token_amount_base_in(
                    swap_in_after_deduct_fee,
                    pool.state.pool_pc_vault_amount.into(),
                    pool.state.pool_coin_vault_amount.into(),
                    swap_direction,
                )
                .as_u64();

                if is_coin_to_pc {
                    (
                        pool.state
                            .pool_pc_vault_amount
                            .saturating_add(swap_amount_out),
                        pool.state
                            .pool_coin_vault_amount
                            .saturating_sub(amount_specified),
                    )
                } else {
                    (
                        pool.state
                            .pool_pc_vault_amount
                            .saturating_sub(amount_specified),
                        pool.state
                            .pool_coin_vault_amount
                            .saturating_add(swap_amount_out),
                    )
                }
            } else {
                let swap_in_before_add_fee =
                    Calculator::swap_token_amount_base_out(
                        other_amount_threshold.into(),
                        pool.state.pool_pc_vault_amount.into(),
                        pool.state.pool_coin_vault_amount.into(),
                        swap_direction,
                    );
                let swap_in_after_add_fee = swap_in_before_add_fee
                    .checked_mul(pool.state.swap_fee_denominator.into())
                    .unwrap()
                    .checked_ceil_div(
                        (pool
                            .state
                            .swap_fee_denominator
                            .checked_sub(pool.state.swap_fee_numerator)
                            .unwrap())
                        .into(),
                    )
                    .unwrap()
                    .0
                    .as_u64();

                if is_coin_to_pc {
                    (
                        pool.state
                            .pool_pc_vault_amount
                            .saturating_add(other_amount_threshold),
                        pool.state
                            .pool_coin_vault_amount
                            .saturating_sub(swap_in_after_add_fee),
                    )
                } else {
                    (
                        pool.state
                            .pool_pc_vault_amount
                            .saturating_sub(swap_in_after_add_fee),
                        pool.state
                            .pool_coin_vault_amount
                            .saturating_add(other_amount_threshold),
                    )
                }
            };

            // Update pool amounts
            pool.state.pool_pc_vault_amount = pc_amount;
            pool.state.pool_coin_vault_amount = coin_amount;

            // Update CalculateResult
            let calculate_result = CalculateResult {
                pool_pc_vault_amount: pc_amount,
                pool_coin_vault_amount: coin_amount,
                pool_lp_amount: pool.state.pool_lp_amount,
                swap_fee_numerator: pool.state.swap_fee_numerator,
                swap_fee_denominator: pool.state.swap_fee_denominator,
            };

            // Store or use the calculate_result as needed
            self.raydium_pools.get(amm_id).unwrap().write().await.state =
                calculate_result;

            info!("Updated price: {}", calculate_price(&pool.state));
        } else {
            warn!("Pool not found for AMM ID: {}", amm_id);
        }
    }

    fn _check_arbitrage_opportunity(
        &self,
        _pool: RaydiumAmmPool,
    ) -> Option<f64> {
        None
    }
}

pub async fn receive_entries(
    mut entry_receiver: mpsc::Receiver<Vec<Entry>>,
    mut error_receiver: mpsc::Receiver<String>,
) {
    let mut pools_state = PoolsState::default();
    let mints_of_interest = [
        "3S8qX1MsMqRbiwKg2cQyx7nis1oHMgaCuc9c4VfvVdPN", // mother
        "3B5wuUrMEi5yATD7on46hKfej3pfmd7t1RKgrsN3pump", // billy
        "CTg3ZgYx79zrE1MteDVkmkcGniiFrK1hJ6yiabropump", // neiro
        "GiG7Hr61RVm4CSUxJmgiCoySFQtdiwxtqf64MsRppump", // scf
        "EbZh3FDVcgnLNbh1ooatcDL1RCRhBgTKirFKNoGPpump", // gringo
        "GYKmdfcUmZVrqfcH1g579BGjuzSRijj3LBuwv79rpump", // wdog
        "8Ki8DpuWNxu9VsS3kQbarsCWMcFGWkzzA8pUPto9zBd5", // lockin
    ]
    .iter()
    .map(|p| Pubkey::from_str(p).unwrap())
    .collect::<Vec<_>>();

    // TODO use nice rpc, possibly geyser at later stage
    let rpc_client =
        RpcClient::new("https://api.mainnet-beta.solana.com".to_string());

    initialize_raydium_amm_pools(
        &rpc_client,
        &mut pools_state,
        mints_of_interest,
    )
    .await;

    println!(
        "Initialized Raydium AMM pools: {}",
        pools_state.raydium_pools.len()
    );

    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(entries) = entry_receiver.recv() => {
                    process_entries_batch(entries, &mut pools_state).await;
                }
                Some(error) = error_receiver.recv() => {
                    error!("{}", error);
                }
            }
        }
    });
}

pub async fn initialize_raydium_amm_pools(
    rpc_client: &RpcClient,
    pools_state: &mut PoolsState,
    mints_of_interest: Vec<Pubkey>,
) {
    download_raydium_json(false)
        .await
        .expect("download raydium");
    let jsonstr = std::fs::read_to_string("raydium.json")
        .expect("Failed to read raydium.json");
    let amm_keys_map = raydium::parse_raydium_json(
        Arc::new(jsonstr),
        mints_of_interest.clone(),
    )
    .expect("parse raydium json");
    let amm_program =
        Pubkey::from_str(RAYDIUM_LIQUIDITY_POOL_V4_PUBKEY).expect("pubkey");
    for mint in mints_of_interest.iter() {
        let amm_keys_vec = amm_keys_map.get(mint).unwrap(); // bound to exist
        for amm_keys in amm_keys_vec.iter() {
            let market_keys = openbook::get_keys_for_market(
                rpc_client,
                &amm_keys.market_program,
                &amm_keys.market,
            )
            .await
            .expect("get market keys");
            let state = amm::calculate_pool_vault_amounts(
                rpc_client,
                &amm_program,
                &amm_keys.amm_pool,
                amm_keys,
                &market_keys,
                amm::utils::CalculateMethod::CalculateWithLoadAccount,
            )
            .await
            .expect("calculate pool vault amounts");
            pools_state.raydium_pools.insert(
                amm_keys.amm_pool,
                Arc::new(RwLock::new(RaydiumAmmPool {
                    token: *mint,
                    amm_keys: *amm_keys,
                    state,
                })),
            );
            pools_state
                .raydium_pools_by_mint
                .entry(*mint)
                .or_default()
                .push(amm_keys.amm_pool);
        }
    }
}

pub async fn process_entries_batch(
    entries: Vec<Entry>,
    pools_state: &mut PoolsState,
) {
    info!(
        "OK: entries {} txs: {}",
        entries.len(),
        entries.iter().map(|e| e.transactions.len()).sum::<usize>(),
    );
    for entry in entries {
        for tx in entry.transactions {
            if tx.message.static_account_keys().contains(
                &Pubkey::from_str(WHIRLPOOL).expect("Failed to parse pubkey"),
            ) {
                pools_state.orca_count += 1;
                pools_state.reduce_orca_tx(tx);
            } else if tx.message.static_account_keys().contains(
                &Pubkey::from_str(RAYDIUM_CP)
                    .expect("Failed to parse pubkey"),
            ) {
                pools_state.raydium_cp_count += 1;
                // pools_state.reduce_raydium_cp_tx(tx);
            } else if tx.message.static_account_keys().contains(
                &Pubkey::from_str(RAYDIUM_AMM)
                    .expect("Failed to parse pubkey"),
            ) {
                pools_state.raydium_amm_count += 1;
                println!("Raydium AMM tx: {:?}", tx.signatures);
                pools_state.reduce_raydium_amm_tx(Arc::new(tx)).await;
            };
        }
    }
    info!(
        "orca: {}, raydium cp: {}, raydium amm: {}",
        pools_state.orca_count,
        pools_state.raydium_cp_count,
        pools_state.raydium_amm_count
    );
}
