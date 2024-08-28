use borsh::BorshDeserialize;
use solana_client::nonblocking::rpc_client::RpcClient;
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
use crate::raydium::initialize_raydium_amm_pools;

#[derive(Debug, Default)]
pub struct AlgoConfig {
    // arb_mode listens for raydium txs
    pub arb_mode: bool,
    pub mints_of_interest: Vec<Pubkey>,
    // pump_mode listens for pump tokens
    pub pump_mode: bool,
}

pub async fn receive_entries(
    pools_state: Arc<RwLock<PoolsState>>,
    mut entry_receiver: mpsc::Receiver<Vec<Entry>>,
    mut error_receiver: mpsc::Receiver<String>,
    sig_sender: Arc<mpsc::Sender<String>>,
    algo_config: Arc<AlgoConfig>,
) {
    // TODO use nice rpc, possibly geyser at later stage
    let rpc_client = RpcClient::new(env("RPC_URL").to_string());

    if algo_config.arb_mode {
        let mut _pools_state = pools_state.write().await;
        initialize_raydium_amm_pools(
            &rpc_client,
            &mut _pools_state,
            algo_config.mints_of_interest.clone(),
        )
        .await;

        info!(
            "Initialized Raydium AMM pools: {}",
            _pools_state.raydium_pools.len()
        );
        drop(_pools_state);
    }

    let pools_state = pools_state.clone();
    let algo_config = algo_config.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(entries) = entry_receiver.recv() => {
                    let mut pools_state = pools_state.write().await;
                    process_entries_batch(entries, &mut pools_state, sig_sender.clone(), algo_config.clone()).await;
                }
                Some(error) = error_receiver.recv() => {
                    error!("{}", error);
                }
            }
        }
    });
}

pub async fn process_entries_batch(
    entries: Vec<Entry>,
    pools_state: &mut PoolsState,
    sig_sender: Arc<mpsc::Sender<String>>,
    algo_config: Arc<AlgoConfig>,
) {
    debug!(
        "OK: entries {} txs: {}",
        entries.len(),
        entries.iter().map(|e| e.transactions.len()).sum::<usize>(),
    );
    for entry in entries {
        if algo_config.arb_mode {
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
                    sig_sender
                        .send(tx.signatures[0].to_string())
                        .await
                        .unwrap();
                    pools_state.reduce_raydium_amm_tx(Arc::new(tx)).await;
                };
            }
        } else if algo_config.pump_mode {
            for tx in entry.transactions {
                if tx.message.static_account_keys().contains(
                    &Pubkey::from_str(constants::PUMP_FUN_MINT_AUTHORITY)
                        .expect("Failed to parse pubkey"),
                ) {
                    info!("Pump token created: {}", tx.signatures[0]);
                    for ix in tx.message.instructions().iter() {
                        if let Ok(swap) = PumpSwapIx::try_from_slice(&ix.data)
                        {
                            info!(
                                "Pump token swapped: {} ({} SOL)",
                                swap.amount,
                                swap.max_sol_cost as f64 / 1_000_000_000.0,
                            );
                        } else if let Ok(token_metadata) =
                            PumpCreateIx::try_from_slice(&ix.data)
                        {
                            info!(
                                "{} ${}",
                                token_metadata.name, token_metadata.symbol,
                            );
                        }
                    }
                }
            }
        }
    }
    debug!(
        "orca: {}, raydium cp: {}, raydium amm: {}",
        pools_state.orca_count,
        pools_state.raydium_cp_count,
        pools_state.raydium_amm_count
    );
}

pub fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| {
        panic!("{} env var not set", key);
    })
}
