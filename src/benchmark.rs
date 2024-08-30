use std::collections::HashMap;
use std::ops::Div;
use std::sync::Arc;
use tokio::sync::RwLock;

use futures_util::stream::StreamExt;
use log::info;
use solana_client::nonblocking::pubsub_client::PubsubClient;
use solana_client::rpc_config::{
    RpcTransactionLogsConfig, RpcTransactionLogsFilter,
};
use solana_sdk::commitment_config::CommitmentConfig;

use crate::util::env;

pub type Sigs = Arc<RwLock<Vec<(u64, String)>>>;

pub async fn listen_pubsub(
    pubkeys: Vec<String>,
    sigs: Sigs,
) -> Result<(), Box<dyn std::error::Error>> {
    let pubsub_client = PubsubClient::new(&env("WS_URL")).await?;
    let (mut stream, unsub) = pubsub_client
        .logs_subscribe(
            RpcTransactionLogsFilter::Mentions(pubkeys),
            RpcTransactionLogsConfig {
                commitment: Some(CommitmentConfig::processed()),
            },
        )
        .await?;

    while let Some(data) = stream.next().await {
        let timestamp = chrono::Utc::now().timestamp_millis();
        let mut sigs = sigs.write().await;
        info!("pubsub: {} {}", timestamp, data.value.signature);
        sigs.push((timestamp as u64, data.value.signature));
    }

    unsub().await;

    Ok(())
}

pub fn compare_results(
    pubsub_sigs: Vec<(u64, String)>,
    shreds_sigs: Vec<(u64, String)>,
) {
    let mut miss_count = 0;
    let mut slower_count = 0;
    let mut faster_count = 0;
    let mut shreds_sigs_map: HashMap<String, u64> = HashMap::new();
    for (timestamp, sig) in shreds_sigs.iter() {
        shreds_sigs_map.insert(sig.clone(), *timestamp);
    }

    let mut average_diff = 0f64;
    let mut count = 0;

    for (pubsub_timestamp, sig) in pubsub_sigs.iter() {
        if let Some(shreds_timestamp) = shreds_sigs_map.remove(sig) {
            let diff = shreds_timestamp as f64 - *pubsub_timestamp as f64;
            info!("{} diff: {}", sig, diff);
            average_diff += diff;
            count += 1;
            match shreds_timestamp.cmp(pubsub_timestamp) {
                std::cmp::Ordering::Equal => {}
                std::cmp::Ordering::Less => faster_count += 1,
                std::cmp::Ordering::Greater => slower_count += 1,
            }
        } else {
            miss_count += 1;
        }
    }

    info!("Benchmark results:");
    info!("Pubsub sigs: {}", pubsub_sigs.len());
    info!("Shreds sigs: {}", shreds_sigs.len());
    info!("Miss count: {}", miss_count);
    info!("Slower count: {}", slower_count);
    info!("Faster count: {}", faster_count);
    info!("Average diff: {}", average_diff.div(count as f64));
}
