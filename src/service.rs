use crate::arb::PoolsState;
use crate::entry_processor::ArbEntryProcessor;
use crate::entry_processor::PumpEntryProcessor;
use crate::listener::PACKET_SIZE;
use crate::shred_processor::ShredProcessor;
use log::{error, info};
use reqwest::Url;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tokio::time::{sleep, Duration};

pub enum Mode {
    Arb,
    Pump,
}

pub async fn run(
    bind_address: String,
    post_url: String,
    mode: Mode,
) -> Result<(), Box<dyn std::error::Error>> {
    Url::parse(&post_url)?;

    info!(
        "Starting listener on {}, sending to {}",
        bind_address, post_url
    );

    let socket = Arc::new(
        UdpSocket::bind(bind_address)
            .await
            .expect("Couldn't bind to address"),
    );
    let (entry_tx, entry_rx) = mpsc::channel(2000);
    let (error_tx, error_rx) = mpsc::channel(2000);
    let (sig_tx, mut sig_rx) = mpsc::channel(2000);

    let shred_processor =
        Arc::new(RwLock::new(ShredProcessor::new(entry_tx, error_tx)));

    // metrics loop
    info!("Starting metrics loop");
    let shred_processor_clone = shred_processor.clone();
    let metrics_handle = tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(6)).await;
            {
                let metrics = shred_processor_clone.read().await.metrics();
                info!("metrics: {:?}", metrics);
                drop(metrics);
            }
        }
    });

    info!("Starting sigs rx");
    let sigs_handle = tokio::spawn(async move {
        while let Some(sig) = sig_rx.recv().await {
            let timestamp = chrono::Utc::now().timestamp_millis();
            log::debug!("shreds: {} {}", timestamp, sig);
        }
    });

    info!("Starting shred processor");
    let mut buf = [0u8; PACKET_SIZE]; // max shred size
    let shred_processor = shred_processor.clone();
    let shred_processor_handle = tokio::spawn(async move {
        loop {
            match socket.recv_from(&mut buf).await {
                Ok((received, _)) => {
                    let packet = Vec::from(&buf[..received]);
                    shred_processor
                        .write()
                        .await
                        .collect(Arc::new(packet))
                        .await;
                }
                Err(e) => {
                    error!("Error receiving packet: {:?}", e);
                }
            }
        }
    });

    info!("Starting entry processor");
    let entry_processor_handle = match mode {
        Mode::Arb => tokio::spawn(async move {
            info!("Arb mode");
            let pools_state = Arc::new(RwLock::new(PoolsState::default()));
            pools_state.write().await.initialize().await;
            let mut entry_processor = ArbEntryProcessor::new(
                entry_rx,
                error_rx,
                pools_state.clone(),
                sig_tx,
            );
            entry_processor.receive_entries().await;
        }),
        Mode::Pump => {
            info!("Pump mode");
            tokio::spawn(async move {
                let mut entry_processor = PumpEntryProcessor::new(
                    entry_rx, error_rx, sig_tx, post_url,
                );
                entry_processor.receive_entries().await;
            })
        }
    };

    tokio::signal::ctrl_c().await?;

    info!("Shutting down");

    for handle in [
        metrics_handle,
        sigs_handle,
        shred_processor_handle,
        entry_processor_handle,
    ] {
        handle.abort();
    }

    Ok(())
}
