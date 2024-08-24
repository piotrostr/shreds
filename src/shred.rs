use anyhow::Result;
use solana_ledger::shred::{ReedSolomonCache, ShredFlags, Shredder};
use std::collections::{HashMap, HashSet};

use crate::structs::ShredVariant;
use log::{debug, error, info, trace, warn};
use solana_entry::entry::Entry;
use solana_ledger::shred::{Error, Shred};
use solana_sdk::signature::SIGNATURE_BYTES;

pub fn get_shred_debug_string(shred: Shred) -> String {
    let (block_complete, batch_complete, batch_tick) =
        get_shred_data_flags(shred.payload());
    format!(
        "{} {} shred: {} {} {} {}",
        shred.slot(),
        shred.index(),
        u16::from_le_bytes([shred.payload()[0x56], shred.payload()[0x57]])
            as usize, // size
        block_complete,
        batch_complete,
        batch_tick
    )
}

pub fn debug_shred(shred: Shred) {
    let size_in_header =
        u16::from_le_bytes([shred.payload()[0x56], shred.payload()[0x57]])
            as usize;
    info!(
        "index: {}: payload: {}, size in header: {} zeros: {} variant: {:?}",
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
    ShredVariant::try_from(shred_variant)
        .map_err(|_| Error::InvalidShredVariant)
}

pub fn deserialize_shred(data: Vec<u8>) -> Result<Shred, Error> {
    Shred::new_from_serialized_shred(data)
}

pub fn deserialize_entries(
    payload: &[u8],
) -> Result<Vec<Entry>, Box<dyn std::error::Error + Send + Sync>> {
    if payload.len() < 8 {
        error!("Payload too short: {} bytes", payload.len());
        return Ok(Vec::new());
    }

    let entry_count = u64::from_le_bytes(
        payload[0..8].try_into().expect("entry count parse"),
    );
    if entry_count > 10_000 {
        return Err(format!("entry count: {}", entry_count).into());
    }
    trace!("Entry count prefix: {}", entry_count);
    trace!("First 16 bytes of payload: {:?}", &payload[..16]);

    // SUPER CRUCIAL
    // you cannot just Ok(bincode::deserialize(&payload[8..])?)
    // since the entries are not serialized as a vec, just separate entries
    // each next to the other, took me too long to figure this out :P
    let mut entries = Vec::new();
    let mut cursor = std::io::Cursor::new(&payload[8..]);
    for i in 0..entry_count {
        match bincode::deserialize_from::<_, Entry>(&mut cursor) {
            Ok(entry) => {
                entries.push(entry);
            }
            Err(e) => {
                error!(
                    "Failed to deserialize entry {}/{}: {}",
                    i, entry_count, e
                );
            }
        }
    }

    Ok(entries)
}

const OFFSET_OF_SHRED_VARIANT: usize = SIGNATURE_BYTES;

pub fn shred_data(shred: &Shred) -> Result<&[u8], Error> {
    let variant =
        ShredVariant::try_from(shred.payload()[OFFSET_OF_SHRED_VARIANT])?;
    let (data_start, size) = match variant {
        ShredVariant::MerkleData { .. } => {
            let size = u16::from_le_bytes([
                shred.payload()[0x56],
                shred.payload()[0x57],
            ]) as usize;
            (0x58usize, size.saturating_sub(0x58))
        }
        ShredVariant::LegacyData => {
            (0x56, shred.payload().len().saturating_sub(0x56))
        }
        _ => return Err(Error::InvalidShredVariant),
    };

    let data_end = data_start.saturating_add(size);
    if data_end > shred.payload().len() {
        return Err(Error::InvalidPayloadSize(shred.payload().len()));
    }
    Ok(&shred.payload()[data_start..data_end])
}

pub fn load_shreds(raw_shreds: Vec<Vec<u8>>) -> HashMap<u64, Vec<Shred>> {
    let mut shreds_by_slot: HashMap<u64, Vec<Shred>> = HashMap::new();
    for raw_shred in raw_shreds {
        if raw_shred.len() == 29 {
            continue;
        }
        let shred =
            Shred::new_from_serialized_shred(raw_shred).expect("new shred");
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
    let aligned =
        data_shreds.iter().zip(index..).all(|(s, i)| s.index() == i);
    let data_complete = {
        let shred = data_shreds.last().expect("last shred");
        shred.data_complete() || shred.last_in_slot()
    };
    if !aligned || !data_complete {
        if data_shreds.is_empty() {
            return Err("No data shreds".into());
        }
        if code_shreds.is_empty() {
            return Err("No code shreds".into());
        }
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
        match missing_indices.len() <= code_shreds.len() {
            true => {
                warn!(
                    "Missing indices: {:?}, trying to repair",
                    missing_indices
                );
            }
            false => {
                return Err(format!(
                    "Too many missing indices: {:?}",
                    missing_indices
                )
                .into());
            }
        }
        info!("code shreds len: {}", code_shreds.len());
        let data_shreds = data_shreds.to_vec();
        // TODO stupid clone for now
        let all_shreds = data_shreds
            .iter()
            .chain(code_shreds.iter())
            .cloned()
            .collect::<Vec<_>>();
        let data_shreds = match Shredder::try_recovery(
            all_shreds,
            &ReedSolomonCache::default(),
        ) {
            Ok(data_shreds) => data_shreds,
            Err(e) => {
                error!("Failed to repair shreds: {}", e);
                return Err(e.into());
            }
        };
        let aligned =
            data_shreds.iter().zip(index..).all(|(s, i)| s.index() == i);
        let data_complete = {
            let shred = data_shreds.last().expect("last shred");
            shred.data_complete() || shred.last_in_slot()
        };
        if !aligned || !data_complete {
            return Err(format!(
                "Shreds aligned: {} complete: {}, repair no workerino",
                aligned, data_complete
            )
            .into());
        }
    }

    Ok(data_shreds.to_vec())
}

pub fn get_shred_index(
    raw_shred: &[u8],
) -> Result<u32, Box<dyn std::error::Error>> {
    Ok(u32::from_le_bytes(raw_shred[0x49..0x49 + 4].try_into()?))
}

/// get_shred_is_last works for data shreds only
pub fn get_shred_is_last(
    raw_shred: &[u8],
) -> Result<bool, Box<dyn std::error::Error>> {
    match raw_shred.get(0x55) {
        Some(flags) => {
            let flags = ShredFlags::from_bits(*flags).expect("parse flags");
            if flags.contains(ShredFlags::DATA_COMPLETE_SHRED)
                || flags.contains(ShredFlags::LAST_SHRED_IN_SLOT)
            {
                Ok(true)
            } else {
                Ok(false)
            }
        }
        None => Err("Error getting flags".into()),
    }
}

pub fn get_shred_data_flags(raw_shred: &[u8]) -> (bool, bool, u8) {
    let flags = raw_shred[0x55];
    // Extract block_complete (bit 7)
    let block_complete = (flags & 0b1000_0000) != 0;

    // Extract batch_complete (bit 6)
    let batch_complete = (flags & 0b0100_0000) != 0;

    // Extract batch_tick (bits 0-5)
    let batch_tick = flags & 0b0011_1111;

    (block_complete, batch_complete, batch_tick)
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn deserialize_shreds() {
        env_logger::Builder::default()
            .format_module_path(false)
            .filter_level(log::LevelFilter::Info)
            .init();

        let data = std::fs::read_to_string("packets.json")
            .expect("Failed to read packets.json");
        let raw_shreds: Vec<Vec<u8>> =
            serde_json::from_str(&data).expect("Failed to parse JSON");

        // debugging, useful
        {
            let shreds = raw_shreds
                .iter()
                .filter(|shred| shred.len() > 29)
                .map(|shred| deserialize_shred(shred.clone()).expect("shred"))
                .collect::<Vec<_>>();
            let mut shreds_by_slot = HashMap::new();
            for shred in shreds.iter() {
                shreds_by_slot
                    .entry(shred.slot())
                    .or_insert_with(Vec::new)
                    .push(shred.clone());
            }
            for (_, mut shreds) in shreds_by_slot {
                shreds = shreds
                    .iter()
                    .filter(|shred| shred.is_data())
                    .cloned()
                    .collect::<Vec<_>>();
                shreds.sort_by_key(|shred| shred.index());
                shreds.dedup_by_key(|shred| shred.index());
                for shred in shreds {
                    debug!("{}", get_shred_debug_string(shred));
                }
            }
        }

        // Group shreds by slot
        let shreds_by_slot = load_shreds(raw_shreds);

        for (slot, shreds) in &shreds_by_slot {
            debug!("slot: {} shreds: {}", slot, shreds.len());
        }

        // Process shreds for each slot
        for (slot, slot_shreds) in shreds_by_slot {
            let (data_shreds, code_shreds) = preprocess_shreds(slot_shreds);
            info!(
                "Processing slot: {} (data: {}, code: {})",
                slot,
                data_shreds.len(),
                code_shreds.len()
            );
            let data_shreds =
                match validate_and_try_repair(&data_shreds, &code_shreds) {
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
                    info!(
                        "Successfully deserialized {} entries",
                        entries.len()
                    );
                }
                Err(e) => error!("Failed to deserialize entries: {}", e),
            }
        }
    }
}
