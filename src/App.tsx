import { useCallback, useEffect, useRef, useState } from "react";
import * as api from "./api";
import type {
  ApplyResult,
  BackupEntry,
  Environment,
  GatewayToken,
  Profile,
  RemoteConfig,
  LaunchMode,
  ToolId,
  KeyStatus,
  ModelInfo,
} from "./types";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ClaudeIcon, CodexIcon } from "./BrandIcons";
import { LoginPanel } from "./LoginPanel";
import logoUrl from "./assets/logo.png";
import { BRAND } from "./brand";
import { Launcher, PickerDialog, type Page, type ToolAlerts } from "./Launcher";
import { UiIcon } from "./UiIcon";
import { MAX_LOG_LINES, clipLogText, type LogAction, type LogKind, type LogLine } from "./OutputConsole";
import { effortLabel, pickModel } from "./modelFamilies";
import { launchWithConfiguration } from "./launchFlow";
import { profileServiceUrl, serviceUrlFields } from "./serviceUrl";
import { getLang, t, useLang } from "./i18n";
import "./App.css";
import "./Launcher.css";
import "./LiquidGlass.css";

// value 是 Codex 认的英文值，label 是给用户看的。
// 源码里还有 none/max/ultra/persistent，但 none 会被网关过滤掉，
// 其余未进官方文档、上游未必支持，不放出来免得用户选了报错。
type AppId = "codex" | "claude";
const TOOL_ORDER: AppId[] = ["claude", "codex"];
type Busy = null | "scan" | "fetch" | "apply" | "restore" | "revert" | "save" | "launch";

/**
 * 从本工具打开的控制台带上来源：官网三处（官网、控制台、账号页）都会把 utm_* 记成首次来源，
 * 之后在这个浏览器里注册的账号，管理端就能看出是密钥管理工具带来的。
 */
const SITE_URL = `${BRAND.console}?utm_source=key-manager&utm_medium=app&utm_campaign=desktop`;

/** 用系统浏览器打开官网；开发预览时没有 Tauri，回落到 window.open */
async function openSite() {
  try {
    await openUrl(SITE_URL);
  } catch {
    window.open(SITE_URL, "_blank", "noopener,noreferrer");
  }
}

const APP_META: Record<
  AppId,
  { name: string; file: string; Icon: typeof CodexIcon }
> = {
  codex: {
    name: "Codex",
    file: "~/.codex/config.toml",
    Icon: CodexIcon,
  },
  claude: {
    name: "Claude Code",
    file: "~/.claude/settings.json",
    Icon: ClaudeIcon,
  },
};

/** 正在编辑的密钥表单 */
type Draft = {
  id: string;
  name: string;
  /** 用户新输入的 Key。留空 = 沿用已保存的那把 */
  apiKey: string;
  /** 这把密钥此前是否已存过值，决定提示怎么说 */
  hasKey: boolean;
  /** 已存那把的掩码，直接摆进输入框当占位，让用户一眼看见 */
  maskedKey: string;
  codexModel: string;
  codexEffort: string;
  claudeModel: string;
  serviceUrl: string;
};

/** 和 Rust 侧一致：取最小的未占用编号，而不是「个数+1」 */
function nextDefaultName(profiles: Profile[]): string {
  for (let n = 1; ; n++) {
    const candidate = t("密钥 {n}", { n });
    if (!profiles.some((p) => p.name === candidate)) return candidate;
  }
}

const emptyDraft = (): Draft => ({
  id: "",
  name: "",
  apiKey: "",
  hasKey: false,
  maskedKey: "",
  codexModel: "",
  codexEffort: "medium",
  claudeModel: "",
  serviceUrl: BRAND.apiBaseUrl,
});

const LAST_TOOL_KEY = "starnexus.lastToolPage";
const NAV_PAGES: Page[] = ["claude", "codex", "keys", "settings"];
/** 打开 App 时进入上次停留的页面；第一次打开默认进入密钥 */
function initialPage(): Page {
  try {
    const saved = localStorage.getItem(LAST_TOOL_KEY);
    return saved && NAV_PAGES.includes(saved as Page) ? saved as Page : "keys";
  } catch { return "keys"; }
}

export default function App() {
  // 切换语言时整棵界面重新渲染；窗口标题也跟着换
  const lang = useLang();
  useEffect(() => {
    document.title = t("StarNexus 密钥管理工具");
    // 窗口标题、托盘菜单由后端按语言设置；预览环境里没有后端，忽略失败
    api.setUiLanguage(lang).catch(() => {});
  }, [lang]);
  const [page, setPageState] = useState<Page>(initialPage);
  const setPage = (next: Page) => {
    setPageState(next);
    try { localStorage.setItem(LAST_TOOL_KEY, next); } catch { /* 记不住也不影响使用 */ }
  };
  /** 两个工具的体检概况，操作栏据此显示红点 / 黄点 */
  const [toolAlerts, setToolAlerts] = useState<ToolAlerts>({});
  const launchLock = useRef(false);

  const [env, setEnv] = useState<Environment | null>(null);
  const [remote, setRemote] = useState<RemoteConfig | null>(null);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [backups, setBackups] = useState<BackupEntry[]>([]);
  const [scanError, setScanError] = useState<string | null>(null);

  const [baseUrl, setBaseUrl] = useState("");

  const [aliasOpus, setAliasOpus] = useState("");
  const [aliasSonnet, setAliasSonnet] = useState("");
  const [aliasHaiku, setAliasHaiku] = useState("");

  const [cleanup, setCleanup] = useState(true);
  const [overwriteAuth, setOverwriteAuth] = useState(false);
  const [ignoreProxy, setIgnoreProxy] = useState(false);
  const [conservative, setConservative] = useState(false);
  const [showAdvanced, setShowAdvanced] = useState(false);

  /** 有值就是在编辑（id 为空表示新建） */
  const [draft, setDraft] = useState<Draft | null>(null);

  // 登录态提升到这里：登录一次，整个窗口周期内都能用 ——
  // 批量导入、之后再换 Key，都不必重新登录。
  const [loggedAs, setLoggedAs] = useState<string | null>(null);
  const [tokens, setTokens] = useState<GatewayToken[] | null>(null);
  const [showLogin, setShowLogin] = useState(false);
  /** 每把密钥的网关目录（模型 + 支持的接口），按密钥 id；没设模型的密钥会在后台读一次 */
  const [catalogs, setCatalogs] = useState<Record<string, ModelInfo[]>>({});
  const catalogRequested = useRef(new Set<string>());
  /** 登录后同步到的密钥状态，按密钥 id；没登录时为空 */
  const [keyStatus, setKeyStatus] = useState<Record<string, KeyStatus>>({});
  /** 上一次写进输出的同步结果，相同就不重复写 */
  const lastSync = useRef("");

  const [busy, setBusy] = useState<Busy>(null);
  /** 输出面板的记录（本次运行期间，最多保留 MAX_LOG_LINES 行） */
  const [logs, setLogs] = useState<LogLine[]>([]);
  const logId = useRef(0);
  const logBatch = useRef(0);
  /** 写一行输出。extra.group + batch：同一组里只有最新一批的行尾按钮可点 */
  const log = useCallback((kind: LogKind, text: string, extra?: { group?: string; batch?: number; actions?: LogAction[] }) => {
    // id 在更新函数外面生成：开发模式下更新函数会被调用两次，不能在里面自增
    const line: LogLine = { id: ++logId.current, time: new Date().toLocaleTimeString("zh-CN", { hour12: false }), kind, text: clipLogText(text), ...extra };
    setLogs(l => [...l, line].slice(-MAX_LOG_LINES));
  }, []);
  /** 清空时最后一行的 id：之后第一行之前的空缺才算「超出上限被丢掉」 */
  const clearedUpTo = useRef(0);
  const clearLogs = useCallback(() => { clearedUpTo.current = logId.current; setLogs([]); }, []);
  const nextBatch = useCallback(() => ++logBatch.current, []);
  /** 超出上限被丢掉的条数：第一行之前、上次清空之后的空缺 */
  const droppedLogs = logs.length ? logs[0].id - 1 - clearedUpTo.current : 0;
  /** 原来的提示条接口：现在写进输出面板；传 null 不再需要清空 */
  // 后端返回的固定提示也在译文表里，这里统一过一遍 t()；动态内容查不到就原样显示
  const setToast = useCallback((m: { kind: "ok" | "err"; text: string } | null) => { if (m) log(m.kind, t(m.text.replace(/^Error:\s*/, ""))); }, [log]);
  /** 写入配置的结果：写了哪些文件、备份号（可撤销）、警告 */
  const logResult = (r: ApplyResult) => {
    log("ok", t("配置已写入，改动前已备份（{id}）", { id: r.backupId }), { group: "undo", batch: nextBatch(), actions: [{ label: t("撤销这次改动"), run: () => void restore(r.backupId) }] });
    r.paths.forEach(path => log("path", path));
    r.warnings.forEach(w => log("warn", t(w)));
  };

  const codexKeepsAuth = env?.codex.authState === "OfficialLogin" || env?.codex.authState === "ExternalStore";

  /**
   * 重新读取密钥列表、备份，以及本机 Claude / Codex 配置。
   * withEnv 默认跟随「是否已同意读取」：没同意时只读本工具自己的数据。返回读到的配置。
   */
  /** 是否已同意读取本机配置（给 reload 用的同步副本） */
  const configReadRef = useRef<Record<ToolId, boolean>>({ claude: false, codex: false });
  const reload = useCallback(async (url?: string): Promise<Environment | null> => {
    // 只读用户同意过的工具；一个都没同意时不读本机配置，只读本工具自己的数据
    const tools = (["claude", "codex"] as const).filter(x => configReadRef.current[x]);
    try {
      const [e, ps, bs] = await Promise.all([
        tools.length ? api.scanEnvironment(url, [...tools]) : Promise.resolve(null),
        api.listProfiles(),
        api.listBackups().catch(() => [] as BackupEntry[]),
      ]);
      setEnv(e);
      setProfiles(ps);
      setBackups(bs);
      setScanError(null);
      return e;
    } catch (e) {
      setEnv(null);
      setScanError(String(e));
      return null;
    }
  }, []);
  /** 是否已同意读取本机配置；null = 还在读本工具的状态。首次使用时要用户点一下才开始读 */
  const [configRead, setConfigRead] = useState<Record<ToolId, boolean> | null>(null);
  /** 用户在某个工具页点了「读取本机配置」：只记下并读取这个工具 */
  const readConfig = async (tools: ToolId[]) => {
    await Promise.all(tools.map(x => api.markConfigRead(x).catch(() => {})));
    const next = { ...configReadRef.current };
    for (const x of tools) next[x] = true;
    configReadRef.current = next;
    setConfigRead(next);
    return reload(baseUrl || remote?.base_url || BRAND.apiBaseUrl);
  };

  useEffect(() => {
    (async () => {
      const st = await api.loadAppState().catch(() => null);
      if (st) {
        setIgnoreProxy(st.ignoreProxy);
        setCleanup(st.cleanupResidue ?? true);
      }

      const rc = await api.getRemoteConfig(st?.ignoreProxy ?? false);
      setRemote(rc);
      setBaseUrl(rc.base_url);
      setAliasOpus(rc.claude_opus_model);
      setAliasSonnet(rc.claude_sonnet_model);
      setAliasHaiku(rc.claude_haiku_model);

      // 首次使用（没有同意记录、也没有用过的痕迹）先不读本机配置，等用户在工具页点「读取」；
      // 老用户（已经有密钥或写过配置）直接读取，和以前一样
      const legacy = !!st && (st.configReadConsent || st.profiles.length > 0 || !!st.lastConfiguredAt);
      const consent = { claude: legacy || !!st?.configReadTools?.includes("claude"), codex: legacy || !!st?.configReadTools?.includes("codex") };
      configReadRef.current = consent;
      setConfigRead(consent);
      await reload(rc.base_url);
    })();
  }, [reload]);

  // 弹窗按 Esc 关掉，符合用户对弹窗的预期
  useEffect(() => {
    if (!showAdvanced) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setShowAdvanced(false);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [showAdvanced]);

  // —————— 密钥编辑 ——————

  const defaultServiceUrl = baseUrl || remote?.base_url || BRAND.apiBaseUrl;
  /** 所有服务共用一个密钥编辑入口。 */
  const startEdit = (p?: Profile) => {
    setToast(null);
    setDraft(
      p
        ? {
            id: p.id,
            name: p.name,
            apiKey: "",
            hasKey: p.hasKey,
            maskedKey: p.maskedKey,
            // 模型留空 = 跟随默认（按网关目录自动挑），编辑名称或 Key 时不要把默认值固化进去
            codexModel: p.codexModel,
            codexEffort: p.codexEffort || "medium",
            claudeModel: p.claudeModel,
            serviceUrl: profileServiceUrl(p, defaultServiceUrl),
          }
        : {
            ...emptyDraft(),
            name: nextDefaultName(profiles),
            codexModel: "",
            claudeModel: "",
            serviceUrl: defaultServiceUrl,
          },
    );
  };

  const doLogout = async () => {
    await api.gatewayLogout().catch(() => {});
    setLoggedAs(null);
    setTokens(null);
    setKeyStatus({});
    lastSync.current = "";
  };

  /**
   * 登录后同步控制台里的密钥状态：停用、删除的在密钥列表里标出来，并写进输出。
   * 只看从当前账号导入过的密钥；手动粘贴的不受影响。不登录也能正常使用全部功能。
   */
  const syncKeyStatus = async (account = loggedAs) => {
    if (!account) return;
    try {
      const list = await api.gatewayKeyStatus();
      setKeyStatus(Object.fromEntries(list.map(s => [s.profileId, s])));
      const ps = await api.listProfiles().catch(() => profiles);
      const bad = list.filter(s => !s.usable);
      const sig = list.map(s => `${s.profileId}:${s.usable}:${s.missing}`).join(",");
      if (sig === lastSync.current) return;
      lastSync.current = sig;
      const batch = nextBatch();
      log("cmd", `${t("同步控制台密钥状态")} · ${account}`, { group: "sync", batch });
      if (!list.length) log("info", t("本地还没有从这个账号导入的密钥"), { group: "sync", batch });
      else if (!bad.length) log("ok", t("{n} 把导入的密钥都可用", { n: list.length }), { group: "sync", batch });
      for (const s of bad) {
        const name = ps.find(p => p.id === s.profileId)?.name ?? t("一把密钥");
        log("warn", t("「{name}」在控制台{state}，请换一把或到控制台处理", { name, state: s.missing ? t("已被删除") : t(s.note || "已停用") }), {
          group: "sync", batch, actions: [{ label: t("去控制台 ↗"), run: () => void openSite() }],
        });
      }
    } catch (e) {
      log("err", t("同步密钥状态失败：{error}", { error: t(String(e).replace(/^Error:\s*/, "")) }));
    }
  };
  // 登录后、以及登录状态下回到密钥页时同步一次
  useEffect(() => {
    if (loggedAs && page === "keys") void syncKeyStatus(loggedAs);
  }, [loggedAs, page]);

  const saveDraft = async () => {
    if (!draft) return;
    if (!draft.hasKey && !draft.apiKey.trim()) {
      return setToast({ kind: "err", text: t("请先填上 API Key") });
    }
    setBusy("save");
    try {
      const urls = serviceUrlFields(draft.serviceUrl, defaultServiceUrl, profiles.find(p => p.id === draft.id));
      await api.saveProfile({
        id: draft.id,
        name: draft.name.trim(),
        apiKey: draft.apiKey.trim(),
        codexModel: draft.codexModel.trim(),
        codexEffort: draft.codexEffort,
        claudeModel: draft.claudeModel.trim(),
        ...urls,
      });
      setProfiles(await api.listProfiles().catch(() => profiles));
      setDraft(null);
      setToast({ kind: "ok", text: t("已保存") });
    } catch (e) {
      setToast({ kind: "err", text: String(e) });
    } finally {
      setBusy(null);
    }
  };

  /**
   * 删除一把密钥。二次确认走界面内的两步按钮，不用 window.confirm ——
   * Tauri 的 WebView 里原生对话框被禁用，confirm 会直接返回 false，
   * 导致删除静默失效。
   */
  const removeProfile = async (p: Profile) => {
    try {
      await api.deleteProfile(p.id);
      setDraft(null);
      await reload(baseUrl);
      setToast({ kind: "ok", text: t("已删除「{name}」", { name: p.name }) });
    } catch (e) {
      setToast({ kind: "err", text: String(e) });
    }
  };

  // —————— 启用 ——————

  /** 把一把密钥写进某个工具的用户配置（不启动客户端） */
  const enable = async (profileId: string, tool: AppId, rebuild = false, desktop = false): Promise<boolean> => {
    const p = profiles.find((x) => x.id === profileId);
    const claudeAliases = p?.custom
      ? { opus: "", sonnet: "", haiku: "" }
      : { opus: aliasOpus, sonnet: aliasSonnet, haiku: aliasHaiku };
    setBusy("apply");
    log("cmd", t(rebuild ? "重建 {tool} 配置" : "写入 {tool} 配置", { tool: APP_META[tool].name }) + ` · ${p?.name ?? t("所选密钥")}`);
    try {
      const r = await (rebuild ? api.rebuildMinimalConfig : api.applyConfig)({
        profileId,
        baseUrl: baseUrl || remote?.base_url || "",
        wireApi: remote?.wire_api || "responses",
        ignoreProxy,
        configureCodex: tool === "codex",
        model: modelFor("codex", p),
        reasoningEffort: p?.codexEffort || "medium",
        cleanupResidue: cleanup,
        overwriteOfficialAuth: overwriteAuth,
        conservative,
        // Claude 桌面版和 CLI 是两份配置：体检里「修复」按当前启动方式写对应的那份
        configureClaude: tool === "claude" && !desktop,
        configureClaudeDesktop: tool === "claude" && desktop,
        claudeModel: modelFor("claude", p),
        claudeOpusModel: claudeAliases.opus,
        claudeSonnetModel: claudeAliases.sonnet,
        claudeHaikuModel: claudeAliases.haiku,
      });
      logResult(r);
      setToast({
        kind: "ok",
        text: t("{tool} 已改用「{name}」；请重新打开客户端，实际连接尚未验证", { tool: APP_META[tool].name, name: p?.name ?? t("所选密钥") }),
      });
      await reload(baseUrl);
      return true;
    } catch (e) {
      setToast({ kind: "err", text: String(e) });
      return false;
    } finally {
      setBusy(null);
    }
  };

  const launch = async (tool: ToolId, mode: LaunchMode, service: string, directory: string): Promise<boolean> => {
    if (launchLock.current || busy) return false;
    launchLock.current = true;
    setBusy("launch");
    let changed = false;
    try {
      const p = profiles.find(profile => profile.id === service);
      const claudeAliases = p?.custom
        ? { opus: "", sonnet: "", haiku: "" }
        : { opus: aliasOpus, sonnet: aliasSonnet, haiku: aliasHaiku };
      log("cmd", t("启动 {name}", { name: `${tool === "claude" ? "Claude" : "Codex"} ${mode === "cli" ? "CLI" : t("桌面版")}` }) + ` · ${service === "current" ? t("保持现有配置") : service === "official" ? t("官方服务") : p?.name ?? t("所选密钥")}`);
      if (!["current", "official"].includes(service) && !p?.hasKey) throw new Error(t("所选密钥不可用，请重新选择"));
      const receipt = await launchWithConfiguration(tool, mode, service, directory, {
        profileId: service,
        baseUrl: baseUrl || remote?.base_url || BRAND.apiBaseUrl,
        wireApi: remote?.wire_api || "responses", ignoreProxy,
        configureCodex: tool === "codex", configureClaude: tool === "claude",
        model: modelFor("codex", p), reasoningEffort: p?.codexEffort || "medium",
        cleanupResidue: false, overwriteOfficialAuth: false, conservative: false,
        claudeModel: modelFor("claude", p),
        claudeOpusModel: claudeAliases.opus, claudeSonnetModel: claudeAliases.sonnet, claudeHaikuModel: claudeAliases.haiku,
      }, api, r => { changed = true; logResult(r); });
      setToast({ kind: "ok", text: receipt.message });
      return true;
    } catch (e) {
      setToast({ kind: "err", text: t(String(e).replace(/^Error:\s*/, "")) + (changed ? t("（配置已保存，可在上方撤销；尚未确认客户端运行）") : "") });
      return false;
    } finally {
      await reload(baseUrl);
      setBusy(null); launchLock.current = false;
    }
  };

  /**
   * 某把密钥给某个工具用的模型：单独设置过就用它；没设置时用默认模型，
   * 但默认模型这把密钥用不了（比如只在 Chat 接口上有渠道）时，按网关目录换一个能用的。
   */
  const modelFor = (tool: ToolId, p: Profile | undefined): string => {
    if (!p) return "";
    const explicit = tool === "codex" ? p.codexModel : p.claudeModel;
    if (explicit) return explicit;
    // 其他服务：没选模型就不写，由客户端用它自己的默认模型（AI 的默认模型名在别家不一定有）
    if (p.custom) return "";
    const preferred = (tool === "codex" ? remote?.default_model : remote?.claude_default_model) || "";
    return pickModel(tool, preferred, catalogs[p.id]);
  };
  // 没设模型的密钥：后台读一次网关目录（GET，不计费），好让上面挑到这把密钥确实能用的模型
  useEffect(() => {
    if (!remote) return;
    for (const p of profiles) {
      if (!p.hasKey || p.custom || (p.codexModel && p.claudeModel) || catalogRequested.current.has(p.id)) continue;
      catalogRequested.current.add(p.id);
      api.listProfileModels(p.id, baseUrl || remote.base_url || BRAND.apiBaseUrl, ignoreProxy)
        .then(list => setCatalogs(c => ({ ...c, [p.id]: list })))
        .catch(() => { /* 读不到目录就用默认模型，体检会提示 */ });
    }
  }, [profiles, remote]);

  /** 体检：参数与启动时一致，密钥由后端自己从密钥库取 */
  const checkHealth = (tool: ToolId, service: string, mode: LaunchMode) => {
    const p = profiles.find(x => x.id === service);
    const model = p ? modelFor(tool, p) : "";
    return api.healthCheck({ tool, mode, service, model, baseUrl: baseUrl || remote?.base_url || BRAND.apiBaseUrl, ignoreProxy, opusModel: aliasOpus, sonnetModel: aliasSonnet, haikuModel: aliasHaiku });
  };
  const diagnose = (tool: ToolId, mode: LaunchMode, service: string, profileId: string, note: string) => {
    const p = profiles.find(x => x.id === service);
    const model = p ? modelFor(tool, p) : "";
    return api.aiDiagnose({ tool, mode, service, model, profileId, note, lang: getLang(), baseUrl: baseUrl || remote?.base_url || BRAND.apiBaseUrl, ignoreProxy, opusModel: aliasOpus, sonnetModel: aliasSonnet, haikuModel: aliasHaiku });
  };
  const fixHealth = async (tool: ToolId, profileId: string | null, fix: "rewrite" | "rebuild" | "keys" | "settings", mode: LaunchMode = "cli"): Promise<boolean> => {
    if (fix === "rebuild" && tool !== "codex") {
      setToast({ kind: "err", text: t("重建配置仅支持 Codex") });
      return false;
    }
    if (fix === "keys") { setPage("keys"); return false; }
    if (fix === "settings") { setShowAdvanced(true); return false; }
    if (profileId && !busy) return enable(profileId, tool, fix === "rebuild", tool === "claude" && mode === "desktop");
    return false;
  };

  const restore = async (id: string) => {
    setBusy("restore");
    try {
      log("cmd", `${t("还原配置")} · ${id}`);
      await api.restoreBackup(id);
      setLogs(l => l.map(x => x.group === "undo" ? { ...x, actions: undefined } : x));
      setToast({ kind: "ok", text: t("已还原到 {id} 那一次的配置", { id }) });
      await reload(baseUrl);
    } catch (e) {
      setToast({ kind: "err", text: String(e) });
    } finally {
      setBusy(null);
    }
  };

  /** 工具页「更换模型」：只改这把密钥给某个工具用的模型，Key 和名称不动 */
  const saveModel = async (id: string, patch: { claudeModel?: string; codexModel?: string; codexEffort?: string }) => {
    const p = profiles.find(x => x.id === id);
    if (!p) return false;
    setBusy("save");
    log("cmd", `${t("更换模型")} · ${p.name} → ${patch.claudeModel ?? patch.codexModel ?? ""}${patch.codexEffort ? t("（思考深度 {effort}）", { effort: effortLabel(patch.codexEffort) }) : ""}`);
    try {
      await api.saveProfile({
        id, name: p.name, apiKey: "",
        codexModel: p.codexModel, codexEffort: p.codexEffort || "medium", claudeModel: p.claudeModel,
        ...patch,
      });
      setProfiles(await api.listProfiles().catch(() => profiles));
      setToast({ kind: "ok", text: t("模型已保存，启动时写入配置") });
      return true;
    } catch (e) {
      setToast({ kind: "err", text: String(e) });
      return false;
    } finally {
      setBusy(null);
    }
  };
  /** 把配置文件里正在用的 AI 密钥收进密钥列表 */
  const adoptKey = async (tool: ToolId) => {
    if (busy) return;
    setBusy("save");
    log("cmd", `${t("收进密钥列表")} · ${t("{tool} 配置里的密钥", { tool: tool === "claude" ? "Claude" : "Codex" })}`);
    try {
      const r = await api.adoptCurrentKey(tool, baseUrl || remote?.base_url || BRAND.apiBaseUrl, ignoreProxy);
      log("ok", r.existed ? t("这把密钥已经在列表里：「{name}」", { name: t(r.name) }) : t("已收进密钥列表：「{name}」，测试可用", { name: t(r.name) }));
      await reload(baseUrl);
    } catch (e) {
      log("err", t(String(e).replace(/^Error:\s*/, "")));
    } finally {
      setBusy(null);
    }
  };
  /** 这把密钥能用的模型和各自支持的接口（网关目录）。顺手缓存，给自动挑模型用 */
  const listModels = async (id: string) => {
    const list = await api.listProfileModels(id, baseUrl || remote?.base_url || BRAND.apiBaseUrl, ignoreProxy);
    setCatalogs(c => ({ ...c, [id]: list }));
    return list;
  };

  /** 某把密钥正被哪些工具使用（只读标记；切换在页面顶部的「正在使用」里做） */
  const usedBy = (profileId: string) =>
    TOOL_ORDER.filter((tool) => env?.[tool].activeProfileId === profileId).map((tool) => {
      const Icon = APP_META[tool].Icon;
      return (
        <span key={tool} className="in-use" title={t("{tool} 正在使用这把密钥", { tool: APP_META[tool].name })} aria-label={t("{tool} 在用", { tool: APP_META[tool].name })}>
          <Icon className={`brand brand-${tool}`} />
          {t("在用")}
        </span>
      );
    });

  return (
    <div className="launcher-app">
      <div className="workspace-shell">
      <nav className="workspace-nav" aria-label={t("主导航")}>
        <div className="nav-pill liquid-glass">
          <span className="nav-lens" aria-hidden="true" style={{ transform: `translateX(${NAV_PAGES.indexOf(page) * 100}%)` }} />
          {NAV_PAGES.map(id => {
            const label = { claude: "Claude", codex: "Codex", keys: t("密钥"), settings: t("设置") }[id];
            const alert = id === "claude" || id === "codex" ? toolAlerts[id] : undefined;
            const name = id === "keys" && profiles.length ? `${label}（${profiles.length}）`
              : alert ? `${label}（${alert === "error" ? t("有问题") : t("有提醒")}）` : label;
            return <button key={id} className="nav-tab" aria-label={name} aria-current={page === id ? "page" : undefined} disabled={busy !== null} onClick={() => { setPage(id); document.querySelector(".workspace-main")?.scrollTo({ top: 0 }); }}>
              <span className="nav-icon">
                {id === "claude" ? <ClaudeIcon className="brand brand-claude"/> : id === "codex" ? <CodexIcon className="brand brand-codex"/> : <UiIcon name={id === "keys" ? "key" : "settings"}/>}
                {id === "keys" && profiles.length > 0 && <span className="nav-badge" aria-hidden="true">{profiles.length}</span>}
                {alert && <span className={`nav-alert ${alert}`} aria-hidden="true"/>}
              </span>
              <span className="nav-label">{label}</span>
            </button>;
          })}
          <button type="button" className="nav-tab" onClick={openSite} title={t("在浏览器打开 AI 开发者控制台")}>
            <span className="nav-icon"><UiIcon name="grid"/></span>
            <span className="nav-label">{t("控制台")}</span>
          </button>
        </div>
      </nav>
      {/* 导航独占布局区域，只有主内容滚动，短窗口也不会遮挡操作。 */}
      <main className={`workspace-main ${page === "claude" || page === "codex" ? "fill" : ""}`}>
      <Launcher page={page} busy={busy !== null} profiles={profiles} env={env}
        onLaunch={launch} onKeys={() => setPage("keys")}
        onSettings={() => setPage("settings")} onAdvanced={() => setShowAdvanced(true)}
        onRefresh={async () => { await reload(baseUrl); }} checkHealth={checkHealth} onFix={fixHealth} diagnose={diagnose} onAlerts={setToolAlerts} onConsole={openSite}
        modelFor={modelFor} catalogKey={Object.keys(catalogs).join()} onSaveModel={saveModel} listModels={listModels}
        logs={logs} log={log} nextBatch={nextBatch} onClearLogs={clearLogs} droppedLogs={droppedLogs} onPage={setPage} onAdopt={adoptKey}
        onLoginImport={() => { setPage("keys"); setShowLogin(true); }} onManualKey={() => { setPage("keys"); startEdit(); }}
        configRead={configRead} onReadConfig={readConfig} />
      {(page === "claude" || page === "codex") && scanError && <div className="toast err" role="alert">{t("配置读取失败：{error}", { error: t(scanError) })}</div>}
      {page === "keys" && <div className="key-workspace">
      <div className="launch-heading key-heading"><h1>{t("密钥")}</h1>{profiles.length > 0 && <div className="key-actions"><button className="primary" onClick={() => setShowLogin(true)} disabled={busy !== null}><UiIcon name="key"/>{t("登录导入")}</button><button onClick={() => startEdit()} disabled={busy !== null}>{t("手动添加")}</button></div>}</div>

      {scanError && <div className="toast err">{t("读取本机设置时出错：{error}", { error: t(scanError) })}</div>}
      {remote?.notice && <div className="notice">{remote.notice}</div>}

      {/* 密钥列表：只管增删改；哪个工具用哪把，在 Claude / Codex 页面里选 */}
      {/* 控制台入口统一放在底部导航，这里不再重复 */}
      {profiles.length > 0 && <div className="key-section-head"><h2 className="key-section">{t("我的密钥")}</h2></div>}
      {profiles.length > 0 && <section className="card key-list">
      {profiles.map((p) => {
        return (
          <article key={p.id} className="key-row">
            <div className="card-hd">
              <span className="avatar" title={hostOf(profileServiceUrl(p, defaultServiceUrl))}>
                {providerIcon(profileServiceUrl(p, defaultServiceUrl))}
              </span>
              <div className="card-title">
                <strong>{p.name}{p.custom && <span className="key-kind">{t("其他服务")}</span>}</strong>
                <span className="card-sub mono">{p.maskedKey || t("还没填 API Key")}{p.custom && ` · ${hostOf(p.claudeBaseUrl || p.codexBaseUrl)}`}</span>
              </div>
              {keyStatus[p.id] && !keyStatus[p.id].usable && (
                <span className="key-flag" title={t("控制台里的状态，登录后同步")}>{keyStatus[p.id].missing ? t("控制台已删除") : t(keyStatus[p.id].note || "已停用")}</span>
              )}
              {usedBy(p.id)}
              <button className="ghost sm" onClick={() => startEdit(p)} disabled={busy !== null}>
                {t("编辑")}
              </button>
            </div>
          </article>
        );
      })}
      </section>}

      {/* 编辑 / 新建：弹窗 */}
      {draft && (() => {
        const editingProfile = draft.id ? profiles.find(p => p.id === draft.id) : undefined;
        return (
          <Editor
            title={editingProfile ? t("编辑「{name}」", { name: editingProfile.name }) : t("添加密钥")}
            hint={editingProfile ? undefined : t("保存后在 Claude / Codex 页面选用，模型也在那里设置")}
            draft={draft}
            defaultServiceUrl={defaultServiceUrl}
            setDraft={setDraft}
            busy={busy}
            onCancel={() => setDraft(null)}
            onSave={saveDraft}
            onDelete={editingProfile ? () => removeProfile(editingProfile) : undefined}
          />
        );
      })()}

      {showLogin && (
        <LoginPanel
          loggedAs={loggedAs}
          tokens={tokens}
          ignoreProxy={ignoreProxy}
          busy={busy !== null}
          onLoggedIn={(u, list) => {
            setLoggedAs(u);
            setTokens(list);
            void syncKeyStatus(u);
          }}
          onLogout={doLogout}
          onImported={async (created, updated, unchanged, skipped) => {
            setProfiles(await api.listProfiles().catch(() => profiles));
            void syncKeyStatus();
            const parts: string[] = [];
            if (created.length) parts.push(t("新增 {n} 个密钥", { n: created.length }));
            if (updated.length) parts.push(t("更新 {n} 个", { n: updated.length }));
            if (unchanged.length) parts.push(t("{n} 个没有变化", { n: unchanged.length }));
            if (skipped.length) parts.push(t("{n} 个取不到，跳过", { n: skipped.length }));
            setToast({ kind: "ok", text: parts.join(t("，")) || t("没有可导入的") });
            setShowLogin(false);
          }}
          onManualKey={() => { setShowLogin(false); setPage("keys"); startEdit(); }}
          onClose={() => setShowLogin(false)}
        />
      )}

      {!draft && !showLogin && profiles.length === 0 && (
        // 一把密钥都没有：这是用户唯一该做的事，给它主按钮的分量，
        // 别让它输给上面那张不需要操作的「官方」卡。
        <div className="empty">
          <div className="empty-title">{t("先添加一把密钥")}</div>
          <div className="empty-sub">
            {t("支持 AI 控制台导入或手动添加其他渠道的密钥。")}
          </div>
          <div className="row">
            <button
              className="primary"
              onClick={() => setShowLogin(true)}
              disabled={busy !== null}
            >
              {t("登录导入")}{loggedAs ? t("（已登录 {name}）", { name: loggedAs }) : ""}
            </button>
            <button className="ghost" onClick={() => startEdit()} disabled={busy !== null}>
              {t("手动添加")}
            </button>
          </div>
        </div>
      )}

      </div>}

      {showAdvanced && (
        <div
          className="overlay"
          onMouseDown={(e) => {
            // 只有点在遮罩本身才关，点内容不关
            if (e.target === e.currentTarget) setShowAdvanced(false);
          }}
        >
          <div className="modal" role="dialog" aria-modal="true" aria-label={t("高级选项")}>
            <div className="modal-hd">
              <strong>{t("高级选项")}</strong>
              <button className="ghost sm" onClick={() => setShowAdvanced(false)}>
                {t("关闭")}
              </button>
            </div>
            <div className="modal-body">
          <div className="field">
            <label>
              {t("服务地址")}
              <span className="dim"> · {t("一般不用改")}</span>
            </label>
            <input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} spellCheck={false} />
          </div>
          <div className="field">
            <label>
              {t("Claude Code 里 haiku / sonnet / opus 分别用哪个模型")}
              <span className="dim"> · {t("影响后台杂事和快捷切换，留空就不设置")}</span>
            </label>
            <input value={aliasHaiku} onChange={(e) => setAliasHaiku(e.target.value)} placeholder="haiku" spellCheck={false} />
            <input value={aliasSonnet} onChange={(e) => setAliasSonnet(e.target.value)} placeholder="sonnet" spellCheck={false} />
            <input value={aliasOpus} onChange={(e) => setAliasOpus(e.target.value)} placeholder="opus" spellCheck={false} />
          </div>
          <label className="check">
            <input type="checkbox" checked={ignoreProxy} onChange={(e) => setIgnoreProxy(e.target.checked)} />
            <span>
              {t("不走系统代理")}
              <span className="dim"> · {t("连不上、一直转圈时勾这个")}</span>
            </span>
          </label>
          <label className="check">
            <input type="checkbox" checked={conservative} onChange={(e) => setConservative(e.target.checked)} />
            <span>
              {t("Codex 兼容旧版本")}
              <span className="dim"> · {t("配好后 Codex 还提示没登录时勾这个")}</span>
            </span>
          </label>
          {codexKeepsAuth && (
            <label className="check">
              <input type="checkbox" checked={overwriteAuth} disabled={env?.codex.authState === "ExternalStore"} onChange={(e) => setOverwriteAuth(e.target.checked)} />
              <span>
                {t("写入 Codex 时不保留已有登录")}
                <span className="dim"> · {env?.codex.authState === "ExternalStore" ? t("Codex 使用独立认证存储，不能勾选") : t("检测到 ChatGPT 本地登录，勾了会退出官方登录")}</span>
              </span>
            </label>
          )}
          {!!env?.codex.residue.length && (
            <label className="check">
              <input type="checkbox" checked={cleanup} onChange={(e) => setCleanup(e.target.checked)} />
              <span>
                {t("写入时顺手整理 Codex 配置文件")}
                <span className="dim"> · {env.codex.residue.map(r => t(r)).join(t("、"))}</span>
              </span>
            </label>
          )}

          {backups.length > 0 && (
            <div className="backups">
              <label>{t("备份记录 · 出问题时可以退回")}</label>
              {backups.map((b) => (
                <div key={b.id} className="backup-row">
                  <span className="mono dim">{b.created_at}</span>
                  <button className="ghost sm" onClick={() => restore(b.id)} disabled={busy !== null}>
                    {t("还原")}
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
          </div>
        </div>
      )}
      </main>
      </div>
    </div>
  );
}

// ——————————————————————————————————————————————

function Editor(props: {
  title: string;
  hint?: string;
  draft: Draft;
  defaultServiceUrl: string;
  setDraft: (d: Draft) => void;
  busy: Busy;
  onCancel: () => void;
  onSave: () => void;
  onDelete?: () => void;
}) {
  const { title, hint, draft: d, setDraft, busy, onCancel, onSave, onDelete } = props;

  const [showKey, setShowKey] = useState(false);
  const [editingUrl, setEditingUrl] = useState(false);
  const [urlError, setUrlError] = useState("");
  const save = () => {
    try {
      serviceUrlFields(d.serviceUrl, props.defaultServiceUrl);
      setUrlError("");
      onSave();
    } catch (error) {
      setEditingUrl(true);
      setUrlError(t((error as Error).message));
    }
  };
  /** 用户是不是正在输入一把新的 Key */
  const typing = d.apiKey.length > 0;

  return (
    // 点遮罩不关闭，免得误触丢掉填了一半的内容；按 Esc 或「取消」关闭
    <PickerDialog title={title} onClose={onCancel} keepOnOverlay className="key-editor"
      footer={<EditorActions busy={busy} onCancel={onCancel} onSave={save} onDelete={onDelete} />}>
      {hint && <p className="settings-note">{hint}</p>}
      <div className="grid2">
        <div className="field">
          <label>
            {t("名称")}
            <span className="dim"> · {t("只是给你自己看的")}</span>
          </label>
          <input
            value={d.name}
            onChange={(e) => setDraft({ ...d, name: e.target.value })}
            placeholder={t("例如：主号 / 测试号")}
            spellCheck={false}
          />
        </div>
        <div className="field">
          <label>
            API Key
            {d.hasKey && !typing ? (
              <span className="ok-text"> · {t("已保存，不用再填")}</span>
            ) : typing ? (
              <span className="dim"> · {t("保存后会替换掉原来那把")}</span>
            ) : (
              <span className="warn-text"> · {t("还没填")}</span>
            )}
          </label>
          <div className="row">
            <input
              type={showKey || (d.hasKey && !typing) ? "text" : "password"}
              value={typing ? d.apiKey : d.hasKey ? d.maskedKey : d.apiKey}
              onChange={(e) => setDraft({ ...d, apiKey: e.target.value })}
              onFocus={() => {
                if (d.hasKey && !typing) setDraft({ ...d, apiKey: "" });
              }}
              placeholder={d.hasKey ? "" : t("粘贴你的 API Key")}
              // 新建时名称已有默认值，焦点直接落在要粘贴的 Key 上
              data-autofocus={d.hasKey ? undefined : true}
              spellCheck={false}
              autoComplete="off"
              className={d.hasKey && !typing ? "mono placeholderish" : undefined}
            />
            {typing && (
              <button className="ghost" onClick={() => setShowKey(!showKey)}>
                {showKey ? t("隐藏") : t("显示")}
              </button>
            )}
            {d.hasKey && typing && (
              <button
                className="ghost"
                onClick={() => setDraft({ ...d, apiKey: "" })}
                title={t("放弃修改，继续用原来那把")}
              >
                {t("撤销")}
              </button>
            )}
          </div>
        </div>
      </div>
      <div className="field">
        <label htmlFor={editingUrl ? "key-service-url" : undefined}>{t("服务 URL")}</label>
        {editingUrl ? <input id="key-service-url" className="mono" type="url" value={d.serviceUrl}
          onChange={e => { setDraft({ ...d, serviceUrl: e.target.value }); setUrlError(""); }}
          placeholder={props.defaultServiceUrl} spellCheck={false} autoComplete="off" autoFocus
          disabled={busy !== null} aria-invalid={!!urlError} aria-describedby={urlError ? "key-service-url-error" : undefined} />
          : <button type="button" className="service-url-picker" disabled={busy !== null} onClick={() => setEditingUrl(true)}>
            <span className="mono">{d.serviceUrl}</span><span>{t("更换")}</span>
          </button>}
        {urlError && <span id="key-service-url-error" className="warn-text" role="alert">{urlError}</span>}
      </div>
    </PickerDialog>
  );
}

/** 地址里的主机名，列表里给其他服务标一下是哪家 */
function hostOf(url: string): string {
  try { return new URL(url).host; } catch { return url; }
}

function providerIcon(url: string) {
  const host = hostOf(url).toLowerCase();
  if (host.includes("anthropic.com")) return <ClaudeIcon className="brand brand-claude" />;
  if (host.includes("openai.com")) return <CodexIcon className="brand brand-codex" />;
  if (host === "dkby.com" || host.endsWith(".dkby.com")) return <img src={logoUrl} alt="" />;
  return <UiIcon name="globe" />;
}

function EditorActions(props: { busy: Busy; onCancel: () => void; onSave: () => void; onDelete?: () => void }) {
  const { busy, onCancel, onSave, onDelete } = props;
  const [confirmDelete, setConfirmDelete] = useState(false);
  return (
    <div className={`row end editor-actions${confirmDelete ? " is-confirming" : ""}`}>
      {onDelete && confirmDelete ? (
        <div className="delete-confirm">
          <span className="delete-confirm-note">{t("仅删除本工具保存的密钥；已写入的客户端配置仍保留")}</span>
          <div className="delete-confirm-actions">
            <button className="ghost" onClick={() => setConfirmDelete(false)} disabled={busy !== null}>
              {t("不删了")}
            </button>
            <button
              className="danger"
              onClick={() => {
                setConfirmDelete(false);
                onDelete();
              }}
              disabled={busy !== null}
            >
              {t("确认删除")}
            </button>
          </div>
        </div>
      ) : (
        <>
          {onDelete && <button className="ghost danger-text push-left" onClick={() => setConfirmDelete(true)} disabled={busy !== null}>{t("删除")}</button>}
          <button className="ghost" onClick={onCancel} disabled={busy !== null}>{t("取消")}</button>
          <button className="primary" onClick={onSave} disabled={busy !== null}>{busy === "save" ? t("保存中…") : t("保存")}</button>
        </>
      )}
    </div>
  );
}
