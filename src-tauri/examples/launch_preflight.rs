//! Read-only validation: checks discovery, CLI versions and command preparation, without launching clients.
fn main() {
    tauri::async_runtime::block_on(async {
        let directory = dirs::home_dir().expect("home directory").to_string_lossy().into_owned();
        let mut failed = false;
        for target in dbkey_lib::launcher::Target::all() {
            let result = dbkey_lib::launcher::validate_launch(dbkey_lib::launcher::LaunchRequest {
                target, directory: directory.clone(), service: None,
            }).await;
            println!("{target:?}: {}", match &result { Ok(()) => "ready".into(), Err(e) => e.to_string() });
            failed |= result.is_err();
        }
        if failed { std::process::exit(1); }
    });
}
