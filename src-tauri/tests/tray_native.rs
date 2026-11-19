// Constructs real Windows menu objects with a headless Tauri runtime. No installed app is touched.
#![cfg(windows)]
use dbkey_lib::{
    launcher::Preferences,
    tray_menu::{menu_for_preferences, TrayState},
    ui_lang::{self, Lang},
};
use tauri::test::{mock_builder, mock_context, noop_assets};

#[test]
fn native_tray_menu_has_localized_shortcuts_key_picker_and_stable_recent_ids() {
    let app = mock_builder()
        .manage(TrayState::default())
        .build(mock_context(noop_assets()))
        .unwrap();
    let mut prefs = Preferences {
        project_directory: "C:\\项目 & source".into(),
        recent_directories: vec!["C:\\项目 & source".into(), "C:\\second".into()],
        ..Default::default()
    };
    for (lang, expected_show, expected_launch) in [
        (Lang::ZhCn, "显示主窗口", "启动 Claude（上次配置）"),
        (Lang::ZhHk, "顯示主視窗", "啟動 Claude（上次設定）"),
        (Lang::En, "Show window", "Launch Claude (saved settings)"),
    ] {
        ui_lang::set(lang);
        let menu = menu_for_preferences(app.handle(), &prefs).unwrap();
        let items = menu.items().unwrap();
        assert_eq!(items.len(), 8);
        assert_eq!(
            items[0].as_menuitem().unwrap().text().unwrap(),
            expected_show
        );
        assert_eq!(
            items[2].as_menuitem().unwrap().text().unwrap(),
            expected_launch
        );
        assert_eq!(items[3].id().as_ref(), "launch-codex");
        assert_eq!(items[4].as_submenu().unwrap().items().unwrap().len(), 2);
        let dirs = items[5].as_submenu().unwrap().items().unwrap();
        assert_eq!(dirs.len(), 2);
        let first_id = dirs[0].id().clone();
        prefs.recent_directories.reverse();
        let rebuilt = menu_for_preferences(app.handle(), &prefs).unwrap();
        assert_eq!(
            rebuilt.items().unwrap()[5]
                .as_submenu()
                .unwrap()
                .items()
                .unwrap()[1]
                .id(),
            &first_id
        );
        assert_eq!(items[7].id().as_ref(), "quit");
    }
    prefs.project_directory.clear();
    prefs.recent_directories.clear();
    let menu = menu_for_preferences(app.handle(), &prefs).unwrap();
    assert!(!menu.items().unwrap()[5]
        .as_submenu()
        .unwrap()
        .is_enabled()
        .unwrap());
}
