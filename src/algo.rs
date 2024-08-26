// TODOs
// * in the algo, ensure that ATAs are already created, this saves some ixs
// * take volume into account when calculating profit and best size (flash loans might be an
//   option)
// * there is missing data, likely due to an error somewhere, could be the coding shreds that are
// to be used
// * it might be useful to receive a single data tick and inspect on how the shreds are forwarded
// technically, shreds could be used to maintain ledger altogether, the only thing that is needed
// * pool calculation might be a bit off, this is to verify when putting txs together
// * orca is yet to be implememnted, this is to be done after raydium is working
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
pub const WSOL: &str = "So11111111111111111111111111111111111111112";

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
    pub signatures: Vec<Signature>,
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
                            &parsed_instruction,
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
        parsed_instruction: &ParsedAmmInstruction,
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
                    &ParsedAccounts {
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
                    &ParsedAccounts {
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

    /// TODO the account keys here matter, swap base in can be with a flipped user account source
    /// and destination and then it swaps the token in and out
    async fn update_pool_state_swap(
        &mut self,
        parsed_accounts: &ParsedAccounts,
        amount_specified: u64,
        other_amount_threshold: u64,
        is_swap_base_in: bool,
        signature: &Signature,
    ) {
        if let Some(pool) = self.raydium_pools.get(&parsed_accounts.amm_id) {
            let mut pool = pool.write().await;
            if !(pool.amm_keys.amm_coin_vault
                == parsed_accounts.pool_coin_vault
                && pool.amm_keys.amm_pc_vault
                    == parsed_accounts.pool_pc_vault)
            {
                error!(
                    "Vault mismatch: {} {} {} {} {}",
                    pool.amm_keys.amm_pool,
                    pool.amm_keys.amm_coin_vault,
                    parsed_accounts.pool_coin_vault,
                    pool.amm_keys.amm_pc_vault,
                    parsed_accounts.pool_pc_vault,
                );
                return;
            };

            let swap_direction = if is_swap_base_in {
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

                if is_swap_base_in {
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

                if is_swap_base_in {
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

            let sol_is_pc = pool.amm_keys.amm_pc_mint
                == Pubkey::from_str(WSOL).expect("pubkey");
            let sol_is_coin = pool.amm_keys.amm_coin_mint
                == Pubkey::from_str(WSOL).expect("pubkey");

            let sol_amount = if sol_is_coin {
                if is_swap_base_in {
                    amount_specified
                } else {
                    other_amount_threshold
                }
            } else if sol_is_pc {
                if is_swap_base_in {
                    other_amount_threshold
                } else {
                    amount_specified
                }
            } else {
                0
            } as f64
                / 10u64.pow(9u32) as f64;

            pool.state.pool_pc_vault_amount = pc_amount;
            pool.state.pool_coin_vault_amount = coin_amount;

            let initial_price = calculate_price(&pool.state, &pool.decimals);

            let new_price = calculate_price(&pool.state, &pool.decimals);

            if sol_amount > 10. {
                info!(
                    "large swap: ({}) {}",
                    sol_amount,
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
    }

    fn _check_arbitrage_opportunity(
        &self,
        _pool: RaydiumAmmPool,
    ) -> Option<f64> {
        None
    }
}

pub async fn receive_entries(
    pools_state: Arc<RwLock<PoolsState>>,
    mut entry_receiver: mpsc::Receiver<Vec<Entry>>,
    mut error_receiver: mpsc::Receiver<String>,
    sig_sender: Arc<mpsc::Sender<String>>,
) {
    let mints_of_interest = [
        "3S8qX1MsMqRbiwKg2cQyx7nis1oHMgaCuc9c4VfvVdPN", // mother
        // "EbZh3FDVcgnLNbh1ooatcDL1RCRhBgTKirFKNoGPpump", // gringo
        "GYKmdfcUmZVrqfcH1g579BGjuzSRijj3LBuwv79rpump", // wdog
        "8Ki8DpuWNxu9VsS3kQbarsCWMcFGWkzzA8pUPto9zBd5", // lockin
        "HiHULk2EEF6kGfMar19QywmaTJLUr3LA1em8DyW1pump", // ddc
        "GiG7Hr61RVm4CSUxJmgiCoySFQtdiwxtqf64MsRppump", // scf
        "3B5wuUrMEi5yATD7on46hKfej3pfmd7t1RKgrsN3pump", // billy
        "CTg3ZgYx79zrE1MteDVkmkcGniiFrK1hJ6yiabropump", // neiro
    ]
    .iter()
    .map(|p| Pubkey::from_str(p).unwrap())
    .collect::<Vec<_>>();

    // TODO use nice rpc, possibly geyser at later stage
    let rpc_client = RpcClient::new(env("RPC_URL").to_string());

    let mut _pools_state = pools_state.write().await;
    initialize_raydium_amm_pools(
        &rpc_client,
        &mut _pools_state,
        mints_of_interest,
    )
    .await;

    info!(
        "Initialized Raydium AMM pools: {}",
        _pools_state.raydium_pools.len()
    );
    drop(_pools_state);

    let pools_state = pools_state.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(entries) = entry_receiver.recv() => {
                    let mut pools_state = pools_state.write().await;
                    process_entries_batch(entries, &mut pools_state, sig_sender.clone()).await;
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
    let amm_program = Pubkey::from_str(RAYDIUM_AMM).expect("pubkey");
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
    sig_sender: Arc<mpsc::Sender<String>>,
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
                sig_sender.send(tx.signatures[0].to_string()).await.unwrap();
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

pub fn get_account_key_safely(
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
