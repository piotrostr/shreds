pub fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| {
        panic!("{} env var not set", key);
    })
}
