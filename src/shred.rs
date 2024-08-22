use anyhow::Result;
use std::collections::{HashMap, HashSet};

use crate::structs::ShredVariant;
use log::{debug, error, info, warn};
use solana_entry::entry::Entry;
use solana_ledger::shred::{Error, Shred};
use solana_sdk::signature::SIGNATURE_BYTES;

pub fn debug_shred(shred: Shred) {
    let size_in_header =
        u16::from_le_bytes([shred.payload()[0x56], shred.payload()[0x57]]) as usize;
    info!(
        "shred: index: {}: payload: {}, size in header: {} zero freq: {} variant: {:?}",
        shred.index(),
        shred.payload().len(),
        size_in_header,
        shred.payload().iter().filter(|&&b| b == 0).count(),
        get_shred_variant(shred.payload()).expect("shred variant"),
    );
}

pub fn get_shred_variant(shred: &[u8]) -> Result<ShredVariant, Error> {
    let Some(&shred_variant) = shred.get(OFFSET_OF_SHRED_VARIANT) else {
        return Err(Error::InvalidPayloadSize(shred.len()));
    };
    ShredVariant::try_from(shred_variant).map_err(|_| Error::InvalidShredVariant)
}

pub fn deserialize_shred(data: Vec<u8>) -> Result<Shred, Error> {
    Shred::new_from_serialized_shred(data)
}

pub fn deserialize_entries(payload: &[u8]) -> Result<Vec<Entry>, bincode::Error> {
    if payload.len() < 8 {
        error!("Payload too short: {} bytes", payload.len());
        return Ok(Vec::new());
    }

    let entry_count = u64::from_le_bytes(payload[0..8].try_into().expect("entry count parse"));
    debug!("Entry count prefix: {}", entry_count);
    debug!("First 16 bytes of payload: {:?}", &payload[..16]);

    // Try to deserialize each entry individually
    let mut entries = Vec::new();
    let mut cursor = std::io::Cursor::new(&payload[8..]);
    for i in 0..entry_count {
        match bincode::deserialize_from::<_, Entry>(&mut cursor) {
            Ok(entry) => {
                entries.push(entry);
            }
            Err(e) => {
                error!("Failed to deserialize entry {}: {}", i, e);
            }
        }
    }

    Ok(entries)
}

const OFFSET_OF_SHRED_VARIANT: usize = SIGNATURE_BYTES;

pub fn shred_data(shred: &Shred) -> Result<&[u8], Error> {
    let variant = ShredVariant::try_from(shred.payload()[OFFSET_OF_SHRED_VARIANT])?;
    let (data_start, size) = match variant {
        ShredVariant::MerkleData { .. } => {
            let size = u16::from_le_bytes([shred.payload()[0x56], shred.payload()[0x57]]) as usize;
            (0x58usize, size.saturating_sub(0x58))
        }
        ShredVariant::LegacyData => (0x56, shred.payload().len().saturating_sub(0x56)),
        _ => return Err(Error::InvalidShredVariant),
    };

    let data_end = data_start.saturating_add(size);
    if data_end > shred.payload().len() {
        return Err(Error::InvalidPayloadSize(shred.payload().len()));
    }
    Ok(&shred.payload()[data_start..data_end])
}

pub fn load_shreds(raw_shreds: Vec<Vec<u8>>) -> HashMap<u64, Vec<Shred>> {
    // TODO group the shreds by slot here but beforehand, deduplicate and perform repair using the
    // code shreds to ensure a full block
    // let coding_shreds = Vec::new();
    let mut shreds_by_slot: HashMap<u64, Vec<Shred>> = HashMap::new();
    for raw_shred in raw_shreds {
        if raw_shred.len() == 29 {
            continue;
        }
        let shred = Shred::new_from_serialized_shred(raw_shred).expect("new shred");
        shreds_by_slot.entry(shred.slot()).or_default().push(shred);
    }
    shreds_by_slot
}

pub fn preprocess_shreds(shreds: Vec<Shred>) -> (Vec<Shred>, Vec<Shred>) {
    // split shreds into data and code shreds, coding are only used for recovery
    // only data shreds are later decoded
    let mut data_shreds = Vec::new();
    let mut code_shreds = Vec::new();
    for shred in shreds {
        if shred.is_data() {
            data_shreds.push(shred);
        } else if shred.is_code() {
            code_shreds.push(shred);
        }
    }
    // deduplicate data_shreads and sort by key
    let mut seen = HashSet::new();
    data_shreds.retain(|shred| seen.insert(shred.index()));
    data_shreds.sort_by_key(|shred| shred.index());
    (data_shreds, code_shreds)
}

pub fn debug_shred_sizes(raw_shreds: Vec<Vec<u8>>) {
    let mut shred_sizes = HashMap::new();
    for shred in raw_shreds.iter() {
        *shred_sizes.entry(shred.len()).or_insert(0) += 1;
    }
    info!("shred sizes {:?}", shred_sizes);
}

pub fn deshred(data_shreds: &[Shred]) -> Vec<u8> {
    data_shreds
        .iter()
        .flat_map(|shred| {
            shred_data(shred)
                .map(|data| data.to_vec())
                .unwrap_or_default()
        })
        .collect()
}

pub fn validate_and_try_repair(
    data_shreds: &[Shred],
    code_shreds: &[Shred],
) -> Result<Vec<Shred>, Box<dyn std::error::Error>> {
    let index = data_shreds.first().expect("first shred").index();
    let aligned = data_shreds.iter().zip(index..).all(|(s, i)| s.index() == i);
    if !aligned {
        // find the missing indices
        let mut missing_indices = Vec::new();
        let mut expected_index = index;
        for shred in data_shreds.iter() {
            while expected_index < shred.index() {
                missing_indices.push(expected_index);
                expected_index += 1;
            }
            expected_index += 1;
        }
        warn!("Missing indices: {:?}, trying to repair", missing_indices);
        info!("code shreds len: {}", code_shreds.len());
        // TODO repair here
    }
    let aligned = data_shreds.iter().zip(index..).all(|(s, i)| s.index() == i);
    let data_complete = {
        let shred = data_shreds.last().expect("last shred");
        shred.data_complete() || shred.last_in_slot()
    };
    if !aligned || !data_complete {
        return Err("Shreds are not aligned or data is not complete".into());
    }

    Ok(data_shreds.to_vec())
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn deserialize_shreds() {
        std::env::set_var("RUST_LOG", "info");
        env_logger::init();

        let data = std::fs::read_to_string("packets.json").expect("Failed to read packets.json");
        let raw_shreds: Vec<Vec<u8>> = serde_json::from_str(&data).expect("Failed to parse JSON");

        // Group shreds by slot
        let shreds_by_slot = load_shreds(raw_shreds);

        for (slot, shreds) in &shreds_by_slot {
            info!("slot: {} shreds: {}", slot, shreds.len());
        }

        // Process shreds for each slot
        for (slot, slot_shreds) in shreds_by_slot {
            info!("Processing slot: {}", slot);
            let (data_shreds, code_shreds) = preprocess_shreds(slot_shreds);
            let data_shreds = match validate_and_try_repair(&data_shreds, &code_shreds) {
                Ok(data_shreds) => data_shreds,
                Err(e) => {
                    error!("Failed to validate and repair shreds: {}", e);
                    continue;
                }
            };

            assert!(!data_shreds.is_empty());

            let deshredded_data = deshred(&data_shreds);

            debug!("Deshredded data size: {}", deshredded_data.len());
            match deserialize_entries(&deshredded_data) {
                Ok(entries) => {
                    info!("Successfully deserialized {} entries", entries.len());
                }
                Err(e) => error!("Failed to deserialize entries: {}", e),
            }
        }
    }
}
