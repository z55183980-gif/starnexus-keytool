//! Generate isolated real-client fixtures using the production configuration writers.
//! Invoked by scripts/real-client-e2e.py; never reads state or the credential store.
use dbkey_lib::{claude_config::{self, ClaudeSettings}, codex_auth, codex_config::{self, AuthMode, ProviderConfig}};
use std::{fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let root = PathBuf::from(args.get(1).ok_or("missing fixture directory")?);
    let base = args.get(2).ok_or("missing loopback URL")?;
    let url = reqwest::Url::parse(base)?;
    if url.scheme() != "http" || url.host_str() != Some("127.0.0.1") {
        return Err("fixtures require a local HTTP server".into());
    }
    let key = "sk-local-e2e-not-a-real-key";
    let version = args.get(3).map(String::as_str);
    for (name, mode) in [("codex-selected", codex_config::select_auth_mode(version, false, false)), ("codex-bearer", codex_config::select_auth_mode(version, true, false))] {
        let dir = root.join(name);
        fs::create_dir(&dir)?;
        let cfg = ProviderConfig {
            provider_id: "starnexus_keytool".into(), display_name: "AI".into(),
            base_url: format!("{base}/v1"), wire_api: "responses".into(),
            model: "gpt-5.3-codex".into(), reasoning_effort: "medium".into(),
            api_key: key.into(), auth_mode: mode,
        };
        fs::write(dir.join("config.toml"), codex_config::apply("", &cfg, true)?)?;
        if mode == AuthMode::OpenAiBuiltin {
            fs::write(dir.join("auth.json"), codex_auth::merge_api_key(None, key)?)?;
        }
    }
    let dir = root.join("claude");
    fs::create_dir(&dir)?;
    fs::write(dir.join("settings.json"), claude_config::merge(None, &ClaudeSettings {
        base_url: base.into(), auth_token: key.into(), model: "claude-sonnet-4-5".into(),
        opus_model: "claude-sonnet-4-5".into(), sonnet_model: "claude-sonnet-4-5".into(),
        haiku_model: "claude-sonnet-4-5".into(),
    })?)?;
    Ok(())
}
