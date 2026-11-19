//! Native shortcuts are delivered to the same checked launch flow as the main window.
use crate::{
    error::{AppError, Result},
    launcher, ui_lang,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    Emitter, Manager,
};

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Action {
    Launch { tool: String },
    SwitchKey { tool: String },
    Directory { path: String },
}

#[derive(Default)]
pub struct TrayState {
    recent: Mutex<Vec<String>>,
    pending: Mutex<Option<(Instant, Action)>>,
}

fn directory_id(path: &str) -> String {
    format!("directory-{:x}", Sha256::digest(path.as_bytes()))
}

fn action_for(id: &str, recent: &[String]) -> Option<Action> {
    match id {
        "launch-claude" => Some(Action::Launch {
            tool: "claude".into(),
        }),
        "launch-codex" => Some(Action::Launch {
            tool: "codex".into(),
        }),
        "key-claude" => Some(Action::SwitchKey {
            tool: "claude".into(),
        }),
        "key-codex" => Some(Action::SwitchKey {
            tool: "codex".into(),
        }),
        _ => recent
            .iter()
            .find(|path| directory_id(path) == id)
            .map(|path| Action::Directory { path: path.clone() }),
    }
}

fn recent_directories(prefs: &launcher::Preferences) -> Vec<String> {
    let mut paths = Vec::new();
    for path in &prefs.recent_directories {
        if !path.trim().is_empty() && !paths.contains(path) {
            paths.push(path.clone());
        }
        if paths.len() == 8 {
            break;
        }
    }
    paths
}

pub fn menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    menu_for_preferences(app, &launcher::load_preferences().unwrap_or_default())
}

pub fn menu_for_preferences<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    prefs: &launcher::Preferences,
) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    let item = |id: &str, text: &str| MenuItem::with_id(app, id, text, true, None::<&str>);
    menu.append(&item(
        "show",
        ui_lang::pick("显示主窗口", "顯示主視窗", "Show window", "Показать окно"),
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&item(
        "launch-claude",
        ui_lang::pick(
            "启动 Claude（上次配置）",
            "啟動 Claude（上次設定）",
            "Launch Claude (saved settings)",
            "Запустить Claude (сохранённые настройки)",
        ),
    )?)?;
    menu.append(&item(
        "launch-codex",
        ui_lang::pick(
            "启动 Codex（上次配置）",
            "啟動 Codex（上次設定）",
            "Launch Codex (saved settings)",
            "Запустить Codex (сохранённые настройки)",
        ),
    )?)?;
    let keys = Submenu::new(
        app,
        ui_lang::pick("切换密钥…", "切換密鑰…", "Switch key…", "Сменить ключ…"),
        true,
    )?;
    keys.append(&item("key-claude", "Claude")?)?;
    keys.append(&item("key-codex", "Codex")?)?;
    menu.append(&keys)?;
    let recent = recent_directories(prefs);
    let folders = Submenu::new(
        app,
        ui_lang::pick("最近项目目录", "最近專案目錄", "Recent project folders", "Недавние папки проектов"),
        !recent.is_empty(),
    )?;
    for path in &recent {
        // Escape menu mnemonics; opaque IDs keep arbitrary paths out of command parsing.
        folders.append(&item(&directory_id(path), &path.replace('&', "&&"))?)?;
    }
    *app.state::<TrayState>()
        .recent
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = recent;
    menu.append(&folders)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&item("quit", ui_lang::pick("退出", "退出", "Quit", "Выход"))?)?;
    Ok(menu)
}

pub fn refresh(app: &tauri::AppHandle) {
    if let Some(tray) = app.tray_by_id("main") {
        if let Ok(menu) = menu(app) {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

pub fn dispatch(app: &tauri::AppHandle, id: &str) {
    crate::show_main(app);
    if crate::updates::ensure_supported().is_err() {
        return;
    }
    let state = app.state::<TrayState>();
    let action = state.recent.lock().ok().and_then(|r| action_for(id, &r));
    if let Some(action) = action {
        if let Ok(mut pending) = state.pending.lock() {
            // Coalesce rapid clicks and keep an action until the frontend listener is ready.
            *pending = Some((Instant::now(), action));
        }
        let _ = app.emit("tray-action-pending", ());
    }
}

fn take_fresh(pending: &mut Option<(Instant, Action)>, now: Instant) -> Option<Action> {
    pending
        .take()
        .filter(|(at, _)| now.saturating_duration_since(*at) < Duration::from_secs(30))
        .map(|(_, a)| a)
}

#[tauri::command]
pub fn take_tray_action(state: tauri::State<'_, TrayState>) -> Result<Option<Action>> {
    crate::updates::ensure_supported()?;
    let mut pending = state
        .pending
        .lock()
        .map_err(|_| AppError::Other("托盘操作暂不可用".into()))?;
    Ok(take_fresh(&mut pending, Instant::now()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shortcuts_use_opaque_ids_and_preserve_paths_as_data() {
        let paths: Vec<String> = vec!["C:\\项目 & 'quoted'\\$(noop)".into()];
        assert_eq!(
            action_for(&directory_id(&paths[0]), &paths),
            Some(Action::Directory {
                path: paths[0].clone()
            })
        );
        assert_eq!(
            action_for(&directory_id(&paths[0]), &["another folder".into()]),
            None
        );
        for id in [
            "directory-1",
            "directory--1",
            "directory-../../x",
            "launch-shell",
        ] {
            assert_eq!(action_for(id, &paths), None);
        }
        assert_eq!(
            action_for("key-codex", &[]),
            Some(Action::SwitchKey {
                tool: "codex".into()
            })
        );
    }
    #[test]
    fn actions_are_consumed_once_and_expire_instead_of_launching_later() {
        let now = Instant::now();
        let action = Action::Launch {
            tool: "claude".into(),
        };
        let mut pending = Some((now, action.clone()));
        assert_eq!(take_fresh(&mut pending, now), Some(action.clone()));
        assert_eq!(take_fresh(&mut pending, now), None);
        pending = Some((now, action));
        assert_eq!(
            take_fresh(&mut pending, now + Duration::from_secs(30)),
            None
        );
    }
    #[test]
    fn recent_folders_are_deduplicated_and_bounded() {
        let prefs = launcher::Preferences {
            project_directory: "selected".into(),
            recent_directories: vec!["selected".into(), "".into(), "other".into()],
            ..Default::default()
        };
        assert_eq!(recent_directories(&prefs), ["selected", "other"]);
        let prefs = launcher::Preferences {
            recent_directories: (0..20).map(|i| i.to_string()).collect(),
            ..prefs
        };
        assert_eq!(recent_directories(&prefs).len(), 8);
    }
}
