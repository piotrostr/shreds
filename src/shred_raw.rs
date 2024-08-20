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
        signature: data[0..64].try_into().unwrap(),
        variant: data[64],
        slot: u64::from_le_bytes(data[65..73].try_into().unwrap()),
        shred_index: u32::from_le_bytes(data[73..77].try_into().unwrap()),
        shred_version: u16::from_le_bytes(data[77..79].try_into().unwrap()),
        fec_set_index: u32::from_le_bytes(data[79..83].try_into().unwrap()),
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
                parent_offset: u16::from_le_bytes(data[83..85].try_into().unwrap()),
                data_flags: data[85],
                size: u16::from_le_bytes(data[86..88].try_into().unwrap()),
            };
            (ShredType::Data(header), 88)
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
    use super::*;

    #[test]
    fn test_deserialize_shreds_raw() {
        let data = include_bytes!("../packets.json").to_vec();
        let json = std::str::from_utf8(&data).unwrap();
        let shreds: Vec<Vec<u8>> = serde_json::from_str(json).unwrap();
        for shred in shreds.iter().nth(2) {
            match deserialize_shred(&shred.clone()) {
                Ok(res) => {
                    println!("OK: {:#?} {:#?}", res.shred_type, res.common_header);
                    let entries = deserialize_entries(&res.payload).unwrap();
                    println!("{:#?}", entries);
                }
                Err(e) => {
                    eprintln!("Error: {:?}", e);
                }
            }
        }
    }
}
