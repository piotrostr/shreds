#[cfg(test)]
mod tests {
    use crate::shred_processor::FecSet;
    #[test]
    fn recovery_works() {
        let contents = std::fs::read_to_string("hanging_fec_sets.json")
            .expect("Failed to read hanging_fec_sets.json");
        let fec_sets: Vec<FecSet> = serde_json::from_str(&contents).unwrap();
        println!("FecSets hanging: {}", fec_sets.len());

        fec_sets.iter().for_each(|fec_set| {
            println!("FecSet: {:?}", fec_set);
        });
    }
}
