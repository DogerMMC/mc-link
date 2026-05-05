use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::collections::HashMap;
use crate::crypto;

const MULTICAST_ADDR: &str = "224.0.2.60:4445";
const MULTICAST_IP: &str = "224.0.2.60";
const MULTICAST_PORT: u16 = 4445;

#[derive(Clone, Debug)]
pub struct LanServer {
    pub motd: String,
    pub port: u16,
}

pub struct HostMode {
    running: Arc<Mutex<bool>>,
    game_port: u16,
    motd: String,
    udp_socket: Option<Arc<UdpSocket>>,
    relay_addr: Option<SocketAddr>,
    relay_id: Option<String>,
    room: String,
    password: String,
    log_callback: Arc<Mutex<Option<Box<dyn Fn(String) + Send>>>>,
}

impl HostMode {
    pub fn new() -> Self {
        Self {
            running: Arc::new(Mutex::new(false)),
            game_port: 0,
            motd: String::new(),
            udp_socket: None,
            relay_addr: None,
            relay_id: None,
            room: String::new(),
            password: String::new(),
            log_callback: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_relay(&mut self, relay_addr: SocketAddr, room: String, password: String) {
        self.relay_addr = Some(relay_addr);
        self.room = room;
        self.password = password;
    }

    pub fn set_relay_id(&mut self, relay_id: String) {
        self.relay_id = Some(relay_id);
    }

    pub fn set_log_callback<F>(&self, callback: F)
    where
        F: Fn(String) + Send + 'static,
    {
        *self.log_callback.lock().unwrap() = Some(Box::new(callback));
    }

    fn log(&self, msg: String) {
        println!("{}", msg);
        if let Some(ref callback) = *self.log_callback.lock().unwrap() {
            callback(msg);
        }
    }

    fn pack_packet(&self, data: &[u8]) -> Vec<u8> {
        let encrypted = crypto::encrypt(data, &self.password);
        
        let mut packet = Vec::new();
        packet.push(self.room.len() as u8);
        packet.extend_from_slice(self.room.as_bytes());
        packet.push(self.password.len() as u8);
        packet.extend_from_slice(self.password.as_bytes());
        packet.extend_from_slice(&encrypted);
        
        packet
    }

    pub fn start(&mut self, selected_game_port: u16, motd: String) -> Result<String, String> {
        self.game_port = selected_game_port;
        self.motd = motd;
        let running = self.running.clone();
        *running.lock().unwrap() = true;

        let game_port = self.game_port;
        let relay_addr = self.relay_addr.ok_or("请配置中继服务器")?;

        self.log(format!("[启动] 房主模式，游戏端口: {}", game_port));
        self.log("[启动] Minecraft正在作为服务器监听此端口".to_string());
        self.log("[启动] MC-Link将作为客户端连接到Minecraft...".to_string());

        // 连接到Minecraft的TCP端口（Minecraft正在监听）
        let mc_addr = format!("127.0.0.1:{}", game_port);
        self.log(format!("[启动] 连接到Minecraft: {}", mc_addr));
        
        let mc_stream = TcpStream::connect(&mc_addr)
            .map_err(|e| format!("连接Minecraft失败: {}，请确保Minecraft世界已开放局域网", e))?;
        
        self.log("[启动] 已连接到Minecraft服务器".to_string());
        mc_stream.set_nonblocking(true).map_err(|e| e.to_string())?;
        let mc_stream = Arc::new(Mutex::new(mc_stream));

        // 连接到中继服务器
        self.log(format!("[启动] 连接中继服务器: {}", relay_addr));
        let udp_socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("UDP绑定失败: {}", e))?;
        udp_socket.connect(relay_addr).map_err(|e| format!("连接中继服务器失败: {}", e))?;
        let udp_socket = Arc::new(udp_socket);
        self.udp_socket = Some(udp_socket.clone());

        // 向中继服务器注册为房主
        self.log("[启动] 向中继服务器注册...".to_string());
        let reg_packet = self.pack_packet(b"REGH");
        udp_socket.send(&reg_packet).map_err(|e| format!("注册失败: {}", e))?;
        
        // 等待确认
        let mut buf = [0u8; 1024];
        udp_socket.set_read_timeout(Some(Duration::from_secs(5))).ok();
        match udp_socket.recv(&mut buf) {
            Ok(n) => {
                if let Some(decrypted) = self.try_decrypt_response(&buf[..n]) {
                    if &decrypted == b"REGH_OK" {
                        self.log("[启动] 已注册到中继服务器".to_string());
                    } else {
                        return Err("中继服务器注册失败: 无效响应".to_string());
                    }
                } else {
                    return Err("中继服务器注册失败: 解密失败".to_string());
                }
            }
            _ => {
                return Err("中继服务器注册失败: 超时".to_string());
            }
        }
        udp_socket.set_nonblocking(true).ok();

        // 启动UDP接收线程（从中继接收转发给Minecraft）
        let running_clone = running.clone();
        let udp_socket_clone = udp_socket.clone();
        let mc_stream_clone = mc_stream.clone();
        let room = self.room.clone();
        let password = self.password.clone();
        let log_callback = self.log_callback.clone();
        thread::spawn(move || {
            Self::udp_to_mc_relay(udp_socket_clone, mc_stream_clone, running_clone, room, password, log_callback);
        });

        // TCP转发线程（从Minecraft转发到中继）
        let running_clone = running.clone();
        let udp_socket_clone = udp_socket.clone();
        let mc_stream_clone = mc_stream.clone();
        let room = self.room.clone();
        let password = self.password.clone();
        let log_callback = self.log_callback.clone();
        thread::spawn(move || {
            Self::mc_to_udp_relay(mc_stream_clone, udp_socket_clone, running_clone, room, password, log_callback);
        });

        Ok(format!("房主模式已启动\n已连接到Minecraft端口: {}\n房间: {}", game_port, self.room))
    }

    pub fn scan_lan_servers(&self) -> Result<Vec<LanServer>, String> {
        // 创建UDP socket监听局域网发现
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", MULTICAST_PORT))
            .map_err(|e| format!("绑定失败: {}", e))?;
        
        socket.join_multicast_v4(
            &MULTICAST_IP.parse().unwrap(),
            &"0.0.0.0".parse().unwrap()
        ).map_err(|e| e.to_string())?;

        socket.set_read_timeout(Some(Duration::from_secs(3))).ok();

        let mut servers: HashMap<u16, LanServer> = HashMap::new();
        let mut buf = [0u8; 1024];

        // 监听3秒收集服务器
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            match socket.recv_from(&mut buf) {
                Ok((len, _)) => {
                    if let Ok(data) = String::from_utf8(buf[..len].to_vec()) {
                        if data.contains("[MOTD]") && data.contains("[AD]") {
                            let motd = Self::extract_tag(&data, "[MOTD]", "[/MOTD]");
                            let port_str = Self::extract_tag(&data, "[AD]", "[/AD]");
                            if let Ok(port) = port_str.parse::<u16>() {
                                servers.insert(port, LanServer { motd, port });
                            }
                        }
                    }
                }
                Err(_) => {}
            }
        }

        Ok(servers.into_values().collect())
    }

    fn try_decrypt_response(&self, data: &[u8]) -> Option<Vec<u8>> {
        if data.len() < 2 {
            return None;
        }
        
        let room_len = data[0] as usize;
        if data.len() < 1 + room_len + 1 {
            return None;
        }
        
        let pass_len = data[1 + room_len] as usize;
        if data.len() < 1 + room_len + 1 + pass_len {
            return None;
        }
        
        let encrypted_start = 1 + room_len + 1 + pass_len;
        let encrypted = &data[encrypted_start..];
        
        crypto::decrypt(encrypted, &self.password)
    }

    // Minecraft→UDP (从Minecraft转发到中继)
    fn mc_to_udp_relay(mc_stream: Arc<Mutex<TcpStream>>, udp_socket: Arc<UdpSocket>, running: Arc<Mutex<bool>>, room: String, password: String, log_callback: Arc<Mutex<Option<Box<dyn Fn(String) + Send>>>>) {
        let mut buf = [0u8; 4096];
        let mut packet_count: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut last_log = std::time::Instant::now();
        
        while *running.lock().unwrap() {
            match mc_stream.lock().unwrap().read(&mut buf) {
                Ok(0) => {
                    let msg = format!("[连接] Minecraft连接关闭，共发送 {} 个数据包，{} bytes", packet_count, total_bytes);
                    println!("{}", msg);
                    if let Some(ref callback) = *log_callback.lock().unwrap() {
                        callback(msg);
                    }
                    break;
                }
                Ok(n) => {
                    packet_count += 1;
                    total_bytes += n as u64;
                    
                    // 每5秒或每100个包输出一次统计
                    if packet_count % 100 == 0 || last_log.elapsed().as_secs() >= 5 {
                        let msg = format!("[统计] MC→中继: {} 包, {} bytes (当前 {} bytes)", packet_count, total_bytes, n);
                        println!("{}", msg);
                        if let Some(ref callback) = *log_callback.lock().unwrap() {
                            callback(msg);
                        }
                        last_log = std::time::Instant::now();
                    }
                    
                    let mut payload = Vec::with_capacity(4 + n);
                    payload.extend_from_slice(b"DATA");
                    payload.extend_from_slice(&buf[..n]);
                    
                    let packet = Self::pack_packet_static(&room, &password, &payload);
                    if udp_socket.send(&packet).is_ok() {
                        // 发送成功
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(e) => {
                    let msg = format!("[错误] 读取Minecraft数据失败: {}", e);
                    println!("{}", msg);
                    if let Some(ref callback) = *log_callback.lock().unwrap() {
                        callback(msg);
                    }
                    break;
                }
            }
        }
    }

    // UDP→Minecraft (从中继接收转发给Minecraft)
    fn udp_to_mc_relay(udp_socket: Arc<UdpSocket>, mc_stream: Arc<Mutex<TcpStream>>, running: Arc<Mutex<bool>>, _room: String, password: String, log_callback: Arc<Mutex<Option<Box<dyn Fn(String) + Send>>>>) {
        let mut buf = [0u8; 65535];
        let mut packet_count: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut last_log = std::time::Instant::now();

        while *running.lock().unwrap() {
            match udp_socket.recv(&mut buf) {
                Ok(n) => {
                    if let Some(decrypted) = Self::try_decrypt_response_static(&buf[..n], &password) {
                        if decrypted.len() >= 4 && &decrypted[0..4] == b"DATA" {
                            let data = &decrypted[4..];
                            packet_count += 1;
                            total_bytes += data.len() as u64;
                            
                            // 每5秒或每100个包输出一次统计
                            if packet_count % 100 == 0 || last_log.elapsed().as_secs() >= 5 {
                                let msg = format!("[统计] 中继→MC: {} 包, {} bytes (当前 {} bytes)", packet_count, total_bytes, data.len());
                                println!("{}", msg);
                                if let Some(ref callback) = *log_callback.lock().unwrap() {
                                    callback(msg);
                                }
                                last_log = std::time::Instant::now();
                            }
                            
                            if mc_stream.lock().unwrap().write(data).is_ok() {
                                // 转发成功
                            }
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(_) => break,
            }
        }
        
        let msg = format!("[统计] 中继→MC 结束: {} 包, {} bytes", packet_count, total_bytes);
        println!("{}", msg);
        if let Some(ref callback) = *log_callback.lock().unwrap() {
            callback(msg);
        }
    }

    fn pack_packet_static(room: &str, password: &str, data: &[u8]) -> Vec<u8> {
        let encrypted = crypto::encrypt(data, password);
        
        let mut packet = Vec::new();
        packet.push(room.len() as u8);
        packet.extend_from_slice(room.as_bytes());
        packet.push(password.len() as u8);
        packet.extend_from_slice(password.as_bytes());
        packet.extend_from_slice(&encrypted);
        
        packet
    }

    fn try_decrypt_response_static(data: &[u8], password: &str) -> Option<Vec<u8>> {
        if data.len() < 2 {
            return None;
        }
        
        let room_len = data[0] as usize;
        if data.len() < 1 + room_len + 1 {
            return None;
        }
        
        let pass_len = data[1 + room_len] as usize;
        if data.len() < 1 + room_len + 1 + pass_len {
            return None;
        }
        
        let encrypted_start = 1 + room_len + 1 + pass_len;
        let encrypted = &data[encrypted_start..];
        
        crypto::decrypt(encrypted, password)
    }

    fn extract_tag(s: &str, start: &str, end: &str) -> String {
        if let Some(start_pos) = s.find(start) {
            if let Some(end_pos) = s.find(end) {
                return s[start_pos + start.len()..end_pos].to_string();
            }
        }
        String::new()
    }

    pub fn stop(&mut self) {
        *self.running.lock().unwrap() = false;
    }
}
