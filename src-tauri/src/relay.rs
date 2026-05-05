use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::collections::{HashMap, HashSet};
use crate::crypto;
use rand::Rng;

const RELAY_PORT: u16 = 57894;
const CENTRAL_SERVER_ADDR: &str = "central-server.link.xigo.top:50248";
const PACKET_TIMEOUT: Duration = Duration::from_secs(5);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);
const LATENCY_REPORT_INTERVAL: Duration = Duration::from_secs(300);
const GOSSIP_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct RelayInfo {
    id: String,
    name: String,
    address: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct RoomInfo {
    name: String,
    password_hash: String,
    host_relay_id: String,
    created_at: u64,
}

#[derive(Clone)]
struct Client {
    addr: SocketAddr,
    role: String,
    room: String,
    password: String,
    last_seen: Instant,
}

struct Room {
    host: Option<Client>,
    host_relay_id: Option<String>,
    clients: Vec<Client>,
}

pub struct RelayMode {
    running: Arc<Mutex<bool>>,
    rooms: Arc<Mutex<HashMap<String, Room>>>,
    packet_cache: Arc<Mutex<HashMap<u64, Instant>>>,
    relay_id: String,
    relay_name: String,
    known_relays: Arc<Mutex<HashMap<String, RelayInfo>>>,
    known_rooms: Arc<Mutex<HashMap<String, RoomInfo>>>,
}

impl RelayMode {
    pub fn new() -> Self {
        let id = format!("relay-{}", rand::thread_rng().gen::<u64>());
        let relay_name = format!("Relay-{}", &id[6..]);
        Self {
            running: Arc::new(Mutex::new(false)),
            rooms: Arc::new(Mutex::new(HashMap::new())),
            packet_cache: Arc::new(Mutex::new(HashMap::new())),
            relay_id: id,
            relay_name,
            known_relays: Arc::new(Mutex::new(HashMap::new())),
            known_rooms: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn start(&mut self) -> Result<String, String> {
        let running = self.running.clone();
        *running.lock().unwrap() = true;

        let rooms = self.rooms.clone();
        let packet_cache = self.packet_cache.clone();
        let known_relays = self.known_relays.clone();
        let known_rooms = self.known_rooms.clone();
        let relay_id = self.relay_id.clone();
        let relay_name = self.relay_name.clone();

        let socket = UdpSocket::bind(("0.0.0.0", RELAY_PORT))
            .map_err(|e| format!("中继服务器绑定失败: {}", e))?;
        socket.set_nonblocking(true).map_err(|e| e.to_string())?;
        let socket = Arc::new(socket);

        println!("中继服务器 {} 启动在端口 {}", relay_id, RELAY_PORT);

        let socket_clone = socket.clone();
        let running_clone = running.clone();
        let rooms_clone = rooms.clone();
        let packet_cache_clone = packet_cache.clone();
        let known_relays_clone = known_relays.clone();
        let known_rooms_clone = known_rooms.clone();
        thread::spawn(move || {
            Self::relay_loop(socket_clone, running_clone, rooms_clone, packet_cache_clone, known_relays_clone, known_rooms_clone);
        });

        let socket_clone = socket.clone();
        let running_clone = running.clone();
        let rooms_clone = rooms.clone();
        let packet_cache_clone = packet_cache.clone();
        thread::spawn(move || {
            Self::cleanup_loop(running_clone, rooms_clone, packet_cache_clone);
        });

        let socket_clone = socket.clone();
        let running_clone = running.clone();
        let relay_id_clone = relay_id.clone();
        let relay_name_clone = relay_name.clone();
        let known_relays_clone2 = known_relays.clone();
        thread::spawn(move || {
            Self::central_communication_loop(socket_clone, running_clone, relay_id_clone, relay_name_clone, known_relays_clone2);
        });

        let socket_clone = socket.clone();
        let running_clone = running.clone();
        let relay_id_clone = relay_id.clone();
        let known_relays_clone3 = known_relays.clone();
        let known_rooms_clone2 = known_rooms.clone();
        thread::spawn(move || {
            Self::gossip_loop(socket_clone, running_clone, relay_id_clone, known_relays_clone3, known_rooms_clone2);
        });

        Ok(format!("中继服务器已启动，端口: {}", RELAY_PORT))
    }

    fn relay_loop(
        socket: Arc<UdpSocket>,
        running: Arc<Mutex<bool>>,
        rooms: Arc<Mutex<HashMap<String, Room>>>,
        packet_cache: Arc<Mutex<HashMap<u64, Instant>>>,
        known_relays: Arc<Mutex<HashMap<String, RelayInfo>>>,
        known_rooms: Arc<Mutex<HashMap<String, RoomInfo>>>,
    ) {
        let mut buf = vec![0u8; 65535];
        while *running.lock().unwrap() {
            match socket.recv_from(&mut buf) {
                Ok((len, src)) => {
                    if len < 1 {
                        continue;
                    }
                    
                    let cmd = buf[0];
                    let data = &buf[1..len];
                    
                    match cmd {
                        0x32 => Self::handle_ping(&socket, src),
                        0x33 => continue,
                        _ => Self::handle_game_packet(&socket, &rooms, &packet_cache, cmd, data, src),
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(e) => {
                    println!("接收错误: {}", e);
                }
            }
        }
    }

    fn handle_ping(socket: &UdpSocket, src: SocketAddr) {
        socket.send_to(&[0x33], src).ok();
    }

    fn handle_game_packet(
        socket: &UdpSocket,
        rooms: &Arc<Mutex<HashMap<String, Room>>>,
        packet_cache: &Arc<Mutex<HashMap<u64, Instant>>>,
        cmd: u8,
        data: &[u8],
        src: SocketAddr,
    ) {
        let mut ptr = 0;
        if data.len() < 1 {
            return;
        }
        
        let room_name_len = data[ptr] as usize;
        ptr += 1;
        if data.len() < ptr + room_name_len {
            return;
        }
        
        let room_name_bytes = &data[ptr..ptr + room_name_len];
        let room_name = String::from_utf8_lossy(room_name_bytes).to_string();
        ptr += room_name_len;
        
        if data.len() < ptr + 1 {
            return;
        }
        
        let password_len = data[ptr] as usize;
        ptr += 1;
        if data.len() < ptr + password_len {
            return;
        }
        
        let password_bytes = &data[ptr..ptr + password_len];
        let password = String::from_utf8_lossy(password_bytes).to_string();
        ptr += password_len;
        
        let encrypted_data = &data[ptr..];
        
        let decrypted = match crypto::decrypt(encrypted_data, &password) {
            Some(d) => d,
            None => return,
        };
        
        let checksum = crypto::calculate_checksum(&decrypted);
        {
            let mut cache = packet_cache.lock().unwrap();
            if cache.contains_key(&checksum) {
                return;
            }
            cache.insert(checksum, Instant::now());
        }
        
        if decrypted.len() >= 4 {
            let inner_cmd = &decrypted[0..4];
            match inner_cmd {
                b"REGH" => Self::handle_register_host(&socket, &rooms, &room_name, &password, src),
                b"REGC" => Self::handle_register_client(&socket, &rooms, &room_name, &password, src),
                b"DATA" => Self::handle_forward_data(&socket, &rooms, &packet_cache, &room_name, &password, &decrypted[4..], src),
                _ => {}
            }
        }
    }

    fn handle_register_host(
        socket: &UdpSocket,
        rooms: &Arc<Mutex<HashMap<String, Room>>>,
        room_name: &str,
        password: &str,
        src: SocketAddr,
    ) {
        let mut rooms_lock = rooms.lock().unwrap();
        let room_entry = rooms_lock.entry(room_name.to_string()).or_insert(Room {
            host: None,
            host_relay_id: None,
            clients: Vec::new(),
        });
        
        room_entry.host = Some(Client {
            addr: src,
            role: "host".to_string(),
            room: room_name.to_string(),
            password: password.to_string(),
            last_seen: Instant::now(),
        });
        
        let response = pack_packet(room_name, password, b"REGH_OK");
        socket.send_to(&response, src).ok();
        
        println!("房主注册: {} 房间: {}", src, room_name);
    }

    fn handle_register_client(
        socket: &UdpSocket,
        rooms: &Arc<Mutex<HashMap<String, Room>>>,
        room_name: &str,
        password: &str,
        src: SocketAddr,
    ) {
        let mut rooms_lock = rooms.lock().unwrap();
        let room_entry = rooms_lock.entry(room_name.to_string()).or_insert(Room {
            host: None,
            host_relay_id: None,
            clients: Vec::new(),
        });
        
        room_entry.clients.push(Client {
            addr: src,
            role: "client".to_string(),
            room: room_name.to_string(),
            password: password.to_string(),
            last_seen: Instant::now(),
        });
        
        let response = pack_packet(room_name, password, b"REGC_OK");
        socket.send_to(&response, src).ok();
        
        println!("成员注册: {} 房间: {}", src, room_name);
    }

    fn handle_forward_data(
        socket: &UdpSocket,
        rooms: &Arc<Mutex<HashMap<String, Room>>>,
        packet_cache: &Arc<Mutex<HashMap<u64, Instant>>>,
        room_name: &str,
        password: &str,
        data: &[u8],
        src: SocketAddr,
    ) {
        let mut payload = Vec::with_capacity(4 + data.len());
        payload.extend_from_slice(b"DATA");
        payload.extend_from_slice(data);
        
        let packet = pack_packet(room_name, password, &payload);
        
        let rooms_lock = rooms.lock().unwrap();
        if let Some(room_entry) = rooms_lock.get(room_name) {
            let is_host = room_entry.host.as_ref().map(|h| h.addr == src).unwrap_or(false);
            
            if is_host {
                for client in &room_entry.clients {
                    if client.password == password {
                        socket.send_to(&packet, client.addr).ok();
                    }
                }
            } else {
                if let Some(ref host) = room_entry.host {
                    if host.password == password {
                        socket.send_to(&packet, host.addr).ok();
                    }
                }
            }
        }
    }

    fn cleanup_loop(
        running: Arc<Mutex<bool>>,
        rooms: Arc<Mutex<HashMap<String, Room>>>,
        packet_cache: Arc<Mutex<HashMap<u64, Instant>>>,
    ) {
        while *running.lock().unwrap() {
            thread::sleep(Duration::from_secs(10));
            
            let mut cache = packet_cache.lock().unwrap();
            let now = Instant::now();
            cache.retain(|_, timestamp| now.duration_since(*timestamp) < PACKET_TIMEOUT);
            drop(cache);
            
            let mut rooms_lock = rooms.lock().unwrap();
            let now = Instant::now();
            
            for room in rooms_lock.values_mut() {
                if let Some(ref host) = room.host {
                    if now.duration_since(host.last_seen) > Duration::from_secs(60) {
                        room.host = None;
                    }
                }
                
                room.clients.retain(|c| now.duration_since(c.last_seen) < Duration::from_secs(60));
            }
            
            rooms_lock.retain(|_, room| room.host.is_some() || !room.clients.is_empty());
        }
    }

    fn central_communication_loop(
        socket: Arc<UdpSocket>,
        running: Arc<Mutex<bool>>,
        relay_id: String,
        relay_name: String,
        known_relays: Arc<Mutex<HashMap<String, RelayInfo>>>,
    ) {
        let central_addr: SocketAddr = match CENTRAL_SERVER_ADDR.parse() {
            Ok(addr) => addr,
            Err(_) => return,
        };
        
        let my_address = format!("0.0.0.0:{}", RELAY_PORT);
        
        let register_req = serde_json::json!({
            "id": relay_id.clone(),
            "name": relay_name,
            "address": my_address,
        });
        let mut register_packet = vec![0x10];
        register_packet.extend_from_slice(&serde_json::to_vec(&register_req).unwrap_or_default());
        socket.send_to(&register_packet, central_addr).ok();
        
        let mut last_heartbeat = Instant::now();
        let mut last_latency_report = Instant::now();
        
        while *running.lock().unwrap() {
            if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                let heartbeat_req = serde_json::json!({ "id": relay_id.clone() });
                let mut packet = vec![0x11];
                packet.extend_from_slice(&serde_json::to_vec(&heartbeat_req).unwrap_or_default());
                socket.send_to(&packet, central_addr).ok();
                last_heartbeat = Instant::now();
            }
            
            if last_latency_report.elapsed() >= LATENCY_REPORT_INTERVAL {
                let relays = known_relays.lock().unwrap();
                for (other_id, other_relay) in relays.iter() {
                    if other_id != &relay_id {
                        if let Ok(other_addr) = other_relay.address.parse::<SocketAddr>() {
                            if let Some(latency) = measure_latency(&socket, other_addr) {
                                let report_req = serde_json::json!({
                                    "from_id": relay_id.clone(),
                                    "to_id": other_id.clone(),
                                    "latency_ms": latency,
                                });
                                let mut packet = vec![0x14];
                                packet.extend_from_slice(&serde_json::to_vec(&report_req).unwrap_or_default());
                                socket.send_to(&packet, central_addr).ok();
                            }
                        }
                    }
                }
                last_latency_report = Instant::now();
            }
            
            thread::sleep(Duration::from_secs(1));
        }
    }

    fn gossip_loop(
        socket: Arc<UdpSocket>,
        running: Arc<Mutex<bool>>,
        relay_id: String,
        known_relays: Arc<Mutex<HashMap<String, RelayInfo>>>,
        known_rooms: Arc<Mutex<HashMap<String, RoomInfo>>>,
    ) {
        while *running.lock().unwrap() {
            thread::sleep(GOSSIP_INTERVAL);
            
            let relays = known_relays.lock().unwrap();
            if relays.len() > 0 {
                let random_relay = relays.values().next();
                if let Some(relay) = random_relay {
                    if let Ok(addr) = relay.address.parse::<SocketAddr>() {
                        let rooms = known_rooms.lock().unwrap();
                        let room_list: Vec<&RoomInfo> = rooms.values().collect();
                        
                        let gossip_data = serde_json::json!({
                            "rooms": room_list,
                        });
                        let mut packet = vec![0x30];
                        packet.extend_from_slice(&serde_json::to_vec(&gossip_data).unwrap_or_default());
                        socket.send_to(&packet, addr).ok();
                    }
                }
            }
        }
    }

    pub fn stop(&mut self) {
        *self.running.lock().unwrap() = false;
        self.rooms.lock().unwrap().clear();
        self.packet_cache.lock().unwrap().clear();
    }
}

fn pack_packet(room: &str, password: &str, data: &[u8]) -> Vec<u8> {
    let encrypted = crypto::encrypt(data, password);
    
    let mut packet = Vec::new();
    packet.push(room.len() as u8);
    packet.extend_from_slice(room.as_bytes());
    packet.push(password.len() as u8);
    packet.extend_from_slice(password.as_bytes());
    packet.extend_from_slice(&encrypted);
    
    packet
}

fn measure_latency(socket: &UdpSocket, target: SocketAddr) -> Option<u64> {
    let start = Instant::now();
    socket.send_to(&[0x32], target).ok();
    
    let mut buf = vec![0u8; 10];
    socket.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    
    if let Ok((_, _)) = socket.recv_from(&mut buf) {
        Some(start.elapsed().as_millis() as u64)
    } else {
        None
    }
}
