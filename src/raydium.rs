use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use log::{info, warn};
use raydium_library::amm::AmmKeys;
use reqwest::Client;
use serde_json::Value;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use raydium_amm::instruction::{
    AdminCancelOrdersInstruction, ConfigArgs, DepositInstruction,
    InitializeInstruction, InitializeInstruction2, MonitorStepInstruction,
    PreInitializeInstruction, SetParamsInstruction, SimulateInstruction,
    SwapInstructionBaseIn, SwapInstructionBaseOut, WithdrawInstruction,
    WithdrawSrmInstruction,
};
use solana_program::program_error::ProgramError;

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

// this takes long, possibly could make it so that it uses a search index it
// returns all of the
// pools for a given token, to be filtered later for relevant
pub fn parse_raydium_json(
    jsonstr: Arc<String>,
    mints_of_interest: Vec<Pubkey>,
) -> Result<HashMap<Pubkey, Vec<AmmKeys>>, Box<dyn std::error::Error>> {
    let mut result = HashMap::new();
    let all: Value = serde_json::from_str(&jsonstr).unwrap();
    info!("Parsing relevant pools");

    let pools = all["unOfficial"].as_array().unwrap();
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
            let amm_keys = json_to_amm_keys(json);
            result.entry(base_mint).or_insert(vec![]).push(amm_keys);
        } else if mints_of_interest.contains(&quote_mint) {
            let amm_keys = json_to_amm_keys(json);
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

pub fn json_to_amm_keys(json: &Value) -> AmmKeys {
    AmmKeys {
        amm_pool: Pubkey::from_str(json["id"].as_str().unwrap()).unwrap(),
        amm_coin_mint: Pubkey::from_str(json["baseMint"].as_str().unwrap())
            .unwrap(),
        amm_pc_mint: Pubkey::from_str(json["quoteMint"].as_str().unwrap())
            .unwrap(),
        amm_authority: Pubkey::from_str(json["authority"].as_str().unwrap())
            .unwrap(),
        amm_target: Pubkey::from_str(json["targetOrders"].as_str().unwrap())
            .unwrap(),
        amm_coin_vault: Pubkey::from_str(json["baseVault"].as_str().unwrap())
            .unwrap(),
        amm_pc_vault: Pubkey::from_str(json["quoteVault"].as_str().unwrap())
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
        market: Pubkey::from_str(json["marketId"].as_str().unwrap()).unwrap(),
        nonce: u8::default(), // not relevant
    }
}
