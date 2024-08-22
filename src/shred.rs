use crate::structs::ShredVariant;
use solana_entry::entry::Entry;
use solana_ledger::shred::{Error, Shred};
use solana_sdk::signature::SIGNATURE_BYTES;

pub fn debug_shred(shred: Shred) {
    let size_in_header =
        u16::from_le_bytes([shred.payload()[0x56], shred.payload()[0x57]]) as usize;
    println!(
        "shred: parent {} index: {}: payload: {}, size in header: {} zero freq: {} variant: {:?}",
        shred.parent().unwrap(),
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
    if payload.is_empty() {
        return Ok(Vec::new());
    }

    let entry_count = u64::from_le_bytes(payload[0..8].try_into().unwrap());
    println!("Entry count prefix: {}", entry_count);
    bincode::deserialize(&payload[8..])
}

const OFFSET_OF_SHRED_VARIANT: usize = SIGNATURE_BYTES;

pub fn shred_data(shred: &Shred) -> Result<&[u8], Error> {
    let data_start = 0x58;
    let size = u16::from_le_bytes([shred.payload()[0x56], shred.payload()[0x57]]) as usize;

    let data_end = data_start + size;
    if data_end > shred.payload().len() {
        return Err(Error::InvalidPayloadSize(shred.payload().len()));
    }
    let parsed_data = &shred.payload()[data_start..data_end];
    // println!(
    //     "index: {} shred size: {}, payload size: {}, parsed size: {}",
    //     shred.index(),
    //     size,
    //     shred.payload().len(),
    //     parsed_data.len()
    // );
    Ok(parsed_data)
}

#[cfg(test)]
mod tests {

    use super::*;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn deserialize_shreds() {
        let data = std::fs::read_to_string("packets.json").expect("Failed to read packets.json");
        let raw_shreds: Vec<Vec<u8>> = serde_json::from_str(&data).expect("Failed to parse JSON");

        let mut shred_sizes = HashMap::new();
        for shred in raw_shreds.iter() {
            *shred_sizes.entry(shred.len()).or_insert(0) += 1;
        }
        println!("shred sizes {:?}", shred_sizes);

        let mut seen_hashes = HashSet::new();
        let deduped_shreds: Vec<Vec<u8>> = raw_shreds
            .clone()
            .into_iter()
            .filter(|shred| seen_hashes.insert(solana_sdk::hash::hashv(&[shred])))
            .collect();
        println!(
            "deduped shreds: {} total shreds: {}",
            deduped_shreds.len(),
            raw_shreds.len()
        );

        let mut deduped_shred_sizes = HashMap::new();
        for shred in deduped_shreds.iter() {
            *deduped_shred_sizes.entry(shred.len()).or_insert(0) += 1;
        }
        println!("deduped shred sizes {:?}", deduped_shred_sizes);

        // Group shreds by slot
        let mut shreds_by_slot: HashMap<u64, Vec<Shred>> = HashMap::new();
        for raw_shred in raw_shreds {
            if raw_shred.len() == 132 || raw_shred.len() == 29 {
                continue;
            }
            let shred = Shred::new_from_serialized_shred(raw_shred).unwrap();
            shreds_by_slot.entry(shred.slot()).or_default().push(shred);
        }

        for (slot, shreds) in &shreds_by_slot {
            println!("slot: {} shreds: {}", slot, shreds.len());
        }

        // Process shreds for each slot
        for (slot, slot_shreds) in shreds_by_slot {
            println!("Processing slot: {}", slot);

            // stupid clone here, should iter
            let mut data_shreds = Vec::new();
            for shred in slot_shreds {
                if shred.is_data() {
                    data_shreds.push(shred);
                }
            }

            // deduplicate data_shreads by shred.signature()
            let mut seen = HashSet::new();
            data_shreds.retain(|shred| seen.insert(shred.index()));

            // Sort shreds within the slot
            data_shreds.sort_by_key(|shred| shred.index());
            // data_shreds
            //     .iter()
            //     .for_each(|shred| println!("shred: {}", shred.index()));

            // for mut shred in data_shreds {
            //     // if size != 88 {
            //     //     continue;
            //     // }
            //     // println!("shred: {:#?}", shred);
            //     // break;
            //     if let ShredVariant::MerkleData { .. } =
            //         ShredVariant::try_from(shred.payload()[0x40]).unwrap()
            //     {
            //     } else {
            //         panic!("not merkle data");
            //     }
            //     println!(
            //         "shred: parent {} index: {}: payload: {}, size in header: {} zero freq: {}",
            //         shred.parent().unwrap(),
            //         shred.index(),
            //         shred.payload().len(),
            //         0,
            //         shred.payload().iter().filter(|&&b| b == 0).count()
            //     );
            // }

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
                    println!("Missing indices: {:?}", missing_indices);
                    continue;
                }
                let data_complete = {
                    let shred = data_shreds.last().unwrap();
                    shred.data_complete() || shred.last_in_slot()
                };

                if data_complete && aligned {
                    let deshredded_data: Vec<u8> = data_shreds
                        .iter()
                        .flat_map(|shred| shred_data(shred).unwrap().iter().copied())
                        .collect();

                    if !deshredded_data.is_empty() {
                        println!("Deshredded data size: {}", deshredded_data.len());
                        let entries = deserialize_entries(&deshredded_data);
                        println!("Entries: {:?}", entries);
                    } else {
                        println!("Deshredded data is empty");
                    }
                } else {
                    println!("Shreds are not complete or not aligned");
                    println!("data_complete: {}, aligned: {}", data_complete, aligned);
                }
            } else {
                println!("No data shreds found");
            }
        }
    }
}
