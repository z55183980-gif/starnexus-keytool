//! 原生部分的界面语言：窗口标题、托盘菜单、后端起的默认名字。
//!
//! 界面切换语言时前端调用 `set_ui_language` 告诉这里，并写进状态文件，
//! 下次启动在前端加载之前就能用对的语言。没选过时跟随系统；认不出来时用英文兜底，
//! 规则和前端 `src/i18n.ts` 的 detectLang 一致。
//!
//! 只读内存里的缓存，不在这里读状态文件：`state::next_default_name` 会在持有状态锁时调用 `pick`。

use std::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    ZhCn,
    /// 繁体，按香港用语
    ZhHk,
    En,
}

static CURRENT: RwLock<Option<Lang>> = RwLock::new(None);

/// 前端用的语言代码 → Lang
pub fn parse(code: &str) -> Option<Lang> {
    match code {
        "zh-CN" => Some(Lang::ZhCn),
        "zh-HK" => Some(Lang::ZhHk),
        // 早期版本存的繁体代码
        "zh-TW" => Some(Lang::ZhHk),
        "en" => Some(Lang::En),
        _ => None,
    }
}

/// 按系统语言列表挑：香港、澳门、台湾或繁体 → 繁體（香港用语）；其他中文 → 简体；其余以及认不出来 → English
pub fn from_locales<I: IntoIterator<Item = String>>(locales: I) -> Lang {
    for raw in locales {
        let l = raw.to_lowercase().replace('_', "-");
        if l.starts_with("zh-hk") || l.starts_with("zh-mo") || l.starts_with("zh-tw") || l.starts_with("zh-hant") {
            return Lang::ZhHk;
        }
        if l.starts_with("zh") {
            return Lang::ZhCn;
        }
        if !l.is_empty() {
            return Lang::En;
        }
    }
    Lang::En
}

pub fn detect_system() -> Lang {
    from_locales(sys_locale::get_locales())
}

/// 启动时调用一次：用保存过的选择，没有就跟随系统
pub fn init(saved: &str) {
    set(parse(saved).unwrap_or_else(detect_system));
}

pub fn set(lang: Lang) {
    if let Ok(mut c) = CURRENT.write() {
        *c = Some(lang);
    }
}

pub fn current() -> Lang {
    CURRENT.read().ok().and_then(|c| *c).unwrap_or_else(detect_system)
}

/// 按当前语言从三种写法里挑一个
pub fn pick<'a>(zh_cn: &'a str, zh_hk: &'a str, en: &'a str) -> &'a str {
    match current() {
        Lang::ZhCn => zh_cn,
        Lang::ZhHk => zh_hk,
        Lang::En => en,
    }
}

/// 应用名：窗口标题、托盘提示用
pub fn app_name() -> &'static str {
    pick("StarNexus 密钥管理工具", "StarNexus 密鑰管理工具", "StarNexus Key Tool")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_rules_match_frontend() {
        let l = |v: &[&str]| from_locales(v.iter().map(|s| s.to_string()));
        assert_eq!(l(&["zh-HK"]), Lang::ZhHk);
        assert_eq!(l(&["zh-TW"]), Lang::ZhHk);
        assert_eq!(l(&["zh-Hant-CN"]), Lang::ZhHk);
        assert_eq!(l(&["zh_CN"]), Lang::ZhCn);
        assert_eq!(l(&["zh-Hans-SG"]), Lang::ZhCn);
        assert_eq!(l(&["en-US", "zh-CN"]), Lang::En);
        assert_eq!(l(&["ja-JP"]), Lang::En);
        assert_eq!(l(&[]), Lang::En);
    }

    #[test]
    fn parse_codes() {
        assert_eq!(parse("zh-HK"), Some(Lang::ZhHk));
        assert_eq!(parse("zh-TW"), Some(Lang::ZhHk));
        assert_eq!(parse("fr"), None);
    }
}
