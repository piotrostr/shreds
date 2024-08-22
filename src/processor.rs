use log::{debug, error, info};
use solana_entry::entry::Entry;
use solana_sdk::transaction::VersionedTransaction;
use std::collections::{HashMap, HashSet};

use solana_ledger::shred::{layout, Shred, ShredId};
use solana_sdk::clock::Slot;

use crate::listener::PACKET_SIZE;
use crate::shred::{
    deserialize_entries, deshred, get_shred_index, get_shred_is_last,
};
use crate::structs::ShredVariant;

type PacketBuffer = [u8; PACKET_SIZE];
type ShredsBuffer = [[u8; PACKET_SIZE]; MAX_SHREDS_PER_SLOT];

// 65k tps at 32768 shreds per slot, dividing by 2 to save mem
pub const MAX_SHREDS_PER_SLOT: usize = 32_768 / 2;

fn get_empty_packet_buffer() -> PacketBuffer {
    [0; PACKET_SIZE]
}
fn get_empty_shreds_buffer() -> ShredsBuffer {
    [get_empty_packet_buffer(); MAX_SHREDS_PER_SLOT]
}

/// processor uses a special data structure setup to handle full blocks in real-time,
/// TODO complexity
/// this should be a sorted map with a hash set for uniqueness within the vectors
/// keep the shreds sorted by slot
/// now checking for sortedness is O(n), but since we get the information about the
/// data completedness, once we know that the last slot is in, and the map is sorted
/// at all times, it is only about checking the last slot index and total length
/// on each new entry
/// the shreds_by_slot map is accessible through a method, so it is possible to add a callback to
/// check if the slot has the final shred and then compare from the moment flag is set
/// also, those maps and the uniqueness set should be cleared after the slot is processed in
/// handle_slot to free up memory (otherwise it will grow indefinitely)
#[derive(Debug, Default)]
pub struct Processor {
    data_shreds_by_slot: HashMap<Slot, ShredsBuffer>,
    code_shreds_by_slot: HashMap<Slot, ShredsBuffer>,

    data_shreds_counter: HashMap<Slot, u32>,
    code_shreds_counter: HashMap<Slot, u32>,
    uniqueness: HashSet<ShredId>,
    last_shred_index: HashMap<Slot, u32>,

    transactions: HashMap<Slot, Vec<VersionedTransaction>>,
}

impl Processor {
    pub fn new() -> Self {
        Processor {
            data_shreds_by_slot: HashMap::new(),
            code_shreds_by_slot: HashMap::new(),
            data_shreds_counter: HashMap::new(),
            code_shreds_counter: HashMap::new(),
            uniqueness: HashSet::new(),
            last_shred_index: HashMap::new(),
            transactions: HashMap::new(),
        }
    }

    pub async fn insert(&mut self, slot: Slot, raw_shred: Vec<u8>) {
        let variant_raw = raw_shred.get(0x40).expect("grab variant");
        let variant =
            ShredVariant::try_from(*variant_raw).expect("parse variant");
        let is_code = matches!(variant, ShredVariant::MerkleCode { .. })
            || matches!(variant, ShredVariant::LegacyCode { .. });
        let is_last = get_shred_is_last(&raw_shred).expect("is last check");
        let shred_index = get_shred_index(&raw_shred).expect("get index");
        // push the shred to the appropriate map
        if is_code {
            self.code_shreds_by_slot
                .entry(slot)
                .or_insert_with(get_empty_shreds_buffer);
            self.code_shreds_by_slot.entry(slot).and_modify(|v| {
                v[shred_index as usize] =
                    raw_shred.as_slice().try_into().unwrap()
            });
            self.code_shreds_counter.entry(slot).and_modify(|v| *v += 1);
        } else {
            self.data_shreds_by_slot
                .entry(slot)
                .or_insert_with(get_empty_shreds_buffer);
            self.data_shreds_by_slot.entry(slot).and_modify(|v| {
                v[shred_index as usize] =
                    raw_shred.as_slice().try_into().unwrap()
            });
            self.data_shreds_counter.entry(slot).and_modify(|v| *v += 1);
        }
        // insert the last index if this is the last shred
        if is_last {
            self.last_shred_index.insert(slot, shred_index);
        }

        // regardless of the last shred, check if the slot is complete
        // (data might have not all came in but the final shred is there, we wait for more data in
        // that case shreds too)
        let last_index = self.last_shred_index.get(&slot);
        if last_index.is_some()
            && *self.data_shreds_counter.get(&slot).unwrap()
                == *last_index.unwrap()
        {
            // get populated chunk (only part of the allocated buffer)
            let raw_shreds = self.data_shreds_by_slot.remove(&slot).unwrap()
                [..*last_index.unwrap() as usize]
                .to_vec();
            // process shreds if this is the last one
            // send this through channel rather than spwaning thread in general
            tokio::spawn({
                async move {
                    let _ = handle_slot(&raw_shreds).await;
                }
            });
        }
    }

    // TODO it might make sense to process shreds in batches,
    // looking at reference tick too
    pub async fn collect(&mut self, raw_shred: Vec<u8>) {
        // check if it is at least the common header size
        // i suspect shredstream also sends like ping udps, some packets are 29 bytes
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

/// handle_slot should only be called when all shreds for a slot are received
/// and there are no missing shreds
pub async fn handle_slot(
    raw_shreds: &[[u8; 1232]],
) -> Result<Vec<Entry>, Box<dyn std::error::Error>> {
    let shreds = raw_shreds
        .iter()
        .map(|raw_shred| {
            Shred::new_from_serialized_shred(raw_shred.to_vec()).unwrap()
        })
        .collect::<Vec<_>>();

    assert!(!shreds.is_empty());

    // check if all are data
    assert!(shreds.iter().all(|shred| shred.is_data()));

    // check if all are aligned
    let index = shreds.first().expect("first shred").index();
    assert!(shreds.iter().zip(index..).all(|(s, i)| s.index() == i));

    // check if data complete
    let last = shreds.iter().last().expect("last shred");
    assert!(last.last_in_slot() || last.data_complete());

    // process shreds
    let deshredded_data = deshred(&shreds);
    debug!("Deshredded data size: {}", deshredded_data.len());
    match deserialize_entries(&deshredded_data) {
        Ok(entries) => {
            info!("Successfully deserialized {} entries", entries.len());
            Ok(entries)
        }
        Err(e) => {
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
        }
    }
}
