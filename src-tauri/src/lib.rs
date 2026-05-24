mod host;
mod client;
mod crypto;
mod protocol;
mod lan;
mod central;
mod state;
#[macro_use]
mod commands;
use commands::*;
mod tray;

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::Manager;
use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            current_room: Arc::new(Mutex::new(None)),
            is_running: Arc::new(Mutex::new(false)),
            latency_ms: Arc::new(Mutex::new(0)),
            stop_signal: Arc::new(Mutex::new(None)),
        })
        .setup(|app| {
            tray::setup_tray(app.handle())?;

            #[cfg(desktop)]
            {
                use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

                let chord_pressed = Arc::new(AtomicBool::new(false));
                let chord_handler = chord_pressed.clone();

                app.handle().plugin(
                    tauri_plugin_global_shortcut::Builder::new()
                        .with_handler(move |app_handle, shortcut, event| {
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
                                        if let Some(window) = app_handle.get_webview_window("main") {
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
            get_relays,
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
            set_tray_size,
            ping_relay,
            get_ip_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}