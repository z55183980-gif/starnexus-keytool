//! AI 诊断探针：用本机密钥库里已保存的 AI 密钥，查看网关上能用的模型，或真实跑一次诊断。
//! 密钥只在进程内使用，不会打印。
//!
//!     cargo run --example diagnose_probe -- models          列出每把密钥可用的模型（不计费）
//!     cargo run --example diagnose_probe -- catalog         列出网关目录里每个模型支持的接口（不计费）
//!     cargo run --example diagnose_probe -- health [密钥名]  跑一遍体检（只读，不计费）；不给密钥名即「沿用当前配置」
//!     cargo run --example diagnose_probe -- run claude      对 Claude 的真实配置跑一次 AI 诊断（会计费）
//!     cargo run --example diagnose_probe -- run codex

use dbkey_lib::{api, commands, state};

fn main() {
    tauri::async_runtime::block_on(probe());
}

async fn probe() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let st = state::load().expect("读取本地状态失败");
    let base = api::builtin_defaults().base_url;
    let profiles: Vec<_> = st
        .profiles
        .iter()
        .filter(|p| state::load_profile_key(&p.id).ok().flatten().is_some())
        .collect();
    if profiles.is_empty() {
        println!("没有已保存密钥的配置");
        return;
    }

    match args.first().map(String::as_str) {
        Some("models") => {
            for p in profiles {
                let key = state::load_profile_key(&p.id).unwrap().unwrap();
                match api::list_models(&base, &key, st.ignore_proxy).await {
                    Ok(r) => println!("[{}] {}：{}", p.name, r.message, r.models.join(", ")),
                    Err(e) => println!("[{}] 失败：{}", p.name, e),
                }
            }
        }
        Some("catalog") => {
            // 只读：网关目录里每个模型支持的接口（GET /v1/ai/models，不计费）
            for p in profiles {
                let key = state::load_profile_key(&p.id).unwrap().unwrap();
                match api::list_catalog(&base, &key, st.ignore_proxy).await {
                    Ok(Some(list)) => {
                        println!("[{}] {} 个模型", p.name, list.len());
                        for m in list {
                            println!("    {} → {}", m.id, m.endpoints.join(", "));
                        }
                    }
                    Ok(None) => println!("[{}] 网关没有目录接口或返回异常", p.name),
                    Err(e) => println!("[{}] 失败：{}", p.name, e),
                }
            }
        }
        Some("run") => {
            let tool = args.get(1).map(String::as_str).unwrap_or("claude");
            // 用第一把能连通的密钥
            let mut usable = None;
            for p in &profiles {
                let key = state::load_profile_key(&p.id).unwrap().unwrap();
                if api::list_models(&base, &key, st.ignore_proxy).await.is_ok_and(|r| r.ok) {
                    usable = Some(*p);
                    break;
                }
            }
            let Some(p) = usable else { return println!("没有能连通的密钥") };
            println!("使用密钥：{}", p.name);
            let req: commands::DiagnoseRequest = serde_json::from_value(serde_json::json!({
                "tool": tool, "service": "current", "model": "", "baseUrl": base,
                "ignoreProxy": st.ignore_proxy, "mode": "cli", "profileId": p.id,
                "note": args.get(2).cloned().unwrap_or_default(),
            }))
            .unwrap();
            let cache = commands::HealthCache::default();
            let prepared = match commands::prepare_diagnosis(&req, &cache).await {
                Ok(p) => p,
                Err(e) => return println!("准备失败：{}", e),
            };
            println!("模型：{}", prepared.model);
            println!("发送内容含密钥原文：{}", prepared.leaks_key());
            println!("—— 发送的用户消息 ——\n{}\n——", prepared.user_message);
            let started = std::time::Instant::now();
            match prepared.send().await {
                Ok(d) => println!("耗时 {:.1}s\n结果：{}", started.elapsed().as_secs_f32(), serde_json::to_string_pretty(&d).unwrap()),
                Err(e) => println!("耗时 {:.1}s\n失败：{}", started.elapsed().as_secs_f32(), e),
            }
        }
        Some("health") => {
            // 只读：跑一遍体检，打印问题列表。默认「沿用当前配置」；给出密钥名时按「选了这把 AI 密钥」，
            // 别名用 App 的内置默认值
            let service = match args.get(1) {
                Some(name) => match profiles.iter().find(|p| &p.name == name) {
                    Some(p) => p.id.clone(),
                    None => return println!("没有名为 {} 的密钥", name),
                },
                None => "current".to_string(),
            };
            let d = api::builtin_defaults();
            for tool in ["claude", "codex"] {
                let req: commands::HealthRequest = serde_json::from_value(serde_json::json!({
                    "tool": tool, "service": service, "baseUrl": base, "ignoreProxy": st.ignore_proxy,
                    // 第三个参数可指定 Codex 模型，用来验证 K6（模型不支持 Responses 接口）
                    "model": if tool == "claude" { &d.claude_default_model } else { args.get(2).unwrap_or(&d.default_model) },
                    "opusModel": d.claude_opus_model, "sonnetModel": d.claude_sonnet_model, "haikuModel": d.claude_haiku_model,
                }))
                .unwrap();
                match commands::run_health(&req, &commands::HealthCache::default()).await {
                    Ok(issues) => println!("[{}] {}", tool, serde_json::to_string(&issues).unwrap()),
                    Err(e) => println!("[{}] 失败：{}", tool, e),
                }
            }
        }
        Some("env") => {
            // 只读：和 App 启动时「检测本机环境」用的是同一批数据
            match dbkey_lib::launcher::scan() {
                Ok(scan) => {
                    for i in serde_json::to_value(&scan).unwrap()["installations"].as_array().into_iter().flatten() {
                        println!("[安装] {} installed={} path={}", i["target"], i["installed"], i["path"]);
                    }
                }
                Err(e) => println!("[安装] 失败：{}", e),
            }
            match commands::scan_environment(Some(base.clone()), None) {
                Ok(env) => {
                    let v = serde_json::to_value(&env).unwrap();
                    for tool in ["claude", "codex"] {
                        let t = &v[tool];
                        println!("[配置] {} status={} activeProfileId={} unsavedGatewayKey={} model={}", tool, t["configurationStatus"], t["activeProfileId"], t["unsavedGatewayKey"], t["currentModel"]);
                    }
                }
                Err(e) => println!("[配置] 失败：{}", e),
            }
            println!("[环境变量] {:?}", commands::env_overrides());
        }
        Some("adopt") => {
            // 真实收录：测试通过才会存进密钥列表
            let tool = args.get(1).map(String::as_str).unwrap_or("claude");
            let req: commands::AdoptRequest = serde_json::from_value(serde_json::json!({
                "tool": tool, "baseUrl": base, "ignoreProxy": st.ignore_proxy,
            }))
            .unwrap();
            match commands::adopt_current_key(req).await {
                Ok(r) => println!("[{}] 成功：{}", tool, serde_json::to_string(&r).unwrap()),
                Err(e) => println!("[{}] 未收录：{}", tool, e),
            }
        }
        _ => println!("用法：diagnose_probe models | health [密钥名] | run [claude|codex] [报错] | env | adopt [claude|codex]"),
    }
}
