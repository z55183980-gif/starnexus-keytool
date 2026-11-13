use serde::Serialize;

/// 所有错误最终都要变成用户看得懂的中文。
/// 数千用户规模下，一条看不懂的报错就是一张客服工单。
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("找不到 Codex 的设置文件夹：{0}")]
    CodexHomeMissing(String),

    #[error("配置文件的内容有格式问题，没法安全修改：{0}")]
    TomlParse(String),

    #[error("登录信息文件的内容有格式问题，没法安全修改：{0}")]
    JsonParse(String),

    #[error("读写文件时出错：{0}")]
    Io(String),

    #[error("网络连接出错：{0}")]
    Network(String),

    #[error("访问系统密钥库时出错：{0}")]
    Keyring(String),

    #[error("{0}")]
    Other(String),
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e.to_string())
    }
}

impl From<toml_edit::TomlError> for AppError {
    fn from(e: toml_edit::TomlError) -> Self {
        AppError::TomlParse(e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::JsonParse(e.to_string())
    }
}

impl From<keyring::Error> for AppError {
    fn from(e: keyring::Error) -> Self {
        AppError::Keyring(e.to_string())
    }
}

/// Tauri 命令要把错误序列化给前端。
impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
