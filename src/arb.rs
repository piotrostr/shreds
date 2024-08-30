use crate::constants;
use crate::raydium::{
    calculate_price, initialize_raydium_amm_pools, parse_amm_instruction,
    swap_exact_amount, ParsedAccounts, ParsedAmmInstruction, RaydiumAmmPool,
};
use crate::util::env;
use log::{error, info, warn};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::instruction::CompiledInstruction;
use solana_sdk::message::VersionedMessage;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_sdk::transaction::VersionedTransaction;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;

pub fn get_mints_of_interest() -> Vec<Pubkey> {
    [
        "3S8qX1MsMqRbiwKg2cQyx7nis1oHMgaCuc9c4VfvVdPN", // mother
        "EbZh3FDVcgnLNbh1ooatcDL1RCRhBgTKirFKNoGPpump", // gringo
        "GYKmdfcUmZVrqfcH1g579BGjuzSRijj3LBuwv79rpump", // wdog
        "8Ki8DpuWNxu9VsS3kQbarsCWMcFGWkzzA8pUPto9zBd5", // lockin
        "HiHULk2EEF6kGfMar19QywmaTJLUr3LA1em8DyW1pump", // ddc
        "GiG7Hr61RVm4CSUxJmgiCoySFQtdiwxtqf64MsRppump", // scf
        "3B5wuUrMEi5yATD7on46hKfej3pfmd7t1RKgrsN3pump", // billy
        "CTg3ZgYx79zrE1MteDVkmkcGniiFrK1hJ6yiabropump", // neiro
    ]
    .iter()
    .map(|p| Pubkey::from_str(p).unwrap())
    .collect::<Vec<_>>()
}

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
    /// Initialize the state of the pools, this has to be called every time
    /// after struct is created for arb
    pub async fn initialize(&mut self) {
        initialize_raydium_amm_pools(
            &RpcClient::new(env("RPC_URL").to_string()),
            self,
            get_mints_of_interest(),
        )
        .await;
        info!(
            "Initialized Raydium AMM pools: {}",
            self.raydium_pools.len()
        );

        // TODO orca etc
    }

    pub fn reduce_orca_tx(&mut self, _tx: VersionedTransaction) {
        // TODO: Implement Orca transaction processing
    }

    pub async fn reduce_raydium_amm_tx(
        &mut self,
        tx: Arc<VersionedTransaction>,
    ) {
        let raydium_amm_program_id = Pubkey::from_str(constants::RAYDIUM_AMM)
            .expect("Failed to parse Raydium AMM program ID");

        for (idx, instruction) in tx.message.instructions().iter().enumerate()
        {
            let program_id = tx.message.static_account_keys()
                [instruction.program_id_index as usize];

            if program_id == raydium_amm_program_id {
                match parse_amm_instruction(&instruction.data) {
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
            warn!(
                "{} Failed to get account keys for Raydium AMM instruction",
                signature.to_string()
            );
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
                == Pubkey::from_str(constants::WSOL).expect("pubkey");
            let sol_is_coin = pool.amm_keys.amm_coin_mint
                == Pubkey::from_str(constants::WSOL).expect("pubkey");

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
