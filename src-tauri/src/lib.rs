mod host;
mod client;
mod crypto;

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use host::HostMode;
use client::ClientMode;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};
use serde::{Serialize, Deserialize};
use tauri::Emitter;
use tauri::Manager;
use std::io::{Read, Write};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

const CENTRAL_SERVER_ADDR: &str = "127.0.0.1:8878";

fn resolve_address(addr_str: &str) -> Option<SocketAddr> {
    if let Ok(addr) = addr_str.parse::<SocketAddr>() {
        return Some(addr);
    }
    let parts: Vec<&str> = addr_str.rsplitn(2, ':').collect();
    if parts.len() != 2 {
        return None;
    }
    let port = parts[0].parse::<u16>().ok()?;
    let hostname = parts[1];
    std::net::ToSocketAddrs::to_socket_addrs(&(hostname, port)).ok()?.next()
}

pub struct AppState {
    current_room: Arc<Mutex<Option<(String, String)>>>,
    is_running: Arc<Mutex<bool>>,
    latency_ms: Arc<Mutex<u64>>,
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

fn send_request(cmd: u8, data: &[u8]) -> Option<Vec<u8>> {
    let central_addr: SocketAddr = resolve_address(CENTRAL_SERVER_ADDR)?;
    let mut stream = TcpStream::connect(central_addr).ok()?;
    let mut packet = vec![cmd];
    packet.extend_from_slice(data);
    write_packet(&mut stream, &packet).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    read_packet(&mut stream).ok()
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

fn get_relays() -> Option<Vec<RelayInfo>> {
    let response = send_request(0x12, &[])?;
    if !response.is_empty() && response[0] == 0x13 {
        deserialize(&response[1..])
    } else {
        None
    }
}

fn create_room(room_name: &str, password: &str, relay_id: &str) -> Option<RoomInfo> {
    let req = serde_json::json!({"room_name": room_name, "password": password, "relay_id": relay_id});
    let response = send_request(0x20, &serialize(&req))?;
    if response.len() > 1 && response[0] == 0x21 && response[1] == 0x00 {
        deserialize(&response[2..])
    } else {
        None
    }
}

fn get_room(room_name: &str) -> Option<(bool, Option<RoomInfo>)> {
    let req = serde_json::json!({"room_name": room_name});
    let response = send_request(0x22, &serialize(&req))?;
    if !response.is_empty() && response[0] == 0x23 {
        let room_data: serde_json::Value = deserialize(&response[1..])?;
        let exists = room_data.get("exists")?.as_bool()?;
        let room = room_data.get("room")?;
        if exists {
            Some((true, Some(serde_json::from_value(room.clone()).ok()?)))
        } else {
            Some((false, None))
        }
    } else {
        None
    }
}

fn delete_room(room_name: &str) -> bool {
    let req = serde_json::json!({"room_name": room_name});
    if let Some(response) = send_request(0x24, &serialize(&req)) {
        return response.len() >= 2 && response[0] == 0x25 && response[1] == 0x00;
    }
    false
}

fn scan_lan_servers_sync() -> Result<Vec<LanServerInfo>, String> {
    let host = HostMode::new();
    let servers = host.scan_lan_servers()?;
    Ok(servers.into_iter().map(|s| LanServerInfo { motd: s.motd, port: s.port }).collect())
}

fn latency_monitor(relay_addr: SocketAddr, state: Arc<AppState>, window: tauri::Window) {
    std::thread::spawn(move || {
        while *state.is_running.lock().unwrap() {
            std::thread::sleep(Duration::from_secs(3));
            if !*state.is_running.lock().unwrap() {
                break;
            }
            let ping_ok = TcpStream::connect_timeout(&relay_addr, Duration::from_secs(3)).and_then(|mut stream| {
                let start = Instant::now();
                stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                write_packet(&mut stream, &[0x32])?;
                read_packet(&mut stream)?;
                let ms = start.elapsed().as_millis() as u64;
                *state.latency_ms.lock().unwrap() = ms;
                let _ = window.emit("latency-update", ms);
                Ok::<_, std::io::Error>(())
            }).is_ok();
            if !ping_ok {
                *state.latency_ms.lock().unwrap() = 999;
                let _ = window.emit("latency-update", 999u64);
            }
        }
        *state.latency_ms.lock().unwrap() = 0;
        let _ = window.emit("latency-update", 0u64);
    });
}

fn get_cursor_pos() -> (i32, i32) {
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        extern "system" {
            fn GetCursorPos(lpPoint: *mut i32) -> i32;
        }
        let mut pt = [0i32; 2];
        unsafe { GetCursorPos(pt.as_mut_ptr()); }
        (pt[0], pt[1])
    }
    #[cfg(not(windows))]
    { (0, 0) }
}

#[allow(dead_code)]
fn try_set_window_backdrop(window: &tauri::WebviewWindow) {
    #[cfg(windows)]
    {
        use raw_window_handle::HasWindowHandle;
        if let Ok(handle) = window.window_handle() {
            if let raw_window_handle::RawWindowHandle::Win32(win32) = handle.as_raw() {
                let hwnd = win32.hwnd.get() as *mut std::ffi::c_void;
                #[link(name = "dwmapi")]
                extern "system" {
                    fn DwmSetWindowAttribute(
                        hwnd: *mut std::ffi::c_void,
                        dwAttribute: u32,
                        pvAttribute: *const std::ffi::c_void,
                        cbAttribute: u32,
                    ) -> i32;
                }
                let backdrop_type: u32 = 4;
                unsafe {
                    DwmSetWindowAttribute(hwnd, 38, &backdrop_type as *const _ as *const _, 4);
                }
            }
        }
    }
}

fn setup_tray(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    TrayIconBuilder::new()
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("MC Link")
        .on_tray_icon_event(|tray, event| {
            let app = tray.app_handle();
            match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    if let Some(window) = app.get_webview_window("main") {
                        if window.is_visible().unwrap_or(false) {
                            let _ = window.hide();
                        } else {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                }
                TrayIconEvent::Click {
                    button: MouseButton::Right,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    if let Some(existing) = app.get_webview_window("tray-menu") {
                        let _ = existing.close();
                    }

                    let (x, y) = get_cursor_pos();

                    if let Some(window) = tauri::WebviewWindowBuilder::new(
                        app,
                        "tray-menu",
                        tauri::WebviewUrl::App("tray-menu.html".into()),
                    )
                    .position(x as f64, y as f64)
                    .inner_size(180.0, 125.0)
                    .resizable(false)
                    .decorations(false)
                    .always_on_top(true)
                    .skip_taskbar(true)
                    .build()
                    .ok()
                    {
                        let w = window.clone();
                        window.on_window_event(move |event| {
                            if let tauri::WindowEvent::Focused(false) = event {
                                let _ = w.close();
                            }
                        });
                        let _ = window.set_focus();
                    }
                }
                _ => {}
            }
        })
        .build(app)?;

    Ok(())
}

#[tauri::command]
fn scan_lan_servers() -> Result<Vec<LanServerInfo>, String> {
    scan_lan_servers_sync()
}

#[tauri::command]
fn get_latency(state: tauri::State<'_, AppState>) -> u64 {
    *state.latency_ms.lock().unwrap()
}

#[tauri::command]
async fn start_online(
    state: tauri::State<'_, AppState>,
    room_name: String,
    password: String,
    window: tauri::Window,
) -> Result<String, String> {
    if *state.is_running.lock().unwrap() {
        return Err("联机功能已在运行中".to_string());
    }

    window.emit("app-log", "[启动] 获取中继服务器列表...".to_string()).ok();
    let relays = get_relays().ok_or("网络错误，请检查网络连接")?;
    if relays.is_empty() {
        return Err("没有可用的中继服务器".to_string());
    }

    let relay = &relays[0];
    window.emit("app-log", format!("[启动] 选中中继: {} ({})", relay.name, relay.address)).ok();
    let relay_addr = resolve_address(&relay.address).ok_or("网络连接失败")?;

    window.emit("app-log", "[启动] 检查房间状态...".to_string()).ok();
    let (exists, _) = get_room(&room_name).ok_or("网络错误，请检查网络连接")?;
    let is_host = !exists;

    let is_running = state.is_running.clone();
    let current_room = state.current_room.clone();
    let app_state = Arc::new(AppState {
        current_room: current_room.clone(),
        is_running: is_running.clone(),
        latency_ms: state.latency_ms.clone(),
    });

    if is_host {
        window.emit("app-log", "房主模式: 扫描局域网Minecraft服务器...".to_string()).ok();
        let servers = scan_lan_servers_sync()?;
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

        create_room(&room_name, &password, &relay.id).ok_or("创建房间失败，可能房间名已存在")?;
        window.emit("app-log", format!("[启动] 房间已创建: {}", room_name)).ok();

        let w = window.clone();
        let rn = room_name.clone();
        let pw = password.clone();
        let is_running_for_thread = is_running.clone();
        let current_room_for_thread = current_room.clone();

        std::thread::spawn(move || {
            match host_mode.start(mc_port, mc_motd) {
                Ok(msg) => {
                    let _ = w.emit("app-log", msg);
                    *is_running_for_thread.lock().unwrap() = true;
                    *current_room_for_thread.lock().unwrap() = Some((rn, pw));
                }
                Err(e) => {
                    let _ = w.emit("app-log", format!("[错误] {}", e));
                    delete_room(&rn);
                    *is_running_for_thread.lock().unwrap() = false;
                }
            }
        });

        latency_monitor(relay_addr, app_state, window.clone());
        *is_running.lock().unwrap() = true;
        *current_room.lock().unwrap() = Some((room_name, password));

        Ok(format!("房主模式已启动，Minecraft端口: {}", mc_port))
    } else {
        window.emit("app-log", "成员模式: 连接到中继服务器...".to_string()).ok();
        let local_port = 25565u16;
        // 自动寻找可用端口
        let local_port = (local_port..65535).find(|&port| {
            TcpListener::bind(format!("127.0.0.1:{}", port)).is_ok()
        }).unwrap_or(25565u16);
        let w = window.clone();
        let rn = room_name.clone();
        let pw = password.clone();
        let ra = relay_addr;
        let is_running_for_thread = is_running.clone();
        let current_room_for_thread = current_room.clone();

        std::thread::spawn(move || {
            let w2 = w.clone();
            let mut client_mode = ClientMode::new(ra, local_port, "MC Link".to_string());
            client_mode.set_relay(ra, rn.clone(), pw.clone());
            client_mode.set_log_callback(move |msg| {
                let _ = w2.emit("app-log", msg);
            });
            match client_mode.start() {
                Ok(msg) => {
                    let _ = w.emit("app-log", msg);
                    *is_running_for_thread.lock().unwrap() = true;
                    *current_room_for_thread.lock().unwrap() = Some((rn, pw));
                }
                Err(e) => {
                    let _ = w.emit("app-log", format!("[错误] {}", e));
                    *is_running_for_thread.lock().unwrap() = false;
                }
            }
        });

        latency_monitor(relay_addr, app_state, window.clone());
        *is_running.lock().unwrap() = true;
        *current_room.lock().unwrap() = Some((room_name, password));

        Ok(format!("成员模式已启动\n请在Minecraft中连接 127.0.0.1:{}", local_port))
    }
}

#[tauri::command]
async fn stop_online(state: tauri::State<'_, AppState>) -> Result<String, String> {
    *state.is_running.lock().unwrap() = false;
    let room = state.current_room.lock().unwrap().clone();
    if let Some((ref room_name, _)) = room {
        delete_room(room_name);
    }
    *state.current_room.lock().unwrap() = None;
    *state.latency_ms.lock().unwrap() = 0;
    Ok("联机已停止".to_string())
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
async fn close_window(window: tauri::Window) -> Result<String, String> {
    window.hide().ok();
    Ok("已最小化到系统托盘".to_string())
}

#[tauri::command]
async fn exit_app(state: tauri::State<'_, AppState>, app: tauri::AppHandle) -> Result<String, String> {
    *state.is_running.lock().unwrap() = false;
    let room = state.current_room.lock().unwrap().clone();
    if let Some((ref room_name, _)) = room {
        delete_room(room_name);
    }
    *state.current_room.lock().unwrap() = None;
    *state.latency_ms.lock().unwrap() = 0;
    app.exit(0);
    Ok("已退出".to_string())
}

#[tauri::command]
fn show_window(window: tauri::Window) {
    let _ = window.show();
    let _ = window.set_focus();
}

#[tauri::command]
fn show_main_window(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            current_room: Arc::new(Mutex::new(None)),
            is_running: Arc::new(Mutex::new(false)),
            latency_ms: Arc::new(Mutex::new(0)),
        })
        .setup(|app| {
            setup_tray(app.handle())?;

            #[cfg(desktop)]
            {
                use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

                let chord_pressed = Arc::new(AtomicBool::new(false));
                let chord_handler = chord_pressed.clone();

                app.handle().plugin(
                    tauri_plugin_global_shortcut::Builder::new()
                        .with_handler(move |app, shortcut, event| {
                            if event.state() == ShortcutState::Pressed {
                                if shortcut.matches(Modifiers::ALT, Code::KeyM) {
                                    chord_handler.store(true, Ordering::SeqCst);
                                    let reset = chord_handler.clone();
                                    std::thread::spawn(move || {
                                        std::thread::sleep(Duration::from_secs(1));
                                        reset.store(false, Ordering::SeqCst);
                                    });
                                }
                                if shortcut.matches(Modifiers::ALT, Code::KeyO) {
                                    if chord_handler.load(Ordering::SeqCst) {
                                        if let Some(window) = app.get_webview_window("main") {
                                            let _ = window.show();
                                            let _ = window.set_focus();
                                        }
                                    }
                                }
                            }
                        })
                        .build(),
                )?;

                let alt_m = Shortcut::new(Some(Modifiers::ALT), Code::KeyM);
                let alt_o = Shortcut::new(Some(Modifiers::ALT), Code::KeyO);
                if let Err(e) = app.global_shortcut().register(alt_m) {
                    eprintln!("[热键] ALT+M 注册失败(可能已被其他程序占用): {}", e);
                }
                if let Err(e) = app.global_shortcut().register(alt_o) {
                    eprintln!("[热键] ALT+O 注册失败(可能已被其他程序占用): {}", e);
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            scan_lan_servers,
            get_latency,
            start_online,
            stop_online,
            minimize_window,
            maximize_window,
            close_window,
            exit_app,
            show_window,
            show_main_window,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}