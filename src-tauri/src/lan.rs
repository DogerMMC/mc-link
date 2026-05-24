use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use serde::Serialize;

pub const MULTICAST_IP: &str = "224.0.2.60";
pub const MULTICAST_PORT: u16 = 4445;
pub const MULTICAST_ADDR: &str = "224.0.2.60:4445";

#[derive(Clone, Debug, Serialize)]
pub struct LanServer {
    pub motd: String,
    pub port: u16,
}

fn extract_tag(s: &str, start: &str, end: &str) -> String {
    if let Some(start_pos) = s.find(start) {
        if let Some(end_pos) = s.find(end) {
            return s[start_pos + start.len()..end_pos].to_string();
        }
    }
    String::new()
}

pub fn scan_lan_servers() -> Result<Vec<LanServer>, String> {
    let socket = UdpSocket::bind(format!("0.0.0.0:{}", MULTICAST_PORT))
        .map_err(|e| format!("绑定失败: {}", e))?;

    socket
        .join_multicast_v4(&MULTICAST_IP.parse().unwrap(), &"0.0.0.0".parse().unwrap())
        .map_err(|e| e.to_string())?;

    socket.set_read_timeout(Some(Duration::from_secs(3))).ok();

    let mut servers: HashMap<u16, LanServer> = HashMap::new();
    let mut buf = [0u8; 1024];

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        if let Ok((len, _)) = socket.recv_from(&mut buf) {
            if let Ok(data) = String::from_utf8(buf[..len].to_vec()) {
                if data.contains("[MOTD]") && data.contains("[AD]") {
                    let motd = extract_tag(&data, "[MOTD]", "[/MOTD]");
                    let port_str = extract_tag(&data, "[AD]", "[/AD]");
                    if let Ok(port) = port_str.parse::<u16>() {
                        servers.insert(port, LanServer { motd, port });
                    }
                }
            }
        }
    }

    Ok(servers.into_values().collect())
}

pub fn lan_discovery_broadcaster(running: Arc<Mutex<bool>>, motd: String, port: u16) {
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    socket.set_broadcast(true).unwrap();

    let message = format!("[MOTD]{}[/MOTD][AD]{}[/AD]", motd, port);
    let addr: SocketAddr = MULTICAST_ADDR.parse().unwrap();

    while *running.lock().unwrap() {
        socket.send_to(message.as_bytes(), addr).ok();
        std::thread::sleep(Duration::from_millis(1500));
    }
}

#[allow(dead_code)]
pub fn lan_discovery_broadcaster_atomic(running: Arc<AtomicBool>, motd: String, port: u16) {
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    socket.set_broadcast(true).unwrap();

    let message = format!("[MOTD]{}[/MOTD][AD]{}[/AD]", motd, port);
    let addr: SocketAddr = MULTICAST_ADDR.parse().unwrap();

    while !running.load(Ordering::Relaxed) {
        socket.send_to(message.as_bytes(), addr).ok();
        std::thread::sleep(Duration::from_millis(1500));
    }
}