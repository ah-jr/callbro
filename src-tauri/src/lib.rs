use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, WindowEvent,
};
use tauri_plugin_notification::NotificationExt;

fn show_main(app: &AppHandle) {
    // macOS: reveal the Dock icon while the window is on screen so it focuses
    // reliably (important for the incoming-call popup).
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// macOS puts apps with no visible window into "App Nap", which suspends the
/// webview and its WebSocket — so a client hidden in the tray stops receiving
/// calls. Holding a background NSActivity token for the process lifetime opts
/// out of App Nap. (The old NSAppSleepDisabled Info.plist key is ignored since
/// macOS Sierra.) The token is intentionally leaked so it lives forever.
#[cfg(target_os = "macos")]
fn prevent_app_nap() {
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
    let options = NSActivityOptions::from_bits_retain(0x0000_00FF); // NSActivityBackground
    let reason = NSString::from_str("callbro stays reachable in the background");
    let token = NSProcessInfo::processInfo().beginActivityWithOptions_reason(options, &reason);
    std::mem::forget(token);
}

/// Per-install settings, stored in the OS app-config dir.
#[derive(Serialize, Deserialize, Default, Clone)]
struct Config {
    #[serde(default)]
    user_id: String,
    #[serde(default)]
    name: String,
    /// the "team code" shared with everyone; sent on connect
    #[serde(default)]
    join_secret: String,
    /// admin password; only the admin sets it, unlocks layout editing
    #[serde(default)]
    admin_key: String,
    /// optional override of the built-in server URL
    #[serde(default)]
    server_url: String,
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).ok();
    Ok(dir.join("config.json"))
}

fn read_config(app: &AppHandle) -> Config {
    config_path(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_config(app: &AppHandle, cfg: &Config) {
    if let Ok(path) = config_path(app) {
        if let Ok(s) = serde_json::to_string_pretty(cfg) {
            let _ = std::fs::write(path, s);
        }
    }
}

#[tauri::command]
fn load_config(app: AppHandle) -> Result<Config, String> {
    let mut cfg = read_config(&app);
    if cfg.user_id.is_empty() {
        cfg.user_id = uuid::Uuid::new_v4().to_string();
        write_config(&app, &cfg);
    }
    Ok(cfg)
}

#[tauri::command]
fn save_config(app: AppHandle, config: Config) -> Result<(), String> {
    write_config(&app, &config);
    Ok(())
}

/// A call arrived: native OS notification + bring the window to the front.
/// (Chime + spoken name are handled in the webview.)
#[tauri::command]
fn alert(app: AppHandle, from_name: String) {
    let _ = app
        .notification()
        .builder()
        .title("callbro")
        .body(format!("{} tá te chamando", from_name))
        .show();

    show_main(&app);
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.request_user_attention(Some(tauri::UserAttentionType::Critical));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            #[cfg(target_os = "macos")]
            prevent_app_nap();

            // Launch automatically at login (release builds only, so dev runs
            // don't register the debug binary).
            #[cfg(not(debug_assertions))]
            {
                use tauri_plugin_autostart::ManagerExt;
                let _ = app.autolaunch().enable();
            }

            // Menu-bar / system-tray icon so the app stays reachable when its
            // window is hidden. Left-click or "Abrir callbro" shows it.
            let show = MenuItem::with_id(app, "show", "Abrir callbro", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show])?;
            let _tray = TrayIconBuilder::with_id("callbro-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("callbro")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    if event.id.as_ref() == "show" {
                        show_main(app);
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, .. } = event {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;

            Ok(())
        })
        // Clicking the window's close button hides to the tray instead of quitting.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                #[cfg(target_os = "macos")]
                let _ = window
                    .app_handle()
                    .set_activation_policy(tauri::ActivationPolicy::Accessory);
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![load_config, save_config, alert])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_app_handle, _event| {
        // macOS: clicking the Dock icon re-opens the hidden window.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = &_event {
            show_main(_app_handle);
        }
    });
}
