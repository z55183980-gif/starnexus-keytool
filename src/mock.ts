// 开发期界面预览用的假数据。
// 只在 vite dev 且 URL 带 ?mock=<场景> 时生效，生产构建里这段代码会被摇掉。
//
//   http://localhost:1420/?mock=configured   已配置
//   http://localhost:1420/?mock=fresh        全新未配置
//   http://localhost:1420/?mock=legacy       旧脚本残留 + 官方登录
//   http://localhost:1420/?mock=broken       配置文件损坏
//   http://localhost:1420/?mock=nocodex      没装 Codex

import type {
  AppState,
  ApplyRequest,
  ApplyResult,
  BackupEntry,
  Environment,
  GatewayToken,
  Profile,
  RemoteConfig,
  SaveProfileRequest,
  AdoptResult,
  KeyStatus,
  ToolId,
  CodexEnv,
  ClaudeEnv,
  TestResult,
  HealthIssue,
  HealthRequest,
  ModelInfo,
} from "./types";

export type Scenario = "configured" | "fresh" | "legacy" | "broken" | "nocodex";

export function activeScenario(): Scenario | null {
  if (!import.meta.env.DEV) return null;
  const m = new URLSearchParams(location.search).get("mock");
  return m ? (m as Scenario) : null;
}

const HOME_CODEX = "C:\\Users\\PC\\.codex";
const HOME_CLAUDE = "C:\\Users\\PC\\.claude";

export function mockRemote(): RemoteConfig {
  return {
    base_url: "https://gateway.dkby.com/v1",
    wire_api: "responses",
    default_model: "gpt-6-sol",
    recommended_models: [],
    claude_default_model: "claude-opus-5-5",
    claude_recommended_models: [],
    claude_opus_model: "claude-opus-5-5",
    claude_sonnet_model: "claude-sonnet-5-5",
    claude_haiku_model: "claude-sonnet-5-5",
    min_client_version: "",
    notice: "",
    write_mode: "provider_table",
  };
}

/** 预览里保存过的密钥：存在内存里，刷新页面才回到初始数据 */
let savedProfiles: Profile[] | null = null;

export function mockProfiles(s: Scenario): Profile[] {
  if (savedProfiles) return savedProfiles.map(p => ({ ...p }));
  if (s === "fresh") return [];
  const base = [
    { id: "p1", name: "主号", maskedKey: "sk-••••••••a1b2", hasKey: true,
      codexModel: "gpt-5.3-codex", codexEffort: "medium", claudeModel: "claude-opus-4-8", gatewayTokenId: 1 },
    { id: "p2", name: "测试号", maskedKey: "sk-••••••••c3d4", hasKey: true,
      codexModel: "gpt-5-codex-mini", codexEffort: "low", claudeModel: "claude-haiku-4-5-20251001", gatewayTokenId: null },
    // 其他服务：只填了 Claude Code 地址，Codex 那边应显示「没有填地址」
    { id: "p3", name: "Anthropic 官方", maskedKey: "sk-ant-••••e5f6", hasKey: true,
      codexModel: "", codexEffort: "", claudeModel: "", gatewayTokenId: null,
      custom: true, codexBaseUrl: "", claudeBaseUrl: "https://api.anthropic.com" },
  ].map(p => ({ custom: false, codexBaseUrl: "", claudeBaseUrl: "", ...p }));
  return base;
}

/** 预览里的「保存密钥」：改内存里的列表，让保存后的界面和真实程序一致 */
export function mockSaveProfile(s: Scenario, req: SaveProfileRequest): string {
  const list = mockProfiles(s);
  const old = list.find(p => p.id === req.id);
  const id = old?.id || `p${Date.now()}`;
  const next: Profile = {
    ...(old ?? { id, maskedKey: "", hasKey: false }),
    id, name: req.name || old?.name || "新密钥",
    codexModel: req.codexModel, codexEffort: req.codexEffort, claudeModel: req.claudeModel,
    codexBaseUrl: req.codexBaseUrl ?? old?.codexBaseUrl ?? "", claudeBaseUrl: req.claudeBaseUrl ?? old?.claudeBaseUrl ?? "",
    custom: !!((req.codexBaseUrl ?? old?.codexBaseUrl) || (req.claudeBaseUrl ?? old?.claudeBaseUrl)),
    ...(req.apiKey ? { maskedKey: `sk-••••••••${req.apiKey.slice(-4)}`, hasKey: true } : {}),
  } as Profile;
  savedProfiles = old ? list.map(p => p.id === id ? next : p) : [...list, next];
  return id;
}

/** 预览里同意读取的状态只存在内存里：刷新页面会再次看到首次读取的遮罩 */
const configReadTools: string[] = [];
export function mockMarkConfigRead(tool: ToolId) { if (!configReadTools.includes(tool)) configReadTools.push(tool); }

/** 只「读取」同意过的工具，另一个和后端一样返回未读取占位 */
export function mockEnvFor(s: Scenario, tools?: ToolId[]): Environment {
  const env = mockEnv(s);
  const notRead = { homeExists: false, currentBaseUrl: null, currentModel: null, managedByUs: false, activeProfileId: null, unsavedGatewayKey: false, configurationStatus: "unknown" as const, statusMessage: "尚未读取（用户还没同意读取这个工具的配置）" };
  if (tools && !tools.includes("claude")) env.claude = { ...env.claude, ...notRead, settingsState: "missing" as never };
  if (tools && !tools.includes("codex")) env.codex = { ...env.codex, ...notRead, configState: "missing" as never, authState: "Missing", currentReasoningEffort: null, residue: [] };
  return env;
}

export function mockState(): AppState {
  return {
    profiles: [],
    ignoreProxy: false,
    cleanupResidue: true,
    lastConfiguredAt: "",
    configReadConsent: false,
    configReadTools: [...configReadTools],
  };
}

export function mockBackups(s: Scenario): BackupEntry[] {
  if (s === "fresh" || s === "nocodex") return [];
  return [
    { id: "20260918-181530", created_at: "2026-09-18 18:15:30", has_config: true, has_auth: true, has_claude: true },
    { id: "20260918-174402", created_at: "2026-09-18 17:44:02", has_config: true, has_auth: true, has_claude: false },
  ];
}

/** 预览里收录过的工具：收录后配置就对得上那把密钥了 */
const adopted: Partial<Record<ToolId, string>> = {};

export function mockEnvOverrides(s: Scenario): string[] {
  return s === "legacy" ? ["ANTHROPIC_API_KEY"] : [];
}

export async function mockAdopt(s: Scenario, tool: ToolId): Promise<AdoptResult> {
  await new Promise(r => setTimeout(r, 600));
  const id = mockSaveProfile(s, { id: "", name: `${tool === "claude" ? "Claude" : "Codex"} 配置里的密钥`, apiKey: "sk-preview-9f3e", codexModel: "", codexEffort: "", claudeModel: tool === "claude" ? "claude-opus-4-8" : "" });
  adopted[tool] = id;
  return { id, name: `${tool === "claude" ? "Claude" : "Codex"} 配置里的密钥`, existed: false };
}

/** 场景数据里 unsavedGatewayKey 可省略，默认 false */
type Loose<T> = Omit<T, "unsavedGatewayKey"> & { unsavedGatewayKey?: boolean };

export function mockEnv(s: Scenario): Environment {
  const base = mockEnvBase(s);
  const env: Environment = {
    codex: { ...base.codex, unsavedGatewayKey: base.codex.unsavedGatewayKey ?? false },
    claude: { ...base.claude, unsavedGatewayKey: base.claude.unsavedGatewayKey ?? false },
    // 桌面版的第三方推理配置：演示里一律还没启用（用官方登录）
    claudeDesktop: {
      ...base.claude, home: "C:\Users\you\AppData\Local\Claude-3p", settingsState: "missing",
      currentBaseUrl: null, currentModel: null, managedByUs: false, activeProfileId: null, unsavedGatewayKey: false,
      configurationStatus: "official", statusMessage: "未启用第三方推理，使用 Claude 官方登录",
    },
  };
  for (const tool of ["claude", "codex"] as const) {
    const id = adopted[tool];
    if (id) env[tool] = { ...env[tool], activeProfileId: id, unsavedGatewayKey: false, configurationStatus: "configured" } as never;
  }
  return env;
}

function mockEnvBase(s: Scenario): { codex: Loose<CodexEnv>; claude: Loose<ClaudeEnv> } {
  const claudeConfigured = {
    homeExists: true,
    home: HOME_CLAUDE,
    settingsState: "ok" as const,
    currentBaseUrl: "https://gateway.dkby.com",
    currentModel: "claude-opus-4-8",
    managedByUs: true,
    activeProfileId: "p1",
    configurationStatus: "configured" as const,
    statusMessage: "用户配置文件已匹配此密钥；运行中的客户端尚未验证",
  };
  const claudeFresh = {
    homeExists: true,
    home: HOME_CLAUDE,
    settingsState: "ok" as const,
    currentBaseUrl: null,
    currentModel: null,
    managedByUs: false,
    activeProfileId: null,
    configurationStatus: "unknown" as const,
    statusMessage: "尚未确认客户端配置和登录状态",
  };

  switch (s) {
    case "configured":
      return {
        codex: {
          homeExists: true,
          home: HOME_CODEX,
          configState: "ok",
          authState: "ApiKeyOnly",
          currentBaseUrl: "https://gateway.dkby.com/v1",
          currentModel: "gpt-5.3-codex",
          currentReasoningEffort: "medium",
          residue: [],
          managedByUs: true,
          activeProfileId: "p1",
          configurationStatus: "configured" as const,
          statusMessage: "用户配置文件已匹配此密钥；运行中的客户端尚未验证",
        },
        claude: claudeConfigured,
      };

    case "fresh":
      return {
        codex: {
          homeExists: true,
          home: HOME_CODEX,
          configState: "missing",
          authState: "Missing",
          currentBaseUrl: null,
          currentModel: null,
          currentReasoningEffort: null,
          residue: [],
          managedByUs: false,
          activeProfileId: null,
          configurationStatus: "unconfigured" as const,
          statusMessage: "尚未找到用户配置或登录记录；这不代表客户端未安装",
        },
        claude: claudeFresh,
      };

    case "legacy":
      return {
        codex: {
          homeExists: true,
          home: HOME_CODEX,
          configState: "ok",
          authState: "OfficialLogin",
          currentBaseUrl: "https://gateway.dkby.com/v1",
          currentModel: "gpt-5.6-sol",
          currentReasoningEffort: "high",
          residue: ["14 行多余的空行", "2 行末尾有多余空格", "早期版本留下的设置"],
          managedByUs: false,
          activeProfileId: null,
          configurationStatus: "unknown" as const,
          statusMessage: "尚未确认客户端配置和登录状态",
        },
        // 旧版手动配过 AI：配置里有 Key，但没保存在本工具
        claude: { ...claudeConfigured, managedByUs: false, activeProfileId: null, unsavedGatewayKey: true, configurationStatus: "unknown" as const, statusMessage: "网关地址已配置，但凭据未匹配本地密钥；未验证客户端运行状态" },
      };

    case "broken":
      return {
        codex: {
          homeExists: true,
          home: HOME_CODEX,
          configState: { broken: "TOML parse error at line 42, column 8\n  |\n42 | model = \"unclosed\n  |         ^\ninvalid basic string" },
          authState: "ApiKeyOnly",
          currentBaseUrl: null,
          currentModel: null,
          currentReasoningEffort: null,
          residue: [],
          managedByUs: false,
          activeProfileId: null,
          configurationStatus: "broken" as const,
          statusMessage: "配置文件格式损坏，未确认配置状态",
        },
        claude: {
          ...claudeFresh,
          settingsState: { broken: "expected `,` or `}` at line 6 column 3" },
        },
      };

    case "nocodex":
      return {
        codex: {
          homeExists: false,
          home: HOME_CODEX,
          configState: "missing",
          authState: "Missing",
          currentBaseUrl: null,
          currentModel: null,
          currentReasoningEffort: null,
          residue: [],
          managedByUs: false,
          activeProfileId: null,
          configurationStatus: "unconfigured" as const,
          statusMessage: "尚未找到用户配置或登录记录；这不代表客户端未安装",
        },
        claude: { ...claudeFresh, homeExists: false },
      };
  }
}

/** 预览：登录后「测试号」在控制台里已被停用 */
export function mockKeyStatus(): KeyStatus[] {
  return [
    { profileId: "p1", usable: true, missing: false, note: "" },
    { profileId: "p2", usable: false, missing: false, note: "已停用" },
  ];
}

export function mockTokens(): GatewayToken[] {
  return [
    { id: 1, name: "默认令牌", usable: true, note: "", linkedProfile: "主号" },
    { id: 2, name: "Codex 专用", usable: true, note: "", linkedProfile: null },
    { id: 3, name: "新令牌", usable: true, note: "", linkedProfile: null },
    { id: 4, name: "旧的测试令牌", usable: false, note: "已停用", linkedProfile: null },
  ];
}

/** 预览：网关目录。gpt-5 只在 Chat 接口上有渠道（演示「不支持 Codex」）；Claude 走 Messages */
export function mockCatalog(): ModelInfo[] {
  return mockTest().models.map(id => ({
    id,
    endpoints: id === "gpt-5" ? ["chat.completions"]
      : id.startsWith("gpt") ? ["chat.completions", "responses"]
      : id.startsWith("claude") ? ["chat.completions", "messages"]
      : ["chat.completions"],
  }));
}

export function mockTest(): TestResult {
  // 网关 /v1/models 返回的是这个 Key 能用的全部模型，跨厂商混在一起
  const models = [
    "gpt-6-astra", "gpt-5.6-sol", "gpt-5.3-codex", "gpt-5.1-codex-max", "gpt-5",
    "claude-opus-4-8", "claude-opus-4-8-thinking", "claude-sonnet-4-6",
    "claude-haiku-4-5-20251001",
    "gemini-2.5-pro", "deepseek-v3", "qwen3-max", "glm-4.6",
  ];
  return { ok: true, message: `连上了，可以用 ${models.length} 个模型`, models };
}

export function mockApply(req: ApplyRequest): ApplyResult {
  return {
    backupId: "20260918-183012",
    wroteCodexConfig: req.configureCodex,
    wroteCodexAuth: req.configureCodex,
    wroteClaudeSettings: req.configureClaude,
    wroteClaudeDesktop: !!req.configureClaudeDesktop,
    paths: [
      ...(req.configureCodex
        ? [`${HOME_CODEX}\\config.toml`, `${HOME_CODEX}\\auth.json`]
        : []),
      ...(req.configureClaude ? [`${HOME_CLAUDE}\\settings.json`] : []),
    ],
    warnings: ["检测到官方 ChatGPT 登录，已为你保留（密钥写在 config.toml 的 provider 段里）"],
  };
}

/** 体检假数据：broken 场景报文件损坏，legacy 场景报别名不全和环境变量覆盖 */
export function mockHealth(s: Scenario, req: HealthRequest): HealthIssue[] {
  if (s === "broken") return req.tool === "codex"
    ? [{ id: "X1", level: "error", text: "Codex 配置文件格式损坏", fix: "rebuild" }]
    : [{ id: "C1", level: "error", text: "Claude 配置文件格式损坏，需手动修复", fix: "none" }];
  // legacy 场景里，Claude 选了某把 AI 密钥时，模拟这把密钥已被停用
  if (s === "legacy" && req.tool === "claude" && req.service !== "current" && req.service !== "official")
    return [{ id: "K1", level: "error", text: "密钥无效或已停用", fix: "keys" }];
  if (s === "legacy") return req.tool === "claude"
    ? [{ id: "C2", level: "error", text: "模型别名不全，切换模型会报 404", fix: "rewrite" },
       { id: "E1", level: "warn", text: "系统环境变量会覆盖这里的配置", fix: "none" }]
    : [{ id: "K2", level: "error", text: "连不上服务器", fix: "settings" }];
  return [];
}
