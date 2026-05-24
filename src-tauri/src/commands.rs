use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::thread;
use tauri::Emitter;
use tauri::Manager;
use serde::Serialize;
use crate::central;
use crate::client::ClientMode;
use crate::host::HostMode;
use crate::lan;
use crate::protocol;
use crate::state::AppState;
use uapi_sdk_rust::services::GetNetworkIpinfoParams;
use uapi_sdk_rust::Client as UapiClient;

#[derive(Serialize, Clone)]
pub(crate) struct LanServerInfo {
    motd: String,
    port: u16,
}

#[tauri::command]
pub(crate) fn get_relays() -> Result<Vec<central::RelayInfo>, String> {
    central::get_relays().ok_or_else(|| "获取中继列表失败".to_string())
}

#[tauri::command]
pub(crate) fn scan_lan_servers() -> Result<Vec<LanServerInfo>, String> {
    let servers = lan::scan_lan_servers()?;
    Ok(servers.into_iter().map(|s| LanServerInfo { motd: s.motd, port: s.port }).collect())
}

#[tauri::command]
pub(crate) fn get_latency(state: tauri::State<'_, AppState>) -> u64 {
    *state.latency_ms.lock().unwrap()
}

pub(crate) fn latency_monitor(relay_addr: std::net::SocketAddr, state: Arc<AppState>, window: tauri::Window) {
    thread::Builder::new()
        .name("latency-monitor".into())
        .spawn(move || {
            while *state.is_running.lock().unwrap() {
                thread::sleep(Duration::from_secs(3));
                if !*state.is_running.lock().unwrap() {
                    break;
                }
                let ping_ok = std::net::TcpStream::connect_timeout(&relay_addr, Duration::from_secs(3))
                    .and_then(|mut stream| {
                        let start = Instant::now();
                        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                        protocol::write_packet(&mut stream, &[0x32])?;
                        protocol::read_packet(&mut stream)?;
                        let ms = start.elapsed().as_millis() as u64;
                        *state.latency_ms.lock().unwrap() = ms;
                        let _ = window.emit("latency-update", ms);
                        Ok::<_, std::io::Error>(())
                    })
                    .is_ok();
                if !ping_ok {
                    *state.latency_ms.lock().unwrap() = 999;
                    let _ = window.emit("latency-update", 999u64);
                }
            }
            *state.latency_ms.lock().unwrap() = 0;
            let _ = window.emit("latency-update", 0u64);
        })
        .ok();
}

#[tauri::command]
pub(crate) async fn start_online(
    state: tauri::State<'_, AppState>,
    room_name: String,
    password: String,
    window: tauri::Window,
    selected_relay: String,
) -> Result<String, String> {
    if *state.is_running.lock().unwrap() {
        return Err("联机功能已在运行中".to_string());
    }

    let relay_addr: std::net::SocketAddr;
    let relay_id: String;

    if selected_relay == "__auto__" || selected_relay.is_empty() {
        window.emit("app-log", "[启动] 自动选择中继服务器...".to_string()).ok();
        let relays = central::get_relays().ok_or("网络错误，请检查网络连接")?;
        if relays.is_empty() {
            return Err("没有可用的中继服务器".to_string());
        }
        let relay = &relays[0];
        relay_id = relay.id.clone();
        relay_addr = protocol::resolve_address(&relay.address).ok_or("网络连接失败")?;
        window.emit("app-log", format!("[启动] 选中中继: {} ({})", relay.name, relay.address)).ok();
    } else if selected_relay.contains(':') {
        window.emit("app-log", format!("[启动] 自定义中继: {}", selected_relay)).ok();
        relay_addr = protocol::resolve_address(&selected_relay).ok_or("中继地址解析失败")?;
        relay_id = "custom".to_string();
    } else {
        let relays = central::get_relays().ok_or("网络错误，请检查网络连接")?;
        let relay = relays.iter().find(|r| r.id == selected_relay).ok_or("未找到选中的中继服务器")?;
        relay_id = relay.id.clone();
        relay_addr = protocol::resolve_address(&relay.address).ok_or("网络连接失败")?;
        window.emit("app-log", format!("[启动] 选中中继: {} ({})", relay.name, relay.address)).ok();
    }

    window.emit("app-log", "[启动] 检查房间状态...".to_string()).ok();
    let (exists, _) = central::get_room(&room_name).ok_or("网络错误，请检查网络连接")?;
    let is_host = !exists;

    let is_running = state.is_running.clone();
    let current_room = state.current_room.clone();
    let app_state = Arc::new(AppState {
        current_room: current_room.clone(),
        is_running: is_running.clone(),
        latency_ms: state.latency_ms.clone(),
        stop_signal: state.stop_signal.clone(),
    });

    let stop_signal = Arc::new(AtomicBool::new(false));
    *state.stop_signal.lock().unwrap() = Some(stop_signal.clone());

    if is_host {
        window.emit("app-log", "房主模式: 扫描局域网Minecraft服务器...".to_string()).ok();
        let servers = lan::scan_lan_servers()?;
        let mc_port = servers.first().map(|s| s.port).unwrap_or(0);
        let mc_motd = servers.first().map(|s| s.motd.clone()).unwrap_or_default();

        if servers.is_empty() || mc_port == 0 {
            return Err("未找到Minecraft局域网服务器，请先在Minecraft中开启局域网联机".to_string());
        }

        window.emit("app-log", format!("[启动] 发现Minecraft: 端口={}", mc_port)).ok();

        let mut host_mode = HostMode::new();
        host_mode.set_relay(relay_addr, room_name.clone(), password.clone());
        host_mode.set_log_callback({
            let w = window.clone();
            move |msg| {
                let _ = w.emit("app-log", msg);
            }
        });
        host_mode.connect_and_register().map_err(|e| format!("注册到中继服务器失败: {}", e))?;
        window.emit("app-log", "[启动] 已注册到中继服务器".to_string()).ok();

        central::create_room(&room_name, &password, &relay_id).ok_or("创建房间失败，可能房间名已存在")?;
        window.emit("app-log", format!("[启动] 房间已创建: {}", room_name)).ok();

        let w = window.clone();
        let rn = room_name.clone();
        let is_running_for_thread = is_running.clone();
        let current_room_for_thread = current_room.clone();

        let ss = stop_signal.clone();
        thread::Builder::new()
            .name("host-mode-worker".into())
            .spawn(move || {
                let result = host_mode.start(mc_port, mc_motd, ss);
                match &result {
                    Ok(msg) => { let _ = w.emit("app-log", msg); }
                    Err(e) => { let _ = w.emit("app-log", format!("[结束] {}", e)); }
                }
                central::delete_room(&rn);
                *is_running_for_thread.lock().unwrap() = false;
                *current_room_for_thread.lock().unwrap() = None;
            })
            .ok();

        latency_monitor(relay_addr, app_state, window.clone());
        *is_running.lock().unwrap() = true;
        *current_room.lock().unwrap() = Some((room_name, password));

        Ok(format!("房主模式已启动，Minecraft端口: {}", mc_port))
    } else {
        window.emit("app-log", "成员模式: 连接到中继服务器...".to_string()).ok();
        let local_port = 25565u16;
        let w = window.clone();
        let rn = room_name.clone();
        let pw = password.clone();
        let ra = relay_addr;
        let is_running_for_thread = is_running.clone();
        let current_room_for_thread = current_room.clone();

        let ss = stop_signal;
        thread::Builder::new()
            .name("client-mode-worker".into())
            .spawn(move || {
                let w2 = w.clone();
                let mut client_mode = ClientMode::new(local_port, "MC Link".to_string());
                client_mode.set_relay(ra, rn.clone(), pw.clone());
                client_mode.set_log_callback(move |msg| {
                    let _ = w2.emit("app-log", msg);
                });
                let result = client_mode.start(ss);
                match &result {
                    Ok(msg) => { let _ = w.emit("app-log", msg); }
                    Err(e) => { let _ = w.emit("app-log", format!("[结束] {}", e)); }
                }
                *is_running_for_thread.lock().unwrap() = false;
                *current_room_for_thread.lock().unwrap() = None;
            })
            .ok();

        latency_monitor(relay_addr, app_state, window.clone());
        *is_running.lock().unwrap() = true;
        *current_room.lock().unwrap() = Some((room_name, password));

        Ok(format!("成员模式已启动\n请在Minecraft中连接 127.0.0.1:{}", local_port))
    }
}

fn stop_online_inner(state: &AppState) {
    if let Some(sig) = state.stop_signal.lock().unwrap().take() {
        sig.store(true, Ordering::Relaxed);
    }
    thread::sleep(Duration::from_millis(500));
    *state.is_running.lock().unwrap() = false;
    let room = state.current_room.lock().unwrap().clone();
    if let Some((ref room_name, _)) = room {
        central::delete_room(room_name);
    }
    *state.current_room.lock().unwrap() = None;
    *state.latency_ms.lock().unwrap() = 0;
}

#[tauri::command]
pub(crate) async fn stop_online(state: tauri::State<'_, AppState>) -> Result<String, String> {
    stop_online_inner(&state);
    Ok("联机已停止".to_string())
}

#[tauri::command]
pub(crate) fn minimize_window(window: tauri::Window) {
    window.minimize().ok();
}

#[tauri::command]
pub(crate) fn maximize_window(window: tauri::Window) {
    if window.is_maximized().unwrap_or(false) {
        window.unmaximize().ok();
    } else {
        window.maximize().ok();
    }
}

#[tauri::command]
pub(crate) async fn close_window(_window: tauri::Window, state: tauri::State<'_, AppState>, app: tauri::AppHandle) -> Result<String, String> {
    stop_online_inner(&state);
    app.exit(0);
    Ok("已退出".to_string())
}

#[tauri::command]
pub(crate) async fn exit_app(state: tauri::State<'_, AppState>, app: tauri::AppHandle) -> Result<String, String> {
    stop_online_inner(&state);
    app.exit(0);
    Ok("已退出".to_string())
}

#[tauri::command]
pub(crate) fn show_window(window: tauri::Window) {
    let _ = window.show();
    let _ = window.set_focus();
}

#[tauri::command]
pub(crate) fn show_main_window(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[tauri::command]
pub(crate) fn set_tray_size(window: tauri::Window, width: f64, height: f64) {
    let _ = window.emit("tray-resize", serde_json::json!({"width": width, "height": height}));
}

#[tauri::command]
pub(crate) async fn ping_relay(address: String) -> Result<u64, String> {
    let addr = protocol::resolve_address(&address).ok_or("地址解析失败")?;
    let start = Instant::now();
    let mut stream = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3))
        .map_err(|e| format!("连接失败: {}", e))?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    protocol::write_packet(&mut stream, &[0x32]).map_err(|_| "发送失败".to_string())?;
    protocol::read_packet(&mut stream).map_err(|_| "无响应".to_string())?;
    Ok(start.elapsed().as_millis() as u64)
}

#[derive(Serialize)]
pub(crate) struct IpInfo {
    region: String,
    isp: String,
}

#[tauri::command]
pub(crate) async fn get_ip_info(host: String) -> Result<IpInfo, String> {
    let client = UapiClient::builder().build().map_err(|e| format!("创建客户端失败: {}", e))?;
    let params = GetNetworkIpinfoParams::new(&host);
    let resp = client.network().get_network_ipinfo(params).await.map_err(|e| {
        let msg = e.to_string();
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(&msg);
        if let Ok(val) = parsed {
            val.get("message").and_then(|m| m.as_str()).unwrap_or(&msg).to_string()
        } else {
            msg
        }
    })?;
    Ok(IpInfo {
        region: resp.region.unwrap_or_default(),
        isp: resp.isp.unwrap_or_default(),
    })
}