use futures_util::future::join_all;
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use log::{info, warn};
use once_cell::sync::Lazy;
use raydium_amm::math::{CheckedCeilDiv, SwapDirection, U128};
use raydium_library::amm::{self, openbook, AmmKeys, CalculateResult};
use reqwest::Client;
use serde_json::Value;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::{EncodableKey, Signer};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;

use raydium_amm::instruction::{
    AdminCancelOrdersInstruction, ConfigArgs, DepositInstruction,
    InitializeInstruction, InitializeInstruction2, MonitorStepInstruction,
    PreInitializeInstruction, SetParamsInstruction, SimulateInstruction,
    SwapInstructionBaseIn, SwapInstructionBaseOut, WithdrawInstruction,
    WithdrawSrmInstruction,
};
use solana_program::program_error::ProgramError;

use crate::algo::env;
use crate::arb::PoolsState;
use crate::constants;

pub struct ParsedAccounts {
    pub amm_id: Pubkey,
    pub pool_coin_vault: Pubkey,
    pub pool_pc_vault: Pubkey,
}

#[derive(Debug, Clone, Copy)]
pub struct RaydiumDecimals {
    pub coin_decimals: u8,
    pub pc_decimals: u8,
    pub lp_decimals: u8,
}

// TODO there might be sol-token or token-sol both versions, gotta be able to handle both
#[derive(Debug, Clone)]
pub struct RaydiumAmmPool {
    pub token: Pubkey,
    pub amm_keys: AmmKeys,
    pub state: CalculateResult,
    pub decimals: RaydiumDecimals,
}

pub async fn initialize_raydium_amm_pools(
    rpc_client: &RpcClient,
    pools_state: &mut PoolsState,
    mints_of_interest: Vec<Pubkey>,
) {
    info!("Reading in raydium.json (large file)");
    let amm_keys_map =
        parse_raydium_json(RAYDIUM_JSON.clone(), mints_of_interest.clone())
            .expect("parse raydium json");
    let amm_program =
        Pubkey::from_str(constants::RAYDIUM_AMM).expect("pubkey");
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

pub fn calculate_price(
    state: &CalculateResult,
    decimals: &RaydiumDecimals,
) -> Option<u64> {
    if state.pool_coin_vault_amount == 0 {
        return None;
    }
    let pc_decimals = 10u64.pow(decimals.pc_decimals as u32);
    let coin_decimals = 10u64.pow(decimals.coin_decimals as u32);

    // Calculate price maintaining full precision
    let price = (state.pool_pc_vault_amount / pc_decimals)
        / (state.pool_coin_vault_amount / coin_decimals);

    Some(price)
}

#[derive(Debug)]
pub enum ParsedAmmInstruction {
    Initialize(InitializeInstruction),
    Initialize2(InitializeInstruction2),
    MonitorStep(MonitorStepInstruction),
    Deposit(DepositInstruction),
    Withdraw(WithdrawInstruction),
    MigrateToOpenBook,
    SetParams(SetParamsInstruction),
    WithdrawPnl,
    WithdrawSrm(WithdrawSrmInstruction),
    SwapBaseIn(SwapInstructionBaseIn),
    PreInitialize(PreInitializeInstruction),
    SwapBaseOut(SwapInstructionBaseOut),
    SimulateInfo(SimulateInstruction),
    AdminCancelOrders(AdminCancelOrdersInstruction),
    CreateConfigAccount,
    UpdateConfigAccount(ConfigArgs),
}

pub fn parse_amm_instruction(
    data: &[u8],
) -> Result<ParsedAmmInstruction, ProgramError> {
    let (&tag, rest) = data
        .split_first()
        .ok_or(ProgramError::InvalidInstructionData)?;

    match tag {
        0 => {
            let (nonce, rest) = unpack_u8(rest)?;
            let (open_time, _) = unpack_u64(rest)?;
            Ok(ParsedAmmInstruction::Initialize(InitializeInstruction {
                nonce,
                open_time,
            }))
        }
        1 => {
            let (nonce, rest) = unpack_u8(rest)?;
            let (open_time, rest) = unpack_u64(rest)?;
            let (init_pc_amount, rest) = unpack_u64(rest)?;
            let (init_coin_amount, _) = unpack_u64(rest)?;
            Ok(ParsedAmmInstruction::Initialize2(InitializeInstruction2 {
                nonce,
                open_time,
                init_pc_amount,
                init_coin_amount,
            }))
        }
        9 => {
            let (amount_in, rest) = unpack_u64(rest)?;
            let (minimum_amount_out, _) = unpack_u64(rest)?;
            Ok(ParsedAmmInstruction::SwapBaseIn(SwapInstructionBaseIn {
                amount_in,
                minimum_amount_out,
            }))
        }
        11 => {
            let (max_amount_in, rest) = unpack_u64(rest)?;
            let (amount_out, _) = unpack_u64(rest)?;
            Ok(ParsedAmmInstruction::SwapBaseOut(SwapInstructionBaseOut {
                max_amount_in,
                amount_out,
            }))
        }
        // Add other instruction parsing here...
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

fn unpack_u8(input: &[u8]) -> Result<(u8, &[u8]), ProgramError> {
    if !input.is_empty() {
        let (amount, rest) = input.split_at(1);
        let amount = amount[0];
        Ok((amount, rest))
    } else {
        Err(ProgramError::InvalidInstructionData)
    }
}

fn unpack_u64(input: &[u8]) -> Result<(u64, &[u8]), ProgramError> {
    if input.len() >= 8 {
        let (amount, rest) = input.split_at(8);
        let amount = u64::from_le_bytes(amount.try_into().unwrap());
        Ok((amount, rest))
    } else {
        Err(ProgramError::InvalidInstructionData)
    }
}

pub async fn download_raydium_json(
    update: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if Path::new("raydium.json").exists() && !update {
        warn!("raydium.json already exists. Skipping download.");
        return Ok(());
    }
    info!("Downloading raydium.json");

    let url = "https://api.raydium.io/v2/sdk/liquidity/mainnet.json";
    let client = Client::new();
    let res = client.get(url).send().await?;
    let total_size = res.content_length().unwrap_or(0);

    let pb = ProgressBar::new(total_size);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
        .unwrap()
        .progress_chars("#>-"));

    let mut file = File::create("raydium.json")?;
    let mut downloaded: u64 = 0;
    let mut stream = res.bytes_stream();

    while let Some(item) = stream.next().await {
        let chunk = item?;
        file.write_all(&chunk)?;
        let new =
            std::cmp::min(downloaded + (chunk.len() as u64), total_size);
        downloaded = new;
        pb.set_position(new);
    }

    pb.finish_with_message("Download completed");
    Ok(())
}

type Amm = (AmmKeys, RaydiumDecimals);

// this takes long, possibly could make it so that it uses a search index it
// returns all of the
// pools for a given token, to be filtered later for relevant
pub fn parse_raydium_json(
    raydium_json: Arc<Value>,
    mints_of_interest: Vec<Pubkey>,
) -> Result<HashMap<Pubkey, Vec<Amm>>, Box<dyn std::error::Error>> {
    let mut result = HashMap::new();
    info!("Parsing relevant pools");

    let pools = raydium_json["unOfficial"].as_array().unwrap();
    let total_pools = pools.len();

    let pb = ProgressBar::new(total_pools as u64);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
        .unwrap()
        .progress_chars("#>-"));

    for (index, json) in pools.iter().enumerate() {
        let base_mint =
            Pubkey::from_str(json["baseMint"].as_str().unwrap()).unwrap();
        let quote_mint =
            Pubkey::from_str(json["quoteMint"].as_str().unwrap()).unwrap();
        if mints_of_interest.contains(&base_mint) {
            let amm_keys = json_to_amm(json);
            result.entry(base_mint).or_insert(vec![]).push(amm_keys);
        } else if mints_of_interest.contains(&quote_mint) {
            let amm_keys = json_to_amm(json);
            result.entry(quote_mint).or_insert(vec![]).push(amm_keys);
        }

        pb.set_position((index + 1) as u64);
    }

    pb.finish_with_message("Parsing completed");

    if mints_of_interest.len() != result.len() {
        warn!("Not all mints found in raydium.json");
    }
    Ok(result)
}

pub fn json_to_amm(json: &Value) -> (AmmKeys, RaydiumDecimals) {
    (
        AmmKeys {
            amm_pool: Pubkey::from_str(json["id"].as_str().unwrap()).unwrap(),
            amm_coin_mint: Pubkey::from_str(
                json["baseMint"].as_str().unwrap(),
            )
            .unwrap(),
            amm_pc_mint: Pubkey::from_str(
                json["quoteMint"].as_str().unwrap(),
            )
            .unwrap(),
            amm_authority: Pubkey::from_str(
                json["authority"].as_str().unwrap(),
            )
            .unwrap(),
            amm_target: Pubkey::from_str(
                json["targetOrders"].as_str().unwrap(),
            )
            .unwrap(),
            amm_coin_vault: Pubkey::from_str(
                json["baseVault"].as_str().unwrap(),
            )
            .unwrap(),
            amm_pc_vault: Pubkey::from_str(
                json["quoteVault"].as_str().unwrap(),
            )
            .unwrap(),
            amm_lp_mint: Pubkey::from_str(json["lpMint"].as_str().unwrap())
                .unwrap(),
            amm_open_order: Pubkey::from_str(
                json["openOrders"].as_str().unwrap(),
            )
            .unwrap(),
            market_program: Pubkey::from_str(
                json["marketProgramId"].as_str().unwrap(),
            )
            .unwrap(),
            market: Pubkey::from_str(json["marketId"].as_str().unwrap())
                .unwrap(),
            nonce: u8::default(), // not relevant
        },
        RaydiumDecimals {
            coin_decimals: json["baseDecimals"].as_u64().unwrap() as u8,
            pc_decimals: json["quoteDecimals"].as_u64().unwrap() as u8,
            lp_decimals: json["lpDecimals"].as_u64().unwrap() as u8,
        },
    )
}

pub fn swap_exact_amount(
    pc_vault_amount: u64,
    coin_vault_amount: u64,
    swap_fee_numerator: u64,
    swap_fee_denominator: u64,
    swap_direction: SwapDirection,
    amount_specified: u64,
    swap_base_in: bool,
) -> u64 {
    if swap_base_in {
        let swap_fee = U128::from(amount_specified)
            .checked_mul(swap_fee_numerator.into())
            .unwrap()
            .checked_ceil_div(swap_fee_denominator.into())
            .unwrap()
            .0;
        let swap_in_after_deduct_fee =
            U128::from(amount_specified).checked_sub(swap_fee).unwrap();
        raydium_amm::math::Calculator::swap_token_amount_base_in(
            swap_in_after_deduct_fee,
            pc_vault_amount.into(),
            coin_vault_amount.into(),
            swap_direction,
        )
        .as_u64()
    } else {
        let swap_in_before_add_fee =
            raydium_amm::math::Calculator::swap_token_amount_base_out(
                amount_specified.into(),
                pc_vault_amount.into(),
                coin_vault_amount.into(),
                swap_direction,
            );
        swap_in_before_add_fee
            .checked_mul(swap_fee_denominator.into())
            .unwrap()
            .checked_ceil_div(
                (swap_fee_denominator
                    .checked_sub(swap_fee_numerator)
                    .unwrap())
                .into(),
            )
            .unwrap()
            .0
            .as_u64()
    }
}

pub fn unpack<T>(data: &[u8]) -> Option<T>
where
    T: Clone,
{
    let ret = unsafe { &*(&data[0] as *const u8 as *const T) };
    Some(ret.clone())
}
