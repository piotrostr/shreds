use solana_sdk::hash::Hash;
use std::convert::TryInto;

use serde::{Deserialize, Serialize};
use solana_sdk::signature::Signature;
use solana_sdk::transaction::VersionedTransaction;

#[derive(Debug, Serialize, Deserialize)]
pub struct CommonHeader {
    pub signature: Signature,
    pub variant: u8,
    pub slot: u64,
    pub shred_index: u32,
    pub shred_version: u16,
    pub fec_set_index: u32,
}

#[derive(Debug)]
pub struct DataShredHeader {
    pub parent_offset: u16,
    pub data_flags: u8,
    pub size: u16,
}

#[derive(Debug)]
pub struct CodeShredHeader {
    pub num_data_shreds: u16,
    pub num_coding_shreds: u16,
    pub position: u16,
}

#[derive(Debug)]
pub enum ShredType {
    Data(DataShredHeader),
    Code(CodeShredHeader),
}

#[derive(Debug)]
pub struct Shred {
    pub common_header: CommonHeader,
    pub shred_type: ShredType,
    pub payload: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Eq, Clone)]
pub struct Entry {
    pub num_hashes: u64,
    pub hash: Hash,
    pub transactions: Vec<VersionedTransaction>,
}

pub fn deserialize_shred(data: &[u8]) -> Result<Shred, &'static str> {
    if data.len() < 83 {
        return Err("Insufficient data for common header");
    }

    let common_header = CommonHeader {
        signature: data[0x00..64].try_into().unwrap(),
        variant: data[0x40],
        slot: u64::from_le_bytes(data[0x41..0x41 + 8].try_into().unwrap()),
        shred_index: u32::from_le_bytes(data[0x49..0x49 + 4].try_into().unwrap()),
        shred_version: u16::from_le_bytes(data[0x49..0x49 + 2].try_into().unwrap()),
        fec_set_index: u32::from_le_bytes(data[0x4f..0x4f + 4].try_into().unwrap()),
    };

    let auth_type = common_header.variant >> 4;
    let shred_type = common_header.variant & 0xF;

    let (shred_type, payload_start) = match (auth_type, shred_type) {
        (0x5, 0xa) | (0x4, _) => {
            if data.len() < 89 {
                return Err("Insufficient data for code shred header");
            }
            let header = CodeShredHeader {
                num_data_shreds: u16::from_le_bytes(data[83..85].try_into().unwrap()),
                num_coding_shreds: u16::from_le_bytes(data[85..87].try_into().unwrap()),
                position: u16::from_le_bytes(data[87..89].try_into().unwrap()),
            };
            (ShredType::Code(header), 89)
        }
        (0xa, 0x5) | (0x8, _) => {
            if data.len() < 88 {
                return Err("Insufficient data for data shred header");
            }
            let header = DataShredHeader {
                parent_offset: u16::from_le_bytes(data[0x53..0x53 + 2].try_into().unwrap()),
                data_flags: data[0x55],
                size: u16::from_le_bytes(data[0x56..0x56 + 2].try_into().unwrap()),
            };
            (ShredType::Data(header), 0x58)
        }
        _ => return Err("Invalid shred variant"),
    };

    let payload = data[payload_start..].to_vec();

    Ok(Shred {
        common_header,
        shred_type,
        payload,
    })
}

pub fn deserialize_entries(payload: &[u8]) -> Result<Vec<Entry>, bincode::Error> {
    if payload.is_empty() {
        return Ok(Vec::new());
    }

    let entry_count = u64::from_le_bytes(payload[0..8].try_into().unwrap());
    println!("Entry count: {}", entry_count);
    bincode::deserialize(&payload[8..])
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;

    #[test]
    fn test_deserialize_shreds_raw() {
        let data = include_bytes!("../packets.json").to_vec();
        let json = std::str::from_utf8(&data).unwrap();
        let raw_shreds: Vec<Vec<u8>> = serde_json::from_str(json).unwrap();
        let sizes: HashSet<usize> = HashSet::from_iter(raw_shreds.iter().map(|s| s.len()));
        let mut shreds = Vec::new();
        println!("Shred sizes: {:?}", sizes);
        for raw_shred in raw_shreds.iter() {
            if raw_shred.len() == 27 || raw_shred.len() == 132 {
                continue;
            }
            match deserialize_shred(&raw_shred.clone()) {
                Ok(shred) => {
                    // println!("OK: {:#?} {:#?}", shred.shred_type, shred.common_header);
                    shreds.push(shred);
                }
                Err(e) => {
                    panic!("Error deserializing shred: {:?}", e);
                }
            }
        }
        println!("total shreds: {}", shreds.len());
        let mut shreds_per_slot = HashMap::new();
        shreds.iter().for_each(|shred| {
            shreds_per_slot
                .entry(&shred.common_header.slot)
                .or_insert_with(Vec::new);
            shreds_per_slot
                .get_mut(&shred.common_header.slot)
                .unwrap()
                .push(shred);
        });
        // go slot by slot
        for (slot, mut slot_shreds) in shreds_per_slot {
            println!("Slot: {}", slot);
            slot_shreds.sort_by_key(|shred| shred.common_header.shred_index);

            let mut data = Vec::new();
            for shred in shreds.iter() {
                data.extend_from_slice(&shred.payload);
                if let ShredType::Data(header) = &shred.shred_type {
                    if header.data_flags & 0x40 != 0 || header.data_flags & 0x80 != 0 {
                        // Step 4: Deserialize entries from the reconstructed data
                        let entries = deserialize_entries(&data).unwrap();
                        println!("Entries: {:?}", entries);
                    }
                }
            }
        }
    }
}
