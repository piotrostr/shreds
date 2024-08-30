use log::{error, info};
use std::io::Write;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::signal;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{sleep, Duration};

use crate::arb::PoolsState;
use crate::benchmark::Sigs;
use crate::entry_processor::{ArbEntryProcessor, PumpEntryProcessor};
use crate::service::Mode;
use crate::shred_processor::ShredProcessor;

pub const PACKET_SIZE: usize = 1280 - 40 - 8;

pub async fn listen(
    socket: Arc<UdpSocket>,
    received_packets: Arc<Mutex<Vec<Vec<u8>>>>,
) {
    let mut buf = [0u8; PACKET_SIZE]; // max shred size
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((received, _)) => {
                let packet = Vec::from(&buf[..received]);
                received_packets.lock().await.push(packet);
            }
            Err(e) => {
                error!("Error receiving packet: {:?}", e);
            }
        }
    }
}

pub async fn dump_to_file(received_packets: Arc<Mutex<Vec<Vec<u8>>>>) {
    let packets = received_packets.lock().await;
    let mut file =
        std::fs::File::create("packets.json").expect("Couldn't create file");
    let as_json = serde_json::to_string(&packets.clone())
        .expect("Couldn't serialize to json");
    file.write_all(as_json.as_bytes())
        .expect("Couldn't write to file");
    info!("Packets dumped to packets.json");
}

pub async fn run_listener_with_algo(
    bind_addr: &str,
    shreds_sigs: Option<Sigs>,
    mode: Mode,
    post_url: String,
    benchmark: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let socket = Arc::new(
        UdpSocket::bind(bind_addr)
            .await
            .expect("Couldn't bind to address"),
    );
    let (entry_tx, entry_rx) = tokio::sync::mpsc::channel(2000);
    let (error_tx, error_rx) = tokio::sync::mpsc::channel(2000);
    let (sig_tx, mut sig_rx) = tokio::sync::mpsc::channel(2000);
    let shred_processor =
        Arc::new(RwLock::new(ShredProcessor::new(entry_tx, error_tx)));

    info!("Listening on {}", bind_addr);

    // metrics loop
    info!("Starting metrics loop");
    let shred_processor_clone = shred_processor.clone();
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(6)).await;
            {
                let metrics = shred_processor_clone.read().await.metrics();
                info!("metrics: {:?}", metrics);
                drop(metrics);
            }
        }
    });

    info!("Starting shred processor");
    let mut buf = [0u8; PACKET_SIZE]; // max shred size
    let shred_processor = shred_processor.clone();
    tokio::spawn(async move {
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
    match mode {
        Mode::Arb => tokio::spawn(async move {
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
            info!("Starting entries rx (<=> webhook tx) pump mode");
            tokio::spawn(async move {
                let mut entry_processor = PumpEntryProcessor::new(
                    entry_rx, error_rx, sig_tx, post_url,
                );
                entry_processor.receive_entries().await;
            })
        }
    };

    info!("Starting sigs loop");
    tokio::spawn({
        let shreds_sigs = shreds_sigs.clone();
        async move {
            while let Some(sig) = sig_rx.recv().await {
                if benchmark {
                    if let Some(shreds_sigs) = &shreds_sigs {
                        let timestamp = chrono::Utc::now().timestamp_millis();
                        info!("algo: {} {}", timestamp, sig);
                        shreds_sigs
                            .write()
                            .await
                            .push((timestamp as u64, sig.clone()));
                    }
                }
            }
        }
    });

    signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");

    Ok(())
}

pub async fn run_listener_with_save(
    bind_addr: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let socket = Arc::new(
        UdpSocket::bind(bind_addr)
            .await
            .expect("Couldn't bind to address"),
    );
    let received_packets = Arc::new(Mutex::new(Vec::new()));

    info!("Listening on {}", bind_addr);
    let rx = received_packets.clone();
    let socket_clone = socket.clone();

    tokio::spawn(async move {
        listen(socket_clone, rx).await;
    });

    loop {
        let packets = received_packets.lock().await;
        info!("Total packets received: {}", packets.len());
        if packets.len() > 100_000 {
            info!("Dumping packets to file");
            break;
        }
        drop(packets);
        sleep(Duration::from_secs(1)).await;
    }
    dump_to_file(received_packets).await;
    Ok(())
}
