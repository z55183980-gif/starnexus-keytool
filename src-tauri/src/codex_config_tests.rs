//! 针对真实用户配置形态的回归测试。
//!
//! 样本取自实际机器上的 ~/.codex/config.toml —— 带 [marketplaces.*]、
//! [plugins.*]、notify 数组、手改过的 model 和注释，还有旧脚本累积的空行。

use super::*;
use crate::codex_config::AuthMode;

/// 真实用户配置的缩影
const REAL_WORLD: &str = r#"model = "gpt-5.6-sol"
model_provider = "openai"
openai_base_url = "https://gateway.dkby.com/v1"
# model_reasoning_effort = "high"




model_reasoning_effort = "high"


notify = [ "C:\\Users\\Administrator\\AppData\\Local\\OpenAI\\Codex\\codex-computer-use.exe", "turn-ended" ]

[marketplaces.openai-bundled]
source_type = "local"
source = '\\?\C:\Users\Administrator\.codex\.tmp\bundled-marketplaces\openai-bundled'

[plugins."browser@openai-bundled"]
enabled = true

[plugins."pdf@openai-primary-runtime"]
enabled = true
"#;

fn test_cfg() -> ProviderConfig {
    ProviderConfig {
        provider_id: "starnexus_keytool".into(),
        display_name: "AI".into(),
        base_url: "https://gateway.dkby.com/v1".into(),
        wire_api: "responses".into(),
        model: "gpt-5.3-codex".into(),
        reasoning_effort: "medium".into(),
        api_key: "sk-test-123456".into(),
        auth_mode: AuthMode::OpenAiBuiltin,
    }
}

#[test]
fn preserves_plugins_and_marketplaces() {
    let out = apply(REAL_WORLD, &test_cfg(), false).unwrap();

    // 这是最重要的一条：用户的插件段一个都不能丢
    assert!(out.contains("[plugins.\"browser@openai-bundled\"]"));
    assert!(out.contains("[plugins.\"pdf@openai-primary-runtime\"]"));
    assert!(out.contains("[marketplaces.openai-bundled]"));
    assert!(out.contains("notify = ["));
    // 带反斜杠的字面量字符串不能被破坏
    assert!(out.contains(r"\\?\C:\Users\Administrator\.codex"));
}

#[test]
fn preserves_user_comments() {
    let out = apply(REAL_WORLD, &test_cfg(), false).unwrap();
    assert!(
        out.contains("# model_reasoning_effort"),
        "用户的注释必须保留"
    );
}

/// 默认写法：内置 openai + 顶层 openai_base_url，密钥归 auth.json。
/// 这是手动配置多年通行的那套，也是唯一实测能让 Codex 真的带上密钥的。
#[test]
fn builtin_mode_writes_top_level_base_url() {
    let out = apply(REAL_WORLD, &test_cfg(), false).unwrap();

    assert!(out.contains(r#"openai_base_url = "https://gateway.dkby.com/v1""#));
    assert!(!out.contains("[model_providers.starnexus_keytool]"), "默认不该建自定义 provider 段");
    assert!(!out.contains("sk-test-123456"), "密钥绝不能出现在 config.toml 里");
    parse(&out).expect("产出必须能被重新解析");
}

/// model_provider 默认就是 "openai"，已经是它时不该多此一举地重写
#[test]
fn builtin_mode_leaves_openai_provider_untouched() {
    // 样本里本来就是 openai
    let out = apply(REAL_WORLD, &test_cfg(), false).unwrap();
    assert!(out.contains(r#"model_provider = "openai""#));

    // 压根没写这一行的配置，也不该被加上
    let bare = "model = \"x\"\n";
    let out2 = apply(bare, &test_cfg(), false).unwrap();
    assert!(
        !out2.contains("model_provider"),
        "原本没有这一行就别加，默认值本来就是 openai"
    );
}

/// 回归：早期版本把 model_provider 改成了自定义 provider，
/// Codex 于是不再读 auth.json，请求被网关判成 401 Invalid token。
/// 再次配置时必须把它纠回 openai 并清掉那个段。
#[test]
fn builtin_mode_recovers_from_custom_provider() {
    let mut bearer = test_cfg();
    bearer.auth_mode = AuthMode::BearerToken;
    let broken = apply(REAL_WORLD, &bearer, false).unwrap();
    assert!(broken.contains(r#"model_provider = "starnexus_keytool""#));

    let fixed = apply(&broken, &test_cfg(), false).unwrap();
    assert!(fixed.contains(r#"model_provider = "openai""#), "必须纠回内置 provider");
    assert!(!fixed.contains("[model_providers.starnexus_keytool]"), "自定义段必须清掉");
    assert!(!fixed.contains("sk-test-123456"), "段里的密钥必须一起清掉");
    parse(&fixed).unwrap();
}

/// 保住官方登录的那条路：自定义 provider 段 + 段内密钥
#[test]
fn bearer_mode_writes_provider_table() {
    let mut cfg = test_cfg();
    cfg.auth_mode = AuthMode::BearerToken;
    let out = apply(REAL_WORLD, &cfg, false).unwrap();

    assert!(out.contains("[model_providers.starnexus_keytool]"));
    assert!(out.contains(r#"base_url = "https://gateway.dkby.com/v1""#));
    assert!(out.contains(r#"wire_api = "responses""#));
    assert!(out.contains(r#"model_provider = "starnexus_keytool""#));
    parse(&out).expect("产出必须能被重新解析");
}

/// 回归：曾经同时写过 env_key 和 experimental_bearer_token，
/// 导致 Codex 报 "Missing environment variable: OPENAI_API_KEY"。
/// 官方文档要求这几种认证方式互斥，任何模式下都不能出现 env_key。
#[test]
fn never_writes_env_key() {
    for mode in [AuthMode::OpenAiBuiltin, AuthMode::BearerToken] {
        let mut cfg = test_cfg();
        cfg.auth_mode = mode;
        let out = apply(REAL_WORLD, &cfg, false).unwrap();
        assert!(
            !out.contains("env_key"),
            "{:?} 模式下不应写 env_key，否则 Codex 会去找系统环境变量",
            mode
        );
        assert!(!out.contains("requires_openai_auth"), "{:?} 模式", mode);
    }
}

/// 保住官方登录时才把密钥放进 provider 段
#[test]
fn bearer_mode_puts_key_in_provider_table() {
    let mut cfg = test_cfg();
    cfg.auth_mode = AuthMode::BearerToken;
    let out = apply(REAL_WORLD, &cfg, false).unwrap();
    assert!(out.contains(r#"experimental_bearer_token = "sk-test-123456""#));
    assert!(!out.contains("env_key"));
}

/// 只有走自定义 provider 段时才需要清掉顶层的 openai_base_url，
/// 否则两处地址会打架
#[test]
fn bearer_mode_removes_top_level_base_url() {
    let mut cfg = test_cfg();
    cfg.auth_mode = AuthMode::BearerToken;
    let out = apply(REAL_WORLD, &cfg, false).unwrap();
    let doc = parse(&out).unwrap();
    assert!(doc.as_table().get("openai_base_url").is_none());
}

#[test]
fn cleanup_collapses_blank_runs() {
    let before = REAL_WORLD.lines().filter(|l| l.trim().is_empty()).count();
    let out = apply(REAL_WORLD, &test_cfg(), true).unwrap();

    let mut max_run = 0;
    let mut run = 0;
    for line in out.lines() {
        if line.trim().is_empty() {
            run += 1;
            max_run = max_run.max(run);
        } else {
            run = 0;
        }
    }
    assert!(max_run <= 1, "清理后连续空行不应超过 1 行，实际 {}", max_run);
    assert!(before > 1);
    parse(&out).expect("清理后仍须是合法 TOML");
}

#[test]
fn cleanup_skips_docs_with_multiline_strings() {
    // 多行字符串里的空行不能动 —— 宁可不清理
    let src = "model = \"x\"\nnote = \"\"\"\nline1\n\n\n\nline2\n\"\"\"\n";
    let out = apply(src, &test_cfg(), true).unwrap();
    assert!(out.contains("line1\n\n\n\nline2"), "多行字符串内容必须原样保留");
}

#[test]
fn empty_config_produces_valid_output() {
    let out = apply("", &test_cfg(), false).unwrap();
    parse(&out).expect("空配置也要产出合法 TOML");
    assert!(out.contains("openai_base_url"));
}

#[test]
fn apply_is_idempotent() {
    let once = apply(REAL_WORLD, &test_cfg(), true).unwrap();
    let twice = apply(&once, &test_cfg(), true).unwrap();
    assert_eq!(once, twice, "重复运行不应继续改动文件 —— 这正是旧脚本的病根");
}

#[test]
fn revert_removes_our_traces() {
    let configured = apply(REAL_WORLD, &test_cfg(), false).unwrap();
    let reverted = revert_to_official(&configured, "starnexus_keytool").unwrap();

    assert!(!reverted.contains("[model_providers.starnexus_keytool]"));
    assert!(!reverted.contains("experimental_bearer_token"));
    // 用户自己的东西依然在
    assert!(reverted.contains("[plugins.\"browser@openai-bundled\"]"));
    parse(&reverted).unwrap();
}

#[test]
fn detects_residue_in_real_config() {
    let issues = detect_residue(REAL_WORLD);
    assert!(issues.iter().any(|i| i.contains("空行")), "应检出连续空行");
    assert!(
        issues.iter().any(|i| i.contains("早期版本")),
        "应检出遗留写法"
    );
}

#[test]
fn reads_current_pointing() {
    // 旧写法
    assert_eq!(
        current_pointing(REAL_WORLD).as_deref(),
        Some("https://gateway.dkby.com/v1")
    );
    // 新写法
    let configured = apply(REAL_WORLD, &test_cfg(), false).unwrap();
    assert_eq!(
        current_pointing(&configured).as_deref(),
        Some("https://gateway.dkby.com/v1")
    );
}

#[test]
fn broken_toml_is_rejected() {
    assert!(parse("model = \"unclosed").is_err());
    assert!(apply("model = \"unclosed", &test_cfg(), false).is_err());
}

#[test]
fn independent_provider_preserves_other_tools_provider() {
    let original = r#"model_provider = "starnexus"
[model_providers.starnexus]
name = "Other product"
base_url = "https://other.example/v1"
experimental_bearer_token = "other-secret"
"#;
    for auth_mode in [AuthMode::OpenAiBuiltin, AuthMode::BearerToken] {
        let mut cfg = test_cfg(); cfg.auth_mode = auth_mode;
        let out = apply(original, &cfg, false).unwrap();
        let doc = parse(&out).unwrap();
        assert_eq!(doc["model_providers"]["starnexus"]["experimental_bearer_token"].as_str(), Some("other-secret"));
        let reverted = revert_to_official(&out, &cfg.provider_id).unwrap();
        let doc = parse(&reverted).unwrap();
        assert_eq!(doc["model_providers"]["starnexus"]["base_url"].as_str(), Some("https://other.example/v1"));
    }
}
