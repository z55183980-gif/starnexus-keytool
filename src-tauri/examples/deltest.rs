//! 直接验证删除逻辑：列出 -> 删除第一个 -> 再列出。
//! 只动本工具自己的 state.json 和密钥库条目，不碰 Codex/Claude 的配置。
//!
//!     cargo run --example deltest

use dbkey_lib::state;

fn dump(tag: &str) {
    let st = state::load().expect("读取本地状态失败");
    println!("{} profiles={}", tag, st.profiles.len());
    for p in &st.profiles {
        println!(
            "   id={} name={} hasKey={}",
            p.id,
            p.name,
            state::load_profile_key(&p.id).expect("读取密钥库失败").is_some()
        );
    }
}

fn main() {
    dump("[before]");

    let st = state::load().expect("读取本地状态失败");
    let Some(first) = st.profiles.first().cloned() else {
        println!("没有账号可删，先在界面里加一个再跑");
        return;
    };

    println!("\n删除 id={} name={}", first.id, first.name);
    match state::delete_profile(&first.id) {
        Ok(()) => println!("delete_profile 返回 Ok"),
        Err(e) => println!("delete_profile 返回 Err: {}", e),
    }

    dump("\n[after]");

    // 关键：再 load 一次，看有没有被"复活"
    dump("\n[reload again]");

    println!(
        "\n密钥库条目是否已清: {}",
        state::load_profile_key(&first.id).expect("读取密钥库失败").is_none()
    );
}
