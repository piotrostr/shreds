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
    if input.len() >= 1 {
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
