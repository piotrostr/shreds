use borsh::BorshDeserialize;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
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

pub struct ArbEntryProcessor {
    entry_rx: mpsc::Receiver<Vec<Entry>>,
    error_rx: mpsc::Receiver<String>,
    pools_state: Arc<RwLock<PoolsState>>,
    sig_tx: mpsc::Sender<String>,
}

impl ArbEntryProcessor {
    pub fn new(
        entry_rx: mpsc::Receiver<Vec<Entry>>,
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

    pub async fn process_entries(&mut self, entries: Vec<Entry>) {
        let mut pools_state = self.pools_state.write().await;
        debug!(
            "OK: entries {} txs: {}",
            entries.len(),
            entries.iter().map(|e| e.transactions.len()).sum::<usize>(),
        );
        for entry in entries {
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
    entry_rx: mpsc::Receiver<Vec<Entry>>,
    error_rx: mpsc::Receiver<String>,
    sig_tx: mpsc::Sender<String>,
    post_url: String,
    client: reqwest::Client,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CreatePumpTokenEvent {
    pub sig: String,
    // other fields
    pub name: String,
    pub symbol: String,
    pub uri: String,
    pub dev_bought_amount: u64,
    pub dev_max_sol_cost: u64,
    pub num_dev_buy_txs: u64,
}

impl PumpEntryProcessor {
    pub fn new(
        entry_rx: mpsc::Receiver<Vec<Entry>>,
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
    pub async fn process_entries(&self, entries: Vec<Entry>) {
        let events = entries
            .par_iter()
            .map(|entry| {
                entry
                    .transactions
                    .par_iter()
                    .map(|tx| {
                        let mut event = CreatePumpTokenEvent::default();
                        if tx.message.static_account_keys().contains(
                            &Pubkey::from_str(
                                constants::PUMP_FUN_MINT_AUTHORITY,
                            )
                            .expect("Failed to parse pubkey"),
                        ) {
                            // here parse all the required data and send the webhook, this has to go in sync
                            info!("Pump token created: {}", tx.signatures[0]);
                            tx.message.instructions().iter().for_each(|ix| {
                                if let Ok(swap) =
                                    PumpSwapIx::try_from_slice(&ix.data)
                                {
                                    event.dev_bought_amount += swap.amount;
                                    event.dev_max_sol_cost +=
                                        swap.max_sol_cost;
                                    event.num_dev_buy_txs += 1;
                                    info!(
                                        "Pump token swapped: {} ({} SOL)",
                                        swap.amount,
                                        swap.max_sol_cost as f64
                                            / 1_000_000_000.0,
                                    );
                                } else if let Ok(token_metadata) =
                                    PumpCreateIx::try_from_slice(&ix.data)
                                {
                                    event.name = token_metadata.name;
                                    event.symbol = token_metadata.symbol;
                                }
                            });
                        }
                        event.sig = tx.signatures[0].to_string();
                        event
                    })
                    .collect::<Vec<_>>()
            })
            .flatten()
            .collect::<Vec<_>>();

        // this might be tiny bit blocking
        for event in events {
            // self.post_webhook(event.clone()).await;
            info!("Pump token event: {:?}", event);
            self.sig_tx.send(event.sig.clone()).await.unwrap();
        }
    }

    async fn post_webhook(&self, event: CreatePumpTokenEvent) {
        match self.client.post(&self.post_url).json(&event).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    info!("Webhook sent: {:?}", event);
                } else {
                    error!("Failed to send webhook: {:?}", event);
                }
            }
            Err(e) => {
                error!("Failed to send webhook: {:?}", e);
            }
        }
    }
}
