use serde::{Deserialize, Serialize};
use solana_ledger::shred::{layout, Error, Shred, ShredData, ShredType};
use solana_sdk::signature::SIGNATURE_BYTES;
use std::convert::TryFrom;

pub fn get_shred_variant(shred: &[u8]) -> Result<ShredVariant, Error> {
    let Some(&shred_variant) = shred.get(OFFSET_OF_SHRED_VARIANT) else {
        return Err(Error::InvalidPayloadSize(shred.len()));
    };
    ShredVariant::try_from(shred_variant).map_err(|_| Error::InvalidShredVariant)
}

pub fn deserialize_shred(data: Vec<u8>) -> Result<(), Box<dyn std::error::Error>> {
    let shred_variant = get_shred_variant(&data)?;
    let shred_slot = layout::get_slot(&data);
    let shred_type = ShredType::from(shred_variant);
    let shred_version = layout::get_version(&data);
    let id = layout::get_shred_id(&data);
    println!(
        "{:?} {:?} {:?} {:?} {:?}",
        id, shred_variant, shred_slot, shred_type, shred_version
    );
    println!("{:#?}", Shred::new_from_serialized_shred(data.clone()));

    Ok(())
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
    use super::*;

    #[test]
    fn test_deserialize_shreds() {
        let data = include_bytes!("../packets.json").to_vec();
        let json = std::str::from_utf8(&data).unwrap();
        let shreds: Vec<Vec<u8>> = serde_json::from_str(json).unwrap();
        for shred in shreds.iter().take(15) {
            match deserialize_shred(shred.clone()) {
                Ok(_) => {
                    println!("Success");
                }
                Err(e) => {
                    // eprintln!("Error: {:?}", e);
                }
            }
        }
    }
}
