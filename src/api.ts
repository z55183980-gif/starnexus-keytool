import { invoke, Channel } from "@tauri-apps/api/core";
import type { TrayAction } from "./trayFlow";
import * as mock from "./mock";
import type {
  AppState,
  ApplyRequest,
  ApplyResult,
  BackupEntry,
  Environment,
  GatewayToken,
  ImportResult,
  Profile,
  SaveProfileRequest,
  RemoteConfig,
  TestResult,
  LauncherPreferences,
  LauncherScan,
  LaunchRequest,
  HealthIssue,
  HealthRequest,
  AdoptResult,
  ToolId,
  KeyStatus,
  ModelInfo,
} from "./types";

// 每个分支都由 import.meta.env.DEV 直接守卫。
// 生产构建里它会被替换成字面量 false，整段模拟代码连同 ./mock 一起被摇掉。
const DEV = import.meta.env.DEV;
const S = DEV ? mock.activeScenario() : null;

export const takeTrayAction = (): Promise<TrayAction | null> => DEV && S ? Promise.resolve(null) : invoke("take_tray_action");
export const validateProjectDirectory = (path: string): Promise<void> => DEV && S ? Promise.resolve() : invoke("validate_project_directory", { path });

export interface ClientGuide { platform: string; url: string; command: string | null; shell: string; note: string; variants: { shell: string; command: string }[]; canRun: boolean }
export interface ClientVersion { current: string; verifiedBuiltin?: boolean; latest: string | null; updateAvailable: boolean | null; path: string; warning: string | null }
export const isSetupPreview = !!(DEV && S);
export const openSetupTerminal = (): Promise<void> => DEV && S ? Promise.resolve() : invoke("open_setup_terminal");
export interface InstallProgress { phase: "resolve" | "download" | "relay" | "verify" | "install" | "complete" | "installer"; downloaded: number; total: number | null }
export interface InstallOutcome { installed: boolean; message: string }
export const runClientInstall = async (target: import("./types").LaunchTarget, progress: (value: InstallProgress) => void): Promise<InstallOutcome> => {
  if (DEV && S) {
    const pause = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
    const total = 2097152;
    progress({ phase: "resolve", downloaded: 0, total: null }); await pause(500);
    for (let step = 1; step <= 10; step++) { progress({ phase: "download", downloaded: total * step / 10, total }); await pause(180); }
    progress({ phase: "verify", downloaded: 0, total: null }); await pause(600);
    return { installed: false, message: "" };
  }
  const onProgress = new Channel<InstallProgress>(); onProgress.onmessage = progress;
  return invoke("install_client", { target, onProgress });
};
export const clientInstallGuide = (target: import("./types").LaunchTarget): Promise<ClientGuide> => DEV && S
  ? Promise.resolve(mock.mockClientGuide(target))
  : invoke("client_install_guide", { target });
export const openClientDownload = (target: import("./types").LaunchTarget): Promise<void> => DEV && S ? Promise.resolve() : invoke("open_client_download", { target });
export const checkClientVersion = async (target: import("./types").LaunchTarget, ignoreProxy: boolean, checkLatest = true): Promise<ClientVersion> => {
  if (DEV && S) {
    await new Promise(resolve => setTimeout(resolve, 350));
    const state = new URLSearchParams(location.search).get("update") ?? "current";
    const current = target.endsWith("cli") ? "0.169.0" : "26.1111.2228.0";
    return { current, latest: !checkLatest || state === "error" ? null : state === "available" ? (target.endsWith("cli") ? "0.170.0" : "26.1112.2228.0") : current,
      updateAvailable: !checkLatest || state === "error" ? null : state === "available", path: target,
      warning: state === "error" && checkLatest ? "更新检查失败，当前显示本机版本。" : null };
  }
  return invoke("check_client_version", { target, ignoreProxy, checkLatest });
};

export const loadLauncherPreferences = (): Promise<LauncherPreferences> => DEV && S
  ? Promise.resolve({ projectDirectory: "C:\\Projects", recentDirectories: [], claudeMode: "cli", codexMode: "cli", claudeService: "current", codexService: "current", customPaths: {} })
  : invoke("load_launcher_preferences");
export const saveLauncherPreferences = (preferences: LauncherPreferences): Promise<void> => DEV && S
  ? Promise.resolve() : invoke("save_launcher_preferences", { preferences });
export const scanLaunchers = (): Promise<LauncherScan> => DEV && S
  ? mock.mockLauncherScan(S)
  : invoke("scan_launchers");
export const validateLaunch = (req: LaunchRequest): Promise<void> => DEV && S ? Promise.resolve() : invoke("validate_launch", { req });
export const launchClient = (req: LaunchRequest): Promise<{ message: string }> => DEV && S
  ? Promise.resolve({ message: "预览模式：未启动真实程序" }) : invoke("launch_client", { req });

/** 这几个桌面客户端里正在运行的（它们只在启动时读配置） */
export const runningDesktopClients = (targets: import("./types").LaunchTarget[]): Promise<import("./types").LaunchTarget[]> => DEV && S
  ? Promise.resolve(targets) : invoke("running_desktop_clients", { targets });
/** 退出（含后台进程）并重新打开桌面客户端 */
export const restartDesktopClient = (target: import("./types").LaunchTarget): Promise<{ message: string }> => DEV && S
  ? Promise.resolve({ message: "预览模式：未重启真实程序" }) : invoke("restart_desktop_client", { target });

/** Claude 桌面版里是否记着用户选了 Claude.ai 官方登录 */
export const claudeDesktopOfficialLogin = (): Promise<boolean> => DEV && S ? Promise.resolve(true) : invoke("claude_desktop_official_login");
/** Claude 桌面版选了官方登录时，切到第三方推理：退出 → 备份并改选择 → 重新打开 */
export const switchClaudeDesktopToThirdParty = (): Promise<{ backupId: string | null; message: string }> => DEV && S
  ? Promise.resolve({ backupId: null, message: "预览模式：未切换真实程序" }) : invoke("switch_claude_desktop_to_third_party");

/** 读取本机配置。tools：用户同意读取的工具，另一个返回「未读取」占位 */
export const scanEnvironment = (baseUrl?: string, tools?: (ToolId | "claude-cli" | "claude-desktop")[], preferredProfileId?: string): Promise<Environment> =>
  DEV && S
    ? Promise.resolve(mock.mockEnvFor(S, tools?.map(tool => tool === "codex" ? "codex" : "claude")))
    : invoke<Environment>("scan_environment", { baseUrl: baseUrl ?? null, tools: tools ?? null, preferredProfileId: preferredProfileId ?? null });

export const loadAppState = (): Promise<AppState> =>
  DEV && S ? Promise.resolve(mock.mockState()) : invoke<AppState>("load_app_state");

/** 界面语言告诉后端：同步窗口标题、托盘，并记住给下次启动用 */
export const setUiLanguage = (lang: string): Promise<void> =>
  DEV && S ? Promise.resolve() : invoke<void>("set_ui_language", { lang });

/** 记下用户已同意读取某个工具的本机配置 */
export const markConfigRead = (tool: ToolId): Promise<void> =>
  DEV && S ? Promise.resolve(mock.mockMarkConfigRead(tool)) : invoke<void>("mark_config_read", { tool });

export const listProfiles = (): Promise<Profile[]> =>
  DEV && S ? Promise.resolve(mock.mockProfiles(S)) : invoke<Profile[]>("list_profiles");

export const saveProfile = (req: SaveProfileRequest): Promise<string> =>
  DEV && S ? Promise.resolve(mock.mockSaveProfile(S, req)) : invoke<string>("save_profile", { req });

export const deleteProfile = (id: string): Promise<void> =>
  DEV && S ? Promise.resolve() : invoke<void>("delete_profile", { id });

export const getRemoteConfig = (ignoreProxy: boolean): Promise<RemoteConfig> =>
  DEV && S
    ? Promise.resolve(mock.mockRemote())
    : invoke<RemoteConfig>("get_remote_config", { ignoreProxy });

export const testConnection = (
  baseUrl: string,
  apiKey: string,
  ignoreProxy: boolean,
): Promise<TestResult> =>
  DEV && S
    ? Promise.resolve(mock.mockTest())
    : invoke<TestResult>("test_connection", { baseUrl, apiKey, ignoreProxy });

/** 用已保存的 Key 测试，密钥不经过前端 */
/** 这把密钥能用的模型和各自支持的接口（读网关目录，不计费） */
export const listProfileModels = (profileId: string, baseUrl: string, ignoreProxy: boolean): Promise<ModelInfo[]> =>
  DEV && S ? Promise.resolve(mock.mockCatalog()) : invoke<ModelInfo[]>("list_profile_models", { profileId, baseUrl, ignoreProxy });

// ——— 登录取 Key。只走浏览器登录；登录态只在后端内存里，前端只拿得到用户名和掩码 ———

export type LoginResult = { username: string; require2fa: boolean };
export const gatewayLogin = (username: string, password: string, ignoreProxy: boolean): Promise<LoginResult> =>
  DEV && S ? Promise.resolve({username: "demo", require2fa: false}) : invoke("gateway_login", { username, password, ignoreProxy });
export const gatewayLogin2fa = (code: string): Promise<LoginResult> =>
  DEV && S ? Promise.resolve({username: "demo", require2fa: false}) : invoke("gateway_login_2fa", { code });

export const gatewayLogout = (): Promise<void> =>
  DEV && S ? Promise.resolve() : invoke<void>("gateway_logout");

export const gatewayCurrentUser = (): Promise<string | null> =>
  DEV && S ? Promise.resolve(null) : invoke<string | null>("gateway_current_user");

/** 登录后同步：本地导入过的密钥在控制台里是否还可用 */
export const gatewayKeyStatus = (): Promise<KeyStatus[]> =>
  DEV && S ? Promise.resolve(mock.mockKeyStatus()) : invoke<KeyStatus[]>("gateway_key_status");

export const gatewayListTokens = (): Promise<GatewayToken[]> =>
  DEV && S ? Promise.resolve(mock.mockTokens()) : invoke<GatewayToken[]>("gateway_list_tokens");

/** 取回明文并直接存进密钥库，返回掩码 */
export const gatewayAdoptToken = (profileId: string, tokenId: number): Promise<string> =>
  DEV && S
    ? Promise.resolve("sk-••••••••ab12")
    : invoke<string>("gateway_adopt_token", { profileId, tokenId });

/** 一次登录批量导入多把 Key，每把存成一条 */
export const gatewayImportTokens = (tokenIds: number[]): Promise<ImportResult> =>
  DEV && S
    ? Promise.resolve(mock.mockImportTokens(S, tokenIds))
    : invoke<ImportResult>("gateway_import_tokens", { tokenIds });

export const listBackups = (): Promise<BackupEntry[]> =>
  DEV && S ? Promise.resolve(mock.mockBackups(S)) : invoke<BackupEntry[]>("list_backups");

export const restoreBackup = (id: string): Promise<void> =>
  DEV && S ? Promise.resolve(mock.mockRestore()) : invoke<void>("restore_backup", { id });

export const applyConfig = (req: ApplyRequest): Promise<ApplyResult> =>
  DEV && S ? Promise.resolve(mock.mockApply(req)) : invoke<ApplyResult>("apply_config", { req });

export const healthCheck = (req: HealthRequest): Promise<HealthIssue[]> =>
  DEV && S ? Promise.resolve(mock.mockHealth(S, req)) : invoke<HealthIssue[]>("health_check", { req });

/** 会覆盖配置文件的环境变量里，已经设置了哪些（只有变量名） */
export const envOverrides = (): Promise<string[]> =>
  DEV && S ? Promise.resolve(mock.mockEnvOverrides(S)) : invoke<string[]>("env_overrides");

/** 把配置文件里正在用、没保存在本工具的密钥（AI 或第三方）加入列表（后端读取并测试，密钥不经过前端）。
 *  desktop：Claude 桌面版读它自己的第三方推理配置 */
export const adoptCurrentKey = (tool: ToolId, baseUrl: string, ignoreProxy: boolean, desktop = false): Promise<AdoptResult> =>
  DEV && S ? mock.mockAdopt(S, tool) : invoke<AdoptResult>("adopt_current_key", { req: { tool, baseUrl, ignoreProxy, desktop } });


export const rebuildMinimalConfig = (req: ApplyRequest): Promise<ApplyResult> =>
  DEV && S
    ? Promise.resolve(mock.mockApply(req))
    : invoke<ApplyResult>("rebuild_minimal_config", { req });

export const revertToOfficial = (codex: boolean, claude: boolean, claudeDesktop = false): Promise<string> =>
  DEV && S
    ? Promise.resolve(mock.mockRevertOfficial(codex, claude, claudeDesktop))
    : invoke<string>("revert_to_official", { codex, claude, claudeDesktop });

export const gatewayBrowserLoginStart = (ignoreProxy: boolean): Promise<string> =>
  DEV && S ? Promise.resolve("https://dkby.com/desktop-login") : invoke("gateway_browser_login_start", { ignoreProxy });
export const gatewayBrowserLoginFinish = (code: string, state: string, ignoreProxy: boolean): Promise<LoginResult> =>
  DEV && S ? Promise.resolve({username:"demo",require2fa:false}) : invoke("gateway_browser_login_finish", { code, state, ignoreProxy });
export const gatewayBrowserLoginCancel = (): Promise<void> =>
  DEV && S ? Promise.resolve() : invoke("gateway_browser_login_cancel");
