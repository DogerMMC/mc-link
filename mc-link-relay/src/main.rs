use std::collections::HashMap;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::io::{Read, Write};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use aes::Aes256;
use cipher::{BlockEncrypt, BlockDecrypt, KeyInit};
use cipher::generic_array::GenericArray;
use sha2::{Sha256, Digest};

const DEFAULT_CENTRAL_SERVER: &str = "mk.aini2.cn:8878";
const DEFAULT_RELAY_PORT: u16 = 57894;
const DEFAULT_HEARTBEAT_INTERVAL: u64 = 5;
const DEFAULT_LATENCY_CHECK_INTERVAL: u64 = 300;
const MIN_BANDWIDTH_MBPS: f64 = 10.0;
const FALLBACK_CENTRAL_SERVER: &str = "";

// Proxy Protocol V2 签名
const PROXY_PROTOCOL_V2_SIGNATURE: [u8; 12] = [0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A];

#[derive(Debug, Deserialize, Serialize)]
struct Config {
    #[serde(default = "default_central_server")]
    central_server: String,
    #[serde(default = "default_relay_port")]
    relay_port: u16,
    #[serde(default = "default_report_address")]
    report_address: Option<String>,
    #[serde(default = "default_heartbeat_interval")]
    heartbeat_interval: u64,
    #[serde(default = "default_latency_check_interval")]
    latency_check_interval: u64,
    #[serde(default = "default_bandwidth_limit")]
    bandwidth_limit_mbps: Option<f64>,
}

fn default_central_server() -> String {
    DEFAULT_CENTRAL_SERVER.to_string()
}

fn default_relay_port() -> u16 {
    DEFAULT_RELAY_PORT
}

fn default_heartbeat_interval() -> u64 {
    DEFAULT_HEARTBEAT_INTERVAL
}

fn default_latency_check_interval() -> u64 {
    DEFAULT_LATENCY_CHECK_INTERVAL
}

fn default_bandwidth_limit() -> Option<f64> {
    None
}

fn default_report_address() -> Option<String> {
    None
}

impl Default for Config {
    fn default() -> Self {
        Self {
            central_server: DEFAULT_CENTRAL_SERVER.to_string(),
            relay_port: DEFAULT_RELAY_PORT,
            report_address: None,
            heartbeat_interval: DEFAULT_HEARTBEAT_INTERVAL,
            latency_check_interval: DEFAULT_LATENCY_CHECK_INTERVAL,
            bandwidth_limit_mbps: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RoomInfo {
    name: String,
    host_addr: SocketAddr,
}

struct RelayState {
    rooms: Mutex<HashMap<String, RoomInfo>>,
    clients: Mutex<HashMap<String, Arc<Mutex<TcpStream>>>>,
    relay_id: String,
    relay_name: String,
    relay_port: u16,
    report_address: Option<String>,
    total_bytes_sent: Mutex<u64>,
    last_bandwidth_check: Mutex<u64>,
    running: Arc<Mutex<bool>>,
}

impl RelayState {
    fn new(port: u16, report_addr: Option<String>) -> Self {
        Self {
            rooms: Mutex::new(HashMap::new()),
            clients: Mutex::new(HashMap::new()),
            relay_id: Uuid::new_v4().to_string(),
            relay_name: format!("Relay-{}", rand::random::<u32>()),
            relay_port: port,
            report_address: report_addr,
            total_bytes_sent: Mutex::new(0),
            last_bandwidth_check: Mutex::new(0),
            running: Arc::new(Mutex::new(true)),
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

fn load_config() -> Config {
    let config_path = "config.yml";

    if !fs::metadata(config_path).is_ok() {
        let default_config = serde_yaml::to_string(&Config::default()).unwrap();
        fs::write(config_path, default_config).expect("无法创建配置文件");
        log(LogLevel::Info, &format!("已创建默认配置文件: {}", config_path));
        return Config::default();
    }

    match fs::read_to_string(config_path) {
        Ok(content) => {
            match serde_yaml::from_str::<Config>(&content) {
                Ok(mut config) => {
                    if let Some(limit) = config.bandwidth_limit_mbps {
                        if limit < MIN_BANDWIDTH_MBPS {
                            log(LogLevel::Warn, &format!(
                                "带宽限制 {} MB/s 低于最小值 {} MB/s，自动调整",
                                limit, MIN_BANDWIDTH_MBPS
                            ));
                            config.bandwidth_limit_mbps = Some(MIN_BANDWIDTH_MBPS);
                        }
                    }
                    log(LogLevel::Info, &format!("已加载配置文件: {}", config_path));
                    config
                }
                Err(e) => {
                    log(LogLevel::Error, &format!("配置文件解析失败: {}", e));
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            log(LogLevel::Error, &format!("无法读取配置文件: {}", e));
            std::process::exit(1);
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

enum LogLevel {
    Info,
    Warn,
    Error,
}

fn log(level: LogLevel, msg: &str) {
    let now = std::time::SystemTime::now();
    let secs = now.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
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

/// Proxy Protocol V2 头部信息
#[derive(Debug, Clone)]
struct ProxyProtocolHeader {
    src_addr: SocketAddr,
    dst_addr: SocketAddr,
}

/// 解析 Proxy Protocol V2 头部
fn parse_proxy_protocol_v2(data: &[u8]) -> Option<ProxyProtocolHeader> {
    if data.len() < 16 {
        return None;
    }

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
    
    if proto != 0x01 {
        return None;
    }

    let len = u16::from_be_bytes(data[14..16].try_into().unwrap()) as usize;
    let header_data = &data[16..16+len];

    match family {
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

fn derive_key(password: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    hasher.finalize().into()
}

fn encrypt(data: &[u8], password: &str) -> Vec<u8> {
    let key = derive_key(password);
    let cipher = Aes256::new(GenericArray::from_slice(&key));
    
    let block_size = 16;
    let padding_len = block_size - (data.len() % block_size);
    let mut padded = data.to_vec();
    padded.extend(vec![padding_len as u8; padding_len]);
    
    let mut encrypted = Vec::new();
    for chunk in padded.chunks_exact(block_size) {
        let mut block = GenericArray::clone_from_slice(chunk);
        cipher.encrypt_block(&mut block);
        encrypted.extend_from_slice(&block);
    }
    
    encrypted
}

fn decrypt(data: &[u8], password: &str) -> Option<Vec<u8>> {
    if data.len() % 16 != 0 {
        return None;
    }
    
    let key = derive_key(password);
    let cipher = Aes256::new(GenericArray::from_slice(&key));
    
    let mut decrypted = Vec::new();
    for chunk in data.chunks_exact(16) {
        let mut block = GenericArray::clone_from_slice(chunk);
        cipher.decrypt_block(&mut block);
        decrypted.extend_from_slice(&block);
    }
    
    if let Some(&padding_len) = decrypted.last() {
        let padding_len = padding_len as usize;
        if padding_len > 0 && padding_len <= 16 {
            decrypted.truncate(decrypted.len() - padding_len);
            return Some(decrypted);
        }
    }
    
    None
}

fn send_register(stream: &mut TcpStream, state: &RelayState) {
    let address = state.report_address.clone().unwrap_or_else(|| {
        format!("{}:{}", local_ip(), state.relay_port)
    });

    let req = serde_json::json!({
        "id": state.relay_id.clone(),
        "name": state.relay_name.clone(),
        "address": address,
    });

    let mut packet = vec![0x10];
    packet.extend_from_slice(req.to_string().as_bytes());
    
    write_packet(stream, &packet).ok();
    log(LogLevel::Info, "发送注册请求");
    
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    match read_packet(stream) {
        Ok(buf) => {
            if buf.len() >= 2 && buf[0] == 0x10 && buf[1] == 0x00 {
                log(LogLevel::Info, "注册成功");
                return;
            }
        }
        Err(_) => {}
    }
    
    log(LogLevel::Warn, "注册响应未收到，继续运行（依赖心跳维持连接）");
}

fn send_heartbeat(stream: &mut TcpStream, state: &RelayState) {
    let req = serde_json::json!({
        "id": state.relay_id.clone(),
    });

    let mut packet = vec![0x11];
    packet.extend_from_slice(req.to_string().as_bytes());
    write_packet(stream, &packet).ok();
}

fn local_ip() -> String {
    match local_ip_address::local_ip() {
        Ok(ip) => ip.to_string(),
        Err(_) => "127.0.0.1".to_string(),
    }
}

fn resolve_server(addr_str: &str) -> SocketAddr {
    if let Ok(addr) = addr_str.parse::<SocketAddr>() {
        return addr;
    }

    let parts: Vec<&str> = addr_str.rsplitn(2, ':').collect();
    if parts.len() != 2 {
        log(LogLevel::Error, &format!("服务器地址格式错误: {}", addr_str));
        log(LogLevel::Warn, &format!("使用 fallback 地址: {}", FALLBACK_CENTRAL_SERVER));
        return FALLBACK_CENTRAL_SERVER.parse().unwrap();
    }

    let port = match parts[0].parse::<u16>() {
        Ok(p) => p,
        Err(_) => {
            log(LogLevel::Error, &format!("端口解析失败: {}", parts[0]));
            log(LogLevel::Warn, &format!("使用 fallback 地址: {}", FALLBACK_CENTRAL_SERVER));
            return FALLBACK_CENTRAL_SERVER.parse().unwrap();
        }
    };

    let hostname = parts[1];

    match std::net::ToSocketAddrs::to_socket_addrs(&(hostname, port)) {
        Ok(mut addrs) => {
            if let Some(addr) = addrs.next() {
                log(LogLevel::Info, &format!("DNS解析成功: {} -> {}", hostname, addr));
                return addr;
            }
        }
        Err(e) => {
            log(LogLevel::Error, &format!("DNS解析失败: {} ({})", hostname, e));
        }
    }

    log(LogLevel::Error, &format!("使用 fallback 地址: {}", FALLBACK_CENTRAL_SERVER));
    FALLBACK_CENTRAL_SERVER.parse().unwrap()
}

fn is_bandwidth_allowed(state: &RelayState, bytes: usize, bandwidth_limit_mbps: Option<f64>) -> bool {
    if bandwidth_limit_mbps.is_none() {
        return true;
    }

    let limit = bandwidth_limit_mbps.unwrap();
    let now = now_secs();
    let mut total = state.total_bytes_sent.lock().unwrap();
    let mut last_check = state.last_bandwidth_check.lock().unwrap();

    if now - *last_check >= 1 {
        *total = bytes as u64;
        *last_check = now;
        return true;
    }

    *total += bytes as u64;
    let current_mbps = (*total * 8) as f64 / ((now - *last_check + 1) as f64 * 1_000_000.0);

    current_mbps <= limit
}

fn print_help() {
    println!();
    println!("===========================================");
    println!("  MC-Link 中继服务器 - 帮助");
    println!("===========================================");
    println!("  h - 显示帮助");
    println!("  s - 停止服务器");
    println!("  r - 重启服务器");
    println!("  q - 退出");
    println!("===========================================");
    println!();
}

fn main() {
    let config = load_config();
    let bind_addr = format!("0.0.0.0:{}", config.relay_port);

    println!();
    println!("===========================================");
    println!("  MC-Link 中继服务器");
    println!("===========================================");
    println!("  监听端口: {}", config.relay_port);
    println!("  中央服务器: {}", config.central_server);
    println!("  心跳间隔: {}秒", config.heartbeat_interval);
    println!("  延迟探测间隔: {}秒", config.latency_check_interval);
    match config.bandwidth_limit_mbps {
        Some(limit) => println!("  带宽限制: {} MB/s", limit),
        None => println!("  带宽限制: 无限制"),
    }
    match &config.report_address {
        Some(addr) => println!("  上报地址: {}", addr),
        None => println!("  上报地址: 自动检测"),
    }
    println!("===========================================");
    println!();
    println!("按 h 获取帮助");
    println!();

    let _central_addr: SocketAddr = resolve_server(&config.central_server);
    let state = Arc::new(RelayState::new(config.relay_port, config.report_address.clone()));
    let bandwidth_limit = config.bandwidth_limit_mbps;

    let state_for_console = state.clone();
    let heartbeat_interval = config.heartbeat_interval;
    let central_server = config.central_server.clone();

    thread::spawn(move || {
        let central_addr: SocketAddr = resolve_server(&central_server);
        let mut central_stream = match TcpStream::connect(central_addr) {
            Ok(s) => s,
            Err(e) => {
                log(LogLevel::Error, &format!("无法连接中央服务器: {}", e));
                return;
            }
        };
        
        send_register(&mut central_stream, &state_for_console);

        loop {
            thread::sleep(Duration::from_secs(heartbeat_interval));
            if !state_for_console.is_running() {
                break;
            }
            send_heartbeat(&mut central_stream, &state_for_console);
        }
    });

    let listener = match TcpListener::bind(&bind_addr) {
        Ok(l) => l,
        Err(e) => {
            log(LogLevel::Error, &format!("无法绑定端口 {}: {}", bind_addr, e));
            return;
        }
    };
    listener.set_nonblocking(true).ok();

    let listener = Arc::new(Mutex::new(listener));
    let _listener_for_console = listener.clone();
    let state_for_input = state.clone();

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
                    state_for_input.stop();
                }
                "r" => {
                    log(LogLevel::Info, "正在重启服务器...");
                    state_for_input.stop();
                    thread::sleep(Duration::from_secs(1));
                    log(LogLevel::Info, "服务器已重启");
                }
                "q" => {
                    log(LogLevel::Info, "正在关闭服务器...");
                    state_for_input.stop();
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

        if let Ok((mut stream, _)) = listener.lock().unwrap().accept() {
            stream.set_nonblocking(false).ok();
            let real_addr = match read_proxy_protocol_header(&mut stream) {
                Some(header) => {
                    log(LogLevel::Info, &format!("Proxy Protocol V2 解析成功 - 客户端: {}, 中继: {}", header.src_addr, header.dst_addr));
                    header.src_addr
                }
                None => stream.peer_addr().unwrap(),
            };
            let state_clone = state.clone();
            let bandwidth_limit = bandwidth_limit;
            thread::spawn(move || {
                handle_client(stream, state_clone, real_addr, bandwidth_limit);
            });
        }

        thread::sleep(Duration::from_millis(10));
    }

    log(LogLevel::Info, "中继服务器已关闭");
}

fn handle_client(mut stream: TcpStream, state: Arc<RelayState>, addr: SocketAddr, bandwidth_limit: Option<f64>) {
    let addr_str = addr.to_string();
    state.clients.lock().unwrap().insert(addr_str.clone(), Arc::new(Mutex::new(stream.try_clone().unwrap())));

    let mut host_room: Option<String> = None;

    loop {
        match read_packet(&mut stream) {
            Ok(data) => {
                if data.is_empty() {
                    continue;
                }
                if !is_bandwidth_allowed(&state, data.len(), bandwidth_limit) {
                    continue;
                }
                let room = handle_packet(&mut stream, &state, addr, &data);
                if let Some(r) = room {
                    host_room = Some(r);
                }
            }
            Err(_) => break,
        }
    }

    state.clients.lock().unwrap().remove(&addr_str);

    if let Some(room_name) = host_room {
        let rooms = state.rooms.lock().unwrap();
        if let Some(room_info) = rooms.get(&room_name) {
            if room_info.host_addr == addr {
                drop(rooms);
                state.rooms.lock().unwrap().remove(&room_name);
                log(LogLevel::Info, &format!("[房间/注销] 房主断开，房间 {} 已注销", room_name));
            }
        }
    }
}

fn is_custom_protocol(data: &[u8]) -> bool {
    if data.len() < 4 {
        return false;
    }
    let room_len = data[0] as usize;
    if room_len == 0 || room_len > 100 {
        return false;
    }
    if data.len() < 1 + room_len + 1 {
        return false;
    }
    let pass_len = data[1 + room_len] as usize;
    if pass_len == 0 || pass_len > 100 {
        return false;
    }
    if data.len() < 1 + room_len + 1 + pass_len + 16 {
        return false;
    }
    true
}

fn handle_packet(stream: &mut TcpStream, state: &RelayState, src: SocketAddr, data: &[u8]) -> Option<String> {
    if data.is_empty() {
        return None;
    }

    if is_custom_protocol(data) {
        return handle_custom_protocol(stream, state, src, data);
    }

    let cmd = data[0];
    let payload = &data[1..];

    match cmd {
        0x41 => { handle_test_packet(state, src, data); None }
        0x20 => handle_create_room(stream, state, src, payload),
        0x22 => { handle_find_room(stream, state, src, payload); None }
        0x40 => { handle_data(state, src, payload); None }
        0x32 => { handle_ping(stream); None }
        _ => None,
    }
}

fn handle_custom_protocol(stream: &mut TcpStream, state: &RelayState, src: SocketAddr, data: &[u8]) -> Option<String> {
    let room_len = data[0] as usize;
    let room = String::from_utf8_lossy(&data[1..1+room_len]).to_string();
    let pass_len = data[1 + room_len] as usize;
    let password = String::from_utf8_lossy(&data[1+room_len+1..1+room_len+1+pass_len]).to_string();
    let encrypted = &data[1+room_len+1+pass_len..];

    let decrypted = match decrypt(encrypted, &password) {
        Some(d) => d,
        None => return None,
    };

    if decrypted.len() < 4 {
        return None;
    }

    let command = &decrypted[0..4];

    if command == b"REGH" {
        let mut rooms = state.rooms.lock().unwrap();
        if rooms.contains_key(&room) {
            drop(rooms);
            log(LogLevel::Warn, &format!("[REGH] 房间已存在: {}", room));
            return None;
        }
        rooms.insert(room.clone(), RoomInfo {
            name: room.clone(),
            host_addr: src,
        });
        drop(rooms);
        log(LogLevel::Info, &format!("[REGH] 房间创建: {} (来自 {})", room, src));

        let resp_enc = encrypt(b"REGH_OK", &password);
        let mut resp_packet = Vec::new();
        resp_packet.push(room_len as u8);
        resp_packet.extend_from_slice(&data[1..1+room_len]);
        resp_packet.push(pass_len as u8);
        resp_packet.extend_from_slice(&data[1+room_len+1..1+room_len+1+pass_len]);
        resp_packet.extend_from_slice(&resp_enc);
        write_packet(stream, &resp_packet).ok();
        return Some(room);
    }

    if command == b"REGC" {
        let rooms = state.rooms.lock().unwrap();
        let room_info = match rooms.get(&room) {
            Some(info) => info.clone(),
            None => {
                drop(rooms);
                log(LogLevel::Warn, &format!("[REGC] 房间不存在: {}", room));
                let err_enc = encrypt(b"ERRR", &password);
                let mut resp_packet = Vec::new();
                resp_packet.push(room_len as u8);
                resp_packet.extend_from_slice(&data[1..1+room_len]);
                resp_packet.push(pass_len as u8);
                resp_packet.extend_from_slice(&data[1+room_len+1..1+room_len+1+pass_len]);
                resp_packet.extend_from_slice(&err_enc);
                write_packet(stream, &resp_packet).ok();
                return None;
            }
        };
        drop(rooms);

        let resp_enc = encrypt(b"REGC_OK", &password);
        let mut resp_packet = Vec::new();
        resp_packet.push(room_len as u8);
        resp_packet.extend_from_slice(&data[1..1+room_len]);
        resp_packet.push(pass_len as u8);
        resp_packet.extend_from_slice(&data[1+room_len+1..1+room_len+1+pass_len]);
        resp_packet.extend_from_slice(&resp_enc);
        write_packet(stream, &resp_packet).ok();
        log(LogLevel::Info, &format!("[REGC] 成员加入房间: {} (来自 {})", room, src));

        let clients = state.clients.lock().unwrap();
        if let Some(host_stream) = clients.get(&room_info.host_addr.to_string()) {
            let member_join_enc = encrypt(b"MEMBER_JOIN", &password);
            let mut member_join_packet = Vec::new();
            member_join_packet.push(room_len as u8);
            member_join_packet.extend_from_slice(&data[1..1+room_len]);
            member_join_packet.push(pass_len as u8);
            member_join_packet.extend_from_slice(&data[1+room_len+1..1+room_len+1+pass_len]);
            member_join_packet.extend_from_slice(&member_join_enc);
            let mut host_stream = host_stream.lock().unwrap();
            write_packet(&mut host_stream, &member_join_packet).ok();
            log(LogLevel::Info, &format!("[房间/加入] 向房主发送成员加入通知: {}", room));
        }
        drop(clients);
        return Some(room);
    }

    if command == b"MC_READY" {
        let rooms = state.rooms.lock().unwrap();
        if let Some(room_info) = rooms.get(&room) {
            let clients = state.clients.lock().unwrap();
            if let Some(host_stream) = clients.get(&room_info.host_addr.to_string()) {
                let mc_ready_enc = encrypt(b"MC_READY", &password);
                let mut mc_ready_packet = Vec::new();
                mc_ready_packet.push(room_len as u8);
                mc_ready_packet.extend_from_slice(&data[1..1+room_len]);
                mc_ready_packet.push(pass_len as u8);
                mc_ready_packet.extend_from_slice(&data[1+room_len+1..1+room_len+1+pass_len]);
                mc_ready_packet.extend_from_slice(&mc_ready_enc);
                let mut host_stream = host_stream.lock().unwrap();
                write_packet(&mut host_stream, &mc_ready_packet).ok();
                log(LogLevel::Info, &format!("[房间/就绪] 向房主发送MC_READY通知: {}", room));
            }
        }
        return None;
    }

    if command == b"DATA" {
        let rooms = state.rooms.lock().unwrap();
        if let Some(room_info) = rooms.get(&room) {
            if room_info.host_addr != src {
                let clients = state.clients.lock().unwrap();
                if let Some(host_stream) = clients.get(&room_info.host_addr.to_string()) {
                    let mut host_stream = host_stream.lock().unwrap();
                    let mut packet = Vec::new();
                    packet.push(room_len as u8);
                    packet.extend_from_slice(&data[1..1+room_len]);
                    packet.push(pass_len as u8);
                    packet.extend_from_slice(&data[1+room_len+1..1+room_len+1+pass_len]);
                    packet.extend_from_slice(&encrypted);
                    write_packet(&mut host_stream, &packet).ok();
                }
            } else {
                let clients = state.clients.lock().unwrap();
                let mut packet = Vec::new();
                packet.push(room_len as u8);
                packet.extend_from_slice(&data[1..1+room_len]);
                packet.push(pass_len as u8);
                packet.extend_from_slice(&data[1+room_len+1..1+room_len+1+pass_len]);
                packet.extend_from_slice(&encrypted);
                for (addr, client) in clients.iter() {
                    if *addr != room_info.host_addr.to_string() {
                        let mut client = client.lock().unwrap();
                        write_packet(&mut client, &packet).ok();
                    }
                }
            }
        }
        return None;
    }

    let resp_enc = encrypt(b"ERRR", &password);
    let mut resp_packet = Vec::new();
    resp_packet.push(room_len as u8);
    resp_packet.extend_from_slice(&data[1..1+room_len]);
    resp_packet.push(pass_len as u8);
    resp_packet.extend_from_slice(&data[1+room_len+1..1+room_len+1+pass_len]);
    resp_packet.extend_from_slice(&resp_enc);
    write_packet(stream, &resp_packet).ok();
    None
}

#[derive(Deserialize)]
struct CreateRoomReq {
    room_name: String,
}

fn handle_create_room(stream: &mut TcpStream, state: &RelayState, src: SocketAddr, data: &[u8]) -> Option<String> {
    if let Ok(req) = serde_json::from_slice::<CreateRoomReq>(data) {
        let mut rooms = state.rooms.lock().unwrap();

        if rooms.contains_key(&req.room_name) {
            write_packet(stream, &[0x21, 0x01]).ok();
            return None;
        }

        rooms.insert(
            req.room_name.clone(),
            RoomInfo {
                name: req.room_name.clone(),
                host_addr: src,
            },
        );

        log(LogLevel::Info, &format!("[房间/创建] 房间 {} (来自 {})", req.room_name, src));
        write_packet(stream, &[0x21, 0x00]).ok();
        return Some(req.room_name);
    }
    None
}

#[derive(Deserialize)]
struct FindRoomReq {
    room_name: String,
}

fn handle_find_room(stream: &mut TcpStream, state: &RelayState, _src: SocketAddr, data: &[u8]) {
    if let Ok(req) = serde_json::from_slice::<FindRoomReq>(data) {
        let rooms = state.rooms.lock().unwrap();

        if let Some(room) = rooms.get(&req.room_name) {
            let response = serde_json::json!({
                "exists": true,
                "host_addr": room.host_addr.to_string(),
            });
            let mut packet = vec![0x23];
            packet.extend_from_slice(response.to_string().as_bytes());
            write_packet(stream, &packet).ok();
        } else {
            write_packet(stream, &[0x23, 0x00]).ok();
        }
    }
}

fn handle_data(state: &RelayState, src: SocketAddr, data: &[u8]) {
    if data.len() < 4 {
        return;
    }

    let room_name_len = u32::from_be_bytes(data[0..4].try_into().unwrap()) as usize;
    if data.len() < 4 + room_name_len {
        return;
    }

    let room_name = String::from_utf8_lossy(&data[4..4 + room_name_len]);
    let actual_data = &data[4 + room_name_len..];

    let rooms = state.rooms.lock().unwrap();
    if let Some(room) = rooms.get(room_name.as_ref()) {
        if room.host_addr != src {
            let clients = state.clients.lock().unwrap();
            if let Some(host_stream) = clients.get(&room.host_addr.to_string()) {
                let mut host_stream = host_stream.lock().unwrap();
                let mut packet = vec![0x40];
                packet.extend_from_slice(&room_name_len.to_be_bytes());
                packet.extend_from_slice(room_name.as_bytes());
                packet.extend_from_slice(actual_data);
                write_packet(&mut host_stream, &packet).ok();
            }
        }
    }
}

fn handle_ping(stream: &mut TcpStream) {
    log(LogLevel::Info, "[Ping] 收到ping请求，回复pong");
    write_packet(stream, &[0x33]).ok();
}

fn handle_test_packet(state: &RelayState, src: SocketAddr, data: &[u8]) {
    if data.len() < 2 {
        return;
    }

    let room_name_len = data[1] as usize;
    if data.len() < 2 + room_name_len {
        return;
    }

    let room_name = String::from_utf8_lossy(&data[2..2 + room_name_len]);
    let pass_len = data[2 + room_name_len] as usize;
    if data.len() < 2 + room_name_len + 1 + pass_len {
        return;
    }

    let rooms = state.rooms.lock().unwrap();
    if let Some(room) = rooms.get(room_name.as_ref()) {
        if room.host_addr != src {
            let clients = state.clients.lock().unwrap();
            if let Some(host_stream) = clients.get(&room.host_addr.to_string()) {
                let mut host_stream = host_stream.lock().unwrap();
                let packet = vec![0x41];
                write_packet(&mut host_stream, &packet).ok();
                log(LogLevel::Info, &format!("[测试] 已转发测试数据包到房主 {}", room.host_addr));
            }
        } else {
            let clients = state.clients.lock().unwrap();
            let packet = vec![0x41];
            for (addr, client) in clients.iter() {
                if *addr != room.host_addr.to_string() {
                    let mut client = client.lock().unwrap();
                    write_packet(&mut client, &packet).ok();
                    log(LogLevel::Info, &format!("[测试] 已转发测试数据包到成员 {}", addr));
                }
            }
        }
    }
}


