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
    use std::collections::{HashMap, HashSet};

    use super::*;

    #[test]
    fn deserialize_shreds() {
        let data = include_bytes!("../packets.json").to_vec();
        let json = std::str::from_utf8(&data).unwrap();
        let raw_shreds: Vec<Vec<u8>> = serde_json::from_str(json).unwrap();
        let sizes: HashSet<usize> = HashSet::from_iter(raw_shreds.iter().map(|s| s.len()));
        let mut shreds = Vec::new();
        println!("Shred sizes: {:?}", sizes);
        for raw_shred in raw_shreds.iter() {
            if raw_shred.len() == 29 || raw_shred.len() == 132 {
                continue;
            }
            match deserialize_shred(raw_shred.to_vec()) {
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
            shreds_per_slot.entry(shred.slot()).or_insert_with(Vec::new);
            shreds_per_slot
                .get_mut(&shred.slot())
                .unwrap()
                .push(shred.clone());
        });
        println!("total slots: {}", shreds_per_slot.len());
        // go slot by slot
        for (slot, mut slot_shreds) in shreds_per_slot {
            println!("Slot: {}", slot);
            slot_shreds.sort_by_key(|shred| shred.index());

            let mut data = Vec::new();
            for shred in shreds.iter() {
                data.extend_from_slice(shred.payload());
                if shred.data_complete() && shred.is_data() {
                    // Step 4: Deserialize entries from the reconstructed data
                    let entries = bincode::deserialize::<Vec<Entry>>(shred.payload());
                    println!("Entries: {:?}", entries);
                }
            }
            break;
        }
    }
}
