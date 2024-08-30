use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

#[derive(
    BorshDeserialize,
    BorshSerialize,
    Serialize,
    Deserialize,
    Clone,
    Default,
    Copy,
)]
pub struct PumpCreateIx {
    pub method_id: [u8; 8],
    pub name: String,
    pub symbol: String,
    pub uri: String,
}

impl std::fmt::Debug for PumpCreateIx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PumpCreateIx")
            .field("name", &self.name)
            .field("symbol", &self.symbol)
            .field("uri", &self.uri)
            .finish()
    }
}

#[derive(
    BorshDeserialize,
    BorshSerialize,
    Serialize,
    Deserialize,
    Clone,
    Default,
    Copy,
)]
pub struct PumpSwapIx {
    pub method_id: [u8; 8],
    pub amount: u64,
    pub max_sol_cost: u64,
}

impl std::fmt::Debug for PumpSwapIx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PumpSwapIx")
            .field("amount", &self.amount)
            .field("max_sol_cost", &self.max_sol_cost)
            .finish()
    }
}
