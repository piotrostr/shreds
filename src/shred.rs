use serde::{Deserialize, Serialize};
use solana_entry::entry::Entry;
use solana_ledger::shred::{Error, Shred, ShredType};
use solana_sdk::signature::SIGNATURE_BYTES;
use std::convert::TryFrom;

pub fn get_shred_variant(shred: &[u8]) -> Result<ShredVariant, Error> {
    let Some(&shred_variant) = shred.get(OFFSET_OF_SHRED_VARIANT) else {
        return Err(Error::InvalidPayloadSize(shred.len()));
    };
    ShredVariant::try_from(shred_variant).map_err(|_| Error::InvalidShredVariant)
}

pub fn deserialize_shred(data: Vec<u8>) -> Result<Shred, Error> {
    // in prod, those have to be used to group shreds before serializing, might save some cycles
    // let shred_variant = get_shred_variant(&data)?;
    // let shred_slot = layout::get_slot(&data);
    // let shred_type = ShredType::from(shred_variant);
    // let shred_version = layout::get_version(&data);
    // let id = layout::get_shred_id(&data);
    // println!(
    //     "{:?} {:?} {:?} {:?} {:?}",
    //     id, shred_variant, shred_slot, shred_type, shred_version
    // );
    Shred::new_from_serialized_shred(data)
}

const OFFSET_OF_SHRED_VARIANT: usize = SIGNATURE_BYTES;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Deserialize, Serialize)]
#[serde(into = "u8", try_from = "u8")]
pub enum ShredVariant {
    LegacyCode, // 0b0101_1010
    LegacyData, // 0b1010_0101
    // proof_size is the number of Merkle proof entries, and is encoded in the
    // lowest 4 bits of the binary representation. The first 4 bits identify
    // the shred variant:
    //   0b0100_????  MerkleCode
    //   0b0110_????  MerkleCode chained
    //   0b0111_????  MerkleCode chained resigned
    //   0b1000_????  MerkleData
    //   0b1001_????  MerkleData chained
    //   0b1011_????  MerkleData chained resigned
    MerkleCode {
        proof_size: u8,
        chained: bool,
        resigned: bool,
    }, // 0b01??_????
    MerkleData {
        proof_size: u8,
        chained: bool,
        resigned: bool,
    }, // 0b10??_????
}

impl From<ShredVariant> for ShredType {
    #[inline]
    fn from(shred_variant: ShredVariant) -> Self {
        match shred_variant {
            ShredVariant::LegacyCode => ShredType::Code,
            ShredVariant::LegacyData => ShredType::Data,
            ShredVariant::MerkleCode { .. } => ShredType::Code,
            ShredVariant::MerkleData { .. } => ShredType::Data,
        }
    }
}

impl From<ShredVariant> for u8 {
    fn from(shred_variant: ShredVariant) -> u8 {
        match shred_variant {
            ShredVariant::LegacyCode => u8::from(ShredType::Code),
            ShredVariant::LegacyData => u8::from(ShredType::Data),
            ShredVariant::MerkleCode {
                proof_size,
                chained: false,
                resigned: false,
            } => proof_size | 0x40,
            ShredVariant::MerkleCode {
                proof_size,
                chained: true,
                resigned: false,
            } => proof_size | 0x60,
            ShredVariant::MerkleCode {
                proof_size,
                chained: true,
                resigned: true,
            } => proof_size | 0x70,
            ShredVariant::MerkleData {
                proof_size,
                chained: false,
                resigned: false,
            } => proof_size | 0x80,
            ShredVariant::MerkleData {
                proof_size,
                chained: true,
                resigned: false,
            } => proof_size | 0x90,
            ShredVariant::MerkleData {
                proof_size,
                chained: true,
                resigned: true,
            } => proof_size | 0xb0,
            ShredVariant::MerkleCode {
                proof_size: _,
                chained: false,
                resigned: true,
            }
            | ShredVariant::MerkleData {
                proof_size: _,
                chained: false,
                resigned: true,
            } => panic!("Invalid shred variant: {shred_variant:?}"),
        }
    }
}

impl TryFrom<u8> for ShredVariant {
    type Error = Error;
    fn try_from(shred_variant: u8) -> Result<Self, Self::Error> {
        if shred_variant == u8::from(ShredType::Code) {
            Ok(ShredVariant::LegacyCode)
        } else if shred_variant == u8::from(ShredType::Data) {
            Ok(ShredVariant::LegacyData)
        } else {
            let proof_size = shred_variant & 0x0F;
            match shred_variant & 0xF0 {
                0x40 => Ok(ShredVariant::MerkleCode {
                    proof_size,
                    chained: false,
                    resigned: false,
                }),
                0x60 => Ok(ShredVariant::MerkleCode {
                    proof_size,
                    chained: true,
                    resigned: false,
                }),
                0x70 => Ok(ShredVariant::MerkleCode {
                    proof_size,
                    chained: true,
                    resigned: true,
                }),
                0x80 => Ok(ShredVariant::MerkleData {
                    proof_size,
                    chained: false,
                    resigned: false,
                }),
                0x90 => Ok(ShredVariant::MerkleData {
                    proof_size,
                    chained: true,
                    resigned: false,
                }),
                0xb0 => Ok(ShredVariant::MerkleData {
                    proof_size,
                    chained: true,
                    resigned: true,
                }),
                other => {
                    println!("unknown: {:#x}", other);
                    Err(Error::InvalidShredVariant)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    const SIZE_OF_COMMON_SHRED_HEADER: usize = 83;
    const SIZE_OF_DATA_SHRED_HEADERS: usize = 88;
    const SIZE_OF_CODING_SHRED_HEADERS: usize = 89;
    const SIZE_OF_SIGNATURE: usize = SIGNATURE_BYTES;
    const SIZE_OF_SHRED_VARIANT: usize = 1;
    const SIZE_OF_SHRED_SLOT: usize = 8;

    const OFFSET_OF_SHRED_VARIANT: usize = SIZE_OF_SIGNATURE;
    const OFFSET_OF_SHRED_SLOT: usize = SIZE_OF_SIGNATURE + SIZE_OF_SHRED_VARIANT;
    const OFFSET_OF_SHRED_INDEX: usize = OFFSET_OF_SHRED_SLOT + SIZE_OF_SHRED_SLOT;

    use solana_ledger::shred::{layout, ShredFlags};
    use std::collections::{HashMap, HashSet};

    use super::*;

    pub fn extract_data_from_shred(shred: &[u8]) -> Option<Vec<u8>> {
        // The exact offset might need adjustment based on the shred format
        // match Shred::new_from_serialized_shred(shred.to_vec()) {
        //     Ok(result) => Some(result.payload().to_vec()),
        //     Err(_) => None,
        // }
        return shred.get(88..).map(|s| s.to_vec());
    }

    pub fn get_shred_type(shred: &[u8]) -> Result<ShredType, Error> {
        let shred_variant = get_shred_variant(shred)?;
        Ok(ShredType::from(shred_variant))
    }

    pub fn get_index(shred: &[u8]) -> Option<u32> {
        <[u8; 4]>::try_from(shred.get(OFFSET_OF_SHRED_INDEX..)?.get(..4)?)
            .map(u32::from_le_bytes)
            .ok()
    }

    #[test]
    fn deserialize_shreds() {
        let data = std::fs::read_to_string("packets.json").unwrap();
        let raw_shreds: Vec<Vec<u8>> = serde_json::from_str(&data).unwrap();

        // Group shreds by slot
        let mut shreds_by_slot: HashMap<u64, Vec<Vec<u8>>> = HashMap::new();
        for raw_shred in raw_shreds {
            if raw_shred.len() == 132 {
                continue;
            }
            if let Some(slot) = layout::get_slot(&raw_shred) {
                if slot > 384947093 {
                    println!("Slot: {}, shred data len {}", slot, raw_shred.len());
                }
                shreds_by_slot.entry(slot).or_default().push(raw_shred);
            }
        }

        // Process shreds for each slot
        for (slot, mut slot_shreds) in shreds_by_slot {
            println!("Processing slot: {}", slot);

            // Sort shreds within the slot
            slot_shreds.sort_by_key(|shred| get_index(shred).unwrap_or(u32::MAX));

            let mut data_shreds = Vec::new();
            for shred in slot_shreds {
                if let Ok(ShredType::Data) = get_shred_type(&shred) {
                    data_shreds.push(shred);
                }
            }

            // Reconstruct data from shreds
            let mut reconstructed_data = Vec::new();
            for shred in data_shreds {
                if let Some(data) = extract_data_from_shred(&shred) {
                    reconstructed_data.extend_from_slice(&data);
                }

                let flags = shred
                    .get(85)
                    .map(|&f| ShredFlags::from_bits_truncate(f))
                    .unwrap_or_default();
                if flags.contains(ShredFlags::LAST_SHRED_IN_SLOT)
                    && flags.contains(ShredFlags::DATA_COMPLETE_SHRED)
                {
                    match bincode::deserialize::<Vec<Entry>>(&reconstructed_data) {
                        Ok(entries) => println!("OK, {} entries: {:#?}", entries.len(), entries),
                        Err(e) => eprintln!("FAIL {:#?}", e),
                    };
                    reconstructed_data.clear();
                }
            }
        }
    }
}
