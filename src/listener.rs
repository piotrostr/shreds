use log::{error, info};
use std::io::Write;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::signal;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

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

pub async fn run_listener_with_processor(
) -> Result<(), Box<dyn std::error::Error>> {
    let bind_addr = "0.0.0.0:8001";
    let socket = Arc::new(
        UdpSocket::bind(bind_addr)
            .await
            .expect("Couldn't bind to address"),
    );
    let mut processor = Processor::new();

    let mut buf = [0u8; PACKET_SIZE]; // max shred size
    tokio::spawn(async move {
        loop {
            match socket.recv_from(&mut buf).await {
                Ok((received, _)) => {
                    let packet = Vec::from(&buf[..received]);
                    processor.collect(packet).await;
                }
                Err(e) => {
                    error!("Error receiving packet: {:?}", e);
                }
            }
        }
    });

    signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");

    Ok(())
}

/// method used for data collection
pub async fn run_listener_with_save() -> Result<(), Box<dyn std::error::Error>>
{
    let bind_addr = "0.0.0.0:8001";
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
