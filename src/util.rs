use serde::Deserialize;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

pub fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| {
        panic!("{} env var not set", key);
    })
}
/// Helper function for pubkey serialize
pub fn pubkey_to_string<S>(
    pubkey: &Pubkey,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&pubkey.to_string())
}

/// Helper function for pubkey deserialize
pub fn string_to_pubkey<'de, D>(deserializer: D) -> Result<Pubkey, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Pubkey::from_str(&s).map_err(serde::de::Error::custom)
}
