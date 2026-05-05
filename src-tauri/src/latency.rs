use std::net::UdpSocket;
use std::time::{Duration, Instant};
use std::net::SocketAddr;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RelayWithLatency {
    pub id: String,
    pub name: String,
    pub address: String,
    pub latency_ms: Option<u64>,
}

pub async fn test_relay_latency(address: &str) -> Option<u64> {
    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(_) => return None,
    };

    let addr: SocketAddr = match address.parse() {
        Ok(a) => a,
        Err(_) => return None,
    };

    socket.set_read_timeout(Some(Duration::from_secs(2))).ok()?;

    let test_data = b"PING";
    let start = Instant::now();

    if socket.send_to(test_data, addr).is_ok() {
        let mut buf = [0u8; 128];
        if socket.recv_from(&mut buf).is_ok() {
            let elapsed = start.elapsed();
            return Some(elapsed.as_millis() as u64);
        }
    }

    None
}

pub async fn select_best_relay(relays: &[RelayWithLatency]) -> Option<&RelayWithLatency> {
    relays.iter()
        .filter(|r| r.latency_ms.is_some())
        .min_by_key(|r| r.latency_ms.unwrap())
}
