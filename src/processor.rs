use log::{debug, error, info};
use solana_entry::entry::Entry;
use std::collections::{HashMap, HashSet};

use solana_ledger::shred::{layout, Shred, ShredId};
use solana_sdk::clock::Slot;

use crate::shred::{
    deserialize_entries, deshred, get_shred_data_flags, get_shred_index,
};
use crate::structs::ShredVariant;

pub const MAX_SHREDS_PER_SLOT: usize = 32_768 / 2;

#[derive(Debug)]
struct BatchInfo {
    shreds: HashMap<u32, Vec<u8>>,
    highest_index: u32,
    lowest_index: u32,
    is_last_shred: bool,
}

#[derive(Debug, Default)]
pub struct Processor {
    data_shreds: HashMap<Slot, HashMap<u8, BatchInfo>>,
    uniqueness: HashSet<ShredId>,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl Processor {
    pub fn new() -> Self {
        Processor {
            data_shreds: HashMap::new(),
            uniqueness: HashSet::new(),
            handles: Vec::new(),
        }
    }

    pub async fn insert(&mut self, slot: Slot, raw_shred: Vec<u8>) {
        let variant_raw = raw_shred.get(0x40).expect("grab variant");
        let variant =
            ShredVariant::try_from(*variant_raw).expect("parse variant");
        let is_code = matches!(variant, ShredVariant::MerkleCode { .. })
            || matches!(variant, ShredVariant::LegacyCode { .. });
        let shred_index =
            get_shred_index(&raw_shred).expect("get index") as u32;
        let (_, batch_complete, batch_tick) =
            get_shred_data_flags(&raw_shred);

        if !is_code {
            let batch_info = self
                .data_shreds
                .entry(slot)
                .or_default()
                .entry(batch_tick)
                .or_insert_with(|| BatchInfo {
                    shreds: HashMap::new(),
                    highest_index: 0,
                    lowest_index: u32::MAX,
                    is_last_shred: false,
                });

            batch_info.shreds.insert(shred_index, raw_shred);
            batch_info.highest_index =
                batch_info.highest_index.max(shred_index);
            batch_info.lowest_index =
                batch_info.lowest_index.min(shred_index);

            if batch_complete {
                batch_info.is_last_shred = true;
                self.process_batch(slot, batch_tick).await;
            }
        }
    }

    async fn process_batch(&mut self, slot: Slot, batch_tick: u8) {
        if let Some(slot_map) = self.data_shreds.get_mut(&slot) {
            if let Some(batch_info) = slot_map.get_mut(&batch_tick) {
                if batch_info.is_last_shred && is_batch_ready(batch_info) {
                    debug!("Sending Slot {} Batch {}", slot, batch_tick);
                    let batch_shreds = std::mem::take(&mut batch_info.shreds);
                    // let transactions = self.transactions.clone();
                    let handle = tokio::spawn({
                        async move {
                            let entries = handle_batch(
                                batch_shreds.into_values().collect(),
                            )
                            .await;
                            if let Ok(entries) = entries {
                                let new_transactions = entries
                                    .iter()
                                    .flat_map(|entry| {
                                        entry.transactions.clone()
                                    })
                                    .collect::<Vec<_>>();
                                info!(
                                    "Batch {}-{} has {} txs",
                                    slot,
                                    batch_tick,
                                    new_transactions.len(),
                                );
                                debug!(
                                    "Transactions: {:#?}",
                                    new_transactions
                                        .iter()
                                        .flat_map(|t| t.signatures.clone())
                                        .collect::<Vec<_>>()
                                );
                                drop(entries);
                                drop(new_transactions);
                            }
                        }
                    });
                    self.handles.push(handle);

                    // Remove the processed batch
                    slot_map.remove(&batch_tick);

                    // If the slot map is empty, remove it as well
                    if slot_map.is_empty() {
                        self.data_shreds.remove(&slot);
                    }
                } else {
                    debug!(
                        "Slot {} Batch {} is not ready for processing",
                        slot, batch_tick
                    );
                }
            }
        }
    }

    pub async fn collect(&mut self, raw_shred: Vec<u8>) {
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

fn is_batch_ready(batch_info: &BatchInfo) -> bool {
    if !batch_info.is_last_shred {
        return false;
    }

    let expected_count =
        batch_info.highest_index - batch_info.lowest_index + 1;
    batch_info.shreds.len() as u32 == expected_count
}

#[timed::timed(duration(printer = "info!"))]
pub async fn handle_batch(
    raw_shreds: Vec<Vec<u8>>,
) -> Result<Vec<Entry>, Box<dyn std::error::Error>> {
    debug!("Processing batch with {} shreds", raw_shreds.len());

    let mut shreds = raw_shreds
        .into_iter()
        .map(|raw_shred| Shred::new_from_serialized_shred(raw_shred).unwrap())
        .collect::<Vec<_>>();

    shreds.sort_by_key(|shred| shred.index());

    assert!(!shreds.is_empty());
    assert!(shreds.iter().all(|shred| shred.is_data()));

    // Check if batch complete
    let last = shreds.last().expect("last shred");
    assert!(last.data_complete());

    // Process shreds
    let deshredded_data = deshred(&shreds);
    debug!("Deshredded data size: {}", deshredded_data.len());
    match deserialize_entries(&deshredded_data) {
        Ok(entries) => {
            debug!("Successfully deserialized {} entries", entries.len());
            Ok(entries)
        }
        Err(e) => {
            // let debug_shreds = shreds
            //     .iter()
            //     .map(|s| get_shred_debug_string(s.clone()))
            //     .collect::<Vec<_>>();
            error!("Failed to deserialize entries: {:?}", e);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[tokio::test]
    async fn processor_works() {
        std::env::set_var("RUST_LOG", "info");
        env_logger::init();

        let data = std::fs::read_to_string("packets.json")
            .expect("Failed to read packets.json");
        let raw_shreds: Vec<Vec<u8>> =
            serde_json::from_str(&data).expect("Failed to parse JSON");

        let mut processor = Processor::new();
        for chunk in raw_shreds.chunks(100) {
            for raw_shred in chunk {
                processor.collect(raw_shred.to_vec()).await;
            }
            for handle in processor.handles.drain(..) {
                handle.await.expect("Failed to process batch");
            }
        }
    }
}
