use std::collections::HashMap;
use std::str::FromStr;

use log::{error, info};
use solana_entry::entry::Entry;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::transaction::VersionedTransaction;
use tokio::sync::mpsc;

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
    pub orca_token_to_pool: HashMap<Pubkey, OrcaPool>,
    pub raydium_amm_token_to_pool: HashMap<Pubkey, RaydiumAmmPool>,
}

/// those pools contain all of the required pubkeys for making the transactions
/// those are loaded during startup
#[derive(Debug, Default)]
pub struct OrcaPool {}

/// -/-
#[derive(Debug, Default)]
pub struct RaydiumAmmPool {}

impl PoolsState {
    pub fn reduce_orca_tx(&mut self, _tx: VersionedTransaction) {
        // TODO
    }
    pub fn reduce_raydium_amm_tx(&mut self, _tx: VersionedTransaction) {
        // TODO
    }

    pub fn reduce_raydium_cp_tx(&mut self, _tx: VersionedTransaction) {
        panic!("Not implemented yet");
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
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(entries) = entry_receiver.recv() => {
                    process_entries_batch(entries, &mut pools_state);
                }
                Some(error) = error_receiver.recv() => {
                    error!("{}", error);
                }
            }
        }
    });
}

pub fn process_entries_batch(
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
            };
            if tx.message.static_account_keys().contains(
                &Pubkey::from_str(RAYDIUM_CP)
                    .expect("Failed to parse pubkey"),
            ) {
                info!("OK: Found Raydium CP tx {:?}", tx.signatures);
                pools_state.raydium_cp_count += 1;
            };
            if tx.message.static_account_keys().contains(
                &Pubkey::from_str(RAYDIUM_AMM)
                    .expect("Failed to parse pubkey"),
            ) {
                info!("OK: Found Raydium AMM tx {:?}", tx.signatures);
                pools_state.raydium_amm_count += 1;
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
