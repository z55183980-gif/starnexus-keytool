//! Read-only installation probe. Pass an explicit target to exercise a real launch without writing config.
use dbkey_lib::launcher::{self, LaunchRequest, Target};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if let Some(name) = args.get(1) {
        let target: Target = serde_json::from_value(serde_json::Value::String(name.clone()))
            .expect("target must be claude-cli, codex-cli, claude-desktop or codex-desktop");
        let req = LaunchRequest {
            target,
            directory: args.get(2).cloned().unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            }),
        };
        match tauri::async_runtime::block_on(launcher::launch_client(req)) {
            Ok(receipt) => println!("{}", receipt.message),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&launcher::scan().expect("installation scan failed"))
                .unwrap()
        );
    }
}
