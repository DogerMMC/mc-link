use std::net::{TcpListener, TcpStream, UdpSocket, SocketAddr};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use crate::crypto;

const MULTICAST_ADDR: &str = "224.0.2.60:4445";

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

pub struct ClientMode {
    running: Arc<Mutex<bool>>,
    host_addr: SocketAddr,
    local_port: u16,
    motd: String,
    use_relay: bool,
    relay_addr: Option<SocketAddr>,
    relay_stream: Option<Arc<Mutex<TcpStream>>>,
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
            relay_stream: None,
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
            let relay = relay_addr.unwrap();
            self.log(format!("[启动] 连接中继服务器: {}", relay));
            let mut relay_stream = TcpStream::connect(relay).map_err(|e| format!("连接中继服务器失败: {}", e))?;
            self.log("[启动] 已连接到中继服务器".to_string());

            // 向中继服务器注册为成员
            self.log("[启动] 向中继服务器注册...".to_string());
            let reg_packet = self.pack_packet(b"REGC");
            write_packet(&mut relay_stream, &reg_packet).map_err(|e| format!("注册失败: {}", e))?;
            
            // 等待确认（会等待房间在 relay 上就绪）
            relay_stream.set_read_timeout(Some(Duration::from_secs(15))).ok();
            match read_packet(&mut relay_stream) {
                Ok(packet) => {
                    if let Some(decrypted) = self.try_decrypt_response(&packet) {
                        if &decrypted == b"REGC_OK" {
                            self.log("[启动] 已注册到中继服务器".to_string());
                        } else if &decrypted == b"ERRR" {
                            return Err("中继服务器注册失败: 房间尚不存在，请稍后重试".to_string());
                        } else {
                            return Err("中继服务器注册失败: 无效响应".to_string());
                        }
                    } else {
                        return Err("中继服务器注册失败: 解密失败".to_string());
                    }
                }
                _ => {
                    return Err("中继服务器注册失败: 超时，请确认房主已创建房间".to_string());
                }
            }

            let relay_reader = relay_stream.try_clone().map_err(|e| e.to_string())?;
            relay_reader.set_read_timeout(Some(Duration::from_millis(100))).ok();
            let relay_writer = Arc::new(Mutex::new(relay_stream));
            let relay_reader = Arc::new(Mutex::new(relay_reader));
            self.relay_stream = Some(relay_writer.clone());
            let local_clients = Arc::new(Mutex::new(Vec::new()));

            // 启动局域网发现广播
            let running_clone = running.clone();
            let motd_clone = motd.clone();
            let local_port_clone = local_port;
            thread::spawn(move || {
                Self::lan_discovery_broadcaster(running_clone, motd_clone, local_port_clone);
            });

            // 线程1：接收中继数据并广播到所有本地客户端
            let running_clone = running.clone();
            let relay_stream_clone = relay_reader.clone();
            let local_clients_clone = local_clients.clone();
            let room = self.room.clone();
            let password = self.password.clone();
            thread::spawn(move || {
                Self::tcp_relay_to_local_relay(relay_stream_clone, local_clients_clone, running_clone, room, password);
            });

            // 线程2：接受本地连接并转发数据给中继
            let running_clone = running.clone();
            let relay_stream_clone = relay_writer.clone();
            let local_clients_clone = local_clients.clone();
            let room = self.room.clone();
            let password = self.password.clone();
            thread::spawn(move || {
                Self::local_to_tcp_relay(tcp_listener, relay_stream_clone, local_clients_clone, running_clone, room, password);
            });

            Ok(format!("成员模式(中继)已启动，本地端口: {}, 房间: {}", local_port, self.room))
        } else {
            // 直连模式
            let host_stream = TcpStream::connect(host_addr).map_err(|e| format!("连接房主失败: {}", e))?;
            host_stream.set_nonblocking(false).ok();
            let host_stream = Arc::new(Mutex::new(host_stream));
            let local_clients = Arc::new(Mutex::new(Vec::new()));
            
            // 启动局域网发现广播
            let running_clone = running.clone();
            let motd_clone = motd.clone();
            let local_port_clone = local_port;
            thread::spawn(move || {
                Self::lan_discovery_broadcaster(running_clone, motd_clone, local_port_clone);
            });

            // 线程1：接收房主数据并广播到所有本地客户端
            let running_clone = running.clone();
            let host_stream_clone = host_stream.clone();
            let local_clients_clone = local_clients.clone();
            thread::spawn(move || {
                Self::host_to_tcp_forward(host_stream_clone, local_clients_clone, running_clone);
            });

            // 线程2：接受本地连接并转发给房主
            let running_clone = running.clone();
            let host_stream_clone = host_stream.clone();
            let local_clients_clone = local_clients.clone();
            thread::spawn(move || {
                Self::local_to_host_forward(tcp_listener, host_stream_clone, local_clients_clone, running_clone);
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

    fn host_to_tcp_forward(host_stream: Arc<Mutex<TcpStream>>, local_clients: Arc<Mutex<Vec<TcpStream>>>, running: Arc<Mutex<bool>>) {        
        while *running.lock().unwrap() {
            // 从房主接收数据转发给所有本地客户端
            let mut buf = [0u8; 4096];
            match host_stream.lock().unwrap().read(&mut buf) {
                Ok(n) if n > 0 => {
                    let mut clients = local_clients.lock().unwrap();
                    clients.retain_mut(|client| {
                        client.write(&buf[..n]).is_ok()
                    });
                }
                Err(_) => break,
                _ => {}
            }
            
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn local_to_host_forward(listener: TcpListener, host_stream: Arc<Mutex<TcpStream>>, local_clients: Arc<Mutex<Vec<TcpStream>>>, running: Arc<Mutex<bool>>) {
        while *running.lock().unwrap() {
            if let Ok((stream, _)) = listener.accept() {
                stream.set_nonblocking(false).ok();
                local_clients.lock().unwrap().push(stream);
            }

            local_clients.lock().unwrap().retain_mut(|stream| {
                let mut buf = [0u8; 4096];
                match stream.read(&mut buf) {
                    Ok(0) => false,
                    Ok(n) => {
                        host_stream.lock().unwrap().write(&buf[..n]).ok();
                        true
                    }
                    Err(_) => false,
                }
            });

            thread::sleep(Duration::from_millis(1));
        }
    }

    // 中继模式：TCP中继→本地
    fn tcp_relay_to_local_relay(relay_stream: Arc<Mutex<TcpStream>>, local_clients: Arc<Mutex<Vec<TcpStream>>>, running: Arc<Mutex<bool>>, _room: String, password: String) {
        while *running.lock().unwrap() {
            match read_packet(&mut relay_stream.lock().unwrap()) {
                Ok(packet) => {
                    if let Some(decrypted) = Self::try_decrypt_response_static(&packet, &password) {
                        if decrypted.len() >= 4 && &decrypted[0..4] == b"DATA" {
                            let data = &decrypted[4..];
                            let mut clients = local_clients.lock().unwrap();
                            clients.retain_mut(|client| {
                                client.write(data).is_ok()
                            });
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock => {
                    continue;
                }
                Err(_) => break,
            }
        }
    }

    // 中继模式：本地→TCP中继
    fn local_to_tcp_relay(listener: TcpListener, relay_stream: Arc<Mutex<TcpStream>>, local_clients: Arc<Mutex<Vec<TcpStream>>>, running: Arc<Mutex<bool>>, room: String, password: String) {
        let mut notified = false;
        while *running.lock().unwrap() {
            if let Ok((stream, _)) = listener.accept() {
                println!("新本地TCP连接(中继模式)");
                stream.set_nonblocking(false).ok();
                local_clients.lock().unwrap().push(stream);
                
                if !notified {
                    notified = true;
                    let ready_packet = Self::pack_packet_static(&room, &password, b"MC_READY");
                    write_packet(&mut relay_stream.lock().unwrap(), &ready_packet).ok();
                    println!("已通知中继：Minecraft客户端已连接");
                }
            }

            local_clients.lock().unwrap().retain_mut(|stream| {
                let mut buf = [0u8;4096];
                match stream.read(&mut buf) {
                    Ok(0) => {
                        println!("TCP连接关闭");
                        false
                    }
                    Ok(n) => {
                        let mut payload = Vec::with_capacity(4 + n);
                        payload.extend_from_slice(b"DATA");
                        payload.extend_from_slice(&buf[..n]);
                        
                        let packet = Self::pack_packet_static(&room, &password, &payload);
                        write_packet(&mut relay_stream.lock().unwrap(), &packet).ok();
                        true
                    }
                    Err(_) => false,
                }
            });

            thread::sleep(Duration::from_millis(1));
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

    #[allow(dead_code)]
    pub fn send_test_packet(&self) {
        if self.use_relay {
            if let Some(ref stream) = self.relay_stream {
                let test_data = b"MC_LINK_TEST_PACKET";
                let encrypted = crypto::encrypt(test_data, &self.password);
                
                let mut packet = vec![0x41];
                let room_name = self.room.as_bytes();
                packet.push(room_name.len() as u8);
                packet.extend_from_slice(room_name);
                let pass = self.password.as_bytes();
                packet.push(pass.len() as u8);
                packet.extend_from_slice(pass);
                packet.extend_from_slice(&encrypted);
                
                let mut stream = stream.lock().unwrap();
                let _ = write_packet(&mut stream, &packet);
                println!("[测试] 已发送测试数据包到中继服务器");
            }
        } else {
            if let Some(ref stream) = self.relay_stream {
                let test_data = [0x41, 0x00, 0x00, 0x00, 0x00];
                let mut stream = stream.lock().unwrap();
                let _ = stream.write(&test_data);
                println!("[测试] 已发送测试数据包到房主");
            }
        }
    }

    #[allow(dead_code)]
    pub fn stop(&mut self) {
        *self.running.lock().unwrap() = false;
    }
}
