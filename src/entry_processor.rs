use borsh::BorshDeserialize;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use solana_sdk::clock::Slot;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;

use log::{debug, error, info};
use solana_entry::entry::Entry;
use solana_sdk::pubkey::Pubkey;
use tokio::sync::mpsc;

use crate::arb::PoolsState;
use crate::constants;
use crate::pump::{PumpCreateIx, PumpSwapIx};
use crate::util::{pubkey_to_string, string_to_pubkey};

// those are virtual btw
// Initial SOL reserves: 30,000,000,000 lamports (30 SOL)
// Initial token reserves: 1,073,000,000,000,000 tokens
pub const DEFAULT_SOL_INITIAL_RESERVES: u64 = 30_000_000_000;
pub const DEFAULT_TOKEN_INITIAL_RESERVES: u64 = 1_073_000_000_000_000;

pub struct EntriesWithMeta {
    pub entries: Vec<Entry>,
    pub slot: Slot,
}

pub struct ArbEntryProcessor {
    entry_rx: mpsc::Receiver<EntriesWithMeta>,
    error_rx: mpsc::Receiver<String>,
    pools_state: Arc<RwLock<PoolsState>>,
    sig_tx: mpsc::Sender<String>,
}

impl ArbEntryProcessor {
    pub fn new(
        entry_rx: mpsc::Receiver<EntriesWithMeta>,
        error_rx: mpsc::Receiver<String>,
        pools_state: Arc<RwLock<PoolsState>>,
        sig_tx: mpsc::Sender<String>,
    ) -> Self {
        ArbEntryProcessor {
            entry_rx,
            error_rx,
            pools_state,
            sig_tx,
        }
    }

    pub async fn receive_entries(&mut self) {
        loop {
            tokio::select! {
                Some(entries) = self.entry_rx.recv() => {
                    self.process_entries(entries).await;
                }
                Some(error) = self.error_rx.recv() => {
                    error!("{}", error);
                }
            }
        }
    }

    pub async fn process_entries(
        &mut self,
        entries_with_meta: EntriesWithMeta,
    ) {
        let mut pools_state = self.pools_state.write().await;
        debug!(
            "OK: entries {} txs: {}",
            entries_with_meta.entries.len(),
            entries_with_meta
                .entries
                .iter()
                .map(|e| e.transactions.len())
                .sum::<usize>(),
        );
        for entry in entries_with_meta.entries {
            for tx in entry.transactions {
                if tx.message.static_account_keys().contains(
                    &Pubkey::from_str(constants::WHIRLPOOL)
                        .expect("Failed to parse pubkey"),
                ) {
                    pools_state.orca_count += 1;
                    pools_state.reduce_orca_tx(tx);
                } else if tx.message.static_account_keys().contains(
                    &Pubkey::from_str(constants::RAYDIUM_CP)
                        .expect("Failed to parse pubkey"),
                ) {
                    pools_state.raydium_cp_count += 1;
                    // pools_state.reduce_raydium_cp_tx(tx);
                } else if tx.message.static_account_keys().contains(
                    &Pubkey::from_str(constants::RAYDIUM_AMM)
                        .expect("Failed to parse pubkey"),
                ) {
                    pools_state.raydium_amm_count += 1;
                    self.sig_tx
                        .send(tx.signatures[0].to_string())
                        .await
                        .unwrap();
                    pools_state.reduce_raydium_amm_tx(Arc::new(tx)).await;
                };
            }
            debug!(
                "orca: {}, raydium cp: {}, raydium amm: {}",
                pools_state.orca_count,
                pools_state.raydium_cp_count,
                pools_state.raydium_amm_count
            );
        }
    }
}

pub struct PumpEntryProcessor {
    entry_rx: mpsc::Receiver<EntriesWithMeta>,
    error_rx: mpsc::Receiver<String>,
    sig_tx: mpsc::Sender<String>,
    post_url: String,
    client: reqwest::Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePumpTokenEvent {
    pub sig: String,
    pub slot: Slot,
    #[serde(
        serialize_with = "pubkey_to_string",
        deserialize_with = "string_to_pubkey"
    )]
    pub mint: Pubkey,
    #[serde(
        serialize_with = "pubkey_to_string",
        deserialize_with = "string_to_pubkey"
    )]
    pub bounding_curve: Pubkey,
    #[serde(
        serialize_with = "pubkey_to_string",
        deserialize_with = "string_to_pubkey"
    )]
    pub associated_bounding_curve: Pubkey,
    pub name: String,
    pub symbol: String,
    pub uri: String,
    pub dev_bought_amount: u64,
    pub dev_max_sol_cost: u64,
    pub num_dev_buy_txs: u64,
    pub virtual_sol_reserves: u64,
    pub virtual_token_reserves: u64,
}

impl Default for CreatePumpTokenEvent {
    fn default() -> Self {
        CreatePumpTokenEvent {
            sig: "".to_string(),
            slot: 0,
            mint: Pubkey::default(),
            bounding_curve: Pubkey::default(),
            associated_bounding_curve: Pubkey::default(),
            name: "".to_string(),
            symbol: "".to_string(),
            uri: "".to_string(),
            dev_bought_amount: 0,
            dev_max_sol_cost: 0,
            num_dev_buy_txs: 0,
            virtual_sol_reserves: DEFAULT_SOL_INITIAL_RESERVES,
            virtual_token_reserves: DEFAULT_TOKEN_INITIAL_RESERVES,
        }
    }
}

impl PumpEntryProcessor {
    pub fn new(
        entry_rx: mpsc::Receiver<EntriesWithMeta>,
        error_rx: mpsc::Receiver<String>,
        sig_tx: mpsc::Sender<String>,
        post_url: String,
    ) -> Self {
        PumpEntryProcessor {
            entry_rx,
            error_rx,
            sig_tx,
            post_url,
            client: reqwest::Client::new(),
        }
    }

    pub async fn receive_entries(&mut self) {
        loop {
            tokio::select! {
                Some(entries) = self.entry_rx.recv() => {
                    self.process_entries(entries).await;
                }
                Some(error) = self.error_rx.recv() => {
                    error!("{}", error);
                }
            }
        }
    }

    /// TODO each vec of entries should be included metadata about slot of deshred
    pub async fn process_entries(&self, entries_with_meta: EntriesWithMeta) {
        let events = entries_with_meta
            .entries
            .par_iter()
            .map(|entry| {
                entry
                    .transactions
                    .par_iter()
                    .filter_map(|tx| {
                        let mut event = CreatePumpTokenEvent::default();
                        let account_keys = tx.message.static_account_keys();
                        if account_keys.len() == 18
                            && account_keys.contains(
                                &Pubkey::from_str(
                                    constants::PUMP_FUN_MINT_AUTHORITY,
                                )
                                .expect("Failed to parse pubkey"),
                            )
                        {
                            println!("Found pump tx: {:#?}", tx);
                            event.mint = account_keys[1];
                            event.bounding_curve = account_keys[3];
                            event.associated_bounding_curve = account_keys[4];
                            tx.message.instructions().iter().for_each(|ix| {
                                if let Ok(swap) =
                                    PumpSwapIx::try_from_slice(&ix.data)
                                {
                                    event.dev_bought_amount += swap.amount;
                                    event.dev_max_sol_cost +=
                                        swap.max_sol_cost;
                                    event.num_dev_buy_txs += 1;
                                    event.virtual_sol_reserves +=
                                        deduct_fee(swap.max_sol_cost);
                                    event.virtual_token_reserves -=
                                        swap.amount;
                                } else if let Ok(token_metadata) =
                                    PumpCreateIx::try_from_slice(&ix.data)
                                {
                                    event.name = token_metadata.name;
                                    event.symbol = token_metadata.symbol;
                                    event.uri = token_metadata.uri;
                                }
                            });
                        } else {
                            return None;
                        }
                        event.sig = tx.signatures[0].to_string();
                        event.slot = entries_with_meta.slot;
                        Some(event)
                    })
                    .collect::<Vec<_>>()
            })
            .flatten()
            .collect::<Vec<_>>();

        // this might be tiny bit blocking
        for event in events {
            info!(
                "Sending webhook: {}",
                serde_json::to_string_pretty(&event).expect("pretty")
            );
            self.post_webhook(event.clone()).await;
            self.sig_tx.send(event.sig.clone()).await.unwrap();
        }
    }

    async fn post_webhook(&self, event: CreatePumpTokenEvent) {
        let url = self.post_url.clone() + "/v2/pump-buy";
        match self.client.post(url.clone()).json(&event).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    info!("Webhook sent: {:?}", event);
                } else {
                    error!("Failed to send webhook to {}: {:?}", url, event);
                }
            }
            Err(e) => {
                error!("Failed to send webhook: {:?}", e);
            }
        }
    }
}

/// deduct_fee takes the 1% fee from the amount of SOL out
/// e.g. if you buy 1 sol worth of the token at start, the max_sol_amount will
/// amount to 1.01 sol, only 1 sol goes to the pool, 0.01 is the fee
pub fn deduct_fee(sol_amount: u64) -> u64 {
    (sol_amount * 100) / 101
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deduct_fee() {
        assert_eq!(deduct_fee(1010000000), 1000000000);
        assert_eq!(deduct_fee(2020000000), 2000000000);
    }
}
