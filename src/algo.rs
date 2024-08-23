use std::str::FromStr;

use log::{error, info};
use solana_entry::entry::Entry;
use solana_sdk::pubkey::Pubkey;
use tokio::sync::mpsc;

pub const WHIRLPOOL: &str = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc";

pub async fn receive_entries(
    mut entry_receiver: mpsc::Receiver<Vec<Entry>>,
    mut error_receiver: mpsc::Receiver<String>,
) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(entries) = entry_receiver.recv() => {
                    process_entries_batch(entries);
                }
                Some(error) = error_receiver.recv() => {
                    error!("{}", error);
                }
            }
        }
    });
}

pub fn process_entries_batch(entries: Vec<Entry>) {
    info!(
        "OK: entries {} txs: {}",
        entries.len(),
        entries.iter().map(|e| e.transactions.len()).sum::<usize>(),
    );
    for entry in entries {
        for tx in entry.transactions {
            if tx.message.static_account_keys().contains(
                &Pubkey::from_str(WHIRLPOOL).expect("Failed to parse pubkey"),
            ) {
                info!("OK: Found whirlpool tx {:#?}", tx);
            };
        }
    }
}
