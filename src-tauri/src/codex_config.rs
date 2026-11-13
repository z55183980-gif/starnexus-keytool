use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use toml_edit::{value, DocumentMut, Item, Table};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// provider id，会成为 [model_providers.<id>] 的段名
    pub provider_id: String,
    pub display_name: String,
    pub base_url: String,
    /// "responses" —— 已确认 starnexus 网关提供 /v1/responses
    pub wire_api: String,
    pub model: String,
    pub reasoning_effort: String,
    pub api_key: String,
    /// 认证写法。Codex 文档明确要求这几种方式互斥，绝不能同时写。
    pub auth_mode: AuthMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum AuthMode {
    /// 默认，也是手动配置多年通行的写法：
    ///   model_provider = "openai" + 顶层 openai_base_url，密钥放 auth.json。
    /// 关键在于 "openai" 是 Codex 的内置 provider，只有它会去读 auth.json；
    /// 换成自定义 provider 段后这条链就断了，请求会被判成 401。
    OpenAiBuiltin,
    /// 想保住官方 ChatGPT 登录时用：自定义 provider 段 + 段内密钥，完全不碰 auth.json。
    /// Codex 要求几种认证方式互斥，此模式下绝不能再写 env_key。
    BearerToken,
}

/// 旧脚本留下的遗留键，接管时清理掉
const LEGACY_TOP_LEVEL: &[&str] = &["openai_base_url", "openai_api_key"];

pub fn parse(config_text: &str) -> Result<DocumentMut> {
    config_text
        .parse::<DocumentMut>()
        .map_err(|e| AppError::TomlParse(e.to_string()))
}

/// 在用户现有的 config.toml 上叠加我们的配置。
///
/// 只动 3 个顶层键 + 1 个 [model_providers.<id>] 段。
/// [marketplaces.*]、[plugins.*]、notify、用户注释 —— 一个字不动。
pub fn apply(config_text: &str, cfg: &ProviderConfig, cleanup: bool) -> Result<String> {
    let mut doc = parse(config_text)?;

    {
        let root = doc.as_table_mut();

        // 模型留空（其他服务没选模型）就不写，由客户端用它自己的默认模型
        if cfg.model.trim().is_empty() {
            root.remove("model");
        } else {
            root["model"] = value(&cfg.model);
        }
        root["model_reasoning_effort"] = value(&cfg.reasoning_effort);

        match cfg.auth_mode {
            AuthMode::OpenAiBuiltin => {
                // model_provider 的默认值就是 "openai"，不写比写更保险。
                // 只有它被设成了别的值（比如早期版本写的自定义 provider）才纠回来。
                let needs_fix = root
                    .get("model_provider")
                    .and_then(|i| i.as_str())
                    .map(|v| v != "openai")
                    .unwrap_or(false);
                if needs_fix {
                    root["model_provider"] = value("openai");
                }
                root["openai_base_url"] = value(&cfg.base_url);
                // 密钥归 auth.json 管，绝不明文留在这里
                root.remove("openai_api_key");
            }
            AuthMode::BearerToken => {
                root["model_provider"] = value(&cfg.provider_id);
                for k in LEGACY_TOP_LEVEL {
                    root.remove(k);
                }
            }
        }
    }

    // 内置写法不需要自定义 provider 段，有残留就连根清掉
    if cfg.auth_mode == AuthMode::OpenAiBuiltin {
        let root = doc.as_table_mut();
        if let Some(p) = root.get_mut("model_providers").and_then(|i| i.as_table_mut()) {
            p.remove(&cfg.provider_id);
            if p.is_empty() {
                root.remove("model_providers");
            }
        }
        let out = doc.to_string();
        return Ok(if cleanup { collapse_blank_lines(&out) } else { out });
    }

    // [model_providers.<id>]
    {
        let providers = doc
            .as_table_mut()
            .entry("model_providers")
            .or_insert(Item::Table({
                let mut t = Table::new();
                t.set_implicit(true);
                t
            }));

        let providers_tbl = providers
            .as_table_mut()
            .ok_or_else(|| AppError::TomlParse("model_providers 不是一个表".into()))?;
        providers_tbl.set_implicit(true);

        let entry = providers_tbl
            .entry(&cfg.provider_id)
            .or_insert(Item::Table(Table::new()));
        let tbl = entry
            .as_table_mut()
            .ok_or_else(|| AppError::TomlParse(
                format!("model_providers.{} 不是一个表", cfg.provider_id)
            ))?;

        tbl["name"] = value(&cfg.display_name);
        tbl["base_url"] = value(&cfg.base_url);
        tbl["wire_api"] = value(&cfg.wire_api);

        // 走到这里只可能是 BearerToken 模式 —— 内置写法在上面已提前返回
        tbl["experimental_bearer_token"] = value(&cfg.api_key);

        // Codex 文档：env_key / experimental_bearer_token / auth 互斥。
        // 一旦写了 env_key，Codex 会去读系统环境变量，读不到直接报
        // "Missing environment variable: OPENAI_API_KEY"，且优先级压过 bearer token。
        tbl.remove("env_key");
        tbl.remove("env_key_instructions");
        tbl.remove("requires_openai_auth");
    }

    let out = doc.to_string();
    Ok(if cleanup { collapse_blank_lines(&out) } else { out })
}

/// 把 Codex 指回官方：移除我们的段和指向，其余不动
pub fn revert_to_official(config_text: &str, provider_id: &str) -> Result<String> {
    let mut doc = parse(config_text)?;
    {
        let root = doc.as_table_mut();
        root.remove("model_provider");
        for k in LEGACY_TOP_LEVEL {
            root.remove(k);
        }
        if let Some(p) = root.get_mut("model_providers").and_then(|i| i.as_table_mut()) {
            p.remove(provider_id);
            if p.is_empty() {
                root.remove("model_providers");
            }
        }
    }
    Ok(doc.to_string())
}

/// 折叠旧脚本累积的连续空行。
///
/// 多行字符串（""" / '''）里的空行不能动，检测到就整体跳过清理 ——
/// 宁可不清理，也绝不能改坏用户的值。
fn collapse_blank_lines(s: &str) -> String {
    if s.contains("\"\"\"") || s.contains("'''") {
        return s.to_string();
    }

    let mut out = String::with_capacity(s.len());
    let mut blank_run = 0usize;
    for line in s.lines() {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue; // 连续空行最多保留 1 行
            }
        } else {
            blank_run = 0;
        }
        // 顺手剥掉行尾空格（旧脚本 echo 留下的）
        out.push_str(line.trim_end());
        out.push('\n');
    }
    // 收掉文件末尾多余的空行
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// 读出当前 config.toml 指向哪里，用于界面上显示现状
pub fn current_pointing(config_text: &str) -> Option<String> {
    let doc = config_text.parse::<DocumentMut>().ok()?;
    let root = doc.as_table();

    if let Some(p) = root.get("model_provider").and_then(|i| i.as_str()) {
        if let Some(url) = root
            .get("model_providers")
            .and_then(|i| i.as_table())
            .and_then(|t| t.get(p))
            .and_then(|i| i.as_table())
            .and_then(|t| t.get("base_url"))
            .and_then(|i| i.as_str())
        {
            return Some(url.to_string());
        }
        if p != "openai" { return None; }
    }
    // 回落到旧写法
    root.get("openai_base_url")
        .and_then(|i| i.as_str())
        .map(|s| s.to_string())
}

/// 读出 provider 段里的密钥（只在「保住官方登录」那种模式下会有）
pub fn current_bearer_token(config_text: &str, provider_id: &str) -> Option<String> {
    let doc = config_text.parse::<DocumentMut>().ok()?;
    doc.as_table()
        .get("model_providers")?
        .as_table()?
        .get(provider_id)?
        .as_table()?
        .get("experimental_bearer_token")?
        .as_str()
        .map(|s| s.to_string())
}

pub fn current_reasoning_effort(config_text: &str) -> Option<String> {
    let doc = config_text.parse::<DocumentMut>().ok()?;
    doc.as_table()
        .get("model_reasoning_effort")
        .and_then(|i| i.as_str())
        .map(|s| s.to_string())
}

pub fn current_model(config_text: &str) -> Option<String> {
    let doc = config_text.parse::<DocumentMut>().ok()?;
    doc.as_table()
        .get("model")
        .and_then(|i| i.as_str())
        .map(|s| s.to_string())
}

/// 检测旧脚本留下的内伤，用于界面提示"是否顺手清理"
pub fn detect_residue(config_text: &str) -> Vec<String> {
    let mut issues = Vec::new();

    let mut max_blank_run = 0usize;
    let mut run = 0usize;
    let mut trailing_ws = 0usize;
    for line in config_text.lines() {
        if line.trim().is_empty() {
            run += 1;
            max_blank_run = max_blank_run.max(run);
        } else {
            run = 0;
            if line != line.trim_end() {
                trailing_ws += 1;
            }
        }
    }

    if max_blank_run >= 3 {
        issues.push(format!("{} 行多余的空行", max_blank_run));
    }
    if trailing_ws > 0 {
        issues.push(format!("{} 行末尾有多余空格", trailing_ws));
    }
    if config_text.contains("openai_base_url") {
        issues.push("早期版本留下的设置".into());
    }
    if config_text.contains("openai_api_key") {
        issues.push("API Key 直接写在了配置文件里".into());
    }
    if config_text.starts_with('\u{feff}') {
        issues.push("文件开头有多余的隐藏字符".into());
    }

    issues
}

#[cfg(test)]
#[path = "codex_config_tests.rs"]
mod tests;
