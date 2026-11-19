import { useCallback, useEffect, useRef, useState } from "react";
import { UiIcon } from "./UiIcon";
import { t } from "./i18n";
import * as api from "./api";
import { installedKinds } from "./InstallStep";
import type { ConfigurationStatus, Environment, HealthIssue, LaunchMode, LaunchTarget, LauncherScan, ToolId } from "./types";

export type StageId = "install" | "version" | "config" | "health";
type StageStatus = "pending" | "running" | "ok" | "warn" | "error";
type Stage = { status: StageStatus; summary: string };
/** 体检发现的问题 */
export type GuideIssue = { tool: ToolId; level: "error" | "warn"; text: string };

const STAGES: StageId[] = ["install", "version", "config", "health"];
const STAGE_LABEL: Record<StageId, string> = { install: "检测安装", version: "读取版本", config: "读取配置", health: "体检" };
const TOOLS: ToolId[] = ["claude", "codex"];
const TOOL_NAME: Record<ToolId, string> = { claude: "Claude Code", codex: "Codex" };
/** 每项至少停留这么久，进度不至于一闪而过 */
const MIN_STAGE_MS = 450;

const sleep = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
async function paced<T>(work: Promise<T>): Promise<T> { const [value] = await Promise.all([work, sleep(MIN_STAGE_MS)]); return value; }
const reason = (cause: unknown) => t(String(cause).replace(/^Error:\s*/, ""));

export function configLabel(status: ConfigurationStatus) {
  return t(status === "configured" ? "已配置 AI" : status === "custom" ? "已配置其他服务" : status === "official" ? "使用官方账号" : status === "broken" ? "配置损坏" : "未配置");
}
const READY: ConfigurationStatus[] = ["configured", "custom", "official"];

const freshStages = (): Record<StageId, Stage> => ({ install: { status: "pending", summary: "" }, version: { status: "pending", summary: "" }, config: { status: "pending", summary: "" }, health: { status: "pending", summary: "" } });

/**
 * 引导里的整机检查：检测安装 → 读取版本 → 读取配置 → 体检，依次执行并报告进度。
 * 某一项失败不会中断后面几项；装完程序后回到窗口、或安装完成时，只重新检测安装并补读版本。
 */
export function useEnvCheck({ ignoreProxy, readEnvironment, checkHealth }: {
  ignoreProxy: boolean;
  /** 记下用户同意读取两个工具的配置，并读回本机配置 */
  readEnvironment: () => Promise<Environment | null>;
  checkHealth: (tool: ToolId, service: string, mode: LaunchMode) => Promise<HealthIssue[]>;
}) {
  const [phase, setPhase] = useState<"idle" | "running" | "done">("idle");
  const [stages, setStages] = useState(freshStages);
  const [scan, setScan] = useState<LauncherScan | null>(null);
  const [scanFailed, setScanFailed] = useState(false);
  const [env, setEnv] = useState<Environment | null>(null);
  const [issues, setIssues] = useState<GuideIssue[]>([]);
  const [versions, setVersions] = useState<Record<string, api.ClientVersion | null>>({});
  const requested = useRef(new Map<string, Promise<api.ClientVersion | null>>());
  const running = useRef(false);
  const scanLock = useRef(false);
  const set = (id: StageId, status: StageStatus, summary = "") => setStages(current => ({ ...current, [id]: { status, summary } }));

  const keyOf = (scanned: LauncherScan | null, target: LaunchTarget) => `${target}|${scanned?.installations.find(item => item.target === target)?.path ?? ""}`;
  /** 读已安装程序的版本；读过的不重复读 */
  const loadVersions = useCallback((scanned: LauncherScan) => Promise.all(scanned.installations.filter(item => item.installed).map(item => {
    const key = `${item.target}|${item.path ?? ""}`;
    let pending = requested.current.get(key);
    if (!pending) {
      pending = api.checkClientVersion(item.target, ignoreProxy, true).catch(() => null);
      requested.current.set(key, pending);
      void pending.then(value => setVersions(current => ({ ...current, [key]: value })));
    }
    return pending;
  })), [ignoreProxy]);

  /** 只重新检测安装（回到窗口、装完程序后），顺带补读新装程序的版本 */
  const rescan = useCallback(async () => {
    if (scanLock.current || running.current) return;
    scanLock.current = true;
    try { const result = await api.scanLaunchers(); setScan(result); setScanFailed(false); void loadVersions(result); }
    catch { setScanFailed(true); }
    finally { scanLock.current = false; }
  }, [loadVersions]);

  const run = useCallback(async () => {
    if (running.current) return;
    running.current = true;
    setPhase("running"); setStages(freshStages()); setIssues([]);
    requested.current.clear(); setVersions({});

    set("install", "running");
    let scanned: LauncherScan | null = null;
    try {
      scanned = await paced(api.scanLaunchers()); setScan(scanned); setScanFailed(false);
      const found = scanned.installations.filter(item => item.installed).length;
      set("install", "ok", t("找到 {n} / {total} 个程序", { n: found, total: scanned.installations.length }));
    } catch (cause) { setScanFailed(true); set("install", "error", reason(cause)); }

    set("version", "running");
    if (!scanned) set("version", "warn", t("未检测到安装，已跳过"));
    else {
      const list = await paced(loadVersions(scanned));
      const outdated = list.filter(value => value?.updateAvailable).length, failed = list.filter(value => !value).length;
      set("version", failed && failed === list.length ? "warn" : "ok",
        !list.length ? t("没有已安装的程序") : failed === list.length ? t("版本读取失败") : outdated ? t("{n} 个可更新", { n: outdated }) : t("都是最新版"));
    }

    set("config", "running");
    let environment: Environment | null = null;
    try {
      environment = await paced(readEnvironment());
      if (!environment) throw new Error(t("读取失败"));
      setEnv(environment);
      const broken = TOOLS.some(tool => environment![tool].configurationStatus === "broken");
      set("config", broken ? "warn" : "ok", TOOLS.map(tool => `${TOOL_NAME[tool]}${t("：")}${configLabel(environment![tool].configurationStatus)}`).join(t("；")));
    } catch (cause) { set("config", "error", reason(cause)); }

    set("health", "running");
    try {
      // 装了哪种就按哪种查；都没装但已有配置的，按命令行查
      const checks = TOOLS.flatMap(tool => {
        const kinds = installedKinds(scanned, tool) ?? [];
        const modes = kinds.length ? kinds : environment && READY.includes(environment[tool].configurationStatus) ? ["cli" as const] : [];
        return modes.map(mode => ({ tool, mode }));
      });
      // 体检本身会报环境变量覆盖，不再单独查
      const results = await paced(Promise.all(checks.map(({ tool, mode }) => checkHealth(tool, "current", mode).then(list => list.map(issue => ({ tool, level: issue.level, text: t(issue.text) }))).catch(() => [] as GuideIssue[]))));
      const found: GuideIssue[] = [];
      for (const issue of results.flat()) if (!found.some(item => item.tool === issue.tool && item.text === issue.text)) found.push(issue);
      setIssues(found);
      set("health", found.length ? "warn" : "ok", found.length ? t("{n} 个问题", { n: found.length }) : t("未发现问题"));
    } catch (cause) { set("health", "error", reason(cause)); }

    running.current = false;
    setPhase("done");
  }, [loadVersions, readEnvironment, checkHealth]);

  // 安装器或官网装完回到窗口时，重新检测安装
  useEffect(() => {
    if (phase !== "done") return;
    const onReturn = () => { if (document.visibilityState !== "hidden") void rescan(); };
    window.addEventListener("focus", onReturn); document.addEventListener("visibilitychange", onReturn);
    return () => { window.removeEventListener("focus", onReturn); document.removeEventListener("visibilitychange", onReturn); };
  }, [phase, rescan]);

  const versionOf = (target: LaunchTarget) => versions[keyOf(scan, target)];
  /** 装完或更新完：丢掉这一项的版本，重新检测后再读 */
  const afterInstall = async (target: LaunchTarget) => {
    const key = keyOf(scan, target);
    requested.current.delete(key);
    setVersions(current => { const next = { ...current }; delete next[key]; return next; });
    await rescan();
  };
  const failed = STAGES.some(id => stages[id].status === "error");
  return { phase, stages, scan, scanFailed, env, issues, failed, run, rescan, versionOf, afterInstall };
}

/** 检查进度：进行中逐项显示；完成后收成一行总结，可展开看每项结果 */
export function CheckPanel({ check, busy }: { check: ReturnType<typeof useEnvCheck>; busy: boolean }) {
  const { phase, stages, issues, failed } = check;
  const [open, setOpen] = useState(false);
  const finished = STAGES.filter(id => !["pending", "running"].includes(stages[id].status)).length;
  const runningNow = STAGES.some(id => stages[id].status === "running");
  const percent = Math.round((finished + (runningNow ? .5 : 0)) / STAGES.length * 100);
  const list = <ol className="check-stages">{STAGES.map(id => {
    const { status, summary } = stages[id];
    return <li key={id} className={`check-stage ${status}`}>
      <span className="check-mark" aria-hidden="true">{status === "running" ? <span className="check-spinner"/> : status === "ok" ? <UiIcon name="check"/> : status === "warn" || status === "error" ? <UiIcon name="alert"/> : null}</span>
      <span className="check-text"><strong>{t(STAGE_LABEL[id])}</strong>{summary && <small>{summary}</small>}</span>
    </li>;
  })}</ol>;

  if (phase !== "done") return <div className="check-panel" role="status" aria-live="polite">
    <div className="check-progress"><span style={{ width: `${percent}%` }}/></div>
    {list}
  </div>;

  return <div className={`check-panel done ${failed ? "failed" : issues.length ? "issues" : ""}`}>
    <button type="button" className="check-summary" aria-expanded={open} onClick={() => setOpen(value => !value)}>
      <UiIcon name={failed || issues.length ? "alert" : "check"}/>
      <span>{failed ? t("检查未全部完成") : issues.length ? t("检查完成 · {n} 个问题", { n: issues.length }) : t("检查完成 · 未发现问题")}</span>
      <span className="check-toggle">{t(open ? "收起" : "详情")}</span>
    </button>
    {open && list}
    {failed && <button type="button" className="check-retry" disabled={busy} onClick={() => void check.run()}><UiIcon name="refresh"/>{t("重试")}</button>}
  </div>;
}
