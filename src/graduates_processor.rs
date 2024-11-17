use log::{error, info};
use solana_program::pubkey::Pubkey;
use solana_sdk::message::VersionedMessage;
use solana_sdk::transaction::VersionedTransaction;
use std::str::FromStr;
use tokio::sync::mpsc;

use crate::entry_processor::EntriesWithMeta;

const PUMP_MIGRATION_PROGRAM: &str =
    "39azUYFWPz3VHgKCf3VChUwbpURdCHRxjWVowf5jUJjg";
const RAYDIUM_LP_PROGRAM: &str =
    "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8";

lazy_static::lazy_static! {
    static ref PUMP_MIGRATION_PUBKEY: Pubkey = Pubkey::from_str(PUMP_MIGRATION_PROGRAM).unwrap();
    static ref RAYDIUM_LP_PUBKEY: Pubkey = Pubkey::from_str(RAYDIUM_LP_PROGRAM).unwrap();
}

pub struct GraduatesProcessor {
    entry_rx: mpsc::Receiver<EntriesWithMeta>,
    error_rx: mpsc::Receiver<String>,
    sig_tx: mpsc::Sender<String>,
}

fn filter_transaction(transaction: &VersionedTransaction) -> bool {
    if let VersionedMessage::V0(message) = &transaction.message {
        // Check if the transaction is signed by PUMP_MIGRATION_PROGRAM
        let is_signed_by_pump = message
            .account_keys
            .iter()
            .any(|key| key == &*PUMP_MIGRATION_PUBKEY);

        // Check if any instruction uses RAYDIUM_LP_PROGRAM
        let uses_raydium = message.instructions.iter().any(|instruction| {
            let program_id =
                &message.account_keys[instruction.program_id_index as usize];
            program_id == &*RAYDIUM_LP_PUBKEY
        });

        return is_signed_by_pump && uses_raydium;
    }
    false
}

impl GraduatesProcessor {
    pub fn new(
        entry_rx: mpsc::Receiver<EntriesWithMeta>,
        error_rx: mpsc::Receiver<String>,
        sig_tx: mpsc::Sender<String>,
    ) -> Self {
        Self {
            entry_rx,
            error_rx,
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
        for entry in entries_with_meta.entries {
            for tx in entry.transactions {
                if filter_transaction(&tx) {
                    info!("Found matching transaction: {:?}", tx.signatures);
                    if let Some(sig) = tx.signatures.first() {
                        if let Err(e) =
                            self.sig_tx.send(sig.to_string()).await
                        {
                            error!("Failed to send signature: {}", e);
                        }
                    }
                }
            }
        }
    }
}

