//! 干跑：拿真实机器上的 ~/.codex/config.toml 演算一遍写入结果，只在内存里算，不碰磁盘。
//!
//!     cargo run --example dryrun
//!
//! 用来在动真格之前确认：用户的 [plugins.*] / [marketplaces.*] / 注释是否真的完好。

use dbkey_lib::claude_config::{self, ClaudeSettings};
use dbkey_lib::codex_auth;
use dbkey_lib::codex_config::{self, AuthMode, ProviderConfig};
use dbkey_lib::paths;
use std::collections::BTreeSet;
use std::fs;

fn section_names(s: &str) -> BTreeSet<String> {
    s.lines()
        .map(|l| l.trim())
        .filter(|l| l.starts_with('[') && l.ends_with(']'))
        .map(|l| l.to_string())
        .collect()
}

fn main() {
    let cfg_path = paths::config_path().expect("无法定位 config.toml");
    println!("配置文件: {}\n", cfg_path.display());

    let Ok(original) = fs::read_to_string(&cfg_path) else {
        println!("读不到 config.toml，跳过");
        return;
    };

    println!("原文件 {} 字节, {} 行", original.len(), original.lines().count());

    match codex_config::parse(&original) {
        Ok(_) => println!("TOML 解析: 通过"),
        Err(e) => {
            println!("TOML 解析: 失败 -> {e}");
            println!("（真实运行时此处会拒绝写入，保护用户文件）");
            return;
        }
    }

    let residue = codex_config::detect_residue(&original);
    println!("检出残渣: {:?}", residue);
    println!(
        "当前指向: {:?} / 模型: {:?}",
        codex_config::current_pointing(&original),
        codex_config::current_model(&original)
    );

    let provider = ProviderConfig {
        provider_id: "starnexus_keytool".into(),
        display_name: "AI".into(),
        base_url: "https://gateway.dkby.com/v1".into(),
        wire_api: "responses".into(),
        model: "gpt-5.3-codex".into(),
        reasoning_effort: "medium".into(),
        api_key: "sk-DRYRUN-NOT-A-REAL-KEY".into(),
        auth_mode: AuthMode::OpenAiBuiltin,
    };

    let produced = codex_config::apply(&original, &provider, true).expect("apply 失败");

    // 核心断言：段落只许增加我们那一个，不许少任何一个
    let before = section_names(&original);
    let after = section_names(&produced);

    let lost: Vec<_> = before.difference(&after).collect();
    let added: Vec<_> = after.difference(&before).collect();

    println!("\n段落数: {} -> {}", before.len(), after.len());
    println!("新增: {:?}", added);
    println!("丢失: {:?}", lost);

    let comments_before = original.lines().filter(|l| l.trim_start().starts_with('#')).count();
    let comments_after = produced.lines().filter(|l| l.trim_start().starts_with('#')).count();
    println!("注释行: {} -> {}", comments_before, comments_after);

    println!("产出 {} 字节, {} 行", produced.len(), produced.lines().count());

    // 幂等性：再跑一次应该完全不变
    let twice = codex_config::apply(&produced, &provider, true).expect("二次 apply 失败");
    println!("幂等: {}", if twice == produced { "是" } else { "否 <- 有问题" });

    println!("\n--- 产出的前 12 行 ---");
    for l in produced.lines().take(12) {
        println!("{l}");
    }

    // auth.json 侧
    println!("\n--- auth.json ---");
    let auth_path = paths::auth_path().expect("无法定位 auth.json");
    let auth_text = fs::read_to_string(&auth_path).ok();
    let state = codex_auth::inspect(auth_text.as_deref());
    println!("状态: {:?}", state);

    if let Some(t) = &auth_text {
        let keys_before: Vec<String> = serde_json::from_str::<serde_json::Value>(t)
            .ok()
            .and_then(|v| v.as_object().map(|o| o.keys().cloned().collect()))
            .unwrap_or_default();
        let merged = codex_auth::merge_api_key(Some(t), "sk-DRYRUN").expect("merge 失败");
        let keys_after: Vec<String> = serde_json::from_str::<serde_json::Value>(&merged)
            .ok()
            .and_then(|v| v.as_object().map(|o| o.keys().cloned().collect()))
            .unwrap_or_default();
        println!("键 {:?} -> {:?}", keys_before, keys_after);

        let lost: Vec<_> = keys_before.iter().filter(|k| !keys_after.contains(k)).collect();
        println!("丢失的键: {:?}", lost);
    }

    // ——— Claude Code ———
    println!("\n--- Claude Code settings.json ---");
    let cpath = paths::claude_settings_path().expect("无法定位 settings.json");
    println!("{}", cpath.display());
    let ctext = fs::read_to_string(&cpath).ok();
    println!("状态: {:?}", claude_config::inspect(ctext.as_deref()));

    let mut claude_ok = true;
    if let Some(t) = &ctext {
        let keys_before: Vec<String> = serde_json::from_str::<serde_json::Value>(t)
            .ok()
            .and_then(|v| v.as_object().map(|o| o.keys().cloned().collect()))
            .unwrap_or_default();

        let settings = ClaudeSettings {
            base_url: claude_config::derive_anthropic_base_url("https://gateway.dkby.com/v1"),
            auth_token: "sk-DRYRUN".into(),
            model: "claude-opus-4-8".into(),
            opus_model: "claude-opus-4-8".into(),
            sonnet_model: "claude-sonnet-4-6".into(),
            haiku_model: "claude-haiku-4-5-20251001".into(),
        };
        println!("推导出的 ANTHROPIC_BASE_URL: {}", settings.base_url);

        let merged = claude_config::merge(Some(t), &settings).expect("merge 失败");
        let mv: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let keys_after: Vec<String> = mv
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();

        let lost_keys: Vec<_> = keys_before.iter().filter(|k| !keys_after.contains(k)).collect();
        println!("顶层键 {:?}", keys_before);
        println!("丢失的键: {:?}", lost_keys);
        println!(
            "写入 env: BASE_URL={} MODEL={} HAIKU={}",
            mv["env"]["ANTHROPIC_BASE_URL"],
            mv["env"]["ANTHROPIC_MODEL"],
            mv["env"]["ANTHROPIC_DEFAULT_HAIKU_MODEL"]
        );

        let twice_c = claude_config::merge(Some(&merged), &settings).expect("二次 merge 失败");
        println!("幂等: {}", if twice_c == merged { "是" } else { "否 <- 有问题" });
        claude_ok = lost_keys.is_empty() && twice_c == merged;

        if keys_before.iter().any(|k| k == "model") {
            println!("注意: 顶层已有 model 键（未改动），运行时 env.ANTHROPIC_MODEL 优先");
        }
    }

    // env_key 是那次 "Missing environment variable" 事故的根因，产出里绝不能有
    let has_env_key = produced.contains("env_key");
    println!("产出含 env_key: {}", if has_env_key { "是 <- 有问题" } else { "否" });

    // 段落只许增加我们那一个（已存在时则不增），一个都不许少
    let verdict = lost.is_empty() && added.len() <= 1 && twice == produced && claude_ok && !has_env_key;
    println!("\n结论: {}", if verdict { "安全" } else { "有问题，需排查" });
}
