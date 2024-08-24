use log::{error, info};
use std::io::Write;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::signal;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{sleep, Duration};

use crate::algo;
use crate::processor::Processor;

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
) -> Result<Vec<(u64, String)>, Box<dyn std::error::Error>> {
    let socket = Arc::new(
        UdpSocket::bind(bind_addr)
            .await
            .expect("Couldn't bind to address"),
    );
    let (entry_sender, entry_receiver) = tokio::sync::mpsc::channel(2000);
    let (error_sender, error_receiver) = tokio::sync::mpsc::channel(2000);
    let (sig_sender, mut sig_receiver) = tokio::sync::mpsc::channel(2000);
    let mut processor = Processor::new(entry_sender, error_sender);

    let mut buf = [0u8; PACKET_SIZE]; // max shred size
    tokio::spawn(async move {
        loop {
            match socket.recv_from(&mut buf).await {
                Ok((received, _)) => {
                    let packet = Vec::from(&buf[..received]);
                    processor.collect(Arc::new(packet)).await;
                }
                Err(e) => {
                    error!("Error receiving packet: {:?}", e);
                }
            }
        }
    });

    tokio::spawn(async move {
        algo::receive_entries(
            entry_receiver,
            error_receiver,
            Arc::new(sig_sender),
        )
        .await;
    });

    let sigs = Arc::new(RwLock::new(Vec::new()));
    tokio::spawn({
        let sigs = sigs.clone();
        async move {
            while let Some(sig) = sig_receiver.recv().await {
                let mut sigs = sigs.write().await;
                let timestamp = chrono::Utc::now().timestamp();
                sigs.push((timestamp as u64, sig.clone()));
            }
        }
    });

    signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");

    Ok(sigs.clone().read().await.clone())
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
    let receiver = received_packets.clone();
    let socket_clone = socket.clone();

    let listen_handle = tokio::spawn(async move {
        listen(socket_clone, receiver).await;
    });

    let packets_clone = received_packets.clone();
    tokio::spawn(async move {
        loop {
            let packets = packets_clone.lock().await;
            info!("Total packets received: {}", packets.len());
            drop(packets);
            sleep(Duration::from_secs(1)).await;
        }
    });

    signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
    info!("Ctrl+C received, dumping packets to file...");
    dump_to_file(received_packets).await;
    listen_handle.abort();

    Ok(())
}
