use tauri::Manager;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

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

pub fn setup_tray(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    TrayIconBuilder::new()
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("MC Link")
        .on_tray_icon_event(|tray, event| {
            let app_handle = tray.app_handle();
            match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    if let Some(window) = app_handle.get_webview_window("main") {
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
                    if let Some(existing) = app_handle.get_webview_window("tray-menu") {
                        let _ = existing.close();
                    }

                    let (x, y) = get_cursor_pos();

                    if let Some(window) = tauri::WebviewWindowBuilder::new(
                        app_handle,
                        "tray-menu",
                        tauri::WebviewUrl::App("tray-menu.html".into()),
                    )
                    .position(x as f64, y as f64)
                    .inner_size(180.0, 125.0)
                    .resizable(false)
                    .decorations(false)
                    .always_on_top(true)
                    .skip_taskbar(true)
                    .transparent(true)
                    .build()
                    .ok()
                    {
                        try_set_window_backdrop(&window);
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