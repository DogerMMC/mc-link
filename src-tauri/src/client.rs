use std::net::{TcpListener, TcpStream, UdpSocket, SocketAddr};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use crate::crypto;

const MULTICAST_ADDR: &str = "224.0.2.60:4445";
const MULTICAST_IP: &str = "224.0.2.60";
const MULTICAST_PORT: u16 = 4445;

pub struct ClientMode {
    running: Arc<Mutex<bool>>,
    host_addr: SocketAddr,
    local_port: u16,
    motd: String,
    use_relay: bool,
    relay_addr: Option<SocketAddr>,
    room: String,
    password: String,
    log_callback: Arc<Mutex<Option<Box<dyn Fn(String) + Send>>>>,
}

impl ClientMode {
    pub fn new(host_addr: SocketAddr, local_port: u16, motd: String) -> Self {
        Self {
            running: Arc::new(Mutex::new(false)),
            host_addr,
            local_port,
            motd,
            use_relay: false,
            relay_addr: None,
            room: String::new(),
            password: String::new(),
            log_callback: Arc::new(Mutex::new(None)),
        }
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

    pub fn set_relay(&mut self, relay_addr: SocketAddr, room: String, password: String) {
        self.use_relay = true;
        self.relay_addr = Some(relay_addr);
        self.room = room;
        self.password = password;
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

    pub fn start(&mut self) -> Result<String, String> {
        let running = self.running.clone();
        *running.lock().unwrap() = true;

        let local_port = self.local_port;
        let host_addr = self.host_addr;
        let motd = self.motd.clone();
        let use_relay = self.use_relay;
        let relay_addr = self.relay_addr;

        // 启动本地TCP监听
        let tcp_listener = TcpListener::bind(format!("127.0.0.1:{}", local_port))
            .map_err(|e| format!("TCP绑定失败: {}", e))?;
        tcp_listener.set_nonblocking(true).map_err(|e| e.to_string())?;

        if use_relay {
            // 使用中继模式
            let udp_socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("UDP绑定失败: {}", e))?;
            let relay = relay_addr.unwrap();
            udp_socket.connect(relay).map_err(|e| format!("连接中继服务器失败: {}", e))?;
            let udp_socket = Arc::new(udp_socket);

            // 向中继服务器注册为成员
            let reg_packet = self.pack_packet(b"REGC");
            udp_socket.send(&reg_packet).map_err(|e| format!("注册失败: {}", e))?;
            
            // 等待确认
            let mut buf = [0u8; 1024];
            udp_socket.set_read_timeout(Some(Duration::from_secs(5))).ok();
            match udp_socket.recv(&mut buf) {
                Ok(n) => {
                    // 解密响应
                    if let Some(decrypted) = self.try_decrypt_response(&buf[..n]) {
                        if &decrypted == b"REGC_OK" {
                            println!("已注册到中继服务器");
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

            // 启动局域网发现广播
            let running_clone = running.clone();
            let motd_clone = motd.clone();
            let local_port_clone = local_port;
            thread::spawn(move || {
                Self::lan_discovery_broadcaster(running_clone, motd_clone, local_port_clone);
            });

            // UDP接收线程（从中继接收转发给TCP）
            let running_clone = running.clone();
            let udp_socket_clone = udp_socket.clone();
            let room = self.room.clone();
            let password = self.password.clone();
            thread::spawn(move || {
                Self::udp_to_tcp_relay(udp_socket_clone, running_clone, room, password);
            });

            // TCP转发线程（从TCP转发给中继）
            let running_clone = running.clone();
            let udp_socket_clone = udp_socket.clone();
            let room = self.room.clone();
            let password = self.password.clone();
            thread::spawn(move || {
                Self::tcp_to_udp_relay(tcp_listener, udp_socket_clone, running_clone, room, password);
            });

            Ok(format!("成员模式(中继)已启动，本地端口: {}, 房间: {}", local_port, self.room))
        } else {
            // 直连模式
            let udp_socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("UDP绑定失败: {}", e))?;
            udp_socket.connect(host_addr).map_err(|e| format!("连接房主失败: {}", e))?;
            let udp_socket = Arc::new(udp_socket);

            // 启动局域网发现广播
            let running_clone = running.clone();
            let motd_clone = motd.clone();
            let local_port_clone = local_port;
            thread::spawn(move || {
                Self::lan_discovery_broadcaster(running_clone, motd_clone, local_port_clone);
            });

            // UDP接收线程 - 接收房主的UDP数据转发到TCP
            let running_clone = running.clone();
            let udp_socket_clone = udp_socket.clone();
            thread::spawn(move || {
                Self::udp_to_tcp_forward(udp_socket_clone, running_clone);
            });

            // TCP转发线程
            let running_clone = running.clone();
            let udp_socket_clone = udp_socket.clone();
            thread::spawn(move || {
                Self::tcp_to_udp_forward(tcp_listener, udp_socket_clone, running_clone);
            });

            Ok(format!("成员模式已启动，本地端口: {}, 房主: {}", local_port, host_addr))
        }
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

    fn lan_discovery_broadcaster(running: Arc<Mutex<bool>>, motd: String, port: u16) {
        let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
        socket.set_broadcast(true).unwrap();

        let message = format!("[MOTD]{}[/MOTD][AD]{}[/AD]", motd, port);
        let addr: SocketAddr = MULTICAST_ADDR.parse().unwrap();

        while *running.lock().unwrap() {
            socket.send_to(message.as_bytes(), addr).ok();
            thread::sleep(Duration::from_millis(1500));
        }
    }

    fn udp_to_tcp_forward(udp_socket: Arc<UdpSocket>, running: Arc<Mutex<bool>>) {
        let mut buf = [0u8; 4096];
        while *running.lock().unwrap() {
            match udp_socket.recv(&mut buf) {
                Ok(n) => {
                    println!("收到UDP数据 {} bytes", n);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(_) => break,
            }
        }
    }

    fn tcp_to_udp_forward(listener: TcpListener, udp_socket: Arc<UdpSocket>, running: Arc<Mutex<bool>>) {
        let mut clients: Vec<(TcpStream, SocketAddr)> = Vec::new();

        while *running.lock().unwrap() {
            match listener.accept() {
                Ok((stream, addr)) => {
                    println!("新本地TCP连接: {}", addr);
                    stream.set_nonblocking(true).ok();
                    clients.push((stream, addr));
                }
                Err(_) => {}
            }

            clients.retain_mut(|(stream, addr)| {
                let mut buf = [0u8; 4096];
                match stream.read(&mut buf) {
                    Ok(0) => {
                        println!("TCP连接关闭: {}", addr);
                        false
                    }
                    Ok(n) => {
                        udp_socket.send(&buf[..n]).ok();
                        true
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => true,
                    Err(_) => false,
                }
            });

            thread::sleep(Duration::from_millis(1));
        }
    }

    // 中继模式：TCP→UDP
    fn tcp_to_udp_relay(listener: TcpListener, udp_socket: Arc<UdpSocket>, running: Arc<Mutex<bool>>, room: String, password: String) {
        let mut clients: Vec<TcpStream> = Vec::new();

        while *running.lock().unwrap() {
            match listener.accept() {
                Ok((stream, _)) => {
                    println!("新本地TCP连接(中继模式)");
                    stream.set_nonblocking(true).ok();
                    clients.push(stream);
                }
                Err(_) => {}
            }

            clients.retain_mut(|stream| {
                let mut buf = [0u8; 4096];
                match stream.read(&mut buf) {
                    Ok(0) => {
                        println!("TCP连接关闭");
                        false
                    }
                    Ok(n) => {
                        // 发送给中继服务器: DATA + 数据
                        let mut payload = Vec::with_capacity(4 + n);
                        payload.extend_from_slice(b"DATA");
                        payload.extend_from_slice(&buf[..n]);
                        
                        let packet = Self::pack_packet_static(&room, &password, &payload);
                        udp_socket.send(&packet).ok();
                        true
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => true,
                    Err(_) => false,
                }
            });

            thread::sleep(Duration::from_millis(1));
        }
    }

    // 中继模式：UDP→TCP
    fn udp_to_tcp_relay(udp_socket: Arc<UdpSocket>, running: Arc<Mutex<bool>>, room: String, password: String) {
        let mut tcp_clients: Vec<TcpStream> = Vec::new();
        let mut buf = [0u8; 65535];

        while *running.lock().unwrap() {
            match udp_socket.recv(&mut buf) {
                Ok(n) => {
                    // 解密响应
                    if let Some(decrypted) = Self::try_decrypt_response_static(&buf[..n], &password) {
                        if decrypted.len() >= 4 && &decrypted[0..4] == b"DATA" {
                            // 转发给所有TCP客户端
                            let data = &decrypted[4..];
                            tcp_clients.retain_mut(|client| {
                                client.write(data).is_ok()
                            });
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(_) => break,
            }
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

    pub fn stop(&mut self) {
        *self.running.lock().unwrap() = false;
    }
}
