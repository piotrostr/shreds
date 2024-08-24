use futures_util::stream::StreamExt;
use log::info;
use solana_client::nonblocking::pubsub_client::PubsubClient;
use solana_client::rpc_config::{
    RpcTransactionLogsConfig, RpcTransactionLogsFilter,
};
use solana_sdk::commitment_config::CommitmentConfig;

use crate::algo::env;

pub async fn listen_pubsub(
    pubkeys: Vec<String>,
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
        info!("{:?}", data.value.signature);
    }

    unsub().await;

    Ok(())
}
