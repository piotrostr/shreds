// TODOs
// 1) in the algo, ensure that ATAs are already created, this saves some ixs
// 2) validate the pool update is correct
// 3) implement calculate amount out for a given amount for both pools for profit search
// 4) take volume into account when calculating profit and best size (flash loans might be an
//    option)
use raydium_amm::math::{Calculator, CheckedCeilDiv, SwapDirection, U128};
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

use crate::raydium::{self, ParsedAmmInstruction};
use raydium_library::amm::{AmmKeys, CalculateResult};

#[derive(Debug, Default)]
pub struct PoolsState {
    pub raydium_cp_count: u64,
    pub raydium_amm_count: u64,
    pub orca_count: u64,
    pub orca_token_to_pool: HashMap<Pubkey, Arc<OrcaPool>>,
    pub raydium_pools: HashMap<Pubkey, Arc<RwLock<RaydiumAmmPool>>>,
}

#[derive(Debug, Default)]
pub struct OrcaPool {}

// TODO there might be sol-token or token-sol both versions, gotta be able to handle both
#[derive(Debug, Clone)]
pub struct RaydiumAmmPool {
    pub token: Pubkey,
    pub program_id: Pubkey,
    pub amm_keys: AmmKeys,
    pub state: CalculateResult,
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
                        println!(
                            "Error parsing instruction {}: {:?}",
                            idx, e
                        );
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
            _ => println!(
                "Unhandled instruction type: {:?}",
                parsed_instruction
            ),
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

pub const WHIRLPOOL: &str = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc";
pub const RAYDIUM_CP: &str = "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C";
pub const RAYDIUM_AMM: &str = "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8";

pub async fn receive_entries(
    mut entry_receiver: mpsc::Receiver<Vec<Entry>>,
    mut error_receiver: mpsc::Receiver<String>,
) {
    let mut pools_state = PoolsState::default();

    initialize_raydium_amm_pools(&mut pools_state);

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

fn initialize_raydium_amm_pools(_pools_state: &mut PoolsState) {
    // TODO: Initialize Raydium AMM pools with their details
    // This should populate the raydium_pools HashMap
    // Example:
    // let pool = RaydiumAmmPool {
    //     id: Pubkey::from_str("...").unwrap(),
    //     base_mint: Pubkey::from_str("...").unwrap(),
    //     // ... initialize other fields
    // };
    // pools_state.raydium_pools.insert(pool.id, Arc::new(RwLock::new(pool)));
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
