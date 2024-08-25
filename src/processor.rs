use log::{error, info, warn};
use solana_entry::entry::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::mpsc;

use solana_ledger::shred::{layout, Shred, ShredId};
use solana_sdk::clock::Slot;

use crate::shred::{
    deserialize_entries, deshred, get_coding_shred_header, get_fec_set_index,
    get_last_in_slot, get_shred_index, is_shred_data, CodingShredHeader,
};

pub const MAX_SHREDS_PER_SLOT: usize = 32_768 / 2;

struct BatchSuccess {
    slot: Slot,
    fec_set_index: u32,
}

#[derive(Debug)]
struct FecSet {
    data_shreds: HashMap<u32, Arc<Vec<u8>>>,
    coding_shreds: HashMap<u32, Arc<Vec<u8>>>,
    num_expected_data: Option<u16>,
    num_expected_coding: Option<u16>,
    is_last_in_slot: bool,
}

#[derive(Debug)]
pub struct Processor {
    fec_sets: HashMap<(Slot, u32), FecSet>, // (slot, fec_set_index) -> FecSet
    uniqueness: HashSet<ShredId>,
    _handles: Vec<tokio::task::JoinHandle<()>>,
    entry_sender: mpsc::Sender<Vec<Entry>>,
    error_sender: mpsc::Sender<String>,
    success_sender: mpsc::Sender<BatchSuccess>,
    success_receiver: mpsc::Receiver<BatchSuccess>,
    total_collected: u128,
    total_processed: u128,
}

impl Processor {
    pub fn new(
        entry_sender: mpsc::Sender<Vec<Entry>>,
        error_sender: mpsc::Sender<String>,
    ) -> Self {
        let (success_sender, success_receiver) = mpsc::channel(2000);
        Processor {
            fec_sets: HashMap::new(),
            uniqueness: HashSet::new(),
            _handles: Vec::new(),
            entry_sender,
            error_sender,
            total_collected: 0,
            total_processed: 0,
            success_sender,
            success_receiver,
        }
    }

    pub async fn cleanup_processed_batches(&mut self) {
        while let Ok(success) = self.success_receiver.try_recv() {
            let key = (success.slot, success.fec_set_index);
            if let Some(fec_set) = self.fec_sets.remove(&key) {
                info!(
                    "Slot {} FEC set {} processed successfully",
                    success.slot, success.fec_set_index
                );
                self.total_processed += fec_set.data_shreds.len() as u128
                    + fec_set.coding_shreds.len() as u128;
            }
        }
    }

    pub async fn insert(&mut self, slot: Slot, raw_shred: Arc<Vec<u8>>) {
        let is_data = is_shred_data(&raw_shred);
        let index = get_shred_index(&raw_shred).expect("get index");
        let fec_set_index =
            get_fec_set_index(&raw_shred).expect("get fec set index");

        self.total_collected += 1;

        let fec_set = self
            .fec_sets
            .entry((slot, fec_set_index))
            .or_insert_with(|| FecSet {
                data_shreds: HashMap::new(),
                coding_shreds: HashMap::new(),
                num_expected_data: None,
                num_expected_coding: None,
                is_last_in_slot: false,
            });

        if is_data {
            fec_set.data_shreds.insert(index, raw_shred.clone());
            fec_set.is_last_in_slot |= get_last_in_slot(&raw_shred);
        } else {
            fec_set.coding_shreds.insert(index, raw_shred.clone());
            // Update expected counts from coding shred
            if fec_set.num_expected_data.is_none()
                || fec_set.num_expected_coding.is_none()
            {
                if let Ok(CodingShredHeader {
                    num_data_shreds,
                    num_coding_shreds,
                    ..
                }) = get_coding_shred_header(&raw_shred)
                {
                    fec_set.num_expected_data = Some(num_data_shreds);
                    fec_set.num_expected_coding = Some(num_coding_shreds);
                }
            }
        }

        if Self::is_fec_set_complete(fec_set) {
            self.process_fec_set(slot, fec_set_index).await;
        }
    }

    fn is_fec_set_complete(fec_set: &FecSet) -> bool {
        if let (Some(expected_data), Some(expected_coding)) =
            (fec_set.num_expected_data, fec_set.num_expected_coding)
        {
            let total_shreds =
                fec_set.data_shreds.len() + fec_set.coding_shreds.len();
            let total_expected =
                expected_data as usize + expected_coding as usize;

            // We consider the set complete if:
            // 1. We have all the expected data shreds, or
            // 2. We have enough total shreds to reconstruct the missing data shreds
            fec_set.data_shreds.len() == expected_data as usize
                || total_shreds >= total_expected
        } else {
            // If we don't know the expected counts yet, we can't consider the set complete
            false
        }
    }

    async fn process_fec_set(&mut self, slot: Slot, fec_set_index: u32) {
        if let Some(fec_set) = self.fec_sets.remove(&(slot, fec_set_index)) {
            let mut shreds: Vec<Shred> = fec_set
                .data_shreds
                .values()
                .map(|raw_shred| {
                    Shred::new_from_serialized_shred(raw_shred.to_vec())
                        .unwrap()
                })
                .collect();

            // If we're missing data shreds, we should attempt to reconstruct them here
            // using the coding shreds. For now, we'll just log a warning.
            if let Some(expected_data) = fec_set.num_expected_data {
                if shreds.len() < expected_data as usize {
                    warn!(
                        "Missing {} data shreds in FEC set {}",
                        expected_data as usize - shreds.len(),
                        fec_set_index
                    );
                    // TODO: Implement reconstruction of missing data shreds
                }
            }

            shreds.sort_by_key(|shred| shred.index());

            let deshredded_data = deshred(&shreds);
            match deserialize_entries(&deshredded_data) {
                Ok(entries) => {
                    if let Err(e) = self.entry_sender.send(entries).await {
                        error!("Failed to send entries: {:?}", e);
                    }
                    if let Err(e) = self
                        .success_sender
                        .send(BatchSuccess {
                            slot,
                            fec_set_index,
                        })
                        .await
                    {
                        error!("Failed to send success: {:?}", e);
                    }
                }
                Err(e) => {
                    if let Err(e) = self
                        .error_sender
                        .send(format!(
                            "{}-{} total: {} {:?}",
                            slot,
                            fec_set.data_shreds.len()
                                + fec_set.coding_shreds.len(),
                            fec_set_index,
                            e,
                        ))
                        .await
                    {
                        error!("Failed to send error: {:?}", e);
                    }
                }
            }
        }
    }

    pub async fn collect(&mut self, raw_shred: Arc<Vec<u8>>) {
        if raw_shred.len() < 0x58 {
            return;
        }
        match layout::get_shred_id(&raw_shred) {
            Some(shred_id) => {
                if !self.uniqueness.insert(shred_id) {
                    return;
                }
                self.insert(shred_id.slot(), raw_shred.clone()).await;
            }
            None => {
                error!("Error getting shred id");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::algo::{self, PoolsState};
    use log::info;
    use tokio::sync::RwLock;

    #[tokio::test]
    async fn processor_works() {
        dotenv::dotenv().ok();
        env_logger::Builder::default()
            .format_module_path(false)
            .filter_level(log::LevelFilter::Info)
            .init();

        let data = std::fs::read_to_string("packets.json")
            .expect("Failed to read packets.json");
        let raw_shreds: Vec<Vec<u8>> =
            serde_json::from_str(&data).expect("Failed to parse JSON");

        let (entry_sender, entry_receiver) = mpsc::channel(2000);
        let (error_sender, error_receiver) = mpsc::channel(2000);
        let (sig_sender, mut sig_receiver) = mpsc::channel(2000);

        tokio::spawn(async move {
            while let Some(sig) = sig_receiver.recv().await {
                let timestamp = chrono::Utc::now().timestamp_millis();
                info!("shreds: {} {}", timestamp, sig);
            }
        });

        let mut processor = Processor::new(entry_sender, error_sender);
        for raw_shred in raw_shreds {
            processor.collect(Arc::new(raw_shred)).await;
        }

        let pools_state = Arc::new(RwLock::new(PoolsState::default()));

        algo::receive_entries(
            pools_state.clone(),
            entry_receiver,
            error_receiver,
            Arc::new(sig_sender),
        )
        .await;

        for handle in processor._handles.drain(..) {
            handle.await.expect("Failed to process batch");
        }

        info!("Cleaning up processed batches");
        processor.cleanup_processed_batches().await;

        info!("Total collected: {}", processor.total_collected);
        info!("Total processed: {}", processor.total_processed);
        processor.fec_sets.iter().for_each(|((slot, fec_set_index), fec_set)| {
            info!(
                "Slot {} FEC set {} - Data: {}/{:?}, Coding: {}/{:?}, Last in slot: {}",
                slot,
                fec_set_index,
                fec_set.data_shreds.len(),
                fec_set.num_expected_data,
                fec_set.coding_shreds.len(),
                fec_set.num_expected_coding,
                fec_set.is_last_in_slot
            )
        });

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let pools_state = pools_state.read().await;
        info!(
            "Pools state: orca txs: {} raydium txs: {}",
            pools_state.orca_count, pools_state.raydium_amm_count,
        );
    }
}
