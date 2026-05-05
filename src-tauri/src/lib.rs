mod host;
mod client;
mod relay;
mod crypto;
mod latency;
mod routing;

use std::sync::Mutex;
use host::HostMode;
use client::ClientMode;
use relay::RelayMode;
use latency::RelayWithLatency;
use routing::{RelayTopology, RelayNode};
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};
use serde::{Serialize, Deserialize};
use tauri::{Emitter, Manager};

const CENTRAL_SERVER_ADDR: &str = "central-server.link.xigo.top:50248";

pub struct AppState {
    host_mode: Mutex<Option<HostMode>>,
    client_mode: Mutex<Option<ClientMode>>,
    relay_mode: Mutex<Option<RelayMode>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelayInfo {
    id: String,
    name: String,
    address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RoomInfo {
    name: String,
    password_hash: String,
    host_relay_id: String,
    created_at: u64,
}

#[derive(Serialize, Clone)]
struct LanServerInfo {
    motd: String,
    port: u16,
}

fn serialize<T: Serialize>(data: &T) -> Vec<u8> {
    serde_json::to_vec(data).unwrap_or_default()
}

fn deserialize<'a, T: Deserialize<'a>>(data: &'a [u8]) -> Option<T> {
    serde_json::from_slice(data).ok()
}

fn send_request(socket: &UdpSocket, cmd: u8, data: &[u8]) -> Option<Vec<u8>> {
    let central_addr: SocketAddr = match CENTRAL_SERVER_ADDR.parse() {
        Ok(addr) => addr,
        Err(_) => return None,
    };
    
    let mut packet = vec![cmd];
    packet.extend_from_slice(data);
    
    socket.send_to(&packet, central_addr).ok()?;
    
    let mut buf = vec![0u8; 65535];
    socket.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    
    match socket.recv_from(&mut buf) {
        Ok((len, _)) => Some(buf[..len].to_vec()),
        Err(_) => None,
    }
}

fn get_relays(socket: &UdpSocket) -> Option<Vec<RelayInfo>> {
    let response = send_request(socket, 0x12, &[])?;
    if response.len() > 0 && response[0] == 0x13 {
        deserialize(&response[1..])
    } else {
        None
    }
}

fn get_topology(socket: &UdpSocket) -> Option<RelayTopology> {
    let response = send_request(socket, 0x15, &[])?;
    if response.len() > 0 && response[0] == 0x16 {
        let topology_data: serde_json::Value = deserialize(&response[1..])?;
        let relays_data = topology_data.get("relays")?.as_array()?;
        let matrix_data = topology_data.get("latency_matrix")?.as_object()?;
        
        let mut relays = Vec::new();
        for r in relays_data {
            relays.push(RelayNode {
                id: r.get("id")?.as_str()?.to_string(),
                address: r.get("address")?.as_str()?.to_string(),
            });
        }
        
        let mut latency_matrix = std::collections::HashMap::new();
        for (from, tos) in matrix_data {
            let mut inner_map = std::collections::HashMap::new();
            let tos = tos.as_object()?;
            for (to, latency) in tos {
                inner_map.insert(to.to_string(), latency.as_u64()?);
            }
            latency_matrix.insert(from.to_string(), inner_map);
        }
        
        Some(RelayTopology { relays, latency_matrix })
    } else {
        None
    }
}

fn create_room(socket: &UdpSocket, room_name: &str, password: &str, relay_id: &str) -> Option<RoomInfo> {
    let req = serde_json::json!({
        "room_name": room_name,
        "password": password,
        "relay_id": relay_id,
    });
    let response = send_request(socket, 0x20, &serialize(&req))?;
    if response.len() > 1 && response[0] == 0x21 && response[1] == 0x00 {
        deserialize(&response[2..])
    } else {
        None
    }
}

fn get_room(socket: &UdpSocket, room_name: &str) -> Option<(bool, Option<RoomInfo>)> {
    let req = serde_json::json!({ "room_name": room_name });
    let response = send_request(socket, 0x22, &serialize(&req))?;
    if response.len() > 0 && response[0] == 0x23 {
        let room_data: serde_json::Value = deserialize(&response[1..])?;
        let exists = room_data.get("exists")?.as_bool()?;
        let room = room_data.get("room")?;
        
        if exists {
            let room_info: RoomInfo = serde_json::from_value(room.clone()).ok()?;
            Some((true, Some(room_info)))
        } else {
            Some((false, None))
        }
    } else {
        None
    }
}

fn scan_lan_servers_sync() -> Result<Vec<LanServerInfo>, String> {
    let host = HostMode::new();
    let servers = host.scan_lan_servers()?;
    
    let info_list: Vec<LanServerInfo> = servers.into_iter()
        .map(|s| LanServerInfo { motd: s.motd, port: s.port })
        .collect();
    
    Ok(info_list)
}

#[tauri::command]
fn scan_lan_servers() -> Result<Vec<LanServerInfo>, String> {
    scan_lan_servers_sync()
}

#[tauri::command]
async fn start_online(
    state: tauri::State<'_, AppState>,
    room_name: String,
    password: String,
    window: tauri::Window,
) -> Result<String, String> {
    let socket = match UdpSocket::bind(("0.0.0.0", 0)) {
        Ok(s) => s,
        Err(e) => return Err(format!("绑定UDP socket失败: {}", e)),
    };
    
    let servers = scan_lan_servers_sync()?;
    
    let mut mc_port = 0;
    let mut mc_motd = String::new();
    
    if let Some(server) = servers.first() {
        mc_port = server.port;
        mc_motd = server.motd.clone();
    }
    
    window.emit("app-log", "正在获取拓扑信息...").ok();
    
    let topology = match get_topology(&socket) {
        Some(t) => t,
        None => return Err("获取拓扑失败".to_string()),
    };
    
    if topology.relays.is_empty() {
        return Err("没有可用的中继服务器".to_string());
    }
    
    window.emit("app-log", format!("找到 {} 个中继服务器，正在测试延迟...", topology.relays.len())).ok();
    
    let mut relays_with_latency = Vec::new();
    for relay in &topology.relays {
        let latency = latency::test_relay_latency(&relay.address).await;
        relays_with_latency.push(RelayWithLatency {
            id: relay.id.clone(),
            name: relay.address.clone(),
            address: relay.address.clone(),
            latency_ms: latency,
        });
    }
    
    let my_best_relay = latency::select_best_relay(&relays_with_latency).await;
    
    let my_relay_id = match my_best_relay {
        Some(r) => r.id.clone(),
        None => return Err("没有可用的中继服务器".to_string()),
    };
    
    let my_relay_addr = match my_best_relay {
        Some(r) => r.address.clone(),
        None => return Err("没有可用的中继服务器".to_string()),
    };
    
    window.emit("app-log", format!("选择的本地中继: {} (延迟: {}ms)", 
        my_relay_addr, my_best_relay.as_ref().unwrap().latency_ms.unwrap_or(0))).ok();
    
    let room_exists = get_room(&socket, &room_name).ok_or("无法查询房间")?;
    
    if room_exists.0 {
        let room = room_exists.1.ok_or("房间不存在".to_string())?;
        if room.password_hash != password {
            return Err("房间名或密码错误".to_string());
        }
        
        let host_relay_id = room.host_relay_id;
        
        window.emit("app-log", format!("加入房间: {}", room_name)).ok();
        
        let addr: SocketAddr = my_relay_addr.parse().map_err(|_| "无效的中继地址".to_string())?;
        
        let mut client = ClientMode::new(addr, 25565, format!("房间: {}", room_name));
        client.set_relay(addr, room_name.clone(), password);
        
        let window_clone = window.clone();
        client.set_log_callback(move |msg| {
            let _ = window_clone.emit("app-log", msg);
        });
        
        let result = client.start()?;
        *state.client_mode.lock().unwrap() = Some(client);
        
        return Ok(result);
    } else {
        let _ = create_room(&socket, &room_name, &password, &my_relay_id);
        window.emit("app-log", format!("创建房间: {}", room_name)).ok();
        
        let addr: SocketAddr = my_relay_addr.parse().map_err(|_| "无效的中继地址".to_string())?;
        
        let mut host = HostMode::new();
        host.set_relay(addr, room_name.clone(), password);
        host.set_relay_id(my_relay_id);
        
        let window_clone = window.clone();
        host.set_log_callback(move |msg| {
            let _ = window_clone.emit("app-log", msg);
        });
        
        if servers.is_empty() {
            return Err("未找到Minecraft局域网服务器，请先开启".to_string());
        }
        
        let result = host.start(mc_port, mc_motd)?;
        *state.host_mode.lock().unwrap() = Some(host);
        
        return Ok(result);
    }
}

#[tauri::command]
fn stop_online(state: tauri::State<AppState>) -> Result<String, String> {
    if let Some(ref mut host) = *state.host_mode.lock().unwrap() {
        host.stop();
    }
    *state.host_mode.lock().unwrap() = None;
    
    if let Some(ref mut client) = *state.client_mode.lock().unwrap() {
        client.stop();
    }
    *state.client_mode.lock().unwrap() = None;
    
    Ok("联机已停止".to_string())
}

#[tauri::command]
fn start_relay_mode(state: tauri::State<AppState>) -> Result<String, String> {
    let mut relay = RelayMode::new();
    let result = relay.start()?;
    *state.relay_mode.lock().unwrap() = Some(relay);
    Ok(result)
}

#[tauri::command]
fn stop_relay_mode(state: tauri::State<AppState>) -> Result<String, String> {
    if let Some(ref mut relay) = *state.relay_mode.lock().unwrap() {
        relay.stop();
    }
    *state.relay_mode.lock().unwrap() = None;
    Ok("中继模式已停止".to_string())
}

#[tauri::command]
fn minimize_window(window: tauri::Window) {
    window.minimize().ok();
}

#[tauri::command]
fn maximize_window(window: tauri::Window) {
    if window.is_maximized().unwrap_or(false) {
        window.unmaximize().ok();
    } else {
        window.maximize().ok();
    }
}

#[tauri::command]
fn close_window(window: tauri::Window) {
    window.close().ok();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            host_mode: Mutex::new(None),
            client_mode: Mutex::new(None),
            relay_mode: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            scan_lan_servers,
            start_online,
            stop_online,
            start_relay_mode,
            stop_relay_mode,
            minimize_window,
            maximize_window,
            close_window
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
