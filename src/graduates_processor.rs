use log::{error, info};
use solana_sdk::compute_budget;
use tokio::sync::mpsc;

use crate::entry_processor::EntriesWithMeta;

const PUMP_MIGRATION_PROGRAM: &str =
    "39azUYFWPz3VHgKCf3VChUwbpURdCHRxjWVowf5jUJjg";
const RAYDIUM_LP_PROGRAM: &str =
    "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8";

pub struct GraduatesProcessor {
    entry_rx: mpsc::Receiver<EntriesWithMeta>,
    error_rx: mpsc::Receiver<String>,
    sig_tx: mpsc::Sender<String>,
}

use solana_program::pubkey::Pubkey;
use solana_sdk::transaction::VersionedTransaction;

fn parse_transaction_details(tx: &VersionedTransaction) -> (Pubkey, Pubkey) {
    // Get the first instruction's program ID
    let program_id = match &tx.message {
        solana_sdk::message::VersionedMessage::Legacy(message) => message
            .instructions[0]
            .program_id(message.account_keys.as_slice()),
        solana_sdk::message::VersionedMessage::V0(message) => {
            &message.account_keys
                [message.instructions[0].program_id_index as usize]
        }
    };

    // Get the signer (first account in the account_keys)
    let signer = match &tx.message {
        solana_sdk::message::VersionedMessage::Legacy(message) => {
            message.account_keys[0]
        }
        solana_sdk::message::VersionedMessage::V0(message) => {
            message.account_keys[0]
        }
    };

    (*program_id, signer)
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
                let (program_id, signer) = parse_transaction_details(&tx);
                println!("Program ID: {}, signer: {}", program_id, signer);
                if program_id == compute_budget::id() {
                    info!("{:?}", tx);
                    self.sig_tx.send(signer.to_string()).await.unwrap();
                }
                // if program_id.to_string() == PUMP_MIGRATION_PROGRAM {
                //     info!("Pump migration detected");
                // } else if program_id.to_string() == RAYDIUM_LP_PROGRAM {
                //     info!("Raydium LP detected");
                // }
            }
        }
    }
}
