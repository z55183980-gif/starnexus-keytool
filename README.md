# StarNexus 密钥管理工具

独立的 StarNexus / 星域互联 Claude 与 Codex 密钥管理和启动工具。界面 AI 功能统一使用「AI」名称。

## 地址与登录

- 官网：https://dkby.com
- 控制台：https://dkby.com/keys
- 官网登录：工具生成 https://dkby.com/desktop-login 授权请求；官网账号页仍为 https://dkby.com/sign-in
- API：https://gateway.dkby.com/v1
- Claude 根地址：https://gateway.dkby.com
- 程序内账号登录：POST /api/user/login；两步验证：POST /api/user/login/2fa。
- 密钥分页：GET /api/token/?p=1&page_size=100；完整密钥：POST /api/token/:id/key。
- 官网登录通过专属 com.dkby.starnexus.keytool 协议返回一次性授权码，使用 PKCE S256 + state 校验；两分钟过期且只能换取一次会话。网站会话只保留在工具后端内存，密钥保存在系统凭据库。
- 工具内账号密码登录仍可用。标题和按钮统一为「登录账号」。
- 第三方登录、两步验证和网站人机验证均在官网原有登录页面完成，成功后返回工具导入密钥。浏览器若未自动打开应用，点击官网授权页「返回密钥管理工具」。
- Windows 安装包注册本产品自己的回调协议；Windows 免安装版也会在运行时为本用户注册该协议。

## 独立性

| 项目 | 本工具 |
| --- | --- |
| 安装名称 | StarNexus Key Tool |
| 应用 ID / 系统凭据库服务名 | com.dkby.starnexus.keytool |
| 可执行文件 | starnexus-keytool |
| 本地状态和启动偏好 | ~/.starnexus-keytool/ |
| 配置备份 | CODEX_HOME/.starnexus-keytool-backups/ |
| Codex 自定义 provider ID | starnexus_keytool |

安装器不卸载其他产品，不注册其他产品的登录协议，不导入其他产品的本地密钥。源码、依赖、构建缓存、产物和发布仓库均独立。

注意：Claude / Codex 客户端自身的用户配置由这些客户端共用。只有用户主动应用密钥或选择服务启动时，本工具才修改对应客户端配置，并提前备份。两个密钥工具不能同时控制同一客户端的当前服务；如需不同 CLI 配置，可使用 CODEX_HOME / CLAUDE_CONFIG_DIR。

## 构建

```powershell
npm ci
npm test
npm run build
cd src-tauri
cargo test --all-targets
cd ..
npm run tauri build
```

Windows 输出 NSIS EXE 和 MSI。macOS 使用独立仓库的 GitHub Actions，在 Mac runner 上生成 Intel + Apple Silicon 通用 DMG / APP ZIP。产物放在 artifacts；不自动部署网站或发布公开 Release。

当前默认未签名：Windows 可能提示未知发布者；macOS 需要用户允许安装。正式外部分发可在本仓库配置独立的签名证书与公证流程。

Logo 直接复制自 D:/projects/starnexus/web/default/public/logo.png；安装图标通过 Tauri icon 生成。源功能快照版本见 source-revision.txt；此后本项目独立维护，不回写原项目。
