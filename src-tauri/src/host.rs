use std::net::{SocketAddr, TcpStream, Shutdown};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use crate::protocol;

pub struct HostMode {
    running: Arc<Mutex<bool>>,
    game_port: u16,
    motd: String,
    relay_stream: Option<Arc<Mutex<TcpStream>>>,
    relay_addr: Option<SocketAddr>,
    #[allow(dead_code)]
    relay_id: Option<String>,
    room: String,
    password: String,
    log_callback: Arc<Mutex<Option<Box<dyn Fn(String) + Send>>>>,
    test_callback: Arc<Mutex<Option<Box<dyn Fn() + Send>>>>,
}

impl HostMode {
    pub fn new() -> Self {
        Self {
            running: Arc::new(Mutex::new(false)),
            game_port: 0,
            motd: String::new(),
            relay_stream: None,
            relay_addr: None,
            relay_id: None,
            room: String::new(),
            password: String::new(),
            log_callback: Arc::new(Mutex::new(None)),
            test_callback: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_relay(&mut self, relay_addr: SocketAddr, room: String, password: String) {
        self.relay_addr = Some(relay_addr);
        self.room = room;
        self.password = password;
    }

    #[allow(dead_code)]
    pub fn set_relay_id(&mut self, relay_id: String) {
        self.relay_id = Some(relay_id);
    }

    pub fn set_log_callback<F>(&self, callback: F)
    where
        F: Fn(String) + Send + 'static,
    {
        *self.log_callback.lock().unwrap() = Some(Box::new(callback));
    }

    #[allow(dead_code)]
    pub fn set_test_callback<F>(&self, callback: F)
    where
        F: Fn() + Send + 'static,
    {
        *self.test_callback.lock().unwrap() = Some(Box::new(callback));
    }

    fn log(&self, msg: String) {
        println!("{}", msg);
        if let Some(ref callback) = *self.log_callback.lock().unwrap() {
            callback(msg);
        }
    }

    fn pack_packet(&self, data: &[u8]) -> Vec<u8> {
        protocol::pack_packet(&self.room, &self.password, data)
    }

    fn try_decrypt_response(&self, data: &[u8]) -> Option<Vec<u8>> {
        protocol::try_decrypt_response(data, &self.password)
    }

    pub fn connect_and_register(&mut self) -> Result<(), String> {
        let relay_addr = self.relay_addr.ok_or("请配置中继服务器")?;

        self.log(format!("[启动] 连接中继服务器: {}", relay_addr));
        let relay_stream = TcpStream::connect(relay_addr).map_err(|e| format!("连接中继服务器失败: {}", e))?;
        relay_stream.set_nodelay(true).ok();
        self.log("[启动] 已连接到中继服务器".to_string());

        self.log("[启动] 向中继服务器注册...".to_string());
        let reg_packet = self.pack_packet(b"REGH");
        let mut stream_clone = relay_stream.try_clone().map_err(|e| e.to_string())?;
        stream_clone.set_nodelay(true).ok();
        protocol::write_packet(&mut stream_clone, &reg_packet).map_err(|e| format!("注册失败: {}", e))?;

        stream_clone.set_read_timeout(Some(Duration::from_secs(5))).ok();
        match protocol::read_packet(&mut stream_clone) {
            Ok(packet) => {
                if let Some(decrypted) = self.try_decrypt_response(&packet) {
                    if &decrypted == b"REGH_OK" {
                        self.log("[启动] 已注册到中继服务器".to_string());
                    } else if &decrypted == b"ERRR" {
                        return Err("中继服务器注册失败: 房间已存在".to_string());
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

        self.relay_stream = Some(Arc::new(Mutex::new(relay_stream)));
        Ok(())
    }

    pub fn start(&mut self, selected_game_port: u16, motd: String, stop_signal: Arc<AtomicBool>) -> Result<String, String> {
        self.game_port = selected_game_port;
        self.motd = motd;
        let running = self.running.clone();
        *running.lock().unwrap() = true;
        let game_port = self.game_port;

        let relay_stream = self.relay_stream.clone().ok_or("请先调用 connect_and_register")?;

        self.log(format!("[启动] 房主模式，游戏端口: {}", game_port));

        let relay_stream = relay_stream.lock().unwrap().try_clone().map_err(|e| e.to_string())?;
        relay_stream.set_nodelay(true).ok();

        let relay_for_read = relay_stream.try_clone().map_err(|e| e.to_string())?;
        relay_for_read.set_nodelay(true).ok();
        relay_for_read.set_read_timeout(Some(Duration::from_millis(200))).ok();
        let relay_for_read = Arc::new(Mutex::new(relay_for_read));

        let relay_for_write = relay_stream;
        let relay_for_write = Arc::new(Mutex::new(relay_for_write));

        let mc_stream: Arc<Mutex<Option<TcpStream>>> = Arc::new(Mutex::new(None));
        let mc_connected = Arc::new(AtomicBool::new(false));

        let ss = stop_signal.clone();
        let relay_reader = relay_for_read.clone();
        let mc_for_read = mc_stream.clone();
        let mc_conn = mc_connected.clone();
        let room = self.room.clone();
        let password = self.password.clone();
        let log_callback = self.log_callback.clone();
        let test_callback = self.test_callback.clone();
        thread::Builder::new()
            .name("host-relay-to-mc".into())
            .spawn(move || {
                Self::tcp_to_mc_relay(relay_reader, mc_for_read, mc_conn, ss, game_port, room, password, log_callback, test_callback);
            })
            .ok();

        let ss = stop_signal;
        let relay_writer = relay_for_write.clone();
        let mc_for_write = mc_stream.clone();
        let mc_conn_for_write = mc_connected.clone();
        let room = self.room.clone();
        let password = self.password.clone();
        let log_callback = self.log_callback.clone();
        thread::Builder::new()
            .name("host-mc-to-relay".into())
            .spawn(move || {
                Self::mc_to_tcp_relay(mc_for_write, relay_writer, mc_conn_for_write, ss, room, password, log_callback);
            })
            .ok();

        Ok(format!("房主模式已启动\n等待成员加入后连接Minecraft\n房间: {}", self.room))
    }

    fn mc_to_tcp_relay(mc_stream: Arc<Mutex<Option<TcpStream>>>, relay_stream: Arc<Mutex<TcpStream>>, mc_connected: Arc<AtomicBool>, stop_signal: Arc<AtomicBool>, room: String, password: String, _log_callback: Arc<Mutex<Option<Box<dyn Fn(String) + Send>>>>) {

        while !mc_connected.load(Ordering::Relaxed) && !stop_signal.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(10));
        }
        if stop_signal.load(Ordering::Relaxed) {
            return;
        }

        while !stop_signal.load(Ordering::Relaxed) {
            let mc_data = {
                let guard = mc_stream.lock().unwrap();
                let stream = match guard.as_ref() {
                    Some(s) => s,
                    None => break,
                };
                let mut buf = [0u8; 4096];
                let mut mc = match stream.try_clone() {
                    Ok(mc) => mc,
                    Err(_) => break,
                };
                mc.set_nodelay(true).ok();
                mc.set_read_timeout(Some(Duration::from_millis(10))).ok();
                match mc.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => Some(buf[..n].to_vec()),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => None,
                    Err(_) => break,
                }
            };

            if let Some(data) = mc_data {
                let mut payload = Vec::with_capacity(4 + data.len());
                payload.extend_from_slice(b"DATA");
                payload.extend_from_slice(&data);

                let packet = protocol::pack_packet(&room, &password, &payload);
                if protocol::write_packet(&mut relay_stream.lock().unwrap(), &packet).is_err() {
                    break;
                }
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }

        if let Some(stream) = mc_stream.lock().unwrap().take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        mc_connected.store(false, Ordering::SeqCst);
    }

    fn tcp_to_mc_relay(relay_stream: Arc<Mutex<TcpStream>>, mc_stream: Arc<Mutex<Option<TcpStream>>>, mc_connected: Arc<AtomicBool>, stop_signal: Arc<AtomicBool>, game_port: u16, _room: String, password: String, log_callback: Arc<Mutex<Option<Box<dyn Fn(String) + Send>>>>, test_callback: Arc<Mutex<Option<Box<dyn Fn() + Send>>>>) {
        let log = |msg: String| {
            println!("{}", msg);
            if let Some(ref callback) = *log_callback.lock().unwrap() {
                callback(msg);
            }
        };

        let mut connected = false;

        while !stop_signal.load(Ordering::Relaxed) {
            let packet = {
                let mut relay = match relay_stream.lock() {
                    Ok(r) => r,
                    Err(_) => break,
                };
                match protocol::read_packet(&mut relay) {
                    Ok(p) => p,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                        continue;
                    }
                    Err(_) => break,
                }
            };

            if !packet.is_empty() && packet[0] == 0x41 {
                if let Some(ref callback) = *test_callback.lock().unwrap() {
                    callback();
                }
                continue;
            }

            let decrypted = match protocol::try_decrypt_response(&packet, &password) {
                Some(d) => d,
                None => continue,
            };

            if decrypted.len() < 4 || &decrypted[0..4] != b"DATA" {
                continue;
            }
            let data = &decrypted[4..];

            if !connected {
                let mc_addr = format!("127.0.0.1:{}", game_port);
                log(format!("[启动] 连接到Minecraft: {}", mc_addr));
                match TcpStream::connect(&mc_addr) {
                    Ok(mut stream) => {
                        stream.set_nodelay(true).ok();
                        stream.set_read_timeout(Some(Duration::from_millis(50))).ok();
                        log("[启动] 已连接到Minecraft服务器".to_string());
                        if stream.write(data).is_err() {
                            log("[错误] 写入Minecraft失败".to_string());
                            break;
                        }
                        stream.flush().ok();
                        mc_stream.lock().unwrap().replace(stream);
                        mc_connected.store(true, Ordering::SeqCst);
                        connected = true;
                    }
                    Err(e) => {
                        log(format!("[错误] 连接Minecraft失败: {}", e));
                        break;
                    }
                }
                continue;
            }

            let mc_write_ok = {
                let guard = mc_stream.lock().unwrap();
                match guard.as_ref() {
                    Some(mc) => match mc.try_clone() {
                        Ok(mut mc_clone) => {
                            mc_clone.set_nodelay(true).ok();
                            mc_clone.write_all(data).is_ok() && mc_clone.flush().is_ok()
                        }
                        Err(_) => false,
                    },
                    None => false,
                }
            };

            if !mc_write_ok {
                break;
            }
        }

        if let Some(stream) = mc_stream.lock().unwrap().take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        mc_connected.store(false, Ordering::SeqCst);
    }

    #[allow(dead_code)]
    pub fn send_test_packet(&self) {
        if let Some(ref stream) = self.relay_stream {
            let packet = protocol::relay_test_packet(&self.room, &self.password);
            let mut stream = stream.lock().unwrap();
            let _ = protocol::write_packet(&mut stream, &packet);
            println!("[测试] 已发送测试数据包");
        }
    }

    #[allow(dead_code)]
    pub fn stop(&mut self) {
        *self.running.lock().unwrap() = false;
    }
}