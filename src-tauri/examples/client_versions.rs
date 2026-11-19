//! Read-only, opt-in smoke probe of installed CLIs and upstream stable release metadata.
fn main() {
    tauri::async_runtime::block_on(async {
        for target in [
            dbkey_lib::launcher::Target::ClaudeCli,
            dbkey_lib::launcher::Target::CodexCli,
            dbkey_lib::launcher::Target::ClaudeDesktop,
            dbkey_lib::launcher::Target::CodexDesktop,
        ] {
            match dbkey_lib::client_setup::check_client_version(target, false, Some(true)).await {
                Ok(status) => println!("{target:?}: {}", serde_json::to_string(&status).unwrap()),
                Err(error) => println!("{target:?}: {error}"),
            }
        }
    });
}
