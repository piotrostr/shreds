use log::{error, info, warn};
use serde_json::json;
use solana_entry::entry::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::mpsc;

use solana_ledger::shred::{
    layout, ReedSolomonCache, Shred, ShredId, Shredder,
};
use solana_sdk::clock::Slot;

use crate::shred::{
    deserialize_entries, deshred, get_coding_shred_header, get_fec_set_index,
    get_last_in_slot, get_shred_index, is_shred_data, CodingShredHeader,
};

pub const MAX_SHREDS_PER_SLOT: usize = 32_768 / 2;

pub struct FecSetSuccess {
    pub slot: Slot,
    pub fec_set_index: u32,
}

#[derive(Debug)]
struct FecSet {
    data_shreds: HashMap<u32, Arc<Vec<u8>>>,
    coding_shreds: HashMap<u32, Arc<Vec<u8>>>,
    num_expected_data: Option<u16>,
    num_expected_coding: Option<u16>,
    is_last_in_slot: bool,
    processed: bool,
}

#[derive(Debug)]
pub struct Processor {
    fec_sets: HashMap<(Slot, u32), FecSet>, // (slot, fec_set_index) -> FecSet
    uniqueness: HashSet<ShredId>,
    _handles: Vec<tokio::task::JoinHandle<()>>,
    entry_sender: mpsc::Sender<Vec<Entry>>,
    _error_sender: mpsc::Sender<String>,
    total_collected_data: u128,
    total_processed_data: u128,
    total_collected_coding: u128,
    fec_set_success: u128,
    fec_set_failure: u128,
}

impl Processor {
    pub fn new(
        entry_sender: mpsc::Sender<Vec<Entry>>,
        error_sender: mpsc::Sender<String>,
    ) -> Self {
        Processor {
            fec_sets: HashMap::new(),
            uniqueness: HashSet::new(),
            _handles: Vec::new(),
            entry_sender,
            _error_sender: error_sender,
            total_collected_data: 0,
            total_processed_data: 0,
            total_collected_coding: 0,
            fec_set_success: 0,
            fec_set_failure: 0,
        }
    }

    pub fn metrics(&self) -> String {
        let metrics = json!({
            "total_collected_data": self.total_collected_data,
            "total_collected_coding": self.total_collected_coding,
            "total_processed_data": self.total_processed_data,
            "fec_set_success_count": self.fec_set_success,
            "fec_set_failure_count": self.fec_set_failure,
            "fec_sets_remaining": self.fec_sets.len(),
            "fec_sets_summary": {
                "total_count": self.fec_sets.len(),
                "incomplete_count": self.fec_sets
                    .values()
                    .filter(|set| !Self::is_fec_set_complete(set)).count(),
            }
        });

        serde_json::to_string_pretty(&metrics)
            .unwrap_or_else(|_| "Error serializing metrics".to_string())
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
                processed: false,
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

            fec_set.data_shreds.len() == expected_data as usize
                || total_shreds >= total_expected
        } else {
            false
        }
    }

    async fn process_fec_set(&mut self, slot: Slot, fec_set_index: u32) {
        let fec_set = match self.fec_sets.get_mut(&(slot, fec_set_index)) {
            Some(set) => set,
            None => return,
        };

        if fec_set.processed {
            return;
        }

        let expected_data_shreds =
            fec_set.num_expected_data.unwrap_or(1) as usize;
        let mut data_shreds: Vec<Shred> = fec_set
            .data_shreds
            .values()
            .filter_map(|raw_shred| {
                Shred::new_from_serialized_shred(raw_shred.to_vec()).ok()
            })
            .collect();

        if data_shreds.len() < expected_data_shreds {
            let coding_shreds: Vec<Shred> = fec_set
                .coding_shreds
                .values()
                .filter_map(|raw_shred| {
                    Shred::new_from_serialized_shred(raw_shred.to_vec()).ok()
                })
                .collect();

            info!("Attempting to recover missing data shreds for slot {} FEC set {}", slot, fec_set_index);
            match Shredder::try_recovery(
                data_shreds
                    .iter()
                    .chain(coding_shreds.iter())
                    .cloned()
                    .collect(),
                &ReedSolomonCache::default(),
            ) {
                Ok(recovered_shreds) => {
                    info!(
                        "Recovered {} data shreds for slot {} FEC set {}",
                        recovered_shreds.len(),
                        slot,
                        fec_set_index
                    );
                    data_shreds.extend(
                        recovered_shreds.into_iter().filter(|s| s.is_data()),
                    );
                }
                Err(e) => {
                    warn!("Failed to recover data shreds for slot {} FEC set {}: {:?}", 
                    slot, fec_set_index, e);
                }
            }
        }

        if data_shreds.is_empty() {
            error!(
                "No valid data shreds found for slot {} FEC set {}",
                slot, fec_set_index
            );
            return;
        }

        data_shreds.sort_by_key(|shred| shred.index());
        let deshredded_data = deshred(&data_shreds);

        match deserialize_entries(&deshredded_data) {
            Ok(entries) => {
                self.fec_set_success += 1;
                self.total_processed_data += data_shreds.len() as u128;
                fec_set.processed = true;
                if let Err(e) = self.entry_sender.send(entries).await {
                    error!(
                        "Failed to send entries for slot {} FEC set {}: {:?}",
                        slot, fec_set_index, e
                    );
                }
            }
            Err(e) => {
                self.fec_set_failure += 1;
                error!("Failed to deserialize entries for slot {} FEC set {}: {:?}", 
                slot, fec_set_index, e);
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
                log::debug!("shreds: {} {}", timestamp, sig);
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

        info!("{}", processor.metrics());

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let pools_state = pools_state.read().await;
        info!(
            "Pools state: orca txs: {} raydium txs: {}",
            pools_state.orca_count, pools_state.raydium_amm_count,
        );
    }
}
