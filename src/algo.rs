// TODOs
// 1) in the algo, ensure that ATAs are already created, this saves some ixs
// 2) validate the pool update is correct
// 3) implement calculate amount out for a given amount for both pools for profit search
// 4) take volume into account when calculating profit and best size (flash loans might be an
//    option)
//  * there might be missing data, there has to be a constant stream for resolving
//  * the pool calculation has to involve the slippage and the exact amount that someone is to
//  receive, it should amount to the right amount per slot since the transactions are in the
//  ordering as accepted per validator, so just by calculating the price it should work
use futures_util::future::join_all;
use once_cell::sync::Lazy;
use serde_json::Value;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::{EncodableKey, Signer};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;

use log::{debug, error, info, warn};
use solana_entry::entry::Entry;
use solana_program::instruction::CompiledInstruction;
use solana_sdk::message::VersionedMessage;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::transaction::VersionedTransaction;
use tokio::sync::mpsc;

use crate::raydium::{
    self, calculate_price, swap_exact_amount, ParsedAccounts,
    ParsedAmmInstruction, RaydiumAmmPool,
};
use raydium_library::amm::{self, openbook};

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
                            &tx.signatures[0],
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
        signature: &Signature,
    ) {
        let amm_id_index = 1; // Amm account index
        let pool_coin_token_account_index = 5; // Pool Coin Token Account index
        let pool_pc_token_account_index = 6; // Pool Pc Token Account index

        let amm_id =
            get_account_key_safely(message, instruction, amm_id_index);
        let pool_coin_vault = get_account_key_safely(
            message,
            instruction,
            pool_coin_token_account_index,
        );
        let pool_pc_vault = get_account_key_safely(
            message,
            instruction,
            pool_pc_token_account_index,
        );
        if amm_id.is_none()
            || pool_coin_vault.is_none()
            || pool_pc_vault.is_none()
        {
            warn!("Failed to get account keys for Raydium AMM instruction");
            return;
        }
        let amm_id = amm_id.unwrap();
        let pool_coin_vault = pool_coin_vault.unwrap();
        let pool_pc_vault = pool_pc_vault.unwrap();

        match parsed_instruction {
            ParsedAmmInstruction::SwapBaseOut(swap_instruction) => {
                self.update_pool_state_swap(
                    ParsedAccounts {
                        amm_id,
                        pool_coin_vault,
                        pool_pc_vault,
                    },
                    swap_instruction.max_amount_in,
                    swap_instruction.amount_out,
                    false,
                    signature,
                )
                .await;
            }
            ParsedAmmInstruction::SwapBaseIn(swap_instruction) => {
                self.update_pool_state_swap(
                    ParsedAccounts {
                        amm_id,
                        pool_coin_vault,
                        pool_pc_vault,
                    },
                    swap_instruction.amount_in,
                    swap_instruction.minimum_amount_out,
                    true,
                    signature,
                )
                .await;
            }
            // Handle other instruction types...
            _ => {
                warn!("Unhandled instruction type: {:?}", parsed_instruction)
            }
        }
    }
    async fn update_pool_state_swap(
        &mut self,
        parsed_accounts: ParsedAccounts,
        amount_specified: u64,
        other_amount_threshold: u64,
        is_swap_base_in: bool,
        signature: &Signature,
    ) {
        if let Some(pool) = self.raydium_pools.get(&parsed_accounts.amm_id) {
            let mut pool = pool.write().await;
            let is_coin_to_pc = pool.amm_keys.amm_coin_vault
                == parsed_accounts.pool_coin_vault
                && pool.amm_keys.amm_pc_vault
                    == parsed_accounts.pool_pc_vault;

            let swap_direction = if is_coin_to_pc {
                raydium_amm::math::SwapDirection::Coin2PC
            } else {
                raydium_amm::math::SwapDirection::PC2Coin
            };

            let (pc_amount, coin_amount) = if is_swap_base_in {
                let swap_amount_out = swap_exact_amount(
                    pool.state.pool_pc_vault_amount,
                    pool.state.pool_coin_vault_amount,
                    pool.state.swap_fee_numerator,
                    pool.state.swap_fee_denominator,
                    swap_direction,
                    amount_specified,
                    true,
                );

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
                let swap_amount_in = swap_exact_amount(
                    pool.state.pool_pc_vault_amount,
                    pool.state.pool_coin_vault_amount,
                    pool.state.swap_fee_numerator,
                    pool.state.swap_fee_denominator,
                    swap_direction,
                    other_amount_threshold,
                    false,
                );

                if is_coin_to_pc {
                    (
                        pool.state
                            .pool_pc_vault_amount
                            .saturating_add(other_amount_threshold),
                        pool.state
                            .pool_coin_vault_amount
                            .saturating_sub(swap_amount_in),
                    )
                } else {
                    (
                        pool.state
                            .pool_pc_vault_amount
                            .saturating_sub(swap_amount_in),
                        pool.state
                            .pool_coin_vault_amount
                            .saturating_add(other_amount_threshold),
                    )
                }
            };

            let initial_price = calculate_price(&pool.state, &pool.decimals);

            // Update pool amounts
            pool.state.pool_pc_vault_amount = pc_amount;
            pool.state.pool_coin_vault_amount = coin_amount;

            let new_price = calculate_price(&pool.state, &pool.decimals);

            info!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "event": "Swap",
                    "signature": signature.to_string(),
                    "swap_direction": format!("{:?}", swap_direction),
                    "is_swap_base_in": is_swap_base_in,
                    "amm_id": parsed_accounts.amm_id.to_string(),
                    "pc_mint": pool.amm_keys.amm_pc_mint.to_string(),
                    "amount_specified": amount_specified,
                    "coin_mint": pool.amm_keys.amm_coin_mint.to_string(),
                    "other_amount_threshold": other_amount_threshold,
                    "initial_price": initial_price,
                    "new_price": new_price,
                }))
                .unwrap()
            );
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
        "HiHULk2EEF6kGfMar19QywmaTJLUr3LA1em8DyW1pump", // ddc
    ]
    .iter()
    .map(|p| Pubkey::from_str(p).unwrap())
    .collect::<Vec<_>>();

    // TODO use nice rpc, possibly geyser at later stage
    let rpc_client = RpcClient::new(env("RPC_URL").to_string());

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

static RAYDIUM_JSON: Lazy<Arc<Value>> = Lazy::new(|| {
    if !std::path::Path::new("raydium.json").exists() {
        panic!("raydium.json not found, download it first");
    }
    let json_str = std::fs::read_to_string("raydium.json")
        .expect("Failed to read raydium.json");
    let json_value: Value = serde_json::from_str(&json_str)
        .expect("Failed to parse raydium.json");
    Arc::new(json_value)
});

pub async fn initialize_raydium_amm_pools(
    rpc_client: &RpcClient,
    pools_state: &mut PoolsState,
    mints_of_interest: Vec<Pubkey>,
) {
    info!("Reading in raydium.json (large file)");
    let amm_keys_map = raydium::parse_raydium_json(
        RAYDIUM_JSON.clone(),
        mints_of_interest.clone(),
    )
    .expect("parse raydium json");
    let amm_program =
        Pubkey::from_str(RAYDIUM_LIQUIDITY_POOL_V4_PUBKEY).expect("pubkey");
    let payer = Keypair::read_from_file(env("FUND_KEYPAIR_PATH"))
        .expect("Failed to read keypair");
    let fee_payer = payer.pubkey();

    // Fetch results
    let futures = mints_of_interest.iter().map(|mint| {
        let amm_keys_map = amm_keys_map.clone();
        async move {
            let amm_keys_vec = amm_keys_map.get(mint).unwrap(); // bound to exist
            let mut results = Vec::new();
            for (amm_keys, decimals) in amm_keys_vec.iter() {
                info!("Loading AMM keys for pool: {:?}", amm_keys.amm_pool);
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
                    amm::utils::CalculateMethod::Simulate(fee_payer),
                )
                .await
                .expect("calculate pool vault amounts");
                results.push((*mint, *amm_keys, state, *decimals));
            }
            results
        }
    });

    // Join all futures
    let all_results = join_all(futures).await;

    // Update pools_state
    for results in all_results {
        for (mint, amm_keys, state, decimals) in results {
            pools_state.raydium_pools.insert(
                amm_keys.amm_pool,
                Arc::new(RwLock::new(RaydiumAmmPool {
                    token: mint,
                    amm_keys,
                    state,
                    decimals,
                })),
            );
            pools_state
                .raydium_pools_by_mint
                .entry(mint)
                .or_default()
                .push(amm_keys.amm_pool);
        }
    }
}

pub async fn process_entries_batch(
    entries: Vec<Entry>,
    pools_state: &mut PoolsState,
) {
    debug!(
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
                // println!("Raydium AMM tx: {:?}", tx.signatures);
                pools_state.reduce_raydium_amm_tx(Arc::new(tx)).await;
            };
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
fn get_account_key_safely(
    message: &VersionedMessage,
    instruction: &CompiledInstruction,
    account_index: usize,
) -> Option<Pubkey> {
    instruction
        .accounts
        .get(account_index)
        .and_then(|&index| message.static_account_keys().get(index as usize))
        .copied()
}
