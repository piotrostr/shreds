use log::{debug, error, info, warn};
use solana_entry::entry::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::mpsc;

use solana_ledger::shred::{
    layout, ReedSolomonCache, Shred, ShredId, Shredder,
};
use solana_sdk::clock::Slot;

use crate::shred::{
    deserialize_entries, deshred, get_coding_shred_header,
    get_expected_coding_shreds, get_fec_set_index, get_last_in_slot,
    get_shred_index, is_shred_data, CodingShredHeader,
};

pub const MAX_SHREDS_PER_SLOT: usize = 32_768 / 2;

struct FecSetSuccess {
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
    success_sender: mpsc::Sender<FecSetSuccess>,
    success_receiver: mpsc::Receiver<FecSetSuccess>,
    total_collected_data: u128,
    total_processed_data: u128,
    total_collected_coding: u128,
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
            total_collected_data: 0,
            total_processed_data: 0,
            total_collected_coding: 0,
            success_sender,
            success_receiver,
        }
    }

    pub async fn cleanup_processed_batches(&mut self) {
        while let Ok(success) = self.success_receiver.try_recv() {
            let key = (success.slot, success.fec_set_index);
            if let Some(fec_set) = self.fec_sets.remove(&key) {
                debug!(
                    "Slot {} FEC set {} processed successfully",
                    success.slot, success.fec_set_index
                );
                self.total_processed_data +=
                    fec_set.data_shreds.len() as u128;
            }
        }
    }

    pub async fn insert(&mut self, slot: Slot, raw_shred: Arc<Vec<u8>>) {
        let is_data = is_shred_data(&raw_shred);
        let index = get_shred_index(&raw_shred).expect("get index");
        let fec_set_index =
            get_fec_set_index(&raw_shred).expect("get fec set index");

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
            self.total_collected_data += 1;
            fec_set.data_shreds.insert(index, raw_shred.clone());
            fec_set.is_last_in_slot |= get_last_in_slot(&raw_shred);
        } else {
            self.total_collected_coding += 1;
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
        if let Some(fec_set) = self.fec_sets.get(&(slot, fec_set_index)) {
            let data_shreds: Vec<Shred> = fec_set
                .data_shreds
                .values()
                .map(|raw_shred| {
                    Shred::new_from_serialized_shred(raw_shred.to_vec())
                        .unwrap()
                })
                .collect();

            // If we're missing data shreds, we should attempt to reconstruct them here
            // using the coding shreds. For now, we'll just log a warning.
            let mut data_shreds =
                if let Some(expected_data) = fec_set.num_expected_data {
                    if data_shreds.len() < expected_data as usize {
                        warn!(
                            "Missing {} data shreds in FEC set {}",
                            expected_data as usize - data_shreds.len(),
                            fec_set_index
                        );
                        let coding_shreds: Vec<Shred> = fec_set
                            .coding_shreds
                            .values()
                            .map(|raw_shred| {
                                Shred::new_from_serialized_shred(
                                    raw_shred.to_vec(),
                                )
                                .unwrap()
                            })
                            .collect();
                        match Shredder::try_recovery(
                            data_shreds
                                .iter()
                                .chain(coding_shreds.iter())
                                .cloned()
                                .collect::<Vec<_>>(),
                            &ReedSolomonCache::default(),
                        ) {
                            Ok(recovered_shreds) => {
                                info!(
                                    "Recovered {} data shreds in FEC set {}",
                                    recovered_shreds.len(),
                                    fec_set_index
                                );
                                recovered_shreds
                            }
                            Err(e) => {
                                error!("Failed to repair shreds: {}", e);
                                data_shreds
                            }
                        }
                    } else {
                        data_shreds
                    }
                } else {
                    data_shreds
                };

            if let Some(expected_data) = fec_set.num_expected_data {
                if data_shreds.len() < expected_data as usize {
                    error!(
                        "Failed to recover data shreds for slot {} FEC set {}",
                        slot, fec_set_index
                    );
                    return;
                }
            }

            data_shreds.sort_by_key(|shred| shred.index());

            let deshredded_data = deshred(&data_shreds);
            match deserialize_entries(&deshredded_data) {
                Ok(entries) => {
                    if let Err(e) = self.entry_sender.send(entries).await {
                        error!("Failed to send entries: {:?}", e);
                    }
                    if let Err(e) = self
                        .success_sender
                        .send(FecSetSuccess {
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
                debug!("shreds: {} {}", timestamp, sig);
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

        info!("Total collected data: {}", processor.total_collected_data);
        info!(
            "Total collected coding: {}",
            processor.total_collected_coding
        );
        info!("Total processed (data): {}", processor.total_processed_data);
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
