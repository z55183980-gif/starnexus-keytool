import { invoke } from "@tauri-apps/api/core";
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
  DiagnoseRequest,
  Diagnosis,
  AdoptResult,
  ToolId,
  KeyStatus,
  ModelInfo,
} from "./types";

// 每个分支都由 import.meta.env.DEV 直接守卫。
// 生产构建里它会被替换成字面量 false，整段模拟代码连同 ./mock 一起被摇掉。
const DEV = import.meta.env.DEV;
const S = DEV ? mock.activeScenario() : null;

export const loadLauncherPreferences = (): Promise<LauncherPreferences> => DEV && S
  ? Promise.resolve({ projectDirectory: "C:\\Projects", recentDirectories: [], claudeMode: "cli", codexMode: "cli", claudeService: "current", codexService: "current", customPaths: {} })
  : invoke("load_launcher_preferences");
export const saveLauncherPreferences = (preferences: LauncherPreferences): Promise<void> => DEV && S
  ? Promise.resolve() : invoke("save_launcher_preferences", { preferences });
export const scanLaunchers = (): Promise<LauncherScan> => DEV && S
  ? Promise.resolve({ platform: "windows", installations: (["claude-desktop", "claude-cli", "codex-desktop", "codex-cli"] as const).map(target => ({ target, installed: S !== "nocodex" || !target.startsWith("codex"), path: "C:\\Applications\\" + target + ".exe", message: "界面预览示例" })) })
  : invoke("scan_launchers");
export const validateLaunch = (req: LaunchRequest): Promise<void> => DEV && S ? Promise.resolve() : invoke("validate_launch", { req });
export const launchClient = (req: LaunchRequest): Promise<{ message: string }> => DEV && S
  ? Promise.resolve({ message: "预览模式：未启动真实程序" }) : invoke("launch_client", { req });

/** 读取本机配置。tools：用户同意读取的工具，另一个返回「未读取」占位 */
export const scanEnvironment = (baseUrl?: string, tools?: ToolId[]): Promise<Environment> =>
  DEV && S
    ? Promise.resolve(mock.mockEnvFor(S, tools))
    : invoke<Environment>("scan_environment", { baseUrl: baseUrl ?? null, tools: tools ?? null });

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
    ? Promise.resolve({ created: ["Codex 专用"], updated: ["主号"], unchanged: [], skipped: [] })
    : invoke<ImportResult>("gateway_import_tokens", { tokenIds });

export const listBackups = (): Promise<BackupEntry[]> =>
  DEV && S ? Promise.resolve(mock.mockBackups(S)) : invoke<BackupEntry[]>("list_backups");

export const restoreBackup = (id: string): Promise<void> =>
  DEV && S ? Promise.resolve() : invoke<void>("restore_backup", { id });

export const applyConfig = (req: ApplyRequest): Promise<ApplyResult> =>
  DEV && S ? Promise.resolve(mock.mockApply(req)) : invoke<ApplyResult>("apply_config", { req });

export const healthCheck = (req: HealthRequest): Promise<HealthIssue[]> =>
  DEV && S ? Promise.resolve(mock.mockHealth(S, req)) : invoke<HealthIssue[]>("health_check", { req });

/** 会覆盖配置文件的环境变量里，已经设置了哪些（只有变量名） */
export const envOverrides = (): Promise<string[]> =>
  DEV && S ? Promise.resolve(mock.mockEnvOverrides(S)) : invoke<string[]>("env_overrides");

/** 把配置文件里正在用、没保存在本工具的 AI 密钥收进列表（后端读取并测试，密钥不经过前端） */
export const adoptCurrentKey = (tool: ToolId, baseUrl: string, ignoreProxy: boolean): Promise<AdoptResult> =>
  DEV && S ? mock.mockAdopt(S, tool) : invoke<AdoptResult>("adopt_current_key", { req: { tool, baseUrl, ignoreProxy } });

export const aiDiagnose = (req: DiagnoseRequest): Promise<Diagnosis> =>
  DEV && S ? mock.mockDiagnose(S, req) : invoke<Diagnosis>("ai_diagnose", { req });

export const rebuildMinimalConfig = (req: ApplyRequest): Promise<ApplyResult> =>
  DEV && S
    ? Promise.resolve(mock.mockApply(req))
    : invoke<ApplyResult>("rebuild_minimal_config", { req });

export const revertToOfficial = (codex: boolean, claude: boolean, claudeDesktop = false): Promise<string> =>
  DEV && S
    ? Promise.resolve("20260918-183012")
    : invoke<string>("revert_to_official", { codex, claude, claudeDesktop });
