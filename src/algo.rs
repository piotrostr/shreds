use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;

use log::{error, info};
use solana_entry::entry::Entry;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::transaction::VersionedTransaction;
use tokio::sync::mpsc;

use crate::raydium::{self, ParsedAmmInstruction};

// this has to contain the pool information for a given token
// i want to be able to add those tokens manually for starters,
// like PubkeySomethingasdfasdf...pump, etc etc; those are probably going to be added with their
// corresponding pubkeys, the amm program pubkey, all of the required accounts for the swap tx
// then it tracks the pool state and looks for arbitrage opps every time there is a new transaction
// that updates the pool as a callback
// as soon as there is profit to be made, send the transaction and set profitable in/out token for
// each of the pools
// easy A->B B->A arbitrage, orca and raydium only
// 1. at start, fetch the pool state at starting slot
// 2. for every new slot, include the newly received transactions
// 3. after each batch, check if there is an arbitrage opportunity
// 4. if there is, send the transaction superfast, probably tipping etc gonna be crucial
// no on-chain program for swapping just yet, just ensure that transaction is profitable by
// calculating the final amount out and adding the fees
#[derive(Debug, Default)]
pub struct PoolsState {
    pub raydium_cp_count: u64, // TODO add this later, test with AMM first
    pub raydium_amm_count: u64,
    pub orca_count: u64,
    pub orca_token_to_pool: HashMap<Pubkey, Arc<OrcaPool>>,
    pub raydium_amm_token_to_pool:
        HashMap<Pubkey, Arc<RwLock<RaydiumAmmPool>>>,
}

/// those pools contain all of the required pubkeys for making the transactions
/// those are loaded during startup
#[derive(Debug, Default)]
pub struct OrcaPool {}

/// Raydium AMM pool information
#[derive(Debug, Clone)]
pub struct RaydiumAmmPool {
    pub id: Pubkey,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub lp_mint: Pubkey,
    pub program_id: Pubkey,
    pub authority: Pubkey,
    pub open_orders: Pubkey,
    pub target_orders: Pubkey,
    pub base_vault: Pubkey,
    pub quote_vault: Pubkey,
    pub withdraw_queue: Pubkey,
    pub lp_vault: Pubkey,
    pub market_program_id: Pubkey,
    pub market_id: Pubkey,
    pub market_authority: Pubkey,
    pub market_base_vault: Pubkey,
    pub market_quote_vault: Pubkey,
    pub market_bids: Pubkey,
    pub market_asks: Pubkey,
    pub market_event_queue: Pubkey,
    pub base_balance: u64,
    pub quote_balance: u64,
}

impl PoolsState {
    pub fn reduce_orca_tx(&mut self, _tx: VersionedTransaction) {
        // TODO
    }

    pub async fn reduce_raydium_amm_tx(
        &mut self,
        tx: Arc<VersionedTransaction>,
    ) {
        // Extract the program ID and instruction data from the transaction
        let program_id = tx.message.static_account_keys()[0];
        let instruction = &tx.message.instructions()[0];
        let instruction_data = instruction.data.as_slice();

        // Parse the instruction
        match raydium::parse_amm_instruction(instruction_data) {
            Ok(parsed_instruction) => {
                match parsed_instruction {
                    ParsedAmmInstruction::SwapBaseIn(swap_instruction) => {
                        println!(
                            "SwapBaseIn: amount_in={}, minimum_amount_out={}",
                            swap_instruction.amount_in,
                            swap_instruction.minimum_amount_out
                        );
                        // Update pool state based on swap
                    }
                    ParsedAmmInstruction::SwapBaseOut(swap_instruction) => {
                        println!(
                            "SwapBaseOut: max_amount_in={}, amount_out={}",
                            swap_instruction.max_amount_in,
                            swap_instruction.amount_out
                        );
                        // Update pool state based on swap
                    }
                    // Handle other instruction types...
                    _ => println!(
                        "Unhandled instruction type: {:?}",
                        parsed_instruction
                    ),
                }

                // Check for arbitrage opportunity
                if let Some(pool) =
                    self.raydium_amm_token_to_pool.get(&program_id)
                {
                    let pool = pool.read().await;
                    if let Some(profit) =
                        self.check_arbitrage_opportunity(pool.clone())
                    {
                        info!(
                            "Arbitrage opportunity found with profit: {}",
                            profit
                        );
                        // TODO: Implement arbitrage execution
                    }
                }
            }
            Err(e) => {
                println!("Error parsing instruction: {:?}", e);
            }
        }
    }

    pub fn reduce_raydium_cp_tx(&mut self, _tx: VersionedTransaction) {
        panic!("Not implemented yet");
    }

    fn check_arbitrage_opportunity(
        &self,
        _pool: RaydiumAmmPool,
    ) -> Option<f64> {
        // TODO: Implement arbitrage checking logic
        // This should calculate if there's a profitable arbitrage opportunity
        // and return the potential profit if there is one
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

    // Initialize Raydium AMM pools
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
    // This should populate the raydium_amm_token_to_pool HashMap
    // with the pool details similar to the TypeScript example
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
        // this counter is ineffective, only for testing purposes
        for tx in entry.transactions {
            if tx.message.static_account_keys().contains(
                &Pubkey::from_str(WHIRLPOOL).expect("Failed to parse pubkey"),
            ) {
                info!("OK: Found whirlpool tx {:?}", tx.signatures);
                pools_state.orca_count += 1;
                pools_state.reduce_orca_tx(tx);
            } else if tx.message.static_account_keys().contains(
                &Pubkey::from_str(RAYDIUM_CP)
                    .expect("Failed to parse pubkey"),
            ) {
                info!("OK: Found Raydium CP tx {:?}", tx.signatures);
                pools_state.raydium_cp_count += 1;
                pools_state.reduce_raydium_cp_tx(tx);
            } else if tx.message.static_account_keys().contains(
                &Pubkey::from_str(RAYDIUM_AMM)
                    .expect("Failed to parse pubkey"),
            ) {
                info!("OK: Found Raydium AMM tx {:?}", tx.signatures);
                pools_state.raydium_amm_count += 1;
                println!("{:?}", tx);
                pools_state.reduce_raydium_amm_tx(Arc::new(tx)).await;
                panic!("Raydium AMM tx found");
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
