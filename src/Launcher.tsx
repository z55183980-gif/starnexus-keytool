import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { diagnosticReport } from "./diagnosticReport";
import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { runTrayAction, TrayConsumer } from "./trayFlow";
import * as mock from "./mock";
import { needsConnectionGuide } from "./onboarding";
import { ConnectionGuide, type ImportDialog } from "./ConnectionGuide";
import { ClientSetup } from "./ClientSetup";
import { ClientInstallStatus } from "./ClientInstallStatus";
import { ClientVersionCache } from "./clientVersionCache";
import * as api from "./api";
import { ClaudeIcon, CodexIcon } from "./BrandIcons";
import { UiIcon } from "./UiIcon";
import { ServiceKeyRow } from "./ServiceKeyRow";
import { OutputConsole, type LogAction, type LogKind, type LogLine } from "./OutputConsole";
import { RepairCard, type RepairView } from "./RepairCard";
import { EFFORTS, effortLabel, groupByFamily, supports } from "./modelFamilies";
import { LANGS, setLang, t, useLang } from "./i18n";
import { UpdatePanel, useUpdates } from "./Updates";
import { profileTitle, supportsTool, type Environment, type HealthFix, type HealthIssue, type LauncherPreferences, type LauncherScan, type LaunchMode, type LaunchTarget, type Profile, type ToolId, type ModelInfo } from "./types";

const TOOLS = { claude: { name: "Claude", cli: "Claude Code", Icon: ClaudeIcon }, codex: { name: "Codex", cli: "Codex CLI", Icon: CodexIcon } };
const TOOL_IDS: ToolId[] = ["claude", "codex"];
/** 地址里的主机名，用来给第三方服务起名字 */
const hostOf = (url?: string | null) => { if (!url) return ""; try { return new URL(url).host; } catch { return url; } };
/** 启动、体检、诊断都只认配置文件里当前生效的服务 */
const LIVE = "current";
const TARGETS: LaunchTarget[] = ["claude-desktop", "claude-cli", "codex-desktop", "codex-cli"];
const targetName = (target: LaunchTarget) => { const [tool, mode] = target.split("-") as [ToolId, LaunchMode]; return mode === "cli" ? TOOLS[tool].cli : t("{name} 桌面版", { name: TOOLS[tool].name }); };
const folderName = (path: string) => path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path;

export type Page = "welcome" | ToolId | "keys" | "settings";

/** 居中的玻璃弹窗：按 Esc 关闭，点遮罩也关闭（表单类弹窗传 keepOnOverlay 关掉这条）；打开时焦点落在当前选中项或第一个输入框上 */
export function PickerDialog(props: { title: string; onClose: () => void; footer?: React.ReactNode; children: React.ReactNode; className?: string; keepOnOverlay?: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  const { onClose } = props;
  // 调用方传的 onClose 每次渲染都是新函数：存进 ref，避免下面的副作用每次渲染都重跑、把焦点抢回去
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  // 只在打开时设一次焦点
  useEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    (ref.current?.querySelector<HTMLElement>('.picker-body [data-autofocus]:not(:disabled)')
      ?? ref.current?.querySelector<HTMLElement>('.picker-body [aria-checked="true"]:not(:disabled)')
      ?? ref.current?.querySelector<HTMLElement>('.picker-body input:not(:disabled)')
      ?? ref.current?.querySelector<HTMLElement>('.picker-body button:not(:disabled)')
      ?? ref.current)?.focus();
    return () => { if (opener?.isConnected) opener.focus(); };
  }, []);
  useEffect(() => {
    const previousOverflow = document.body.style.overflow;
    const main = document.querySelector<HTMLElement>(".workspace-main");
    const previousMainOverflow = main?.style.overflow;
    document.body.style.overflow = "hidden";
    if (main) main.style.overflow = "hidden";
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") { e.preventDefault(); closeRef.current(); }
      if (e.key !== "Tab") return;
      const items = Array.from(ref.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), a[href], summary, [tabindex="0"]') ?? [])
        .filter(el => el.getClientRects().length > 0);
      const first = items[0], last = items[items.length - 1];
      if (!first) { e.preventDefault(); ref.current?.focus(); }
      else if (e.shiftKey && (document.activeElement === first || !ref.current?.contains(document.activeElement))) { e.preventDefault(); last.focus(); }
      else if (!e.shiftKey && (document.activeElement === last || !ref.current?.contains(document.activeElement))) { e.preventDefault(); first.focus(); }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      document.body.style.overflow = previousOverflow;
      if (main) main.style.overflow = previousMainOverflow ?? "";
    };
  }, []);
  return <div className="overlay picker-overlay" onMouseDown={e => { if (!props.keepOnOverlay && e.target === e.currentTarget) onClose(); }}>
    <div className={`picker-sheet ${props.className ?? ""}`} role="dialog" aria-modal="true" aria-label={props.title} ref={ref} tabIndex={-1}>
      <div className="picker-hd"><strong>{props.title}</strong><button type="button" className="icon-round" aria-label={t("关闭")} onClick={onClose}><UiIcon name="close"/></button></div>
      <div className="picker-body">{props.children}</div>
      {props.footer && <div className="picker-ft">{props.footer}</div>}
    </div>
  </div>;
}
/** 一键修复的状态：进行中 / 等 AI 方案里的选择 / 选启动后遇到的症状 / 结果 */
type RepairState =
  | { phase: "busy"; title: string; steps: string[]; current: number }
  | { phase: "result"; level: "ok" | "warn" | "error"; title: string; detail?: string; steps?: string[]; backupId?: string; next?: LogAction; };
/** 备份后重建：Codex 重建 config.toml；Claude Code（CLI）只修复损坏的 env。桌面版没有这条路 */
export const canRebuild = (tool: ToolId, mode: LaunchMode): boolean => tool === "codex" || (tool === "claude" && mode === "cli");
/** 体检已经能确定的写入类修复：配置损坏先备份修复，其次重写 */
const localFix = (tool: ToolId, mode: LaunchMode, issues: HealthIssue[]): HealthFix | null =>
  canRebuild(tool, mode) && issues.some(i => i.fix === "rebuild") ? "rebuild"
    : issues.some(i => i.fix === "rewrite") ? "rewrite" : null;
const errText = (e: unknown) => String(e).replace(/^Error:\s*/, "");

/** 每个工具的体检概况，给操作栏画红点 */
export type ToolAlerts = Partial<Record<ToolId, "error" | "warn">>;

export function Launcher(props: {
  page: Page;
  busy: boolean;
  profiles: Profile[];
  env: Environment | null;
  onLaunch: (tool: ToolId, mode: LaunchMode, service: string, directory: string) => Promise<boolean>;
  onSaveConfiguration: (tool: ToolId, mode: LaunchMode, service: string) => Promise<boolean>;
  onKeys: (tool?: ToolId) => void;
  onEditProfile: (profile: Profile) => void;
  onTestProfile: (profile: Profile) => Promise<void>;
  onSettings: () => void;
  onAdvanced: () => void;
  onRefresh: () => Promise<void>;
  checkHealth: (tool: ToolId, service: string, mode: LaunchMode) => Promise<HealthIssue[]>;
  /** 写入配置时返回这次改动的备份号（可撤销）；跳转页面或没写入时返回 null */
  onFix: (tool: ToolId, profileId: string | null, fix: Exclude<HealthFix, "none" | "model">, mode: LaunchMode) => Promise<string | null>;
  /** 用备份号还原配置 */
  onRestore: (backupId: string) => Promise<boolean>;
  onAlerts: (alerts: ToolAlerts) => void;
  /** 在浏览器打开开发者控制台 */
  onConsole: () => void;
  /** 某把密钥给某个工具实际用的模型（没单独设置时按网关目录自动挑） */
  modelFor: (tool: ToolId, p: Profile) => string;
  /** 目录缓存变化的标记：自动挑的模型变了要重新体检 */
  catalogKey: string;
  /** 保存某把密钥给某个工具用的模型 */
  /** 保存密钥的模型；apply 给出时，随即用这把密钥重新写入该工具的配置 */
  onSaveModel: (profileId: string, patch: { claudeModel?: string; codexModel?: string; codexEffort?: string }, apply?: { tool: ToolId; mode: LaunchMode }) => Promise<boolean>;
  /** 这把密钥能用的模型 */
  listModels: (profileId: string) => Promise<ModelInfo[]>;
  /** 输出面板：记录由 App 统一保存，工具页在这里画并写入体检、AI 诊断的结果 */
  logs: LogLine[];
  log: (kind: LogKind, text: string, extra?: { group?: string; batch?: number; actions?: LogAction[] }) => void;
  nextBatch: () => number;
  onClearLogs: () => void;
  /** 超出上限被丢掉的更早记录条数 */
  droppedLogs: number;
  onPage: (page: Page) => void;
  /** 把当前配置里的密钥加入密钥列表；desktop = Claude 桌面版的配置 */
  onAdopt: (tool: ToolId, desktop?: boolean) => void;
  /** 每个工具是否已同意读取本机配置；null = 还不知道。没同意的工具页先显示遮罩，不读它的配置 */
  configRead: Record<ToolId, boolean> | null;
  onLoginImport: () => void;
  renderImport: ImportDialog;
  defaultUrl: string;
  ignoreProxy: boolean;
  guideWorking: boolean;
  onGuideWorking: (busy: boolean) => void;
  onGuideApply: (tool: ToolId, mode: LaunchMode, id: string) => Promise<boolean>;
  /** 同意并读取这些工具的本机配置，返回读到的配置 */
  onReadConfig: (tools: ToolId[]) => Promise<Environment | null>;
}) {
  const lang = useLang();
  const updates = useUpdates();
  const [prefs, setPrefs] = useState<LauncherPreferences | null>(null);
  const prefsRef = useRef<LauncherPreferences | null>(null);
  const [scan, setScan] = useState<LauncherScan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [pathDrafts, setPathDrafts] = useState<Partial<Record<LaunchTarget, string>>>({});
  const savingRef = useRef(false);
  const activatingRef = useRef(false);
  const [activating, setActivating] = useState(false);
  /** 两个工具的体检结果。null = 检查中，undefined = 体检本身出错 */
  const [health, setHealth] = useState<Record<ToolId, HealthIssue[] | null | undefined>>({ claude: null, codex: null });
  /** 当前打开的选择弹窗：服务或工作目录。卡片里平时都只显示一行 */
  const [picker, setPicker] = useState<"workdir" | "model" | null>(null);
  /** 卡片里的服务列表：托盘「切换密钥」、输出里的「选择密钥」会把焦点带到这里 */
  const serviceListRef = useRef<HTMLDivElement>(null);
  const focusServices = () => requestAnimationFrame(() => {
    const list = serviceListRef.current;
    if (!list) return;
    list.scrollIntoView({ block: "nearest", behavior: "smooth" });
    list.querySelector<HTMLElement>('.service-enable:not(:disabled)')?.focus({ preventScroll: true });
  });
  /** 「更换模型」弹窗里正在编辑的值 */
  const [modelEdit, setModelEdit] = useState<{ model: string; effort: string }>({ model: "", effort: "medium" });
  /** 正在换模型的那把密钥（不一定是使用中的那把） */
  const [modelProfileId, setModelProfileId] = useState<string | null>(null);
  /** 可用模型列表，按密钥缓存，同一把密钥不重复获取 */
  const [modelList, setModelList] = useState<{ profileId: string; models: ModelInfo[] | null; error?: string } | null>(null);
  /** 设置页：正在修改哪个程序的路径（弹窗） */
  const [pathEdit, setPathEdit] = useState<LaunchTarget | null>(null);
  const [setupTarget, setSetupTarget] = useState<LaunchTarget | null>(null);
  const [officialTargets, setOfficialTargets] = useState<LaunchTarget[]>([]);
  const clientVersions = useRef(new ClientVersionCache()).current;
  const [trayWorking, setTrayWorking] = useState(false);
  /** 会覆盖配置文件的环境变量（只有变量名）；null = 还没读到 */
  const [overrides, setOverrides] = useState<string[] | null>(null);
  /** 等检测结果齐了，把「检测本机环境」写进输出：启动时一次，点刷新再一次 */
  const [envReportWanted, setEnvReportWanted] = useState(false);
  /** 正在逐条输出「读取本机配置」：这期间体检结果先不写，等读取的输出写完再写，顺序不乱 */
  const [streaming, setStreaming] = useState(false);
  /** 已经自动或手动读过一次安装情况，避免重复扫描 */
  const scanned = useRef(false);
  /** 每个工具的一键修复：进行中、等用户选择、或上一次的结果 */
  const [repair, setRepairs] = useState<Partial<Record<ToolId, RepairState>>>({});
  const setRepair = (tool: ToolId, r: RepairState | null) => setRepairs(x => ({ ...x, [tool]: r ?? undefined }));
  const repairing = TOOL_IDS.some(tool => repair[tool]?.phase === "busy");
  /** 输出行上的按钮可能要先切到对应工具页再打开弹窗：切页会清掉弹窗，所以先记下来，切完再执行 */
  const pendingAfterPage = useRef<(() => void) | null>(null);
  const pageRef = useRef(props.page);
  pageRef.current = props.page;
  const refresh = useCallback(async () => {
    setRefreshing(true); setError(null);
    try { const s = await api.scanLaunchers(); setScan(s); return s; }
    catch (e) { setError(String(e)); return null; }
    finally { setRefreshing(false); }
  }, []);
  useEffect(() => {
    let cancelled = false;
    api.loadLauncherPreferences().then(p => {
      if (!cancelled) { prefsRef.current = p; setPrefs(p); setPathDrafts(p.customPaths); }
    }).catch(e => { if (!cancelled) setError(String(e)); });
    return () => { cancelled = true; };
  }, []);
  /** 这个工具的配置是否已同意读取 */
  const consented = (tool: ToolId) => !!props.configRead?.[tool];
  /** 当前启动方式实际读的那份配置：Claude 桌面版读它自己的第三方推理配置，不读 settings.json */
  const desktopClaude = prefs?.claudeMode === "desktop";
  const modeOf = (tool: ToolId): LaunchMode => prefs?.[`${tool}Mode`] ?? "cli";
  /** 体检、日志里的名字：Claude 分桌面版和 Claude Code（两份配置）；Codex 两种启动共用一份配置 */
  const healthName = (tool: ToolId) => tool === "claude" ? (desktopClaude ? t("{name} 桌面版", { name: TOOLS.claude.name }) : TOOLS.claude.cli) : TOOLS[tool].name;
  const cfg = (tool: ToolId, env: Environment | null | undefined = props.env) =>
    tool === "claude" && desktopClaude ? env?.claudeDesktop : env?.[tool];
  const readTools = TOOL_IDS.filter(consented);
  // 已经同意过读取（老用户或之前点过）：启动时自动读取安装情况和环境变量，并写一段「检测本机环境」
  useEffect(() => {
    if (!readTools.length || scanned.current) return;
    scanned.current = true;
    void refresh();
    api.envOverrides().then(setOverrides).catch(() => setOverrides([]));
    setEnvReportWanted(true);
  }, [readTools.join(), refresh]);

  // 两个工具都体检，不只当前页面的那个：操作栏要靠它显示红点。
  // 写入配置、刷新（env 变化）时重查；重查期间保留上一次的结果，避免界面跳动。
  // 启动和体检都只认配置文件里「当前生效」的服务（LIVE），没有另外的「所选服务」。
  useEffect(() => {
    if (!prefs) return;
    let cancelled = false;
    // 只体检同意读取过的工具
    for (const tool of TOOL_IDS.filter(consented)) {
      props.checkHealth(tool, LIVE, modeOf(tool))
        .then(r => { if (!cancelled) setHealth(h => ({ ...h, [tool]: r })); })
        .catch(() => { if (!cancelled) setHealth(h => ({ ...h, [tool]: undefined })); });
    }
    return () => { cancelled = true; };
  }, [!!prefs, readTools.join(), desktopClaude, props.env, props.profiles, props.catalogKey]);
  const { onAlerts } = props;
  useEffect(() => {
    const level = (issues: HealthIssue[] | null | undefined) =>
      !issues?.length ? undefined : issues.some(i => i.level === "error") ? "error" as const : "warn" as const;
    onAlerts({ claude: level(health.claude), codex: level(health.codex) });
  }, [health, onAlerts]);

  // Write each accepted preference change before enabling another launch/change.
  async function save(patch: Partial<LauncherPreferences>): Promise<boolean> {
    if (!prefsRef.current || savingRef.current) return false;
    savingRef.current = true; setSaving(true); setError(null);
    const next = { ...prefsRef.current, ...patch };
    try { await api.saveLauncherPreferences(next); prefsRef.current = next; setPrefs(next); return true; }
    catch (e) { setError(t("启动偏好未保存：{error}", { error: String(e) })); return false; }
    finally { savingRef.current = false; setSaving(false); }
  }
  async function chooseFolder() {
    try {
      const selected = await open({ directory: true, multiple: false, title: t("选择 CLI 工作目录"), defaultPath: prefs?.projectDirectory || undefined });
      if (typeof selected === "string") await save({ projectDirectory: selected, recentDirectories: [selected, ...(prefsRef.current?.recentDirectories ?? []).filter(p => p !== selected)].slice(0, 8) });
    } catch (e) { setError(String(e)); }
  }
  async function chooseProgram(target: LaunchTarget): Promise<LauncherScan | null | undefined> {
    try {
      const selected = await open({ multiple: false, directory: scan?.platform === "macos" && target.endsWith("desktop"), title: t("选择 {name} 程序", { name: targetName(target) }) });
      if (typeof selected === "string") {
        setPathDrafts(p => ({ ...p, [target]: selected }));
        if (await save({ customPaths: { ...prefsRef.current?.customPaths, [target]: selected } })) return await refresh();
        return null;
      }
    } catch (e) { setError(String(e)); return null; }
  }
  async function savePath(target: LaunchTarget, value: string) {
    if (await save({ customPaths: { ...prefsRef.current?.customPaths, [target]: value.trim() } })) {
      setPathDrafts(p => ({ ...p, [target]: value.trim() })); await refresh();
    }
  }
  useEffect(() => {
    setPicker(null); setPathEdit(null); setSetupTarget(null);
    const run = pendingAfterPage.current;
    pendingAfterPage.current = null;
    run?.();
  }, [props.page]);
  const disabled = props.busy || saving || activating || refreshing || trayWorking || !prefs || updates.state.phase !== "idle";
  const recent = prefs ? prefs.recentDirectories.filter(p => p !== prefs.projectDirectory) : [];

  /** 某把密钥给某个工具用的模型；没单独设置就是默认模型 */
  const modelOf = (tool: ToolId, p: Profile) => props.modelFor(tool, p);
  const openModel = (tool: ToolId, p: Profile) => {
    setModelEdit({ model: modelOf(tool, p), effort: p.codexEffort || "medium" });
    setModelProfileId(p.id);
    setPicker("model");
    if (modelList?.profileId === p.id && modelList.models && !modelList.error) return;
    setModelList({ profileId: p.id, models: null });
    props.listModels(p.id)
      .then(models => setModelList(l => l?.profileId === p.id ? { profileId: p.id, models } : l))
      .catch(e => setModelList(l => l?.profileId === p.id ? { profileId: p.id, models: [], error: e instanceof Error ? e.message : String(e) } : l));
  };

  /** 一个工具当前生效的服务，以及修复、AI 诊断各用哪把密钥 */
  const ctxFor = (tool: ToolId) => {
    const config = cfg(tool);
    const activeProfile = props.profiles.find(p => p.id === config?.activeProfileId);
    // 配置文件里正在生效、且保存在本工具里的那把密钥；模型行、换模型都针对它
    const selected = config?.configurationStatus === "configured" && activeProfile?.hasKey && supportsTool(activeProfile, tool) ? activeProfile : undefined;
    // 修复要用一把密钥写入：用当前配置正在用的；当前是官方服务时不提供写入
    const fixProfile = config?.configurationStatus === "official" ? null : activeProfile?.hasKey ? activeProfile.id : null;
    // AI 诊断只用 AI 密钥：当前在用 AI → 任一可用 AI；其他渠道不参与诊断
    const serviceLabel = selected ? profileTitle(selected) : config?.configurationStatus === "official" ? t("官方服务") : t("当前配置");
    return { config, activeProfile, selected, fixProfile, serviceLabel };
  };
  /** 在某个工具页上做事：不在那一页就先切过去 */
  const onToolPage = (tool: ToolId, run: () => void) => {
    if (pageRef.current === tool) return run();
    pendingAfterPage.current = run;
    props.onPage(tool);
  };
  const trayReady = !props.guideWorking && !!prefs && !!props.configRead && (!readTools.length || !!props.env) && !props.busy && !saving && !refreshing && !streaming && !repairing && updates.state.phase === "idle";
  const trayRuntime = useRef({ ready: trayReady, run: async (_action: import("./trayFlow").TrayAction) => {} });
  trayRuntime.current = { ready: trayReady, run: async action => {
    if (!prefsRef.current) return;
    setTrayWorking(true);
    try {
      await runTrayAction(action, prefsRef.current, {
        consented, validateDirectory: api.validateProjectDirectory, save, launch: props.onLaunch,
        showTool: (tool, picker) => onToolPage(tool, () => { if (picker) focusServices(); }),
      });
      if (action.kind === "directory") props.onPage(props.page === "codex" ? "codex" : "claude");
    } finally { setTrayWorking(false); }
  } };
  const [trayConsumer] = useState(() => new TrayConsumer(
    () => trayRuntime.current.ready, api.takeTrayAction,
    action => trayRuntime.current.run(action), error => setError(String(error).replace(/^Error:\s*/, "")),
  ));
  useEffect(() => {
    if (import.meta.env.DEV && mock.activeScenario()) return;
    let stopped = false;
    const unlisten = listen("tray-action-pending", () => { if (!stopped) void trayConsumer.drain(); });
    void unlisten.then(() => { if (!stopped) void trayConsumer.drain(); }).catch(error => setError(String(error)));
    return () => { stopped = true; void unlisten.then(fn => fn()).catch(() => {}); };
  }, [trayConsumer]);
  useEffect(() => { if (trayReady) void trayConsumer.drain(); }, [trayReady, trayConsumer]);
  /** 当前配置的一句话说明；对不上本工具里的密钥时说清楚「未保存在本工具」 */
  const describe = (config: Environment[keyof Environment] | undefined) => {
    if (!config) return t("读取中");
    if (config.unsavedGatewayKey) return t("AI 密钥（未保存在本工具）");
    if (config.configurationStatus === "configured") {
      const p = props.profiles.find(x => x.id === config.activeProfileId && x.hasKey);
      return p ? profileTitle(p) : t("AI 密钥");
    }
    return t(({ official: "官方服务", unconfigured: "尚未配置", custom: "自定义服务", unknown: "未能识别的服务", broken: "配置文件损坏" } as const)[config.configurationStatus]);
  };

  /** 只属于某个工具的环境变量 */
  const varsOf = (tools: ToolId[], names: string[]) =>
    names.filter(n => (tools.includes("claude") && /^(ANTHROPIC|CLAUDE)_/.test(n)) || (tools.includes("codex") && /^(OPENAI|CODEX)_/.test(n)));
  /** 把检测到的本机环境写进输出（只写同意读取过的工具）。只读，不修改任何文件 */
  const reportEnvironment = () => {
    if (!scan || !props.env || overrides === null || !readTools.length) return false;
    const group = "env", batch = props.nextBatch();
    props.log("cmd", t("检测本机环境（只读取 Claude / Codex 的配置，不会修改）"), { group, batch });
    for (const target of TARGETS.filter(x => readTools.some(tool => x.startsWith(tool)))) {
      const i = scan.installations.find(x => x.target === target);
      if (i?.installed) props.log("ok", t("{name} 已安装", { name: targetName(target) }) + (i.path ? ` · ${i.path}` : ""), { group, batch });
      else props.log("warn", t("{name} 未检测到", { name: targetName(target) }), { group, batch });
    }
    for (const tool of readTools) {
      const config = props.env[tool];
      props.log(config.configurationStatus === "broken" ? "err" : "info",
        t("{tool} 配置：{state}", { tool: tool === "claude" ? TOOLS.claude.cli : TOOLS[tool].name, state: describe(config) }) + (config.currentModel ? ` · ${config.currentModel}` : ""), { group, batch });
      const desktop = props.env.claudeDesktop;
      if (tool === "claude" && desktop) props.log(desktop.configurationStatus === "broken" ? "err" : "info",
        t("{tool} 配置：{state}", { tool: t("{name} 桌面版", { name: TOOLS.claude.name }), state: describe(desktop) }) + (desktop.currentModel ? ` · ${desktop.currentModel}` : ""), { group, batch });
    }
    const vars = varsOf(readTools, overrides);
    if (vars.length) props.log("warn", t("环境变量 {names} 已设置，会覆盖配置文件", { names: vars.join(t("、")) }), { group, batch });
    return true;
  };
  /**
   * 读取本机配置，每完成一步就写一行输出：安装情况 → 两份配置文件 → 环境变量；体检随后自动写。
   * 首次使用的遮罩、设置页的「重新读取本机配置」都走这里。全程只读。
   */
  const readLocalConfig = async (tools: ToolId[]) => {
    if (streaming || !tools.length) return;
    setStreaming(true);
    scanned.current = true;
    setEnvReportWanted(false);
    const group = "env", batch = props.nextBatch();
    props.log("cmd", tools.length === 1 ? t("读取本机 {tool} 配置（只读，不会修改任何内容）", { tool: TOOLS[tools[0]].name }) : t("读取本机配置（只读，不会修改任何内容）"), { group, batch });
    try {
      const s = await refresh();
      if (!s) props.log("err", t("读取程序安装位置失败"), { group, batch });
      for (const target of TARGETS.filter(x => tools.some(tool => x.startsWith(tool)))) {
        const i = s?.installations.find(x => x.target === target);
        if (!i) continue;
        if (i.installed) props.log("ok", t("{name} 已安装", { name: targetName(target) }) + (i.path ? ` · ${i.path}` : ""), { group, batch });
        else props.log("warn", t("{name} 未检测到", { name: targetName(target) }), { group, batch });
      }
      const env = await props.onReadConfig(tools);
      if (!env) props.log("err", t("读取配置文件失败，详情见页面上的错误提示"), { group, batch });
      for (const tool of tools) {
        if (!env) break;
        const config = env[tool];
        const file = tool === "claude" ? "settings.json" : "config.toml";
        const path = `${config.home}${config.home.includes("\\") ? "\\" : "/"}${file}`;
        props.log(config.configurationStatus === "broken" ? "err" : "ok",
          t("读取 {path}", { path }) + ` · ${describe(config)}` + (config.currentModel ? ` · ${config.currentModel}` : ""), { group, batch });
        const desktop = env.claudeDesktop;
        if (tool === "claude" && desktop) props.log(desktop.configurationStatus === "broken" ? "err" : "ok",
          t("读取 {path}", { path: desktop.home }) + ` · ${describe(desktop)}` + (desktop.currentModel ? ` · ${desktop.currentModel}` : ""), { group, batch });
      }
      const all = await api.envOverrides().catch(() => [] as string[]);
      setOverrides(all);
      const names = varsOf(tools, all);
      if (names.length) props.log("warn", t("环境变量 {names} 已设置，会覆盖配置文件", { names: names.join(t("、")) }), { group, batch });
      else props.log("ok", t("环境变量：没有会覆盖配置文件的设置"), { group, batch });
    } catch (cause) { setError(errText(cause)); } finally {
      setStreaming(false);
    }
  };
  useEffect(() => {
    if (envReportWanted && reportEnvironment()) setEnvReportWanted(false);
  }, [envReportWanted, scan, props.env, overrides]);

  /** 体检 / AI 给出的动作 → 按钮（日志行尾、修复结果卡）。写入配置的走一键修复的流程：备份、写入、复检 */
  const fixAction = (tool: ToolId, fix: HealthFix): LogAction | null => {
    const c = ctxFor(tool);
    const pickKey: LogAction = { label: t("选择密钥"), run: () => onToolPage(tool, focusServices) };
    // rebuild_minimal_config rebuilds Codex's config.toml or repairs Claude Code's malformed env.
    // Never expose it for desktop apps, even if a model returns the wrong action.
    if (fix === "rebuild" && !canRebuild(tool, modeOf(tool))) return null;
    switch (fix) {
      case "none": return null;
      case "keys": return { label: t("管理密钥"), run: () => props.onFix(tool, null, "keys", modeOf(tool)) };
      case "settings": return { label: t("去设置"), run: () => props.onFix(tool, null, "settings", modeOf(tool)) };
      case "model": {
        const sel = c.selected;
        return sel?.hasKey ? { label: t("换模型"), run: () => onToolPage(tool, () => openModel(tool, sel)) } : pickKey;
      }
      default: {
        const id = c.fixProfile;
        if (!id) return pickKey;
        return {
          label: fix === "rebuild" ? t(tool === "codex" ? "重建配置" : "备份并修复") : t("修复"),
          run: () => onToolPage(tool, () => void applyFix(tool, fix)),
        };
      }
    }
  };
  /** 把一个工具的体检结果写进输出 */
  const reportHealth = (tool: ToolId, issues: HealthIssue[], title?: string) => {
    const group = `health-${tool}`, batch = props.nextBatch();
    props.log("cmd", (title ?? t("体检 {tool}", { tool: healthName(tool) })) + ` · ${ctxFor(tool).serviceLabel}`, { group, batch });
    if (!issues.length) props.log("ok", t("未发现问题"), { group, batch });
    for (const issue of issues) {
      const actions = [
        ...(issue.id === "K1" ? [{ label: t("去控制台 ↗"), run: props.onConsole }] : []),
        ...[fixAction(tool, issue.fix)].filter((a): a is LogAction => !!a),
      ];
      props.log(issue.level === "error" ? "err" : "warn", t(issue.text), { group, batch, actions });
    }
  };
  // 体检结果有变化（问题不同、换了服务）才写一次，自动重查但结果一样时不刷屏
  const lastHealth = useRef<Record<ToolId, string>>({ claude: "", codex: "" });
  useEffect(() => {
    if (streaming) return;
    for (const tool of TOOL_IDS) {
      const issues = health[tool];
      if (!issues) continue;
      const sig = `${cfg(tool)?.activeProfileId ?? cfg(tool)?.configurationStatus ?? ""}|${modeOf(tool)}|${issues.map(i => i.id).join(",")}`;
      if (lastHealth.current[tool] === sig) continue;
      lastHealth.current[tool] = sig;
      reportHealth(tool, issues);
    }
  }, [health, streaming]);
  /** 重新体检一个工具并写进日志；一键修复、撤销之后用 */
  const recheck = async (tool: ToolId, title?: string) => {
    const issues = await props.checkHealth(tool, LIVE, modeOf(tool));
    lastHealth.current[tool] = `${cfg(tool)?.activeProfileId ?? cfg(tool)?.configurationStatus ?? ""}|${modeOf(tool)}|${issues.map(i => i.id).join(",")}`;
    setHealth(h => ({ ...h, [tool]: issues }));
    reportHealth(tool, issues, title);
    return issues;
  };
  /** 状态卡上的「重新检查」：重新扫描安装情况、配置和环境变量，体检随之自动重跑 */
  const recheckAll = async () => {
    setError(null);
    await Promise.all([refresh(), props.onRefresh(), api.envOverrides().then(setOverrides).catch(() => {})]);
    setEnvReportWanted(true);
  };

  // —————— 一键修复 ——————
  // 换了启动方式，上一次的修复结果就不再对应当前配置：清掉（进行中的不动）
  useEffect(() => {
    setRepairs(x => Object.fromEntries(Object.entries(x).filter(([, r]) => r?.phase === "busy")));
  }, [prefs?.claudeMode, prefs?.codexMode]);
  const STEP_WRITE = t("备份并写入配置"), STEP_VERIFY = t("复检");

  /** 写入配置类的修复：备份 → 写入 → 复检；修完反而多出新的错误时自动还原 */
  const applyFix = async (tool: ToolId, fix: HealthFix) => {
    const c = ctxFor(tool);
    if (fix !== "rewrite" && fix !== "rebuild") {
      setRepair(tool, null);
      fixAction(tool, fix)?.run();
      return;
    }
    if (fix === "rebuild" && !canRebuild(tool, modeOf(tool))) {
      setRepair(tool, { phase: "result", level: "error", title: t("桌面版不支持备份重建，请手动修复配置") });
      return;
    }
    if (!c.fixProfile) {
      setRepair(tool, { phase: "result", level: "warn", title: t("先选择一把密钥"), detail: t("修复要用一把密钥重新写入配置。"), next: { label: t("选择密钥"), run: focusServices } });
      return;
    }
    const steps = [STEP_WRITE, STEP_VERIFY];
    const at = (step: string) => steps.indexOf(step);
    const title = t("正在修复 {tool}", { tool: healthName(tool) });
    const before = health[tool] ?? [];
    setRepair(tool, { phase: "busy", title, steps, current: at(STEP_WRITE) });
    const backupId = await props.onFix(tool, c.fixProfile, fix, modeOf(tool));
    if (typeof backupId !== "string") {
      setRepair(tool, { phase: "result", level: "error", title: t("没能写入配置"), detail: t("原配置没有改动。详情见「设置 → 诊断日志」。") });
      return;
    }
    setRepair(tool, { phase: "busy", title, steps, current: at(STEP_VERIFY) });
    let after: HealthIssue[];
    try {
      after = await recheck(tool, t("修复后体检 {tool}", { tool: healthName(tool) }));
    } catch (e) {
      props.log("err", t("修复后体检失败：{error}", { error: errText(e) }));
      setRepair(tool, { phase: "result", level: "warn", title: t("已写入配置，但复检没有完成"), detail: t("可以点「重新检查」再试一次。"), backupId });
      return;
    }
    const beforeIds = new Set(before.map(i => i.id));
    const worse = after.filter(i => i.level === "error" && !beforeIds.has(i.id));
    if (worse.length) {
      const restored = await props.onRestore(backupId);
      if (restored) await recheck(tool).catch(() => {});
      setRepair(tool, {
        phase: "result", level: "error",
        title: restored ? t("修复没有成功，已恢复原配置") : t("修复后出现新问题"),
        detail: worse.map(i => t(i.text)).join(" · "), backupId: restored ? undefined : backupId,
      });
      return;
    }
    const fixed = before.filter(i => !after.some(a => a.id === i.id)).map(i => t(i.text));
    const left = after.filter(i => i.level === "error");
    const warnsLeft = after.length - left.length;
    setRepair(tool, left.length
      ? { phase: "result", level: "warn", title: t("部分问题仍在"), detail: left.map(i => t(i.text)).join(" · "), backupId }
      : {
        phase: "result", level: "ok", title: t("已修复，复检通过"), backupId,
        detail: [fixed.join(" · "), warnsLeft ? t("还有 {n} 个提醒", { n: warnsLeft }) : ""].filter(Boolean).join(" · ") || undefined,
      });
  };

  const startRepair = async (tool: ToolId) => {
    const fix = localFix(tool, modeOf(tool), health[tool] ?? []);
    if (fix) await applyFix(tool, fix);
  };
  const copyReport = async (tool: ToolId) => {
    const issues = health[tool];
    if (!consented(tool) || !issues) return;
    try {
      await writeText(diagnosticReport(tool, modeOf(tool), props.env, issues, t));
      props.log("ok", t("已复制脱敏诊断信息"));
    } catch { props.log("err", t("复制失败，请重试")); }
  };
  const undoRepair = async (tool: ToolId, backupId: string) => {
    if (await props.onRestore(backupId)) {
      await recheck(tool).catch(() => {});
      setRepair(tool, { phase: "result", level: "ok", title: t("已撤销，配置恢复到修复前") });
    }
  };

  /** 只记 App 写的、不属于任何分组的结果（启动成功 / 失败等）：工具页没有日志面板，用提示条显示几秒 */
  const [flash, setFlash] = useState<LogLine | null>(null);
  const seenLog = useRef(props.logs[props.logs.length - 1]?.id ?? 0);
  useEffect(() => {
    const last = props.logs[props.logs.length - 1];
    if (!last || last.id <= seenLog.current) return;
    seenLog.current = last.id;
    // 修复卡已经在说明结果时（写入、撤销）不再重复弹提示
    const page = props.page === "claude" || props.page === "codex" ? props.page : null;
    if (last.group || (last.kind !== "ok" && last.kind !== "err") || repairing || (page && repair[page])) return;
    setFlash(last);
  }, [props.logs]);
  /** 提示条停留时长：底部倒计时条走完就收起；鼠标悬停时暂停 */
  const flashMs = flash?.kind === "err" ? 8000 : 5000;

  /** 一个工具的完整页面：顶部启动区，下面依次是服务、模型（选了 AI 密钥时）、工作目录（仅 CLI），再下面是输出面板 */
  function toolPage(tool: ToolId) {
    const meta = TOOLS[tool];
    const mode = prefs?.[`${tool}Mode`] ?? "cli";
    const target: LaunchTarget = `${tool}-${mode}`;
    const installed = scan?.installations.find(i => i.target === target);
    const config = cfg(tool);
    // 启动从不写配置：桌面版直接打开；CLI 启动前只确认配置文件没有损坏。
    const passthrough = mode === "desktop";
    const broken = !passthrough && config?.configurationStatus === "broken";
    const { selected } = ctxFor(tool);
    // 列表里哪一项是配置文件里正在生效的：给它挂「当前生效」
    const live = config?.configurationStatus === "official" ? "official"
      : config?.configurationStatus === "configured" ? props.profiles.find(p => p.id === config.activeProfileId && p.hasKey && supportsTool(p, tool))?.id : null;
    // 「切换」只写入配置，不启动应用；启动是另一个动作。
    const activateService = async (value: string) => {
      if (!prefs || savingRef.current || activatingRef.current || props.busy) return;
      activatingRef.current = true; setActivating(true);
      try {
        setRepair(tool, null);
        await props.onSaveConfiguration(tool, mode, value);
      } finally { activatingRef.current = false; setActivating(false); }
    };
    const metaLine = (parts: (string | null | undefined)[], monoFirst?: boolean, action?: React.ReactNode) => <small className="service-meta">{parts.filter(Boolean).map((part, i) => <span key={i} className={monoFirst && i === 0 ? "mono" : undefined} title={part!}>{part}</span>)}{action}</small>;
    const serviceTitle = (title: string, active: boolean, profile?: Profile) => <span className="service-title"><strong>{profile
      ? <button type="button" className="site service-key-title" disabled={disabled} title={t("编辑「{name}」", { name: title })} onClick={() => props.onEditProfile(profile)}>{title}</button>
      : title}</strong>{active && <span className="service-current"><UiIcon name="check"/>{t("使用中")}</span>}</span>;
    const option = (value: string, icon: React.ReactNode, title: string, details: React.ReactNode, opts: { off?: boolean; profile?: Profile } = {}) => {
      const { profile } = opts;
      const className = live === value ? "service-row is-current" : "service-row";
      const switchDisabled = disabled || !!opts.off || !!broken || live === value;
      const contents = <>
        <div className={`service-info ${opts.off ? "is-unavailable" : ""}`}>
          <span className="service-icon" aria-hidden="true">{icon}</span>
          <span className="service-text">{serviceTitle(title, live === value, profile)}{details}</span>
        </div>
        {live !== value && <button type="button" className="service-enable" disabled={switchDisabled} onClick={() => void activateService(value)}
          aria-label={t("切换到 {name}", { name: title })} title={t("写入配置，不启动应用")}>{t("切换")}</button>}
      </>;
      return profile ? <ServiceKeyRow key={value} className={className} title={title} disabled={disabled} actions={[
        { label: "编辑", run: () => props.onEditProfile(profile) },
        { label: "切换", disabled: switchDisabled, run: () => void activateService(value) },
        { label: "测试", disabled: !profile.hasKey, run: () => void props.onTestProfile(profile) },
      ]}>{contents}</ServiceKeyRow> : <div key={value} className={className} role="listitem">{contents}</div>;
    };
    // 当前配置不对应列表里任何一项时（第三方密钥、未保存的 AI 密钥、识别不出……）：
    // 在列表最上面单独一行显示它，读得到密钥的可以直接「加入密钥」
    const status = config?.configurationStatus;
    const needsConnection = needsConnectionGuide(config);
    const guide = needsConnection && !officialTargets.includes(target);
    const inEffect = status !== "unconfigured" && status !== "broken";
    const currentHost = hostOf(config?.currentBaseUrl);
    const currentTitle = config?.unsavedGatewayKey ? t("AI 密钥（未保存）")
      : status === "custom" ? currentHost || t("自定义服务")
      : status === "unconfigured" ? t("还没有配置服务")
      : status === "broken" ? t("配置文件损坏")
      : currentHost ? t("当前配置 · {host}", { host: currentHost }) : t("当前配置");
    const currentHint = status === "broken" ? t("需要先修复")
      : status === "unconfigured" ? t("切换到下面任一项即会写入配置")
      : status === "unknown" && !config?.currentKeyMasked ? t("识别不出具体服务，启动后请在应用内确认")
      : !config?.currentKeyMasked ? t("读不到密钥，不能加入") : null;
    const currentRow = !live && config && <div role="listitem" className={`service-row current-config ${status === "broken" ? "bad" : inEffect ? "is-current" : ""}`}>
      <div className="service-info">
        <span className="service-icon" aria-hidden="true"><UiIcon name={config.currentKeyMasked ? "key" : "settings"}/></span>
        <span className="service-text">{serviceTitle(currentTitle, inEffect)}{metaLine([config.currentKeyMasked, config.currentModel, currentHint], !!config.currentKeyMasked)}
          {config.adoptable && <button type="button" className="site" disabled={disabled} onClick={() => props.onAdopt(tool, tool === "claude" && desktopClaude)} title={t("测试可用后保存到密钥列表，不改动配置")}>{t("加入密钥")}</button>}
        </span>
      </div>
    </div>;
    const issues = health[tool];
    const checking = streaming || refreshing || !scan || issues === null;
    const errors = issues?.filter(i => i.level === "error") ?? [];
    const warns = issues?.filter(i => i.level === "warn") ?? [];
    /** 能修：体检能确定修法，或者有 AI Key 可以让 AI 看 */
    const canRepair = (list: HealthIssue[]) => list.length > 0 && (!!localFix(tool, modeOf(tool), list));
    /** 要你自己处理的错误（密钥失效、连不上、模型不可用）：主按钮直接给对应动作 */
    const blocker = errors.find(i => i.fix === "keys" || i.fix === "settings" || i.fix === "model");
    const blockerAction = blocker ? fixAction(tool, blocker.fix) : null;
    const missing = !!installed && !installed.installed;
    const launchDisabled = disabled || !installed?.installed || !!broken || !props.env && !passthrough;
    const launchNow = async () => {
      if (!prefs || savingRef.current) return;
      setRepair(tool, null);
      const ok = await props.onLaunch(tool, mode, LIVE, prefs.projectDirectory);
      if (ok && mode === "cli") await save({ recentDirectories: [prefs.projectDirectory, ...prefs.recentDirectories.filter(p => p !== prefs.projectDirectory)].slice(0, 8) });
    };
    const r = repair[tool];

    // 主按钮只有一个，随状态变：没装 → 安装；要你处理的错误 → 对应动作；能修的错误 → 一键修复；其余 → 启动
    type Main = { label: string; icon: "terminal" | "desktop" | "sparkle" | "key" | "settings" | "external"; run: () => void; kind: "primary" | "repair"; off?: boolean; title?: string };
    const main: Main = missing ? { label: t("安装 {name}", { name: targetName(target) }), icon: "external", kind: "primary", run: () => setSetupTarget(target) }
      : blockerAction && blocker ? { label: blocker.id === "K1" ? t("换一把密钥") : blockerAction.label, icon: blocker.fix === "settings" ? "settings" : "key", kind: "primary", run: blockerAction.run, off: disabled }
      : errors.length && canRepair(errors) ? { label: t("一键修复"), icon: "sparkle", kind: "repair", run: () => void startRepair(tool), off: disabled }
      : { label: t("启动 {name}", { name: targetName(target) }), icon: mode === "cli" ? "terminal" : "desktop", kind: "primary", run: () => void launchNow(), off: launchDisabled };
    const launchIsMain = !missing && !blocker && !(errors.length && canRepair(errors));
    const secondary: LogAction | null = missing ? { label: t("已安装？设置程序路径"), run: props.onSettings }
      : blocker?.id === "K1" ? { label: t("去控制台 ↗"), run: props.onConsole }
      : !launchIsMain && !launchDisabled ? { label: t("仍然启动"), run: () => void launchNow() }
      : null;

    /**
     * 状态卡：浅色底 + 图标块 + 一行结论，右侧一个轻量动作。高度固定，不管什么状态卡片都不跳；
     * 修复结果的说明、步骤放不下时在卡片里滚动。
     */
    /** 与 statusCard 的分支一致：只有「一切正常」那一行会把次要动作并进来 */
    const statusIsCompactOk = r?.phase !== "result" && !checking && issues !== undefined && !issues.length;
    const statusCard = () => {
      const recheckBtn = <button type="button" className="readiness-action" disabled={disabled} onClick={() => void recheckAll()}>{t("重新检查")}</button>;
      if (r?.phase === "result") return <div className={`readiness-card status-card ${r.level}`} role="status">
        <span className="readiness-icon"><UiIcon name={r.level === "ok" ? "check" : "alert"}/></span>
        <div className="status-card-body">
          <strong>{r.title}</strong>
          {r.detail && <p>{r.detail}</p>}
          {!!r.steps?.length && <ol className="repair-result-steps">{r.steps.map(step => <li key={step}>{step}</li>)}</ol>}
        </div>
        {r.next && <button type="button" className="readiness-action" disabled={disabled} onClick={r.next.run}>{r.next.label}</button>}
        {r.backupId && <button type="button" className="readiness-action" disabled={disabled} onClick={() => void undoRepair(tool, r.backupId!)}>{t("撤销")}</button>}
        <button type="button" className="repair-dismiss" aria-label={t("关闭")} title={t("关闭")} onClick={() => setRepair(tool, null)}><UiIcon name="close"/></button>
      </div>;
      if (checking) return <div className="readiness-card status-card" role="status"><span className="readiness-icon is-spinning"><UiIcon name="refresh"/></span><div className="status-card-body"><strong>{t("正在检查…")}</strong></div></div>;
      if (issues === undefined) return <div className="readiness-card status-card warn"><span className="readiness-icon"><UiIcon name="alert"/></span>
        <div className="status-card-body"><strong>{t("暂时无法检查")}</strong></div>{recheckBtn}</div>;
      if (!issues.length) return <div className="launch-status-bar">
        <span className="launch-ready" role="status"><UiIcon name="check"/>{t("检查通过")}</span>
        <div className="launch-status-actions">{recheckBtn}
          {secondary && <button type="button" className="readiness-action" disabled={disabled} onClick={secondary.run}>{secondary.label}</button>}
        </div>
      </div>;
      return <div className={`readiness-card status-card ${errors.length ? "error" : "warn"}`}>
        <span className="readiness-icon"><UiIcon name="alert"/></span>
        <div className="status-card-body">
          <strong>{errors.length ? t("{n} 个问题会导致启动失败", { n: errors.length }) : t("{n} 个提醒", { n: warns.length })}</strong>
          <p className="clamp" title={issues.map(i => t(i.text)).join(" · ")}>{issues.map(i => t(i.text)).join(" · ")}</p>
        </div>
        {!errors.length && canRepair(warns)
          ? <button type="button" className="readiness-action" disabled={disabled} onClick={() => void startRepair(tool)}><UiIcon name="sparkle"/>{t("一键修复")}</button>
          : recheckBtn}
      </div>;
    };

    /** 修复进行中 / 需要选择时，整块正文换成修复卡 */
    const repairView: RepairView | null = r?.phase === "busy" ? r : null;

    return <div className="tool-page" key={tool}>
      <section className={`tool-hero ${!consented(tool) ? "is-gated" : ""}`} aria-label={t("{tool} 启动", { tool: meta.name })}>
        <div className="tool-hero-head">
          <span className={`launch-brand ${tool}`}><meta.Icon className={`brand brand-${tool}`} /></span>
          <div className="launch-title"><h1>{meta.name}</h1>
            {consented(tool) && !guide && !missing && <ClientInstallStatus key={target} cache={clientVersions} target={target} installation={installed} disabled={disabled || repairing}
              scanning={refreshing} managerOpen={setupTarget === target} onOpen={() => setSetupTarget(target)} />}
          </div>
          {consented(tool) && <div className="launch-modes" role="group" aria-label={t("{tool} 启动方式", { tool: meta.name })}>
            <span className="launch-mode-lens" aria-hidden="true" style={{ transform: `translateX(${mode === "cli" ? 100 : 0}%)` }}/>
            {(["desktop", "cli"] as const).map(m => <button key={m} className="launch-mode-tab" aria-pressed={mode === m} disabled={disabled || repairing || props.guideWorking} onClick={() => save({ [`${tool}Mode`]: m })}><UiIcon name={m === "desktop" ? "desktop" : "terminal"}/>{m === "desktop" ? t("桌面版") : "CLI"}</button>)}
          </div>}
        </div>

        {/* 首次使用：先读取本机配置。点之前什么都不读 */}
        {!consented(tool) ? <div className="read-gate">
          <span className="onboard-mark"><UiIcon name="shield"/></span>
          <strong>{t("开始使用 {tool}", { tool: meta.name })}</strong>
          <span>{t("检查安装和配置，不做修改。")}</span>
          <button className="primary with-icon" disabled={!props.configRead || streaming || props.busy} onClick={() => void readLocalConfig([tool])}>
            <UiIcon name="refresh"/>{streaming ? t("读取中…") : t("检查并继续")}
          </button>
        </div> : missing ? <div className="connection-guide"><h2>{t("未检测到 {name}", { name: targetName(target) })}</h2>
          <button className="primary" disabled={disabled} onClick={() => setSetupTarget(target)}>{t("安装 {name}", { name: targetName(target) })}</button>
          <button className="site" disabled={disabled} onClick={() => void chooseProgram(target)}>{t("已安装？设置程序路径")}</button>
        </div> : !installed || !config ? <div className="connection-guide" role="status"><p>{t(refreshing || streaming || !prefs ? "正在检查…" : "暂时无法检查")}</p><button className="site" disabled={disabled || streaming} onClick={() => void readLocalConfig([tool])}>{t("重新检查")}</button></div>
        : guide ? <ConnectionGuide key={target} tool={tool} disabled={disabled || streaming} working={props.onGuideWorking}
          profiles={props.profiles.filter(profile => profile.hasKey && supportsTool(profile, tool))} defaultUrl={props.defaultUrl} ignoreProxy={props.ignoreProxy} renderImport={props.renderImport}
          apply={id => props.onGuideApply(tool, mode, id)} launchOfficial={async () => {
            if (!prefs) return;
            if (await props.onLaunch(tool, mode, LIVE, prefs.projectDirectory)) setOfficialTargets(previous => [...previous, target]);
          }} /> : repairView ? <RepairCard key={`${tool}-${r?.phase}`} view={repairView} disabled={disabled} /> : <>
        {/* 工作目录（仅 CLI）：启动按钮上方一行，点击打开目录选择 */}
        {mode === "cli" && <button type="button" className="site workdir-inline" aria-haspopup="dialog" aria-label={t("工作目录：{path}，点击更换", { path: prefs?.projectDirectory ?? "" })} title={prefs?.projectDirectory} disabled={disabled} onClick={() => setPicker("workdir")}>
          <UiIcon name="folder"/><span className="workdir-inline-path">{prefs ? prefs.projectDirectory : t("读取中…")}</span><span className="workdir-inline-switch">{t("更换")}</span>
        </button>}
        <div className="launch-controls">
          <button className={`primary launch-button ${main.kind === "repair" ? "is-repair" : ""}`} disabled={main.off} title={main.title} onClick={main.run}><UiIcon name={main.icon}/>{main.label}</button>
          {statusCard()}
          {consented(tool) && issues && <button type="button" className="readiness-action" disabled={disabled} title={t("仅复制体检摘要，不包含密钥、账号信息、本机路径或原始日志。")} onClick={() => void copyReport(tool)}>{t("复制脱敏诊断信息")}</button>}
          {(secondary && !statusIsCompactOk || needsConnection && officialTargets.includes(target)) && <div className="launch-status-actions">
            {secondary && !statusIsCompactOk && <button type="button" className="readiness-action" disabled={disabled} onClick={secondary.run}>{secondary.label}</button>}
            {needsConnection && officialTargets.includes(target) && <button type="button" className="readiness-action" disabled={disabled} onClick={() => setOfficialTargets(previous => previous.filter(item => item !== target))}>{t("更换连接方式")}</button>}
          </div>}
        </div>
        <div className="launch-configuration">

        {/* 服务：只有一个状态「当前生效」，启动就用它；其余项点「切换」立即写入配置 */}
        <div className="service-head">
          <span className="config-label">{t("服务")}</span>
        </div>
        <div className="service-list inline" role="list" aria-label={t("{tool} 使用服务", { tool: meta.name })} ref={serviceListRef}>
          {currentRow}
          {option("official", <meta.Icon className={`brand brand-${tool}`}/>, t("官方服务"), metaLine([t("使用你自己的官方账号登录")]))}
          {props.profiles.map(profile => {
            const unavailable = !profile.hasKey ? t("缺少密钥，请到密钥页补上")
              : !supportsTool(profile, tool) ? t("没有填 {tool} 地址", { tool: meta.cli }) : null;
            const model = modelOf(tool, profile) || t("默认模型");
            const details = unavailable ? metaLine([unavailable])
              : metaLine([profile.maskedKey], true, <button type="button" className="site service-model" disabled={disabled} aria-haspopup="dialog"
                  aria-label={t("{tool} 模型：{model}，点击更换", { tool: meta.name, model })} title={model} onClick={() => openModel(tool, profile)}>
                  <span>{model}</span><b aria-hidden="true">▾</b>
                </button>);
            return option(profile.id, <UiIcon name="key"/>, profileTitle(profile), details, { off: !!unavailable, profile });
          })}
        </div>

        {tool === "claude" && selected?.custom && <small className="dim">{t("第三方 Claude 服务未进行推理连通性验证。")}</small>}

        </div>

        </>}
      </section>

      {picker === "workdir" && <PickerDialog title={t("CLI 工作目录")} onClose={() => setPicker(null)}>
        <div className="workdir-list" role="radiogroup" aria-label={t("工作目录")}>
          {(prefs ? [prefs.projectDirectory, ...recent] : []).map(path => <button key={path} type="button" role="radio" aria-checked={path === prefs?.projectDirectory} className="service-option" disabled={disabled}
            onClick={() => { setPicker(null); if (path !== prefs?.projectDirectory) void save({ projectDirectory: path }); }}>
            <span className="service-radio" aria-hidden="true"/>
            <span className="service-text"><strong>{folderName(path)}</strong><small className="service-meta"><span title={path}>{path}</span></small></span>
          </button>)}
        </div>
        <button type="button" className="workdir-pick" disabled={disabled} onClick={() => { setPicker(null); void chooseFolder(); }}><UiIcon name="folder"/>{t("选择其他文件夹…")}</button>
      </PickerDialog>}

      {picker === "model" && (() => {
        const target = props.profiles.find(p => p.id === modelProfileId && p.hasKey);
        if (!target) return null;
        // 只有使用中的那把换完模型要立即写入配置；其他密钥只记下，切换到它时再写入
        const inUse = selected?.id === target.id;
        const list = modelList?.profileId === target.id ? modelList : null;
        // 本工具常用的那一家排在最前面
        const first = tool === "claude" ? "Claude" : "GPT";
        // 按网关目录标出这个工具用不了的模型（只在别的接口上有渠道），放到各组最后并置灰
        const infoOf = new Map((list?.models ?? []).map(m => [m.id, m]));
        const fits = (id: string) => supports(infoOf.get(id), tool);
        const groups = groupByFamily((list?.models ?? []).map(m => m.id))
          .map(g => ({ ...g, items: [...g.items].sort((a, b) => Number(fits(b)) - Number(fits(a))) }))
          .sort((a, b) => Number(b.items.some(fits)) - Number(a.items.some(fits)) || Number(b.name === first) - Number(a.name === first));
        const value = modelEdit.model.trim();
        const unchanged = value === modelOf(tool, target) && (tool !== "codex" || modelEdit.effort === (target.codexEffort || "medium"));
        return <PickerDialog title={t("{tool} 使用的模型", { tool: meta.name })} onClose={() => setPicker(null)} className="model-dialog"
          footer={<button className="primary sm" disabled={disabled || !value || unchanged} onClick={async () => {
            const ok = await props.onSaveModel(target.id, tool === "codex" ? { codexModel: value, codexEffort: modelEdit.effort } : { claudeModel: value }, inUse ? { tool, mode } : undefined);
            if (ok) setPicker(null);
          }}>{t("保存")}</button>}>
          <p className="settings-note">{t("为「{name}」选择或填写 {tool} 模型。", { name: target.name, tool: meta.name })}</p>
          <div className="model-field">
            <label className="model-field-label" htmlFor="model-input">{t("模型")}</label>
            <input id="model-input" className="path-input mono" value={modelEdit.model} placeholder={t("填写模型名")} spellCheck={false} autoComplete="off" disabled={disabled}
              onChange={e => setModelEdit(m => ({ ...m, model: e.target.value }))}/>
          </div>
          {tool === "codex" && <div className="model-field">
            <span className="model-field-label" id="effort-label">{t("思考深度")}</span>
            {/* 分段选择代替原生下拉框（系统下拉菜单无法统一样式）；说明只显示选中那一档的 */}
            <div className="effort-seg" role="radiogroup" aria-labelledby="effort-label">
              {EFFORTS.map(x => <button key={x.value} type="button" role="radio" aria-checked={modelEdit.effort === x.value} className="effort-opt" disabled={disabled}
                title={t(x.label)} onClick={() => setModelEdit(m => ({ ...m, effort: x.value }))}>{effortLabel(x.value)}</button>)}
            </div>
            {(() => { const hint = t(EFFORTS.find(x => x.value === modelEdit.effort)?.label ?? "").split(" — ")[1]; return hint && <small className="effort-hint">{hint}</small>; })()}
          </div>}
          <div className="model-field">
          <span className="model-field-label">{t("可用模型")}</span>
          <div className="model-list" role="radiogroup" aria-label={t("「{name}」可用的模型", { name: target.name })}>
            {!list?.models ? <p className="settings-note">{t("正在加载可用模型…")}</p>
              : list.error ? <p className="settings-note warn-text">{t("模型加载失败：{error}。可手动填写。", { error: list.error })}</p>
              : !groups.length ? <p className="settings-note">{t("暂无模型，请手动填写。")}</p>
              : groups.map(g => <div className="model-group" key={g.name}>
                <div className="model-group-label">{t(g.name)}</div>
                {g.items.map(m => <button key={m} type="button" role="radio" aria-checked={m === value} className="model-option mono" disabled={disabled || !fits(m)}
                  title={fits(m) ? undefined : t("不支持 {tool} 使用的接口", { tool: meta.name })}
                  onClick={() => setModelEdit(x => ({ ...x, model: m }))}>{m}{!fits(m) ? <span className="model-tag">{t("不支持 {tool}", { tool: meta.name })}</span> : m === value && <UiIcon name="check"/>}</button>)}
              </div>)}
          </div>
          </div>
        </PickerDialog>;
      })()}
    </div>;
  }

  return <div className="launcher-pages" hidden={props.page === "keys"}>
    {props.page === "welcome" && <section className="tool-hero connection-guide"><h1>{t("选择工具")}</h1>
      {TOOL_IDS.map(tool => { const { name, Icon } = TOOLS[tool]; return <button key={tool} onClick={() => props.onPage(tool)}><Icon className={"brand brand-" + tool}/><strong>{name}</strong><UiIcon name="arrow"/></button>; })}
    </section>}
    {(props.page === "claude" || props.page === "codex") && toolPage(props.page)}
    {props.page === "settings" && <div className="settings-page">
      <div className="launch-heading"><h1>{t("设置")}</h1><button className="icon-round" disabled={disabled} title={refreshing ? t("检测中…") : t("重新检测程序")} aria-label={t("重新检测程序")} onClick={refresh}><UiIcon name="refresh"/></button></div>

      {/* 界面语言：简体 / 繁體 / English，默认跟随系统 */}
      <h2 className="settings-section">{t("语言")}</h2>
      <div className="lang-switch" role="radiogroup" aria-label={t("界面语言")} style={{ "--lang-count": LANGS.length } as React.CSSProperties}>
        <span className="lang-lens" aria-hidden="true" style={{ transform: `translateX(${LANGS.findIndex(l => l.id === lang) * 100}%)` }}/>
        {LANGS.map(l => <button key={l.id} className="lang-tab" type="button" role="radio" aria-checked={lang === l.id} onClick={() => setLang(l.id)}>{l.label}</button>)}
      </div>

      {/* 程序路径：每个程序一行，显示检测结果；需要手动指定时点「更改」弹窗 */}
      <h2 className="settings-section">{t("程序路径")}</h2>
      <section className="settings-card">
        {TARGETS.map(target => {
          const installation = scan?.installations.find(i => i.target === target);
          const custom = !!prefs?.customPaths[target];
          const tool = target.startsWith("claude-") ? "claude" : "codex";
          const Icon = TOOLS[tool].Icon;
          return <div className="setting-row" key={target}>
            <span className={`setting-brand ${tool}`} aria-hidden="true">
              <Icon className={`brand brand-${tool}`} />
              <span className={`setting-dot ${installation ? installation.installed ? "found" : "missing" : ""}`} />
            </span>
            <div className="setting-text">
              <strong>{targetName(target)}</strong>
              <span className="setting-sub" title={installation?.path || (installation && t(installation.message))}>{!installation ? (refreshing ? t("检测中…") : t("待检测")) : installation.installed ? installation.path || t("已检测到") : t("未检测到")}{custom ? ` · ${t("手动指定")}` : ""}</span>
            </div>
            <button type="button" className="site" disabled={disabled} onClick={() => setSetupTarget(target)}>{t("安装指引")}</button>
            <button type="button" className="site" disabled={disabled} onClick={() => setPathEdit(target)}>{t("更改")}</button>
          </div>;
        })}
      </section>

      <h2 className="settings-section">{t("高级")}</h2>
      <section className="settings-card">
        {/* 只重新读取已经同意过的工具；一个都没读过时先去工具页读 */}
        <button type="button" className="setting-row setting-link" onClick={() => void readLocalConfig(readTools)} disabled={streaming || props.busy || !readTools.length}>
          <div className="setting-text"><strong>{streaming ? t("读取中…") : t("重新读取本机配置")}</strong><span className="setting-sub">{readTools.length ? t("读取安装、配置和环境变量，详情见诊断日志。") : t("还没有读取过，请先到 Claude 或 Codex 页面读取")}</span></div>
          <span className="setting-chevron" aria-hidden="true"/>
        </button>
        <button type="button" className="setting-row setting-link" onClick={props.onAdvanced} disabled={props.busy}>
          <div className="setting-text"><strong>{t("高级设置")}</strong><span className="setting-sub">{t("服务地址、模型别名、代理、配置备份与回滚")}</span></div>
          <span className="setting-chevron" aria-hidden="true"/>
        </button>
      </section>

      <UpdatePanel />

      {/* 诊断日志：体检、写入、启动、AI 分析的完整记录。工具页不再常驻，排查时到这里看 */}
      <h2 className="settings-section">{t("诊断日志")}</h2>
      <OutputConsole lines={props.logs} busy={disabled} onClear={props.onClearLogs} dropped={props.droppedLogs} />

      {pathEdit && <PickerDialog title={t("{name} 程序路径", { name: targetName(pathEdit) })} onClose={() => setPathEdit(null)}
        footer={<button className="primary sm" disabled={disabled} onClick={async () => { await savePath(pathEdit, pathDrafts[pathEdit] ?? ""); setPathEdit(null); }}>{t("保存")}</button>}>
        <p className="settings-note">{t("留空表示自动检测。")}{(() => { const found = scan?.installations.find(i => i.target === pathEdit); return found?.installed && found.path ? t("当前检测到：{path}", { path: found.path }) : t("目前没有检测到这个程序。"); })()}</p>
        <label className="sr-only" htmlFor="program-path-input">{t("{name} 程序路径", { name: targetName(pathEdit) })}</label>
        <input id="program-path-input" className="path-input" value={pathDrafts[pathEdit] ?? ""} disabled={disabled} placeholder={t("自动检测（留空）")} spellCheck={false}
          onChange={e => setPathDrafts(p => ({ ...p, [pathEdit]: e.target.value }))}/>
        <div className="path-actions">
          <button type="button" disabled={disabled} onClick={async () => { await chooseProgram(pathEdit); setPathEdit(null); }}><UiIcon name="folder"/>{t("浏览选择…")}</button>
          {!!prefs?.customPaths[pathEdit] && <button type="button" className="ghost" disabled={disabled} onClick={async () => { await savePath(pathEdit, ""); setPathEdit(null); }}>{t("恢复自动检测")}</button>}
        </div>
      </PickerDialog>}
    </div>}
    {setupTarget && <PickerDialog title={t(scan?.installations.find(i => i.target === setupTarget)?.installed ? "管理 {name}" : "安装 {name}", { name: targetName(setupTarget) })} onClose={() => setSetupTarget(null)}>
      <ClientSetup key={setupTarget} onVersion={value => clientVersions.set(setupTarget, scan?.installations.find(i => i.target === setupTarget)?.path, value)} target={setupTarget} installation={scan?.installations.find(i => i.target === setupTarget)} refresh={refresh} chooseProgram={() => chooseProgram(setupTarget)} onInstalled={async () => { const p = await api.loadLauncherPreferences(); prefsRef.current = p; setPrefs(p); setPathDrafts(p.customPaths); }} onDone={() => setSetupTarget(null)} disabled={disabled} />
    </PickerDialog>}
    {error ? <div className="toast err" role="alert">{t(error)}</div>
      : flash && (props.page === "claude" || props.page === "codex") && <div key={flash.id} className={`toast ${flash.kind} flash`} role={flash.kind === "err" ? "alert" : "status"} onClick={() => setFlash(null)} style={{ "--flash-ms": `${flashMs}ms` } as React.CSSProperties}>
        <span className="flash-text">{flash.text}</span>
        {flash.undo && <button type="button" className="flash-undo" disabled={props.busy} onClick={event => { event.stopPropagation(); const undo = flash.undo!; setFlash(null); undo(); }}>{t("撤销")}</button>}
        <span className="flash-timer" aria-hidden="true" onAnimationEnd={() => setFlash(null)}/>
      </div>}
  </div>;
}
