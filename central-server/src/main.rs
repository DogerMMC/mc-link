use std::collections::HashMap;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0";
const DEFAULT_LISTEN_PORT: u16 = 8878;
const DEFAULT_EXTERNAL_PORT: u16 = 8878;
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(120);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

// Proxy Protocol V2 签名
const PROXY_PROTOCOL_V2_SIGNATURE: [u8; 12] = [0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A];

#[derive(Debug, Deserialize, Serialize)]
struct Config {
    #[serde(default = "default_listen_addr")]
    listen_addr: String,
    #[serde(default = "default_listen_port")]
    listen_port: u16,
    #[serde(default = "default_external_port")]
    external_port: u16,
}

fn default_listen_addr() -> String {
    DEFAULT_LISTEN_ADDR.to_string()
}

fn default_listen_port() -> u16 {
    DEFAULT_LISTEN_PORT
}

fn default_external_port() -> u16 {
    DEFAULT_EXTERNAL_PORT
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_addr: DEFAULT_LISTEN_ADDR.to_string(),
            listen_port: DEFAULT_LISTEN_PORT,
            external_port: DEFAULT_EXTERNAL_PORT,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelayInfo {
    id: String,
    name: String,
    address: String,
    last_seen: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RoomInfo {
    name: String,
    password_hash: String,
    host_relay_id: String,
    created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LatencyEntry {
    from_id: String,
    to_id: String,
    latency_ms: u64,
    samples: Vec<u64>,
}

const RELAYS_FILE: &str = "relays.json";

struct CentralState {
    relays: Mutex<HashMap<String, RelayInfo>>,
    rooms: Mutex<HashMap<String, RoomInfo>>,
    latencies: Mutex<HashMap<String, LatencyEntry>>,
    running: Arc<Mutex<bool>>,
    addr_to_id: Mutex<HashMap<String, String>>,
}

impl CentralState {
    fn new() -> Self {
        Self {
            relays: Mutex::new(HashMap::new()),
            rooms: Mutex::new(HashMap::new()),
            latencies: Mutex::new(HashMap::new()),
            running: Arc::new(Mutex::new(true)),
            addr_to_id: Mutex::new(HashMap::new()),
        }
    }

    fn save_relays(&self) {
        let relays = self.relays.lock().unwrap();
        if let Ok(json) = serde_json::to_string_pretty(&*relays) {
            if let Err(e) = fs::write(RELAYS_FILE, json) {
                log(LogLevel::Error, &format!("保存中继列表失败: {}", e));
            }
        }
    }

    fn load_relays(&self) {
        if let Ok(content) = fs::read_to_string(RELAYS_FILE) {
            if let Ok(relays) = serde_json::from_str::<HashMap<String, RelayInfo>>(&content) {
                let mut current = self.relays.lock().unwrap();
                *current = relays;
                log(LogLevel::Info, &format!("已加载 {} 个中继服务器", current.len()));
            }
        }
    }

    fn stop(&self) {
        *self.running.lock().unwrap() = false;
    }

    fn is_running(&self) -> bool {
        *self.running.lock().unwrap()
    }

}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

enum LogLevel {
    Info,
    Warn,
    Error,
}

fn log(level: LogLevel, msg: &str) {
    let now = std::time::SystemTime::now();
    let secs = now.duration_since(UNIX_EPOCH).unwrap().as_secs();
    let hours = (secs / 3600) % 24;
    let minutes = (secs / 60) % 60;
    let seconds = secs % 60;
    
    let (level_str, color) = match level {
        LogLevel::Info => ("INFO", "\x1b[32m"),
        LogLevel::Warn => ("WARN", "\x1b[33m"),
        LogLevel::Error => ("ERROR", "\x1b[31m"),
    };
    
    print!("\x1b[s");
    println!("[{:02}:{:02}:{:02} {}{}\x1b[0m] {}", hours, minutes, seconds, color, level_str, msg);
    print!("\x1b[u> ");
    std::io::Write::flush(&mut std::io::stdout()).ok();
}

fn load_config() -> Config {
    match fs::read_to_string("config.yaml") {
        Ok(content) => {
            match serde_yaml::from_str(&content) {
                Ok(config) => {
                    log(LogLevel::Info, "已加载配置文件 config.yaml");
                    config
                }
                Err(e) => {
                    log(LogLevel::Error, &format!("配置文件解析失败，使用默认值: {}", e));
                    Config::default()
                }
            }
        }
        Err(_) => {
            log(LogLevel::Info, "未找到配置文件，正在创建默认配置...");
            let default_config = Config::default();
            if let Ok(yaml) = serde_yaml::to_string(&default_config) {
                if let Err(e) = fs::write("config.yaml", yaml) {
                    log(LogLevel::Error, &format!("创建配置文件失败: {}", e));
                } else {
                    log(LogLevel::Info, "已创建默认配置文件 config.yaml");
                }
            }
            default_config
        }
    }
}

fn read_packet(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(buf)
}

fn write_packet(stream: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
    let len_buf = (data.len() as u32).to_be_bytes();
    stream.write_all(&len_buf)?;
    stream.write_all(data)?;
    Ok(())
}

/// Proxy Protocol V2 头部信息
#[derive(Debug, Clone)]
struct ProxyProtocolHeader {
    src_addr: SocketAddr,
    dst_addr: SocketAddr,
}

/// 解析 Proxy Protocol V2 头部
fn parse_proxy_protocol_v2(data: &[u8]) -> Option<ProxyProtocolHeader> {
    // 检查最小长度（12字节签名 + 1字节版本命令 + 1字节协议族 + 2字节长度）
    if data.len() < 16 {
        return None;
    }

    // 验证签名
    if !data.starts_with(&PROXY_PROTOCOL_V2_SIGNATURE) {
        return None;
    }

    let version_cmd = data[12];
    let version = (version_cmd >> 4) & 0x0F;
    if version != 0x02 {
        return None;
    }

    let family_proto = data[13];
    let family = (family_proto >> 4) & 0x0F;
    let proto = family_proto & 0x0F;
    
    // 只处理 TCP 协议
    if proto != 0x01 {
        return None;
    }

    let len = u16::from_be_bytes(data[14..16].try_into().unwrap()) as usize;
    let header_data = &data[16..16+len];

    match family {
        // IPv4
        0x01 => {
            if header_data.len() < 12 {
                return None;
            }
            let src_ip = IpAddr::V4(Ipv4Addr::new(header_data[0], header_data[1], header_data[2], header_data[3]));
            let dst_ip = IpAddr::V4(Ipv4Addr::new(header_data[4], header_data[5], header_data[6], header_data[7]));
            let src_port = u16::from_be_bytes(header_data[8..10].try_into().unwrap());
            let dst_port = u16::from_be_bytes(header_data[10..12].try_into().unwrap());
            
            Some(ProxyProtocolHeader {
                src_addr: SocketAddr::new(src_ip, src_port),
                dst_addr: SocketAddr::new(dst_ip, dst_port),
            })
        }
        // IPv6
        0x02 => {
            if header_data.len() < 36 {
                return None;
            }
            let src_ip_bytes: [u8; 16] = header_data[0..16].try_into().unwrap();
            let dst_ip_bytes: [u8; 16] = header_data[16..32].try_into().unwrap();
            let src_ip = IpAddr::V6(Ipv6Addr::from(src_ip_bytes));
            let dst_ip = IpAddr::V6(Ipv6Addr::from(dst_ip_bytes));
            let src_port = u16::from_be_bytes(header_data[32..34].try_into().unwrap());
            let dst_port = u16::from_be_bytes(header_data[34..36].try_into().unwrap());
            
            Some(ProxyProtocolHeader {
                src_addr: SocketAddr::new(src_ip, src_port),
                dst_addr: SocketAddr::new(dst_ip, dst_port),
            })
        }
        _ => None,
    }
}

/// 尝试从流中读取并解析 Proxy Protocol V2 头部
fn read_proxy_protocol_header(stream: &mut TcpStream) -> Option<ProxyProtocolHeader> {
    let mut buf = [0u8; 16];
    match stream.peek(&mut buf) {
        Ok(n) if n >= 12 => {
            if buf[0..12] == PROXY_PROTOCOL_V2_SIGNATURE {
                // 看起来像 Proxy Protocol V2，读取完整头部
                let mut header_buf = vec![0u8; 16];
                match stream.read_exact(&mut header_buf) {
                    Ok(_) => {
                        let len = u16::from_be_bytes(header_buf[14..16].try_into().unwrap()) as usize;
                        let mut addr_buf = vec![0u8; len];
                        match stream.read_exact(&mut addr_buf) {
                            Ok(_) => {
                                let mut full_header = header_buf;
                                full_header.extend(addr_buf);
                                parse_proxy_protocol_v2(&full_header)
                            }
                            Err(_) => None,
                        }
                    }
                    Err(_) => None,
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

fn print_help() {
    println!();
    println!("===========================================");
    println!("  MC Link 中央服务器 - 帮助");
    println!("===========================================");
    println!("  h - 显示帮助");
    println!("  s - 停止服务器");
    println!("  r - 重启服务器");
    println!("  c - 显示中继列表");
    println!("  q - 退出");
    println!("===========================================");
    println!();
}

fn start_server(state: Arc<CentralState>, config: &Config) -> Option<TcpListener> {
    let bind_addr = format!("{}:{}", config.listen_addr, config.listen_port);
    match TcpListener::bind(&bind_addr) {
        Ok(listener) => {
            log(LogLevel::Info, &format!("中央服务器启动在 {}", bind_addr));

            let state_clone = state.clone();
            thread::spawn(move || {
                cleanup_thread(state_clone);
            });

            Some(listener)
        }
        Err(e) => {
            log(LogLevel::Error, &format!("无法绑定端口 {}: {}", bind_addr, e));
            None
        }
    }
}

fn main() {
    let config = load_config();
    let state = Arc::new(CentralState::new());

    println!();
    println!("===========================================");
    println!("  MC Link 中央服务器");
    println!("===========================================");
    println!("  监听地址: {}", config.listen_addr);
    println!("  监听端口: {}", config.listen_port);
    println!("===========================================");
    println!();
    println!("按 h 获取帮助");
    println!();

    state.load_relays();

    let listener = Arc::new(Mutex::new(start_server(state.clone(), &config)));

    let state_for_console = state.clone();
    let listener_for_console = listener.clone();

    thread::spawn(move || {
        loop {
            print!("> ");
            std::io::Write::flush(&mut std::io::stdout()).ok();
            
            let mut input = String::new();
            match std::io::stdin().read_line(&mut input) {
                Ok(0) => {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
                Err(_) => {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
                _ => {}
            }
            
            let input = input.trim();
            match input {
                "h" => print_help(),
                "s" => {
                    log(LogLevel::Info, "正在停止服务器...");
                    state_for_console.stop();
                }
                "r" => {
                    log(LogLevel::Info, "正在重启服务器...");
                    state_for_console.stop();
                    thread::sleep(Duration::from_secs(1));
                    let new_listener = start_server(state_for_console.clone(), &config);
                    if new_listener.is_some() {
                        *listener_for_console.lock().unwrap() = new_listener;
                        log(LogLevel::Info, "服务器已重启");
                    }
                }
                "c" => {
                    let relays = state_for_console.relays.lock().unwrap();
                    println!("\n中继列表 (共 {} 个):", relays.len());
                    for (_id, relay) in relays.iter() {
                        println!("  - {} @ {} (最后活跃: {}s前)", relay.name, relay.address, now_secs() - relay.last_seen);
                    }
                    println!();
                }
                "q" => {
                    log(LogLevel::Info, "正在关闭服务器...");
                    state_for_console.stop();
                    state_for_console.save_relays();
                    break;
                }
                "" => {}
                _ => {
                    log(LogLevel::Warn, &format!("未知命令: {}", input));
                    print_help();
                }
            }
        }
    });

    loop {
        if !state.is_running() {
            break;
        }

        if let Some(ref listener) = *listener.clone().lock().unwrap() {
            match listener.accept() {
                Ok((mut stream, addr)) => {
                    let real_addr = match read_proxy_protocol_header(&mut stream) {
                        Some(header) => {
                            log(LogLevel::Info, &format!("Proxy Protocol V2 解析成功 - 客户端: {}, 中继: {}", header.src_addr, header.dst_addr));
                            header.src_addr
                        }
                        None => addr,
                    };
                    let state_clone = state.clone();
                    thread::spawn(move || {
                        handle_client(&mut stream, state_clone, real_addr);
                    });
                }
                Err(_) => {}
            }
        }

        thread::sleep(Duration::from_millis(10));
    }

    state.save_relays();
    log(LogLevel::Info, "中央服务器已关闭");
}

fn handle_client(stream: &mut TcpStream, state: Arc<CentralState>, addr: SocketAddr) {
    loop {
        match read_packet(stream) {
            Ok(data) => {
                if data.is_empty() {
                    continue;
                }
                handle_packet(stream, &state, addr, &data);
            }
            Err(_) => break,
        }
    }

    let addr_str = addr.to_string();
    let mut addr_to_id = state.addr_to_id.lock().unwrap();
    if let Some(relay_id) = addr_to_id.remove(&addr_str) {
        drop(addr_to_id);
        if let Some(relay) = state.relays.lock().unwrap().remove(&relay_id) {
            log(LogLevel::Warn, &format!("[中继/断开] 中继服务器 {} 已离线", relay.name));
        }
    }
}

fn handle_packet(stream: &mut TcpStream, state: &CentralState, src: SocketAddr, data: &[u8]) {
    if data.is_empty() {
        return;
    }

    let cmd = data[0];
    let payload = &data[1..];

    match cmd {
        0x10 => handle_relay_register(stream, state, src, payload),
        0x11 => handle_relay_heartbeat(state, payload),
        0x12 => handle_get_relays(stream, state),
        0x14 => handle_latency_report(state, payload),
        0x15 => handle_get_topology(stream, state),
        0x20 => handle_create_room(stream, state, src, payload),
        0x22 => handle_get_room(stream, state, src, payload),
        0x24 => handle_delete_room(stream, state, src, payload),
        _ => log(LogLevel::Warn, &format!("未知命令: 0x{:02x}", cmd)),
    }
}

#[derive(Deserialize)]
struct RelayRegisterReq {
    id: String,
    name: String,
    address: String,
}

fn handle_relay_register(stream: &mut TcpStream, state: &CentralState, src: SocketAddr, data: &[u8]) {
    if let Ok(req) = serde_json::from_slice::<RelayRegisterReq>(data) {
        let mut relays = state.relays.lock().unwrap();
        let mut addr_to_id = state.addr_to_id.lock().unwrap();

        let relay = RelayInfo {
            id: req.id.clone(),
            name: req.name.clone(),
            address: req.address.clone(),
            last_seen: now_secs(),
        };

        relays.insert(req.id.clone(), relay);
        addr_to_id.insert(src.to_string(), req.id.clone());
        drop(addr_to_id);
        
        log(LogLevel::Info, &format!("[中继/注册] 中继服务器 {} 注册在 {}", req.name, req.address));
        
        let mut response = vec![0x10];
        response.push(0x00);
        write_packet(stream, &response).ok();
    }
}

#[derive(Deserialize)]
struct RelayHeartbeatReq {
    id: String,
}

fn handle_relay_heartbeat(state: &CentralState, data: &[u8]) {
    if let Ok(req) = serde_json::from_slice::<RelayHeartbeatReq>(data) {
        let mut relays = state.relays.lock().unwrap();
        if let Some(relay) = relays.get_mut(&req.id) {
            relay.last_seen = now_secs();
        }
    }
}

fn handle_get_relays(stream: &mut TcpStream, state: &CentralState) {
    let relays = state.relays.lock().unwrap();
    let relay_list: Vec<&RelayInfo> = relays.values().collect();

    let mut response = vec![0x13];
    if let Ok(json) = serde_json::to_string(&relay_list) {
        response.extend_from_slice(json.as_bytes());
    }

    write_packet(stream, &response).ok();
}

#[derive(Deserialize)]
struct LatencyReportReq {
    from_id: String,
    to_id: String,
    latency_ms: u64,
}

fn handle_latency_report(state: &CentralState, data: &[u8]) {
    if let Ok(req) = serde_json::from_slice::<LatencyReportReq>(data) {
        let key = format!("{}->{}", req.from_id, req.to_id);
        let mut latencies = state.latencies.lock().unwrap();

        let entry = latencies.entry(key).or_insert_with(|| LatencyEntry {
            from_id: req.from_id.clone(),
            to_id: req.to_id.clone(),
            latency_ms: 0,
            samples: Vec::new(),
        });

        entry.samples.push(req.latency_ms);
        if entry.samples.len() > 10 {
            entry.samples.remove(0);
        }

        let avg = entry.samples.iter().sum::<u64>() / entry.samples.len() as u64;
        entry.latency_ms = avg;
    }
}

fn handle_get_topology(stream: &mut TcpStream, state: &CentralState) {
    let relays = state.relays.lock().unwrap();
    let latencies = state.latencies.lock().unwrap();

    let relay_list: Vec<&RelayInfo> = relays.values().collect();

    let mut latency_matrix: HashMap<String, HashMap<String, u64>> = HashMap::new();
    for (key, entry) in latencies.iter() {
        let parts: Vec<&str> = key.split("->").collect();
        if parts.len() == 2 {
            latency_matrix
                .entry(parts[0].to_string())
                .or_insert_with(HashMap::new)
                .insert(parts[1].to_string(), entry.latency_ms);
        }
    }

    let response_data = serde_json::to_string(&serde_json::json!({
        "relays": relay_list,
        "latency_matrix": latency_matrix
    })).unwrap();

    let mut response = vec![0x16];
    response.extend_from_slice(response_data.as_bytes());

    write_packet(stream, &response).ok();
}

#[derive(Deserialize)]
struct CreateRoomReq {
    room_name: String,
    password: String,
    relay_id: String,
}

fn handle_create_room(stream: &mut TcpStream, state: &CentralState, src: SocketAddr, data: &[u8]) {
    if let Ok(req) = serde_json::from_slice::<CreateRoomReq>(data) {
        let mut rooms = state.rooms.lock().unwrap();

        if rooms.contains_key(&req.room_name) {
            let mut response = vec![0x21];
            response.push(0x01);
            write_packet(stream, &response).ok();
            return;
        }

        let room = RoomInfo {
            name: req.room_name.clone(),
            password_hash: req.password,
            host_relay_id: req.relay_id,
            created_at: now_secs(),
        };

        rooms.insert(req.room_name.clone(), room.clone());
        log(LogLevel::Info, &format!("房间创建: {} (来自 {})", req.room_name, src));

        let mut response = vec![0x21, 0x00];
        if let Ok(json) = serde_json::to_string(&room) {
            response.extend_from_slice(json.as_bytes());
        }
        write_packet(stream, &response).ok();
    }
}

#[derive(Deserialize)]
struct GetRoomReq {
    room_name: String,
}

fn handle_get_room(stream: &mut TcpStream, state: &CentralState, src: SocketAddr, data: &[u8]) {
    if let Ok(req) = serde_json::from_slice::<GetRoomReq>(data) {
        let rooms = state.rooms.lock().unwrap();

        if rooms.get(&req.room_name).is_some() {
            log(LogLevel::Info, &format!("[房间/加入] 玩家加入房间: {} (来自 {})", req.room_name, src));
        } else {
            log(LogLevel::Info, &format!("[房间/查询] 查询房间: {} (来自 {}, 不存在)", req.room_name, src));
        }

        let response_data = if let Some(room) = rooms.get(&req.room_name) {
            serde_json::to_string(&serde_json::json!({
                "exists": true,
                "room": room
            })).unwrap()
        } else {
            serde_json::to_string(&serde_json::json!({
                "exists": false,
                "room": null
            })).unwrap()
        };

        let mut response = vec![0x23];
        response.extend_from_slice(response_data.as_bytes());
        write_packet(stream, &response).ok();
    }
}

#[derive(Deserialize)]
struct DeleteRoomReq {
    room_name: String,
}

fn handle_delete_room(stream: &mut TcpStream, state: &CentralState, src: SocketAddr, data: &[u8]) {
    if let Ok(req) = serde_json::from_slice::<DeleteRoomReq>(data) {
        let mut rooms = state.rooms.lock().unwrap();
        
        if rooms.remove(&req.room_name).is_some() {
            log(LogLevel::Info, &format!("房间已删除: {} (来自 {})", req.room_name, src));
            write_packet(stream, &[0x25, 0x00]).ok();
        } else {
            log(LogLevel::Warn, &format!("删除房间失败: {} 不存在 (来自 {})", req.room_name, src));
            write_packet(stream, &[0x25, 0x01]).ok();
        }
    }
}

fn cleanup_thread(state: Arc<CentralState>) {
    loop {
        thread::sleep(CLEANUP_INTERVAL);

        let now = now_secs();

        let mut relays = state.relays.lock().unwrap();
        relays.retain(|id, relay| {
            let alive = now - relay.last_seen < HEARTBEAT_TIMEOUT.as_secs();
            if !alive {
                log(LogLevel::Warn, &format!("移除离线中继: {} (ID: {})", relay.name, id));
            }
            alive
        });
        drop(relays);

        let mut rooms = state.rooms.lock().unwrap();
        let before = rooms.len();
        rooms.retain(|_, room| now - room.created_at < 86400);
        let removed = before - rooms.len();
        if removed > 0 {
            log(LogLevel::Info, &format!("清理过期房间: {} 个", removed));
        }
    }
}
