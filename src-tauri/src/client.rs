use std::net::{TcpListener, TcpStream, SocketAddr, Shutdown};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use crate::protocol;
use crate::lan;

pub struct ClientMode {
    running: Arc<Mutex<bool>>,
    local_port: u16,
    motd: String,
    relay_addr: SocketAddr,
    relay_stream: Option<Arc<Mutex<TcpStream>>>,
    room: String,
    password: String,
    log_callback: Arc<Mutex<Option<Box<dyn Fn(String) + Send>>>>,
}

impl ClientMode {
    pub fn new(local_port: u16, motd: String) -> Self {
        Self {
            running: Arc::new(Mutex::new(false)),
            local_port,
            motd,
            relay_addr: "127.0.0.1:0".parse().unwrap(),
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
        self.relay_addr = relay_addr;
        self.room = room;
        self.password = password;
    }

    fn pack_packet(&self, data: &[u8]) -> Vec<u8> {
        protocol::pack_packet(&self.room, &self.password, data)
    }

    fn try_decrypt_response(&self, data: &[u8]) -> Option<Vec<u8>> {
        protocol::try_decrypt_response(data, &self.password)
    }

    pub fn start(&mut self, stop_signal: Arc<AtomicBool>) -> Result<String, String> {
        *self.running.lock().unwrap() = true;

        let motd = self.motd.clone();

        let mut local_port = self.local_port;
        let tcp_listener = loop {
            match TcpListener::bind(format!("0.0.0.0:{}", local_port)) {
                Ok(listener) => break listener,
                Err(e) => {
                    if local_port >= 65535 {
                        return Err(format!("TCP绑定失败: 无法找到可用端口 (从{}开始尝试至65535)", self.local_port));
                    }
                    self.log(format!("[启动] 端口 {} 被占用 ({}), 尝试下一个...", local_port, e));
                    local_port += 1;
                }
            }
        };
        self.log(format!("[启动] 本地监听端口: {}", local_port));

        let relay = self.relay_addr;
        self.log(format!("[启动] 连接中继服务器: {}", relay));
        let mut relay_stream = TcpStream::connect(relay).map_err(|e| format!("连接中继服务器失败: {}", e))?;
        relay_stream.set_nodelay(true).ok();
        self.log("[启动] 已连接到中继服务器".to_string());

        self.log("[启动] 向中继服务器注册...".to_string());
        let reg_packet = self.pack_packet(b"REGC");
        protocol::write_packet(&mut relay_stream, &reg_packet).map_err(|e| format!("注册失败: {}", e))?;

        relay_stream.set_read_timeout(Some(Duration::from_secs(15))).ok();
        match protocol::read_packet(&mut relay_stream) {
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
            Err(e) => {
                return Err(format!("中继服务器注册失败: 超时({})，请确认房主已创建房间", e));
            }
        }

        let relay_reader = relay_stream.try_clone().map_err(|e| format!("克隆中继连接失败: {}", e))?;
        relay_reader.set_nodelay(true).ok();
        relay_reader.set_read_timeout(Some(Duration::from_millis(200))).ok();
        let relay_writer = Arc::new(Mutex::new(relay_stream));
        let relay_reader = Arc::new(Mutex::new(relay_reader));
        self.relay_stream = Some(relay_writer.clone());

        let local_clients = Arc::new(Mutex::new(Vec::new()));

        let running_clone = self.running.clone();
        let motd_clone = motd.clone();
        thread::Builder::new()
            .name("client-lan-broadcast".into())
            .spawn(move || {
                Self::start_lan_broadcast(running_clone, motd_clone, local_port);
            })
            .map_err(|e| format!("启动LAN广播线程失败: {}", e))?;

        let ss = stop_signal.clone();
        let relay_stream_clone = relay_reader.clone();
        let local_clients_clone = local_clients.clone();
        let password_clone = self.password.clone();
        thread::Builder::new()
            .name("client-relay-to-local".into())
            .spawn(move || {
                Self::tcp_relay_to_local_relay(relay_stream_clone, local_clients_clone, ss, password_clone);
            })
            .map_err(|e| format!("启动中继→本地转发线程失败: {}", e))?;

        let ss = stop_signal;
        let relay_stream_clone = relay_writer.clone();
        let local_clients_clone = local_clients.clone();
        let room_clone = self.room.clone();
        let password_clone = self.password.clone();
        thread::Builder::new()
            .name("client-local-to-relay".into())
            .spawn(move || {
                Self::local_to_relay_forward(tcp_listener, relay_stream_clone, local_clients_clone, ss, room_clone, password_clone);
            })
            .map_err(|e| format!("启动本地→中继转发线程失败: {}", e))?;

        Ok(format!("成员模式已启动，本地端口: {}, 房间: {}", local_port, self.room))
    }

    fn start_lan_broadcast(running: Arc<Mutex<bool>>, motd: String, port: u16) {
        Self::log_debug(format!("LAN广播线程启动，端口: {}", port));
        lan::lan_discovery_broadcaster(running, motd, port);
        Self::log_debug("LAN广播线程退出".to_string());
    }

    fn local_to_relay_forward(listener: TcpListener, relay_stream: Arc<Mutex<TcpStream>>, local_clients: Arc<Mutex<Vec<Arc<Mutex<Option<TcpStream>>>>>>, stop_signal: Arc<AtomicBool>, room: String, password: String) {
        Self::log_debug("本地→中继转发线程启动，监听中...".to_string());
        listener.set_nonblocking(true).ok();

        while !stop_signal.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, addr)) => {
                    stream.set_nonblocking(false).ok();
                    stream.set_nodelay(true).ok();
                    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
                    Self::log_debug(format!("新Minecraft连接(中继模式): {}", addr));
                    let client_ref = Arc::new(Mutex::new(Some(stream.try_clone().unwrap())));
                    local_clients.lock().unwrap().push(client_ref.clone());

                    let relay = relay_stream.clone();
                    let ss = stop_signal.clone();
                    let r = room.clone();
                    let pw = password.clone();
                    let client_cleanup = client_ref.clone();
                    thread::Builder::new()
                        .name(format!("client-handler-{}", addr.port()))
                        .spawn(move || {
                            let mut buf = [0u8; 4096];
                            while !ss.load(Ordering::Relaxed) {
                                match stream.read(&mut buf) {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        let mut payload = Vec::with_capacity(4 + n);
                                        payload.extend_from_slice(b"DATA");
                                        payload.extend_from_slice(&buf[..n]);
                                        let packet = protocol::pack_packet(&r, &pw, &payload);
                                        {
                                            let mut relay_guard = relay.lock().unwrap();
                                            if protocol::write_packet(&mut relay_guard, &packet).is_err() {
                                                break;
                                            }
                                            relay_guard.flush().ok();
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                            let _ = stream.shutdown(Shutdown::Both);
                            client_cleanup.lock().unwrap().take();
                        })
                        .ok();
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => {
                    Self::log_debug(format!("accept失败: {}", e));
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        Self::log_debug("本地→中继转发线程退出".to_string());
    }

    fn tcp_relay_to_local_relay(relay_stream: Arc<Mutex<TcpStream>>, local_clients: Arc<Mutex<Vec<Arc<Mutex<Option<TcpStream>>>>>>, stop_signal: Arc<AtomicBool>, password: String) {
        Self::log_debug("中继→本地转发线程启动".to_string());
        while !stop_signal.load(Ordering::Relaxed) {
            let packet = {
                let mut relay = match relay_stream.lock() {
                    Ok(r) => r,
                    Err(_) => break,
                };
                match protocol::read_packet(&mut relay) {
                    Ok(p) => p,
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock => {
                        continue;
                    }
                    Err(e) => {
                        Self::log_debug(format!("从中继读取失败: {}", e));
                        break;
                    }
                }
            };

            let decrypted = match protocol::try_decrypt_response(&packet, &password) {
                Some(d) => d,
                None => continue,
            };

            if decrypted.len() < 4 || &decrypted[0..4] != b"DATA" {
                continue;
            }
            let data = &decrypted[4..];

            let data_vec = data.to_vec();
            let mut clients = local_clients.lock().unwrap();
            clients.retain_mut(|client| {
                let mut guard = client.lock().unwrap();
                match guard.as_mut() {
                    Some(stream) => {
                        stream.set_nodelay(true).ok();
                        if stream.write_all(&data_vec).is_ok() && stream.flush().is_ok() {
                            true
                        } else {
                            guard.take();
                            false
                        }
                    }
                    None => false,
                }
            });
        }
        Self::log_debug("中继→本地转发线程退出".to_string());
    }

    fn log_debug(msg: String) {
        println!("[DEBUG] {}", msg);
    }

    #[allow(dead_code)]
    pub fn stop(&mut self) {
        *self.running.lock().unwrap() = false;
    }
}