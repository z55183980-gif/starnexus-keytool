// 与 Rust 侧结构保持一致

export type ConfigState = "missing" | "ok" | { broken: string };
export type SettingsState = "missing" | "ok" | { broken: string };

export type AuthState = "Missing" | "OfficialLogin" | "ApiKeyOnly" | "Unparsable" | "ExternalStore";

export type ConfigurationStatus = "unconfigured" | "configured" | "official" | "custom" | "unknown" | "broken";

export type ToolId = "claude" | "codex";
export type LaunchMode = "desktop" | "cli";
export type LaunchTarget = `${ToolId}-${LaunchMode}`;
export interface LauncherPreferences {
  projectDirectory: string;
  recentDirectories: string[];
  claudeMode: LaunchMode;
  codexMode: LaunchMode;
  claudeService: string;
  codexService: string;
  customPaths: Partial<Record<LaunchTarget, string>>;
}
export interface LauncherInstallation {
  target: LaunchTarget;
  installed: boolean;
  path: string | null;
  message: string;
}
export interface LauncherScan {
  platform: string;
  installations: LauncherInstallation[];
}
export interface LaunchRequest { target: LaunchTarget; directory: string }

export interface Profile {
  id: string;
  name: string;
  maskedKey: string;
  hasKey: boolean;
  codexModel: string;
  codexEffort: string;
  claudeModel: string;
  /** 其他服务（不是 AI）：自己填的地址。填了哪个，就能给哪个工具用 */
  custom: boolean;
  codexBaseUrl: string;
  claudeBaseUrl: string;
}

/** 这把密钥能不能给某个工具用：AI 密钥都能；其他服务要填了那个工具的地址 */
export const supportsTool = (p: Profile, tool: "claude" | "codex") =>
  !p.custom || !!(tool === "codex" ? p.codexBaseUrl : p.claudeBaseUrl);

/** 列表、选择里显示的名字：AI 密钥带「AI ·」，其他服务直接用名字 */
export const profileTitle = (p: Profile) => (p.custom ? p.name : `AI · ${p.name}`);

/** 登录后，本地密钥在控制台里的状态（只覆盖从当前账号导入过的密钥） */
export interface KeyStatus { profileId: string; usable: boolean; missing: boolean; note: string }

export interface GatewayToken {
  id: number;
  name: string;
  usable: boolean;
  note: string;
  /** 已导入过就是本地那条的名字，界面据此显示「更新」而不是勾选 */
  linkedProfile: string | null;
}

export interface ImportResult {
  created: string[];
  updated: string[];
  unchanged: string[];
  skipped: string[];
}

export interface SaveProfileRequest {
  id: string;
  name: string;
  apiKey: string;
  codexModel: string;
  codexEffort: string;
  claudeModel: string;
  /** 其他服务的地址；都空就是 AI 密钥 */
  codexBaseUrl?: string;
  claudeBaseUrl?: string;
}

export interface CodexEnv {
  homeExists: boolean;
  home: string;
  configState: ConfigState;
  authState: AuthState;
  currentBaseUrl: string | null;
  currentModel: string | null;
  currentReasoningEffort: string | null;
  residue: string[];
  managedByUs: boolean;
  activeProfileId: string | null;
  /** 配置指向本网关、带着一把 Key，但没保存在本工具 */
  unsavedGatewayKey: boolean;
  configurationStatus: ConfigurationStatus;
  statusMessage: string;
}

export interface ClaudeEnv {
  homeExists: boolean;
  home: string;
  settingsState: SettingsState;
  currentBaseUrl: string | null;
  currentModel: string | null;
  managedByUs: boolean;
  activeProfileId: string | null;
  /** 配置指向本网关、带着一把 Key，但没保存在本工具 */
  unsavedGatewayKey: boolean;
  configurationStatus: ConfigurationStatus;
  statusMessage: string;
}

export interface Environment {
  codex: CodexEnv;
  /** Claude Code CLI：~/.claude/settings.json */
  claude: ClaudeEnv;
  /** Claude 桌面版：它自己的第三方推理配置（不读 settings.json） */
  claudeDesktop: ClaudeEnv;
}

export interface RemoteConfig {
  base_url: string;
  wire_api: string;
  default_model: string;
  recommended_models: string[];
  claude_default_model: string;
  claude_recommended_models: string[];
  claude_opus_model: string;
  claude_sonnet_model: string;
  claude_haiku_model: string;
  min_client_version: string;
  notice: string;
  write_mode: string;
}

export interface TestResult {
  ok: boolean;
  message: string;
  models: string[];
}

export interface BackupEntry {
  id: string;
  created_at: string;
  has_config: boolean;
  has_auth: boolean;
  has_claude: boolean;
  has_claude_desktop?: boolean;
}

export interface AppState {
  profiles: Profile[];
  ignoreProxy: boolean;
  cleanupResidue: boolean;
  lastConfiguredAt: string;
  /** 旧版：一次同意读取两个工具 */
  configReadConsent: boolean;
  /** 用户同意读取配置的工具；没同意的工具不读它的配置文件 */
  configReadTools: string[];
}

export interface ApplyRequest {
  profileId: string;
  baseUrl: string;
  wireApi: string;
  ignoreProxy: boolean;

  configureCodex: boolean;
  model: string;
  reasoningEffort: string;
  cleanupResidue: boolean;
  overwriteOfficialAuth: boolean;
  conservative: boolean;

  configureClaude: boolean;
  claudeModel: string;
  claudeOpusModel: string;
  claudeSonnetModel: string;
  claudeHaikuModel: string;
  /** 写 Claude 桌面版自己的第三方推理配置（和 configureClaude 写的 CLI 配置是两份） */
  configureClaudeDesktop?: boolean;
}

export interface ApplyResult {
  backupId: string;
  wroteCodexConfig: boolean;
  wroteCodexAuth: boolean;
  wroteClaudeSettings: boolean;
  wroteClaudeDesktop?: boolean;
  paths: string[];
  warnings: string[];
}

/** 体检（health_check）的问题行；fix 只会是已有的动作 */
export type HealthFix = "rewrite" | "rebuild" | "keys" | "settings" | "model" | "none";
export interface HealthIssue { id: string; level: "error" | "warn"; text: string; fix: HealthFix }
/** mode：Claude 桌面版与 CLI 读两份配置，体检按启动方式分开查 */
export interface HealthRequest { tool: ToolId; service: string; model: string; baseUrl: string; ignoreProxy: boolean; opusModel: string; sonnetModel: string; haikuModel: string; mode: LaunchMode }
/** 把配置文件里正在用的 AI 密钥收进密钥列表的结果 */
export interface AdoptResult { id: string; name: string; existed: boolean }

/** 网关目录里的模型和它支持的接口（responses / messages / chat.completions…）；endpoints 为空 = 未知 */
export interface ModelInfo { id: string; endpoints: string[] }


export function isBroken(s: ConfigState | SettingsState): s is { broken: string } {
  return typeof s === "object" && s !== null && "broken" in s;
}
