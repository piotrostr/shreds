use log::{error, info};
use std::{
    net::UdpSocket,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

const PACKET_DATA_SIZE: usize = 1024; // use correct size from your context

fn listen(socket: UdpSocket, received_packets: Arc<Mutex<Vec<Vec<u8>>>>) {
    let mut buf = [0u8; PACKET_DATA_SIZE];
    loop {
        match socket.recv(&mut buf) {
            Ok(received) => {
                info!("Received packet of size: {}", received);
                let packet = Vec::from(&buf[..received]);
                received_packets.lock().unwrap().push(packet);
            }
            Err(e) => {
                error!("Error receiving packet: {:?}", e);
            }
        }
    }
}

fn main() {
    env_logger::init();
    let bind_addr = "0.0.0.0:4000"; // Change this to the port you want to listen to.
    let socket = UdpSocket::bind(bind_addr).expect("Couldn't bind to address");
    let received_packets = Arc::new(Mutex::new(Vec::new()));

    info!("Listening on {}", bind_addr);
    let receiver = received_packets.clone();

    thread::spawn(move || {
        listen(socket, receiver);
    });

    loop {
        // Optionally process the received packets
        let packets = received_packets.lock().unwrap();
        info!("Total packets received: {}", packets.len());
        thread::sleep(Duration::from_secs(5));
    }
}
