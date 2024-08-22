use crate::structs::ShredVariant;
use log::{debug, error, info};
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
        get_shred_variant(shred.payload()).unwrap()
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

    let entry_count = u64::from_le_bytes(payload[0..8].try_into().unwrap());
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

#[cfg(test)]
mod tests {

    use super::*;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn deserialize_shreds() {
        std::env::set_var("RUST_LOG", "info");
        env_logger::init();
        let data = std::fs::read_to_string("packets.json").expect("Failed to read packets.json");
        let raw_shreds: Vec<Vec<u8>> = serde_json::from_str(&data).expect("Failed to parse JSON");

        let mut shred_sizes = HashMap::new();
        for shred in raw_shreds.iter() {
            *shred_sizes.entry(shred.len()).or_insert(0) += 1;
        }
        info!("shred sizes {:?}", shred_sizes);

        // Group shreds by slot
        let mut shreds_by_slot: HashMap<u64, Vec<Shred>> = HashMap::new();
        for raw_shred in raw_shreds {
            if raw_shred.len() == 29 {
                continue;
            }
            let shred = Shred::new_from_serialized_shred(raw_shred).unwrap();
            shreds_by_slot.entry(shred.slot()).or_default().push(shred);
        }

        for (slot, shreds) in &shreds_by_slot {
            info!("slot: {} shreds: {}", slot, shreds.len());
        }

        // Process shreds for each slot
        for (slot, slot_shreds) in shreds_by_slot {
            info!("Processing slot: {}", slot);

            // stupid clone here, should iter
            let mut data_shreds = Vec::new();
            for shred in slot_shreds {
                if shred.is_data() {
                    data_shreds.push(shred);
                }
            }
            for shred in data_shreds.iter() {
                if let ShredVariant::MerkleData { .. } =
                    ShredVariant::try_from(shred.payload()[0x40]).unwrap()
                {
                } else {
                    panic!("not merkle data");
                }
            }

            // deduplicate data_shreads and sort by key
            let mut seen = HashSet::new();
            data_shreds.retain(|shred| seen.insert(shred.index()));
            data_shreds.sort_by_key(|shred| shred.index());

            if !data_shreds.is_empty() {
                let index = data_shreds.first().unwrap().index();
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
                    error!("Missing indices: {:?}", missing_indices);
                    continue;
                }
                let data_complete = {
                    let shred = data_shreds.last().unwrap();
                    shred.data_complete() || shred.last_in_slot()
                };

                if data_complete && aligned {
                    let deshredded_data: Vec<u8> = data_shreds
                        .iter()
                        .flat_map(|shred| {
                            shred_data(shred)
                                .map(|data| data.to_vec())
                                .unwrap_or_default()
                        })
                        .collect();

                    if !deshredded_data.is_empty() {
                        debug!("Deshredded data size: {}", deshredded_data.len());
                        match deserialize_entries(&deshredded_data) {
                            Ok(entries) => {
                                info!("Successfully deserialized {} entries", entries.len());
                            }
                            Err(e) => error!("Failed to deserialize entries: {}", e),
                        }
                    } else {
                        error!("Deshredded data is empty");
                    }
                } else {
                    error!(
                        "invalid: data_complete: {}, aligned: {}",
                        data_complete, aligned
                    );
                }
            } else {
                error!("No data shreds found");
            }
        }
    }
}
