use log::{error, info};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

const PACKET_DATA_SIZE: usize = 1024; // use correct size from your context

async fn listen(socket: Arc<UdpSocket>, received_packets: Arc<Mutex<Vec<Vec<u8>>>>) {
    let mut buf = [0u8; PACKET_DATA_SIZE];
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

#[tokio::main]
async fn main() {
    std::env::set_var("RUST_LOG", "info");
    env_logger::init();

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

    tokio::spawn(async move {
        listen(socket_clone, receiver).await;
    });

    loop {
        // Optionally process the received packets
        let packets = received_packets.lock().await;
        info!("Total packets received: {}", packets.len());
        drop(packets); // Explicitly drop the lock
        sleep(Duration::from_secs(1)).await;
    }
}
