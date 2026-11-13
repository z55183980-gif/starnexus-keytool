pub mod api;
pub mod atomic;
pub mod backup;
pub mod brand;
pub mod browser_login;
pub mod claude_config;
pub mod claude_desktop;
pub mod codex_auth;
pub mod codex_config;
pub mod commands;
pub mod detect;
pub mod error;
pub mod gateway;
pub mod health;
pub mod launcher;
pub mod paths;
pub mod state;
pub mod ui_lang;

use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, Manager};

/// 托盘图标是否建成。建不成（例如 Linux 缺托盘支持）时关闭按钮照常退出，
/// 否则窗口藏起来就再也找不回来了。
struct TrayReady(AtomicBool);

fn show_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[derive(Clone, serde::Serialize)]
struct BrowserLoginCallback {
    code: String,
    state: String,
    error: String,
}

#[cfg(desktop)]
fn watch_login_callback(app: &tauri::App) {
    use tauri_plugin_deep_link::DeepLinkExt;

    // 安装包会登记协议；开发时（以及 Linux 的免安装包）要在运行时登记一次。
    #[cfg(any(target_os = "linux", windows))]
    let _ = app.deep_link().register_all();

    let handle = app.handle().clone();
    app.deep_link().on_open_url(move |event| {
        for url in event.urls() {
            if url.scheme() != browser_login::SCHEME
                || url.host_str() != Some("auth")
                || url.path().trim_end_matches("/") != "/callback"
            {
                continue;
            }
            let param = |name: &str| {
                url.query_pairs()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value.into_owned())
                    .unwrap_or_default()
            };
            show_main(&handle);
            let _ = handle.emit(
                "browser-login-callback",
                BrowserLoginCallback {
                    code: param("code"),
                    state: param("state"),
                    error: param("error"),
                },
            );
        }
    });
}

/// 托盘菜单项：切换语言时要改它们的文字
#[cfg(desktop)]
struct TrayMenu {
    show: tauri::menu::MenuItem<tauri::Wry>,
    quit: tauri::menu::MenuItem<tauri::Wry>,
}

#[cfg(desktop)]
fn tray_texts() -> (&'static str, &'static str) {
    (
        ui_lang::pick("显示主窗口", "顯示主視窗", "Show window"),
        ui_lang::pick("退出", "退出", "Quit"),
    )
}

/// 按当前界面语言更新窗口标题、托盘提示和托盘菜单
pub(crate) fn apply_native_language(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_title(ui_lang::app_name());
    }
    #[cfg(desktop)]
    {
        if let Some(tray) = app.tray_by_id("main") {
            let _ = tray.set_tooltip(Some(ui_lang::app_name()));
        }
        if let Some(menu) = app.try_state::<TrayMenu>() {
            let (show, quit) = tray_texts();
            let _ = menu.show.set_text(show);
            let _ = menu.quit.set_text(quit);
        }
    }
}

#[cfg(desktop)]
fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let (show_text, quit_text) = tray_texts();
    let show = MenuItem::with_id(app, "show", show_text, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", quit_text, true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    app.manage(TrayMenu { show: show.clone(), quit: quit.clone() });
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip(ui_lang::app_name())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    // 必须最先注册：第二个进程在这里就把控制权交还给已在运行的那个
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        show_main(app);
    }));
    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(browser_login::BrowserLogin::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(gateway::SessionState::new())
        .manage(TrayReady(AtomicBool::new(false)))
        .manage(commands::HealthCache::default())
        .setup(|app| {
            #[cfg(desktop)]
            watch_login_callback(app);
            // 先定界面语言：保存过的选择优先，没有就跟随系统（认不出来用英文）
            ui_lang::init(&state::load().map(|s| s.ui_lang).unwrap_or_default());
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title(ui_lang::app_name());
            }
            #[cfg(desktop)]
            if build_tray(app).is_ok() {
                app.state::<TrayReady>().0.store(true, Ordering::Relaxed);
            }
            Ok(())
        })
        // 点关闭按钮只是收进系统托盘；真正退出走托盘菜单的「退出」
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main"
                    && window.state::<TrayReady>().0.load(Ordering::Relaxed)
                {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::scan_environment,
            launcher::load_launcher_preferences,
            launcher::save_launcher_preferences,
            launcher::scan_launchers,
            launcher::validate_launch,
            launcher::launch_client,
            commands::load_app_state,
            browser_login::gateway_browser_login_start,
            browser_login::gateway_browser_login_finish,
            browser_login::gateway_browser_login_cancel,
            commands::gateway_login,
            commands::gateway_login_2fa,
            commands::gateway_logout,
            commands::gateway_current_user,
            commands::gateway_list_tokens,
            commands::gateway_key_status,
            commands::gateway_adopt_token,
            commands::gateway_import_tokens,
            commands::list_profiles,
            commands::save_profile,
            commands::delete_profile,
            commands::get_remote_config,
            commands::test_connection,
            commands::list_backups,
            commands::restore_backup,
            commands::apply_config,
            commands::revert_to_official,
            commands::rebuild_minimal_config,
            commands::health_check,
            commands::env_overrides,
            commands::list_profile_models,
            commands::set_ui_language,
            commands::mark_config_read,
            commands::adopt_current_key,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
