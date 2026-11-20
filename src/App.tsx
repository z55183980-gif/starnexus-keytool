import { useCallback, useEffect, useRef, useState } from "react";
import * as api from "./api";
import type {
  ApplyResult,
  BackupEntry,
  Environment,
  GatewayToken,
  Profile,
  LaunchMode,
  ToolId,
  KeyStatus,
  LaunchRequest,
  LaunchTarget,
  ModelInfo,
} from "./types";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ClaudeIcon, CodexIcon } from "./BrandIcons";
import type { ImportDialog } from "./ConnectionGuide";
import { LoginPanel } from "./LoginPanel";
import { WelcomeGuide } from "./WelcomeGuide";
import { ConfirmDialog, DESKTOP_NAME, RestartDialog, mergePrompt, type RestartPrompt } from "./RestartDialog";
import type { ApplyKey, ApplyOutcome } from "./ConnectStep";
import { ONBOARDED_KEY, needsOnboarding } from "./onboarding";
import logoUrl from "./assets/logo.png";
import { BRAND } from "./brand";
import { Launcher, PickerDialog, type Page, type ToolAlerts } from "./Launcher";
import { UiIcon } from "./UiIcon";
import { MAX_LOG_LINES, clipLogText, type LogAction, type LogKind, type LogLine } from "./OutputConsole";
import { effortLabel, pickModel } from "./modelFamilies";
import { launchWithConfiguration, saveLaunchConfiguration } from "./launchFlow";
import { profileServiceUrl, serviceUrlFields } from "./serviceUrl";
import { t, useLang } from "./i18n";
import { UpdateBanner, useUpdates } from "./Updates";
import "./App.css";
import "./Launcher.css";
import "./LiquidGlass.css";
import "./Repair.css";

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
const NAV_PAGES = ["claude", "codex", "keys", "settings"] as const;
function initialPage(): Page {
  try {
    const saved = localStorage.getItem(LAST_TOOL_KEY);
    return NAV_PAGES.find(item => item === saved) ?? "welcome";
  } catch { return "welcome"; }
}

export default function App() {
  const updates = useUpdates();
  // 切换语言时整棵界面重新渲染；窗口标题也跟着换
  const lang = useLang();
  useEffect(() => {
    document.title = t("StarNexus 密钥管理工具");
    // 窗口标题、托盘菜单由后端按语言设置；预览环境里没有后端，忽略失败
    api.setUiLanguage(lang).catch(() => {});
  }, [lang]);
  const [page, setPageState] = useState<Page>(initialPage);
  /** 首次打开的引导；完成或跳过后记下，之后不再出现 */
  const [onboarding, setOnboarding] = useState(() => { try { return needsOnboarding(localStorage, location.search); } catch { return false; } });
  const finishOnboarding = () => { try { localStorage.setItem(ONBOARDED_KEY, "1"); } catch { /* 记不住下次再看一遍也无妨 */ } setOnboarding(false); };
  const setPage = (next: Page) => {
    setPageState(next);
    try { localStorage.setItem(LAST_TOOL_KEY, next); } catch { /* 记不住也不影响使用 */ }
  };
  /** 两个工具的体检概况，操作栏据此显示红点 / 黄点 */
  const [toolAlerts, setToolAlerts] = useState<ToolAlerts>({});
  const launchLock = useRef(false);

  const [env, setEnv] = useState<Environment | null>(null);
  const remote = updates.state.policy!.config;
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

  const [guideWorking, setGuideWorking] = useState(false);
  const [busy, setBusy] = useState<Busy>(null);
  useEffect(() => { updates.setBusy(busy !== null || guideWorking); return () => updates.setBusy(false); }, [busy, guideWorking, updates.setBusy]);
  useEffect(() => { updates.setIgnoreProxy(ignoreProxy); }, [ignoreProxy, updates.setIgnoreProxy]);
  /** 输出面板的记录（本次运行期间，最多保留 MAX_LOG_LINES 行） */
  const [logs, setLogs] = useState<LogLine[]>([]);
  const logId = useRef(0);
  const logBatch = useRef(0);
  /** 写一行输出。extra.group + batch：同一组里只有最新一批的行尾按钮可点 */
  const log = useCallback((kind: LogKind, text: string, extra?: { group?: string; batch?: number; actions?: LogAction[]; undo?: () => void }) => {
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
  /** 最近一条错误提示：引导页没有输出面板，写入失败时拿它告诉用户原因 */
  const lastError = useRef("");
  const setToast = useCallback((m: { kind: "ok" | "err"; text: string } | null) => { if (m) { const text = t(m.text.replace(/^Error:\s*/, "")); if (m.kind === "err") lastError.current = text; log(m.kind, text); } }, [log]);
  /** 写入后的弹窗：运行中的桌面版要重启才读新配置 */
  const [restartPrompt, setRestartPrompt] = useState<RestartPrompt | null>(null);
  const promptRestart = useCallback(async (targets: LaunchTarget[]) => {
    const running = targets.length ? await api.runningDesktopClients(targets).catch(() => [] as LaunchTarget[]) : [];
    if (running.length) setRestartPrompt(prev => mergePrompt(prev, { targets: running }));
  }, []);
  /** 通用确认弹窗：返回用户是否确认 */
  type ConfirmAsk = { title: string; body: string; note?: string; confirmLabel: string; resolve: (ok: boolean) => void };
  const [confirmAsk, setConfirmAsk] = useState<ConfirmAsk | null>(null);
  const confirm = (ask: Omit<ConfirmAsk, "resolve">) => new Promise<boolean>(resolve => setConfirmAsk({ ...ask, resolve }));
  /** 写入配置的结果：写了哪些文件、备份号（可撤销）、警告 */
  const logResult = (r: ApplyResult) => {
    log("ok", t("配置已写入，改动前已备份（{id}）", { id: r.backupId }), { group: "undo", batch: nextBatch(), actions: [{ label: t("撤销这次改动"), run: () => void restore(r.backupId) }] });
    r.paths.forEach(path => log("path", path));
    r.warnings.forEach(w => log("warn", t(w)));
    // Claude 桌面版选了官方登录时重启也还是官方登录，留到用 API 启动时再确认切换
    const targets: LaunchTarget[] = [
      ...(r.wroteClaudeDesktop && !r.officialLoginChosen ? ["claude-desktop" as const] : []),
      ...(r.wroteCodexConfig || r.wroteCodexAuth ? ["codex-desktop" as const] : []),
    ];
    void promptRestart(targets);
  };

  const codexKeepsAuth = env?.codex.authState === "OfficialLogin" || env?.codex.authState === "ExternalStore";

  /**
   * 重新读取密钥列表、备份，以及本机 Claude / Codex 配置。
   * withEnv 默认跟随「是否已同意读取」：没同意时只读本工具自己的数据。返回读到的配置。
   */
  /** 是否已同意读取本机配置（给 reload 用的同步副本） */
  const configReadRef = useRef<Record<ToolId, boolean>>({ claude: false, codex: false });
  const reload = useCallback(async (url?: string, preferredProfileId?: string): Promise<Environment | null> => {
    // 只读用户同意过的工具；一个都没同意时不读本机配置，只读本工具自己的数据
    const tools = (["claude", "codex"] as const).filter(x => configReadRef.current[x]);
    try {
      const [e, ps, bs] = await Promise.all([
        tools.length ? api.scanEnvironment(url, [...tools], preferredProfileId) : Promise.resolve(null),
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

      const rc = remote;
      setBaseUrl(rc.base_url);
      setAliasOpus(rc.claude_opus_model);
      setAliasSonnet(rc.claude_sonnet_model);
      setAliasHaiku(rc.claude_haiku_model);

      // 首次使用（没有同意记录、也没有用过的痕迹）先不读本机配置，等用户在工具页点「读取」；
      // 老用户（已经有密钥或写过配置）直接读取，和以前一样
      const legacy = !!st && !st.configReadTools?.length && (st.configReadConsent || st.profiles.length > 0 || !!st.lastConfiguredAt);
      const consent = { claude: legacy || !!st?.configReadTools?.includes("claude"), codex: legacy || !!st?.configReadTools?.includes("codex") };
      configReadRef.current = consent;
      setConfigRead(consent);
      if (consent.claude || consent.codex) setPageState(current => current !== "welcome" ? current : consent.codex ? "codex" : "claude");
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

  /** 把一把密钥写进某个工具的用户配置（不启动客户端）。成功返回这次改动的备份号，可用来撤销 */
  const enable = async (profileId: string, tool: AppId, rebuild = false, desktop = false): Promise<string | null> => {
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
        codexDesktop: tool === "codex" && desktop,
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
        claudeDesktopExtraModels: p?.custom ? [] : remote?.claude_desktop_extra_models ?? [],
      });
      logResult(r);
      setToast({
        kind: "ok",
        text: t("{tool} 已改用「{name}」；请重新打开客户端，实际连接尚未验证", { tool: APP_META[tool].name, name: p?.name ?? t("所选密钥") }),
      });
      await reload(baseUrl);
      return r.backupId;
    } catch (e) {
      setToast({ kind: "err", text: String(e) });
      return null;
    } finally {
      setBusy(null);
    }
  };

  /**
   * 打开桌面版（配置已核对过）。桌面版只在启动时读配置：
   * 用 API 启动而 Claude 桌面版选了官方登录 → 确认后切到第三方推理并重新打开；
   * 已在运行 → 确认后重启（否则只会激活旧窗口，看起来没反应）；都不是就直接打开。
   */
  const launchDesktop = async (tool: ToolId, service: string, req: LaunchRequest): Promise<{ message: string }> => {
    const name = t(DESKTOP_NAME[req.target] ?? req.target);
    const usesApi = tool === "claude" && service !== "official" && (service !== "current" || !!env?.claudeDesktop?.currentBaseUrl);
    if (usesApi && await api.claudeDesktopOfficialLogin().catch(() => false)) {
      const ok = await confirm({
        title: t("切换为 API 启动？"),
        body: t("Claude 桌面版当前使用官方账号登录，切换后将重启。"),
        note: t("进行中的对话会被中断，可随时切回官方登录。"),
        confirmLabel: t("切换并启动"),
      });
      if (!ok) return { message: t("已取消启动") };
      const r = await api.switchClaudeDesktopToThirdParty();
      const backupId = r.backupId;
      if (backupId) log("ok", t("已把 Claude 桌面版切换到第三方推理（{id}）", { id: backupId }), { group: "undo", batch: nextBatch(), actions: [{ label: t("撤销这次改动"), run: () => void restore(backupId) }] });
      return { message: t(r.message) };
    }
    if ((await api.runningDesktopClients([req.target]).catch(() => [] as LaunchTarget[])).length) {
      const ok = await confirm({
        title: t("重启 {names}？", { names: name }),
        body: t("{name} 正在运行，重启后才会使用所选服务。", { name }),
        note: t("进行中的对话会被中断。"),
        confirmLabel: t("立即重启"),
      });
      if (!ok) return { message: t("已取消启动") };
      return api.restartDesktopClient(req.target);
    }
    return api.launchClient(req);
  };

  /** fresh：刚保存过的密钥（如换模型后），避免用到本次渲染里还没更新的 profiles */
  const runClientAction = async (tool: ToolId, mode: LaunchMode, service: string, directory: string, action: "save" | "launch", fresh?: Profile): Promise<boolean> => {
    if (launchLock.current || busy) return false;
    launchLock.current = true;
    setBusy(action === "save" ? "apply" : "launch");
    let changed = false;
    try {
      const p = fresh?.id === service ? fresh : profiles.find(profile => profile.id === service);
      const claudeAliases = p?.custom
        ? { opus: "", sonnet: "", haiku: "" }
        : { opus: aliasOpus, sonnet: aliasSonnet, haiku: aliasHaiku };
      log("cmd", t(action === "save" ? "保存并写入 {name} 配置" : "启动 {name}", { name: `${tool === "claude" ? "Claude" : "Codex"} ${mode === "cli" ? "CLI" : t("桌面版")}` }) + ` · ${service === "current" ? t("当前配置") : service === "official" ? t("官方服务") : p?.name ?? t("所选密钥")}`);
      if (!["current", "official"].includes(service) && !p?.hasKey) throw new Error(t("所选密钥不可用，请重新选择"));
      const request = {
        profileId: service,
        baseUrl: baseUrl || remote?.base_url || BRAND.apiBaseUrl,
        wireApi: remote?.wire_api || "responses", ignoreProxy,
        configureCodex: tool === "codex", configureClaude: tool === "claude",
        model: modelFor("codex", p), reasoningEffort: p?.codexEffort || "medium",
        cleanupResidue: false, overwriteOfficialAuth: false, conservative,
        claudeModel: modelFor("claude", p),
        claudeOpusModel: claudeAliases.opus, claudeSonnetModel: claudeAliases.sonnet, claudeHaikuModel: claudeAliases.haiku,
        claudeDesktopExtraModels: p?.custom ? [] : remote?.claude_desktop_extra_models ?? [],
      };
      if (action === "save") {
        let backupId = "";
        await saveLaunchConfiguration(tool, mode, service, request, api, r => { changed = true; backupId = r.backupId; logResult(r); });
        // 真的改了配置才给撤销：提示条上倒计时 5 秒，期间可以一键还原到写入前
        log("ok", t("配置已保存并写入，可以启动客户端"), backupId ? { undo: () => void restore(backupId) } : undefined);
      } else {
        const receipt = await launchWithConfiguration(tool, mode, service, directory, request,
          mode === "desktop" ? { ...api, launchClient: req => launchDesktop(tool, service, req) } : api);
        setToast({ kind: "ok", text: receipt.message });
      }
      return true;
    } catch (e) {
      setToast({ kind: "err", text: t(String(e).replace(/^Error:\s*/, "")) + (changed ? t("（配置已保存，可在上方撤销；尚未确认客户端运行）") : "") });
      return false;
    } finally {
      await reload(baseUrl, service);
      setBusy(null); launchLock.current = false;
    }
  };

  const launch = (tool: ToolId, mode: LaunchMode, service: string, directory: string) => runClientAction(tool, mode, service, directory, "launch");
  const saveConfiguration = (tool: ToolId, mode: LaunchMode, service: string) => runClientAction(tool, mode, service, "", "save");

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
  /** 体检 / AI 修复的动作。写入了配置时返回备份号（可撤销），其余（跳转页面、没写入）返回 null */
  const fixHealth = async (tool: ToolId, profileId: string | null, fix: "rewrite" | "rebuild" | "keys" | "settings", mode: LaunchMode = "cli"): Promise<string | null> => {
    if (fix === "rebuild" && tool === "claude" && mode === "desktop") {
      setToast({ kind: "err", text: t("桌面版不支持备份重建，请手动修复配置") });
      return null;
    }
    if (fix === "keys") { setPage("keys"); return null; }
    if (fix === "settings") { setShowAdvanced(true); return null; }
    if (profileId && !busy) return enable(profileId, tool, fix === "rebuild", tool === "claude" && mode === "desktop");
    return null;
  };

  const restore = async (id: string): Promise<boolean> => {
    setBusy("restore");
    try {
      log("cmd", `${t("还原配置")} · ${id}`);
      await api.restoreBackup(id);
      setLogs(l => l.map(x => x.group === "undo" ? { ...x, actions: undefined } : x));
      setToast({ kind: "ok", text: t("已还原到 {id} 那一次的配置", { id }) });
      await reload(baseUrl);
      // 还原可能改了任一客户端的配置：运行中的桌面版同样要重启
      void promptRestart(["claude-desktop", "codex-desktop"]);
      return true;
    } catch (e) {
      setToast({ kind: "err", text: String(e) });
      return false;
    } finally {
      setBusy(null);
    }
  };

  /** 工具页「更换模型」：只改这把密钥给某个工具用的模型，Key 和名称不动 */
  const saveModel = async (id: string, patch: { claudeModel?: string; codexModel?: string; codexEffort?: string }, apply?: { tool: ToolId; mode: LaunchMode }) => {
    const p = profiles.find(x => x.id === id);
    if (!p) return false;
    setBusy("save");
    log("cmd", `${t("更换模型")} · ${p.name} → ${patch.claudeModel ?? patch.codexModel ?? ""}${patch.codexEffort ? t("（思考深度 {effort}）", { effort: effortLabel(patch.codexEffort) }) : ""}`);
    try {
      await api.saveProfile({
        id, name: p.name, apiKey: "",
        codexModel: p.codexModel, codexEffort: p.codexEffort || "medium", claudeModel: p.claudeModel,
        codexBaseUrl: p.codexBaseUrl, claudeBaseUrl: p.claudeBaseUrl,
        ...patch,
      });
      const list = await api.listProfiles().catch(() => profiles);
      setProfiles(list);
      // 这把密钥正在生效时，换完模型直接写入配置，不需要再手动「启用」
      if (apply) {
        setBusy(null);
        return await runClientAction(apply.tool, apply.mode, id, "", "save", list.find(x => x.id === id));
      }
      setToast({ kind: "ok", text: t("模型选择已保存") });
      return true;
    } catch (e) {
      setToast({ kind: "err", text: String(e) });
      return false;
    } finally {
      setBusy(null);
    }
  };
  /** 把配置文件里正在用、没保存在本工具的密钥（AI 或第三方）加入密钥列表 */
  const adoptKey = async (tool: ToolId, desktop = false) => {
    if (busy) return;
    setBusy("save");
    log("cmd", `${t("加入密钥")} · ${t("{tool} 配置里的密钥", { tool: tool === "claude" ? (desktop ? t("{name} 桌面版", { name: "Claude" }) : "Claude") : "Codex" })}`);
    try {
      const r = await api.adoptCurrentKey(tool, baseUrl || remote?.base_url || BRAND.apiBaseUrl, ignoreProxy, desktop);
      log("ok", r.existed ? t("这把密钥已经在列表里：「{name}」", { name: t(r.name) }) : t("已加入密钥列表：「{name}」，测试可用", { name: t(r.name) }));
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

  const testProfile = async (profile: Profile) => {
    if (busy || !profile.hasKey) return;
    setBusy("fetch");
    try {
      const models = await listModels(profile.id);
      log("ok", t("{name}：已读取模型列表（{n} 个）", { name: profile.name, n: models.length }));
    } catch (error) {
      log("err", t("{name}：测试失败 · {error}", { name: profile.name, error: t(String(error).replace(/^Error:\s*/, "")) }));
    } finally { setBusy(null); }
  };

  const loginPanel = (ready?: Parameters<ImportDialog>[0], close = () => setShowLogin(false), manual = () => { close(); setPage("keys"); startEdit(); }) => (
    <LoginPanel
      guided={!!ready}
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
      onImported={async (created, updated, unchanged, skipped, profileIds) => {
        const list = await api.listProfiles();
        setProfiles(list);
        if (ready) {
          const profile = list.find(item => profileIds.includes(item.id) && item.hasKey);
          if (!profile) throw new Error(t("导入失败，请重试"));
          ready(profile.id); return;
        }
        void syncKeyStatus();
        const parts: string[] = [];
        if (created.length) parts.push(t("新增 {n} 个密钥", { n: created.length }));
        if (updated.length) parts.push(t("更新 {n} 个", { n: updated.length }));
        if (unchanged.length) parts.push(t("{n} 个没有变化", { n: unchanged.length }));
        if (skipped.length) parts.push(t("{n} 个取不到，跳过", { n: skipped.length }));
        setToast({ kind: "ok", text: parts.join(t("，")) || t("没有可导入的") });
        setShowLogin(false);
      }}
      onManualKey={manual}
      onClose={close}
    />
  );

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

  /** 引导最后一步：先记下同意读取这些工具的本机配置，再逐个形态写入（各自备份）；一个失败不影响其他 */
  const applyGuideKey: ApplyKey = async (targets, profileId) => {
    await readConfig(targets.map(item => item.tool));
    const list = await api.listProfiles(); setProfiles(list);
    const profile = list.find(item => item.id === profileId);
    const outcome: ApplyOutcome[] = [];
    for (const { tool, modes } of targets) {
      let ok = true, error = "";
      for (const mode of modes) {
        lastError.current = "";
        if (!await runClientAction(tool, mode, profileId, "", "save", profile)) { ok = false; error = lastError.current; break; }
      }
      outcome.push({ tool, ok, error });
    }
    return outcome;
  };
  const restartDialog = <>
    {restartPrompt && <RestartDialog prompt={restartPrompt} onClose={() => setRestartPrompt(null)} log={(kind, text) => setToast({ kind: kind === "info" ? "ok" : kind, text })} />}
    {confirmAsk && <ConfirmDialog title={confirmAsk.title} body={confirmAsk.body} note={confirmAsk.note}
      cancelLabel={t("取消")} confirmLabel={confirmAsk.confirmLabel} workingLabel={confirmAsk.confirmLabel}
      onCancel={() => { confirmAsk.resolve(false); setConfirmAsk(null); }}
      onConfirm={async () => { confirmAsk.resolve(true); setConfirmAsk(null); }} />}
  </>;
  if (onboarding) return <>
    <WelcomeGuide onFinish={tools => { finishOnboarding(); setPage(tools[0]); }} onSkip={finishOnboarding}
      connect={{ profiles, defaultUrl: defaultServiceUrl, ignoreProxy, renderImport: loginPanel, applyKey: applyGuideKey, readEnvironment: () => readConfig(["claude", "codex"]), checkHealth }} />
    {restartDialog}
  </>;

  return (
    <div className="launcher-app">
      <div className="workspace-shell">
      <UpdateBanner />
      <nav className="workspace-nav" aria-label={t("主导航")}>
        <div className="nav-pill liquid-glass">
          {page !== "welcome" && <span className="nav-lens" aria-hidden="true" style={{ transform: `translateX(${NAV_PAGES.indexOf(page) * 100}%)` }} />}
          {NAV_PAGES.map(id => {
            const label = { claude: "Claude", codex: "Codex", keys: t("密钥"), settings: t("设置") }[id];
            const alert = id === "claude" || id === "codex" ? toolAlerts[id] : undefined;
            const name = id === "keys" && profiles.length ? `${label}（${profiles.length}）`
              : alert ? `${label}（${alert === "error" ? t("有问题") : t("有提醒")}）` : label;
            return <button key={id} className="nav-tab" aria-label={name} aria-current={page === id ? "page" : undefined} disabled={busy !== null || guideWorking} onClick={() => { setPage(id); document.querySelector(".workspace-main")?.scrollTo({ top: 0 }); }}>
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
        onEditProfile={startEdit} onTestProfile={testProfile}
        onLaunch={launch} onSaveConfiguration={saveConfiguration} onKeys={() => setPage("keys")}
        onSettings={() => setPage("settings")} onAdvanced={() => setShowAdvanced(true)}
        onRefresh={async () => { await reload(baseUrl); }} checkHealth={checkHealth} onFix={fixHealth} onRestore={restore} onAlerts={setToolAlerts} onConsole={openSite}
        modelFor={modelFor} catalogKey={Object.keys(catalogs).join()} onSaveModel={saveModel} listModels={listModels}
        logs={logs} log={log} nextBatch={nextBatch} onClearLogs={clearLogs} droppedLogs={droppedLogs} onPage={setPage} onAdopt={adoptKey}
        onLoginImport={() => { setPage("keys"); setShowLogin(true); }} renderImport={loginPanel} defaultUrl={defaultServiceUrl} ignoreProxy={ignoreProxy} onGuideWorking={setGuideWorking} guideWorking={guideWorking}
        onGuideApply={async (tool, mode, id) => { const list = await api.listProfiles(); setProfiles(list); return runClientAction(tool, mode, id, "", "save", list.find(profile => profile.id === id)); }}
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

      {showLogin && loginPanel()}

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

      {/* 编辑 / 新建：在当前页面上打开共享弹窗，不切换导航 */}
      {draft && (() => {
        const editingProfile = draft.id ? profiles.find(p => p.id === draft.id) : undefined;
        return (
          <Editor
            title={editingProfile ? t("编辑「{name}」", { name: editingProfile.name }) : t("添加密钥")}
            hint={editingProfile ? undefined : t("保存后在 Claude / Codex 页面选择密钥和模型。")}
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
              <span className="dim"> · {t("用于后台任务和模型切换，留空不设置。")}</span>
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
              <span className="dim"> · {t("强制使用独立网关配置，不覆盖官方登录。")}</span>
            </span>
          </label>
          {codexKeepsAuth && (
            <label className="check">
              <input type="checkbox" checked={overwriteAuth} disabled={env?.codex.authState === "ExternalStore"} onChange={(e) => setOverwriteAuth(e.target.checked)} />
              <span>
                {t("写入 Codex 时不保留已有登录")}
                <span className="dim"> · {env?.codex.authState === "ExternalStore" ? t("Codex 使用独立认证存储，不能勾选") : t("启用后将退出本机 ChatGPT 登录。")}</span>
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
      {restartDialog}
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
