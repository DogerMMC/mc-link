use std::collections::{HashMap, BinaryHeap};
use std::cmp::Ordering;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::io::{Read, Write};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use rand::Rng;

const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0";
const DEFAULT_LISTEN_PORT: u16 = 8878;
const DEFAULT_EXTERNAL_PORT: u16 = 8878;
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(120);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

const PROXY_PROTOCOL_V2_SIGNATURE: [u8; 12] = [0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A];

// ===== 拓扑数据结构 =====

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelayNode {
    id: String,
    name: String,
    address: String,
    last_seen: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LinkMetric {
    node_a: String,
    node_b: String,
    latency_ms: u16,
    packet_loss: f32,
    last_updated: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PathHop {
    node_id: String,
    address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PathResult {
    path_id: String,
    hops: Vec<PathHop>,
    total_latency_ms: u64,
    score: f64,
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

// ===== 拓扑图 =====

struct TopologyGraph {
    nodes: HashMap<String, RelayNode>,
    edges: HashMap<(String, String), LinkMetric>,
}

impl TopologyGraph {
    fn new() -> Self {
        Self { nodes: HashMap::new(), edges: HashMap::new() }
    }

    fn add_or_update_node(&mut self, node: RelayNode) {
        self.nodes.insert(node.id.clone(), node);
    }

    fn remove_node(&mut self, id: &str) {
        self.nodes.remove(id);
        self.edges.retain(|(a, b), _| a != id && b != id);
    }

    fn add_or_update_metric(&mut self, a: &str, b: &str, metric: LinkMetric) {
        let key = if a < b { (a.to_string(), b.to_string()) } else { (b.to_string(), a.to_string()) };
        self.edges.insert(key, metric);
    }

    fn get_neighbors(&self, node_id: &str) -> Vec<(&str, &LinkMetric)> {
        self.edges.iter()
            .filter(|((a, b), _)| a == node_id || b == node_id)
            .map(|((a, b), m)| if a == node_id { (b.as_str(), m) } else { (a.as_str(), m) })
            .collect()
    }
}

// ===== 配置 =====

#[derive(Debug, Deserialize, Serialize)]
struct Config {
    #[serde(default = "default_listen_addr")]
    listen_addr: String,
    #[serde(default = "default_listen_port")]
    listen_port: u16,
    #[serde(default = "default_external_port")]
    external_port: u16,
}

fn default_listen_addr() -> String { DEFAULT_LISTEN_ADDR.to_string() }
fn default_listen_port() -> u16 { DEFAULT_LISTEN_PORT }
fn default_external_port() -> u16 { DEFAULT_EXTERNAL_PORT }

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_addr: DEFAULT_LISTEN_ADDR.to_string(),
            listen_port: DEFAULT_LISTEN_PORT,
            external_port: DEFAULT_EXTERNAL_PORT,
        }
    }
}

const RELAYS_FILE: &str = "relays.json";

struct CentralState {
    relays: Mutex<HashMap<String, RelayNode>>,
    rooms: Mutex<HashMap<String, RoomInfo>>,
    latencies: Mutex<HashMap<String, LatencyEntry>>,
    topology: Mutex<TopologyGraph>,
    active_paths: Mutex<HashMap<String, PathResult>>,
    room_paths: Mutex<HashMap<String, String>>,
    addr_to_id: Mutex<HashMap<String, String>>,
    running: Arc<Mutex<bool>>,
}

impl CentralState {
    fn new() -> Self {
        Self {
            relays: Mutex::new(HashMap::new()),
            rooms: Mutex::new(HashMap::new()),
            latencies: Mutex::new(HashMap::new()),
            topology: Mutex::new(TopologyGraph::new()),
            active_paths: Mutex::new(HashMap::new()),
            room_paths: Mutex::new(HashMap::new()),
            addr_to_id: Mutex::new(HashMap::new()),
            running: Arc::new(Mutex::new(true)),
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
            if let Ok(relays) = serde_json::from_str::<HashMap<String, RelayNode>>(&content) {
                let mut current = self.relays.lock().unwrap();
                *current = relays;
                log(LogLevel::Info, &format!("已加载 {} 个中继服务器", current.len()));
            }
        }
    }

    fn stop(&self) { *self.running.lock().unwrap() = false; }
    fn is_running(&self) -> bool { *self.running.lock().unwrap() }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

enum LogLevel { Info, Warn, Error }

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

    println!("[{:02}:{:02}:{:02} {}{}\x1b[0m] {}", hours, minutes, seconds, color, level_str, msg);
}

fn load_config() -> Config {
    match fs::read_to_string("config.yaml") {
        Ok(content) => match serde_yaml::from_str(&content) {
            Ok(config) => { log(LogLevel::Info, "已加载配置文件 config.yaml"); config }
            Err(e) => { log(LogLevel::Error, &format!("配置文件解析失败: {}", e)); Config::default() }
        }
        Err(_) => {
            log(LogLevel::Info, "未找到配置文件，正在创建默认配置...");
            let default_config = Config::default();
            if let Ok(yaml) = serde_yaml::to_string(&default_config) {
                if fs::write("config.yaml", yaml).is_ok() {
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

#[derive(Debug, Clone)]
struct ProxyProtocolHeader { src_addr: SocketAddr, dst_addr: SocketAddr }

fn parse_proxy_protocol_v2(data: &[u8]) -> Option<ProxyProtocolHeader> {
    if data.len() < 16 || !data.starts_with(&PROXY_PROTOCOL_V2_SIGNATURE) { return None; }
    let version = (data[12] >> 4) & 0x0F;
    if version != 0x02 { return None; }
    let family = (data[13] >> 4) & 0x0F;
    if data[13] & 0x0F != 0x01 { return None; }
    let len = u16::from_be_bytes(data[14..16].try_into().unwrap()) as usize;
    let hd = &data[16..16+len];
    match family {
        0x01 if hd.len() >= 12 => Some(ProxyProtocolHeader {
            src_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(hd[0], hd[1], hd[2], hd[3])), u16::from_be_bytes(hd[8..10].try_into().unwrap())),
            dst_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(hd[4], hd[5], hd[6], hd[7])), u16::from_be_bytes(hd[10..12].try_into().unwrap())),
        }),
        0x02 if hd.len() >= 36 => {
            let src_ip = IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(&hd[0..16]).unwrap()));
            let dst_ip = IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(&hd[16..32]).unwrap()));
            Some(ProxyProtocolHeader {
                src_addr: SocketAddr::new(src_ip, u16::from_be_bytes(hd[32..34].try_into().unwrap())),
                dst_addr: SocketAddr::new(dst_ip, u16::from_be_bytes(hd[34..36].try_into().unwrap())),
            })
        }
        _ => None,
    }
}

fn read_proxy_protocol_header(stream: &mut TcpStream) -> Option<ProxyProtocolHeader> {
    let mut buf = [0u8; 16];
    match stream.peek(&mut buf) {
        Ok(n) if n >= 12 && buf[0..12] == PROXY_PROTOCOL_V2_SIGNATURE => {
            let mut header_buf = vec![0u8; 16];
            stream.read_exact(&mut header_buf).ok()?;
            let len = u16::from_be_bytes(header_buf[14..16].try_into().unwrap()) as usize;
            let mut addr_buf = vec![0u8; len];
            stream.read_exact(&mut addr_buf).ok()?;
            let mut full = header_buf;
            full.extend(addr_buf);
            parse_proxy_protocol_v2(&full)
        }
        _ => None,
    }
}

// ===== Dijkstra 路径规划 =====

fn find_optimal_path(
    graph: &TopologyGraph,
    host_relay_id: &str,
    client_relay_id: &str,
) -> Option<PathResult> {
    if host_relay_id == client_relay_id {
        if let Some(node) = graph.nodes.get(host_relay_id) {
            let hops = vec![
                PathHop { node_id: node.id.clone(), address: node.address.clone() },
            ];
            return Some(PathResult {
                path_id: Uuid::new_v4().to_string(),
                hops,
                total_latency_ms: 0,
                score: 0.0,
            });
        }
        return None;
    }

    struct State {
        node: String,
        cost: f64,
        hop_count: u32,
        path: Vec<String>,
    }
    impl Eq for State {}
    impl PartialEq for State { fn eq(&self, other: &Self) -> bool { self.cost == other.cost } }
    impl Ord for State {
        fn cmp(&self, other: &Self) -> Ordering {
            other.cost.partial_cmp(&self.cost).unwrap_or(Ordering::Equal)
        }
    }
    impl PartialOrd for State {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> { Some(self.cmp(other)) }
    }

    let mut distances: HashMap<String, f64> = HashMap::new();
    let mut heap = BinaryHeap::new();

    distances.insert(host_relay_id.to_string(), 0.0);
    heap.push(State {
        node: host_relay_id.to_string(),
        cost: 0.0,
        hop_count: 0,
        path: vec![host_relay_id.to_string()],
    });

    while let Some(State { node, cost, hop_count, path }) = heap.pop() {
        if &node == client_relay_id && hop_count >= 1 {
            let total_latency = path.windows(2).filter_map(|w| {
                let key = if w[0] < w[1] { (w[0].clone(), w[1].clone()) } else { (w[1].clone(), w[0].clone()) };
                graph.edges.get(&key)
            }).map(|m| m.latency_ms as u64).sum::<u64>();

            let hops: Vec<PathHop> = path.iter().filter_map(|id| {
                graph.nodes.get(id).map(|n| PathHop { node_id: n.id.clone(), address: n.address.clone() })
            }).collect();

            return Some(PathResult {
                path_id: Uuid::new_v4().to_string(),
                hops,
                total_latency_ms: total_latency,
                score: cost,
            });
        }

        if hop_count > 6 { continue; }

        for (neighbor, metric) in graph.get_neighbors(&node) {
            let edge_cost = metric.latency_ms as f64 * (1.0 + metric.packet_loss as f64 * 10.0);
            let next_cost = cost + edge_cost;

            if next_cost < *distances.get(neighbor).unwrap_or(&f64::MAX) {
                let mut p = path.clone();
                p.push(neighbor.to_string());
                distances.insert(neighbor.to_string(), next_cost);
                heap.push(State { node: neighbor.to_string(), cost: next_cost, hop_count: hop_count + 1, path: p });
            }
        }
    }

    None
}

// ===== 路径管理 =====

fn assign_room_path(state: &CentralState, room_name: &str, host_relay_id: &str, client_relay_id: &str) {
    // 找到成员的最佳接入中继
    let topology = state.topology.lock().unwrap();

    let path = if host_relay_id == client_relay_id {
        // 同一中继 → 单跳
        if let Some(node) = topology.nodes.get(host_relay_id) {
            Some(PathResult {
                path_id: Uuid::new_v4().to_string(),
                hops: vec![PathHop { node_id: node.id.clone(), address: node.address.clone() }],
                total_latency_ms: 0,
                score: 0.0,
            })
        } else { None }
    } else {
        find_optimal_path(&topology, host_relay_id, client_relay_id)
    };

    drop(topology);

    if let Some(path) = path {
        log(LogLevel::Info, &format!(
            "[路径] 房间 {} 路径: {} (延迟={}ms 跳数={})",
            room_name,
            path.hops.iter().map(|h| h.node_id.as_str()).collect::<Vec<_>>().join(" -> "),
            path.total_latency_ms,
            path.hops.len()
        ));

        let path_id = path.path_id.clone();
        state.active_paths.lock().unwrap().insert(path_id.clone(), path);
        state.room_paths.lock().unwrap().insert(room_name.to_string(), path_id);
    } else {
        log(LogLevel::Warn, &format!("[路径] 房间 {} 无法找到路径", room_name));
    }
}

fn reroute_affected_paths(state: &CentralState, dead_relay_id: &str) {
    let affected: Vec<(String, String)> = {
        let room_paths = state.room_paths.lock().unwrap();
        let active_paths = state.active_paths.lock().unwrap();
        let relays = state.relays.lock().unwrap();

        room_paths.iter().filter_map(|(room_name, path_id)| {
            if let Some(path) = active_paths.get(path_id) {
                if path.hops.iter().any(|h| h.node_id == dead_relay_id) {
                    // 找房主和成员的中继
                    if let Some(room) = state.rooms.lock().unwrap().get(room_name) {
                        let host_relay = room.host_relay_id.clone();
                        // 找一个不在受影响路径上的中继作为成员中继
                        let client_relay = relays.keys()
                            .find(|id| *id != dead_relay_id)
                            .cloned()
                            .unwrap_or_else(|| host_relay.clone());
                        return Some((room_name.clone(), host_relay));
                    }
                }
            }
            None
        }).collect()
    };

    // 找成员中继：简单方案——选任意在线中继
    let relays_snapshot = state.relays.lock().unwrap();
    let fallback_relays: Vec<String> = relays_snapshot.keys()
        .filter(|id| *id != dead_relay_id)
        .cloned()
        .collect();
    drop(relays_snapshot);

    if fallback_relays.is_empty() {
        log(LogLevel::Error, "[重路由] 没有可用中继节点，无法重路由");
        return;
    }

    for (room_name, host_relay) in &affected {
        let client_relay = &fallback_relays[rand::thread_rng().gen_range(0..fallback_relays.len())];
        log(LogLevel::Warn, &format!("[重路由] 路径 {} 中断，重新规划 {} -> {}", room_name, host_relay, client_relay));
        assign_room_path(state, room_name, host_relay, client_relay);
    }
}

// ===== 控制台 =====

fn print_help() {
    println!();
    println!("===========================================");
    println!("  MC Link 中央服务器 - 帮助");
    println!("===========================================");
    println!("  h - 显示帮助");
    println!("  s - 停止服务器");
    println!("  r - 重启服务器");
    println!("  c - 显示中继列表");
    println!("  t - 显示拓扑");
    println!("  p - 显示活跃路径");
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
            thread::spawn(move || cleanup_thread(state_clone));
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
    println!("  MC Link 中央服务器 v2 (多跳)");
    println!("===========================================");
    println!("  监听地址: {}", config.listen_addr);
    println!("  监听端口: {}", config.listen_port);
    println!("===========================================");
    println!();
    println!("按 h 获取帮助");
    println!();

    state.load_relays();
    {
        let relays = state.relays.lock().unwrap();
        let mut topo = state.topology.lock().unwrap();
        for (id, relay) in relays.iter() {
            topo.add_or_update_node(RelayNode {
                id: id.clone(),
                name: relay.name.clone(),
                address: relay.address.clone(),
                last_seen: relay.last_seen,
            });
        }
    }

    let listener = Arc::new(Mutex::new(start_server(state.clone(), &config)));

    let state_for_console = state.clone();
    let listener_for_console = listener.clone();

    thread::spawn(move || {
        loop {
            print!("> ");
            std::io::Write::flush(&mut std::io::stdout()).ok();

            let mut input = String::new();
            match std::io::stdin().read_line(&mut input) {
                Ok(0) | Err(_) => { thread::sleep(Duration::from_millis(100)); continue; }
                _ => {}
            }

            let input = input.trim();
            match input {
                "h" => print_help(),
                "s" => { log(LogLevel::Info, "正在停止服务器..."); state_for_console.stop(); }
                "r" => {
                    log(LogLevel::Info, "正在重启服务器...");
                    state_for_console.stop();
                    thread::sleep(Duration::from_secs(1));
                    if let Some(l) = start_server(state_for_console.clone(), &config) {
                        *listener_for_console.lock().unwrap() = Some(l);
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
                "t" => {
                    let topo = state_for_console.topology.lock().unwrap();
                    println!("\n拓扑:");
                    for edge in topo.edges.values() {
                        println!("  {} <-> {} 延迟={}ms 丢包={:.0}%",
                            edge.node_a, edge.node_b, edge.latency_ms, edge.packet_loss * 100.0);
                    }
                    if topo.edges.is_empty() {
                        println!("  (空 - 等待探针数据)");
                    }
                    println!();
                }
                "p" => {
                    let paths = state_for_console.active_paths.lock().unwrap();
                    let room_paths = state_for_console.room_paths.lock().unwrap();
                    println!("\n活跃路径:");
                    for (path_id, path) in paths.iter() {
                        let room = room_paths.iter().find(|(_, pid)| *pid == path_id);
                        let room_str = room.map(|(r, _)| r.as_str()).unwrap_or("?");
                        println!("  房间 {} [{}]: {} (延迟={}ms)",
                            room_str, &path_id[..8],
                            path.hops.iter().map(|h| h.node_id.as_str()).collect::<Vec<_>>().join(" -> "),
                            path.total_latency_ms);
                    }
                    if paths.is_empty() { println!("  (无)"); }
                    println!();
                }
                "q" => {
                    log(LogLevel::Info, "正在关闭服务器...");
                    state_for_console.stop();
                    state_for_console.save_relays();
                    break;
                }
                "" => {}
                _ => { log(LogLevel::Warn, &format!("未知命令: {}", input)); print_help(); }
            }
        }
    });

    loop {
        if !state.is_running() { break; }

        if let Some(ref listener) = *listener.clone().lock().unwrap() {
            match listener.accept() {
                Ok((mut stream, addr)) => {
                    let real_addr = match read_proxy_protocol_header(&mut stream) {
                        Some(header) => {
                            log(LogLevel::Info, &format!("Proxy Protocol V2 - 客户端: {}", header.src_addr));
                            header.src_addr
                        }
                        None => addr,
                    };
                    let state_clone = state.clone();
                    thread::spawn(move || handle_client(&mut stream, state_clone, real_addr));
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
                if data.is_empty() { continue; }
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
            state.topology.lock().unwrap().remove_node(&relay_id);
            reroute_affected_paths(&state, &relay_id);
            log(LogLevel::Warn, &format!("[中继/断开] {} 已离线，触发重路由", relay.name));
        }
    }
}

fn handle_packet(stream: &mut TcpStream, state: &CentralState, src: SocketAddr, data: &[u8]) {
    if data.is_empty() { return; }
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
        0x31 => handle_probe_report(state, payload),
        _ => log(LogLevel::Warn, &format!("未知命令: 0x{:02x}", cmd)),
    }
}

// ===== 协议处理 =====

#[derive(Deserialize)]
struct RelayRegisterReq { id: String, name: String, address: String }

fn handle_relay_register(stream: &mut TcpStream, state: &CentralState, src: SocketAddr, data: &[u8]) {
    if let Ok(req) = serde_json::from_slice::<RelayRegisterReq>(data) {
        let mut relays = state.relays.lock().unwrap();
        let mut addr_to_id = state.addr_to_id.lock().unwrap();

        let relay = RelayNode {
            id: req.id.clone(),
            name: req.name.clone(),
            address: req.address.clone(),
            last_seen: now_secs(),
        };

        state.topology.lock().unwrap().add_or_update_node(relay.clone());
        relays.insert(req.id.clone(), relay);
        addr_to_id.insert(src.to_string(), req.id.clone());
        drop(addr_to_id);

        log(LogLevel::Info, &format!("[中继/注册] {} 注册在 {}", req.name, req.address));
        write_packet(stream, &[0x10, 0x00]).ok();
    }
}

#[derive(Deserialize)]
struct RelayHeartbeatReq { id: String }

fn handle_relay_heartbeat(state: &CentralState, data: &[u8]) {
    if let Ok(req) = serde_json::from_slice::<RelayHeartbeatReq>(data) {
        let mut relays = state.relays.lock().unwrap();
        if let Some(relay) = relays.get_mut(&req.id) {
            relay.last_seen = now_secs();
            let mut topo = state.topology.lock().unwrap();
            if let Some(node) = topo.nodes.get_mut(&req.id) {
                node.last_seen = now_secs();
            }
        }
    }
}

fn handle_get_relays(stream: &mut TcpStream, state: &CentralState) {
    let relays = state.relays.lock().unwrap();
    let relay_list: Vec<&RelayNode> = relays.values().collect();
    let mut response = vec![0x13];
    if let Ok(json) = serde_json::to_string(&relay_list) {
        response.extend_from_slice(json.as_bytes());
    }
    write_packet(stream, &response).ok();
}

#[derive(Deserialize)]
struct LatencyReportReq { from_id: String, to_id: String, latency_ms: u64 }

fn handle_latency_report(state: &CentralState, data: &[u8]) {
    if let Ok(req) = serde_json::from_slice::<LatencyReportReq>(data) {
        let key = format!("{}->{}", req.from_id, req.to_id);
        let mut latencies = state.latencies.lock().unwrap();
        let entry = latencies.entry(key).or_insert_with(|| LatencyEntry {
            from_id: req.from_id.clone(), to_id: req.to_id.clone(), latency_ms: 0, samples: Vec::new(),
        });
        entry.samples.push(req.latency_ms);
        if entry.samples.len() > 10 { entry.samples.remove(0); }
        entry.latency_ms = entry.samples.iter().sum::<u64>() / entry.samples.len() as u64;
    }
}

#[derive(Deserialize)]
struct ProbeReport {
    from_id: String,
    to_id: String,
    latency_ms: u16,
    packet_loss: f32,
}

fn handle_probe_report(state: &CentralState, data: &[u8]) {
    if let Ok(report) = serde_json::from_slice::<ProbeReport>(data) {
        let metric = LinkMetric {
            node_a: report.from_id.clone(),
            node_b: report.to_id.clone(),
            latency_ms: report.latency_ms,
            packet_loss: report.packet_loss,
            last_updated: now_secs(),
        };
        state.topology.lock().unwrap().add_or_update_metric(&report.from_id, &report.to_id, metric);
        log(LogLevel::Info, &format!("[探针/上报] {} -> {} 延迟={}ms 丢包={:.0}%",
            report.from_id, report.to_id, report.latency_ms, report.packet_loss * 100.0));
    }
}

fn handle_get_topology(stream: &mut TcpStream, state: &CentralState) {
    let relays = state.relays.lock().unwrap();
    let latencies = state.latencies.lock().unwrap();

    let relay_list: Vec<&RelayNode> = relays.values().collect();
    let mut latency_matrix: HashMap<String, HashMap<String, u64>> = HashMap::new();
    for (key, entry) in latencies.iter() {
        let parts: Vec<&str> = key.split("->").collect();
        if parts.len() == 2 {
            latency_matrix.entry(parts[0].to_string()).or_default().insert(parts[1].to_string(), entry.latency_ms);
        }
    }

    let resp = serde_json::to_string(&serde_json::json!({
        "relays": relay_list,
        "latency_matrix": latency_matrix,
    })).unwrap();

    let mut response = vec![0x16];
    response.extend_from_slice(resp.as_bytes());
    write_packet(stream, &response).ok();
}

#[derive(Deserialize)]
struct CreateRoomReq { room_name: String, password: String, relay_id: String }

fn handle_create_room(stream: &mut TcpStream, state: &CentralState, src: SocketAddr, data: &[u8]) {
    if let Ok(req) = serde_json::from_slice::<CreateRoomReq>(data) {
        let mut rooms = state.rooms.lock().unwrap();
        if rooms.contains_key(&req.room_name) {
            write_packet(stream, &[0x21, 0x01]).ok();
            return;
        }

        let relay_id = req.relay_id.clone();
        let room = RoomInfo {
            name: req.room_name.clone(),
            password_hash: req.password,
            host_relay_id: req.relay_id,
            created_at: now_secs(),
        };

        rooms.insert(req.room_name.clone(), room.clone());
        log(LogLevel::Info, &format!("房间创建: {} (来自 {}, 中继: {})", req.room_name, src, relay_id));

        let mut response = vec![0x21, 0x00];
        if let Ok(json) = serde_json::to_string(&room) {
            response.extend_from_slice(json.as_bytes());
        }
        write_packet(stream, &response).ok();
    }
}

#[derive(Deserialize)]
struct GetRoomReq { room_name: String }

fn handle_get_room(stream: &mut TcpStream, state: &CentralState, src: SocketAddr, data: &[u8]) {
    if let Ok(req) = serde_json::from_slice::<GetRoomReq>(data) {
        let rooms = state.rooms.lock().unwrap();

        if let Some(room) = rooms.get(&req.room_name) {
            log(LogLevel::Info, &format!("[房间/加入] {} 加入房间 {} (来自 {})", room.host_relay_id, req.room_name, src));

            // 找到成员的"最佳"中继（这里简化为第二个可用的中继，实际需要客户端上报）
            let client_relay_id = {
                let relays = state.relays.lock().unwrap();
                relays.keys()
                    .find(|id| *id != &room.host_relay_id)
                    .cloned()
                    .unwrap_or_else(|| room.host_relay_id.clone())
            };

            assign_room_path(state, &req.room_name, &room.host_relay_id, &client_relay_id);

            let path = state.room_paths.lock().unwrap().get(&req.room_name).and_then(|pid| {
                state.active_paths.lock().unwrap().get(pid).cloned()
            });

            let response_data = serde_json::to_string(&serde_json::json!({
                "exists": true,
                "room": room,
                "path": path.map(|p| serde_json::json!({
                    "path_id": p.path_id,
                    "hops": p.hops,
                    "total_latency_ms": p.total_latency_ms,
                }))
            })).unwrap();

            let mut response = vec![0x23];
            response.extend_from_slice(response_data.as_bytes());
            write_packet(stream, &response).ok();
        } else {
            log(LogLevel::Info, &format!("[房间/查询] 房间不存在: {} (来自 {})", req.room_name, src));
            let response_data = serde_json::to_string(&serde_json::json!({
                "exists": false,
            })).unwrap();
            let mut response = vec![0x23];
            response.extend_from_slice(response_data.as_bytes());
            write_packet(stream, &response).ok();
        }
    }
}

#[derive(Deserialize)]
struct DeleteRoomReq { room_name: String }

fn handle_delete_room(stream: &mut TcpStream, state: &CentralState, src: SocketAddr, data: &[u8]) {
    if let Ok(req) = serde_json::from_slice::<DeleteRoomReq>(data) {
        let mut rooms = state.rooms.lock().unwrap();
        if rooms.remove(&req.room_name).is_some() {
            state.room_paths.lock().unwrap().remove(&req.room_name);
            log(LogLevel::Info, &format!("房间已删除: {} (来自 {})", req.room_name, src));
            write_packet(stream, &[0x25, 0x00]).ok();
        } else {
            write_packet(stream, &[0x25, 0x01]).ok();
        }
    }
}

// ===== 后台线程 =====

fn cleanup_thread(state: Arc<CentralState>) {
    loop {
        thread::sleep(CLEANUP_INTERVAL);
        let now = now_secs();
        let timeout = HEARTBEAT_TIMEOUT.as_secs();

        let mut relays = state.relays.lock().unwrap();
        let dead_ids: Vec<String> = relays.iter()
            .filter(|(_, r)| now - r.last_seen > timeout)
            .map(|(id, _)| id.clone())
            .collect();

        for id in dead_ids {
            if let Some(relay) = relays.remove(&id) {
                log(LogLevel::Warn, &format!("清理离线中继: {} (ID: {})", relay.name, id));
                state.topology.lock().unwrap().remove_node(&id);
                reroute_affected_paths(&state, &id);
            }
        }
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