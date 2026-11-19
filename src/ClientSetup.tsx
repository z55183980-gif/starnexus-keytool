import { useEffect, useRef, useState } from "react";
import * as api from "./api";
import { t, useLang } from "./i18n";
import { useUpdates } from "./Updates";
import { UiIcon } from "./UiIcon";
import type { LauncherInstallation, LauncherScan, LaunchTarget } from "./types";
import "./ClientSetup.css";
import { clientUpdateState } from "./clientUpdateState";

export function ClientSetup({ target, installation, refresh, chooseProgram, onInstalled, onDone, disabled, onVersion }: {
  target: LaunchTarget; installation?: LauncherInstallation;
  refresh: () => Promise<LauncherScan | null>;
  chooseProgram: () => Promise<LauncherScan | null | undefined>;
  onInstalled: () => Promise<void>;
  onDone: () => void; disabled: boolean;
  onVersion?: (version: api.ClientVersion) => void;
}) {
  useLang();
  const { ignoreProxy } = useUpdates();
  const [guide, setGuide] = useState<api.ClientGuide | null>(null);
  const [guideError, setGuideError] = useState(false);
  const [version, setVersion] = useState<api.ClientVersion | null>(null);
  const onVersionRef = useRef(onVersion); onVersionRef.current = onVersion;
  useEffect(() => { if (version) onVersionRef.current?.(version); }, [version]);
  const [readingVersion, setReadingVersion] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [copied, setCopied] = useState(false);
  const [variantIndex, setVariantIndex] = useState(0);
  const [installStarted, setInstallStarted] = useState(false);
  const [progress, setProgress] = useState<api.InstallProgress | null>(null);
  const [phase, setPhase] = useState<"idle" | "scan" | "version" | "path" | "external">("idle");
  const [scanResult, setScanResult] = useState<"found" | "missing" | "error" | null>(null);
  const active = useRef(false), busy = useRef(false), returnPending = useRef(false);
  const versionRequest = useRef(0), pendingVersion = useRef<string | null>(null);
  const installed = !!installation?.installed, cli = target.endsWith("cli");
  const locked = disabled || phase !== "idle";
  const updateState = clientUpdateState(installed, version, readingVersion || phase === "version", installStarted);
  useEffect(() => {
    if (version && pendingVersion.current !== null && version.current !== pendingVersion.current) {
      setInstallStarted(false); pendingVersion.current = null; setNotice("");
    }
  }, [version]);
  const selectedCommand = guide?.variants[variantIndex] ?? guide;
  useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
  const loadGuide = async () => {
    setGuideError(false);
    try { const result = await api.clientInstallGuide(target); if (active.current) setGuide(result); }
    catch { if (active.current) setGuideError(true); }
  };
  useEffect(() => { void loadGuide(); }, [target]);
  // Opening the manager checks the installed and available versions together.
  useEffect(() => {
    const request = ++versionRequest.current;
    setVersion(null);
    if (!installed) { setReadingVersion(false); return; }
    let cancelled = false;
    setReadingVersion(true);
    void api.checkClientVersion(target, ignoreProxy, true)
      .then(result => { if (!cancelled && request === versionRequest.current) setVersion(result); })
      .catch(() => {})
      .finally(() => { if (!cancelled && request === versionRequest.current) setReadingVersion(false); });
    return () => { cancelled = true; };
  }, [target, installed, installation?.path, ignoreProxy]);
  const readVersion = async (checkLatest: boolean) => {
    const request = ++versionRequest.current;
    setReadingVersion(true); setVersion(null);
    try {
      const result = await api.checkClientVersion(target, ignoreProxy, checkLatest);
      if (active.current && request === versionRequest.current) {
        setVersion(result);
      }
      return result;
    } finally { if (active.current && request === versionRequest.current) setReadingVersion(false); }
  };
  const run = async (kind: typeof phase, action: () => Promise<void>) => {
    if (busy.current || disabled) return;
    busy.current = true; setPhase(kind); setError(""); setNotice("");
    try { await action(); }
    catch (e) { if (active.current) setError(String(e).replace(/^Error:\s*/, "")); }
    finally { busy.current = false; if (active.current) setPhase("idle"); }
  };
  const acceptScan = async (result: LauncherScan | null, initial = false) => {
    if (!active.current) return;
    if (!result) { ++versionRequest.current; setVersion(null); setReadingVersion(false); setScanResult("error"); return; }
    const found = result.installations.find(i => i.target === target);
    setScanResult(initial ? null : found?.installed ? "found" : "missing"); setVersion(null);
    if (found?.installed) {
      try { await readVersion(true); }
      catch { /* A readable version is not required for a successful installation scan. */ }
    }
  };
  const scan = (initial = false) => run("scan", async () => {
    try { await acceptScan(await refresh(), initial); }
    catch (e) { if (active.current) setScanResult("error"); throw e; }
  });
  const latest = useRef({ scan, disabled }); latest.current = { scan, disabled };
  const initialScan = useRef(false);
  useEffect(() => {
    if (!installation && !disabled && !initialScan.current) {
      initialScan.current = true; void latest.current.scan(true);
    }
  }, [installation, disabled]);
  useEffect(() => {
    const onReturn = () => {
      if (document.visibilityState === "hidden" || !returnPending.current || busy.current || latest.current.disabled) return;
      returnPending.current = false; void latest.current.scan();
    };
    const onLeave = () => { if (!installed) returnPending.current = true; };
    window.addEventListener("blur", onLeave);
    window.addEventListener("focus", onReturn); document.addEventListener("visibilitychange", onReturn);
    return () => { window.removeEventListener("blur", onLeave); window.removeEventListener("focus", onReturn); document.removeEventListener("visibilitychange", onReturn); };
  }, [installed]);
  const openDocs = () => run("external", async () => {
    await api.openClientDownload(target); returnPending.current = true;
    if (active.current) setNotice(api.isSetupPreview ? "预览模式：未打开外部页面" : "返回后自动检测。");
  });
  const startInstall = () => run("external", async () => {
    if (installed) {
      if (updateState !== "available" || failed || missing) return;
      const checked = await readVersion(true);
      if (checked.updateAvailable !== true || checked.warning) return;
    } else if (!installation || failed || installStarted) return;
    setProgress({ phase: "resolve", downloaded: 0, total: null });
    try {
      const result = await api.runClientInstall(target, value => { if (active.current) setProgress(value); });
      if (result.installed) await onInstalled();
      if (!active.current) return;
      pendingVersion.current = result.installed ? null : version?.current ?? "";
      setInstallStarted(!result.installed); setNotice(result.message); returnPending.current = !result.installed;
      if (result.installed) await acceptScan(await refresh());
    } finally { if (active.current) setProgress(null); }
  });
  const missing = scanResult === "missing", failed = scanResult === "error";
  const status = phase === "scan" ? "正在检测安装…" : failed ? "检测未完成" : missing ? "仍未找到程序"
    : installed ? scanResult === "found" ? "已找到程序，可以使用" : "已安装"
    : installation ? "尚未检测到安装" : "尚未检查安装状态";
  return <section className="client-setup" aria-label={t("客户端安装与更新")} aria-busy={phase !== "idle"}>
    <div className={`setup-status ${failed || missing ? "is-warning" : installed ? "is-ready" : ""}`} role="status">
      <UiIcon name={phase === "scan" ? "refresh" : installed && !failed && !missing ? "check" : "alert"}/>
      <div><strong>{t(status)}</strong>{installed && <p className="setup-hint">{version ? t("本机版本：{version}", { version: version.current }) : t(readingVersion ? "正在读取本机版本…" : "版本读取失败，请重试检查。")}</p>}</div>
    </div>
    {!cli && guide?.platform === "windows" && (!installed || updateState === "available") && <p className="setup-hint">{t("需管理员授权，安装到此电脑；可能需重新登录 Windows。")}</p>}
    {installed ? <>
      {installation?.path && <details className="setup-details"><summary>{t("程序路径")}</summary><p className="setup-path">{installation.path}</p></details>}
      {cli && updateState === "available" && <p className="setup-hint">{t("独立安装，不影响终端原有版本。")}</p>}
      {!(version?.warning && updateState === "unknown") && !(notice && updateState === "pending") && <p role="status">{t(updateState === "checking" ? "正在检查版本…" : updateState === "pending" ? "完成系统安装后，请重新检测。" : updateState === "available" ? "发现新版本 {version}" : updateState === "current" ? "已是最新版，无需更新。" : "检查更新后可安装新版本。", { version: version?.latest ?? "" })}</p>}
      {version?.warning && <p className="setup-hint" role="status">{t(version.warning)}</p>}
      {updateState === "available" && !failed && !missing && <button className="primary" disabled={locked} onClick={() => void startInstall()}>{t("更新至 {version}", { version: version!.latest! })}</button>}
      {updateState === "pending" && <button disabled={locked} onClick={() => void scan()}>{t("检测安装结果")}</button>}
    </> : <>
      <p>{t("自动匹配系统，下载并校验安装包。")}</p>
      {cli && <p className="setup-hint">{t("独立安装，不影响终端原有版本。")}</p>}
      <button className="primary" disabled={locked || installStarted || !installation || failed} onClick={() => void startInstall()}><UiIcon name="terminal"/>{t(progress ? "正在准备安装…" : installStarted ? "已准备安装" : "运行安装")}</button>
      {guideError ? <div className="setup-error" role="alert"><p>{t("安装说明加载失败，请重试。")}</p><button disabled={locked} onClick={() => void loadGuide()}>{t("重试")}</button></div>
      : !guide ? <p className="setup-hint">{t("正在读取安装说明…")}</p>
      : guide.command ? <details className="setup-details"><summary>{t("手动安装命令")}</summary>
        <div className="setup-command-card">
          <div className="setup-command-heading"><span><UiIcon name="terminal"/>{guide.variants.length > 1
            ? <select aria-label={t("命令适用终端")} value={variantIndex} disabled={locked} onChange={e => { setVariantIndex(Number(e.target.value)); setCopied(false); }}>
              {guide.variants.map((v, i) => <option key={v.shell} value={i}>{v.shell}</option>)}
            </select> : selectedCommand?.shell}</span>
            <div className="setup-command-actions"><button disabled={locked} aria-label={t("复制安装命令")} onClick={() => void navigator.clipboard.writeText(selectedCommand!.command!).then(() => {
              if (!active.current) return;
              setCopied(true); setError(""); returnPending.current = true; setNotice("已复制，请在终端粘贴执行。");
            }).catch(() => setError("复制失败，请手动选择命令复制"))}>{t(copied ? "已复制" : "复制")}</button>
            </div>
          </div>
          <pre className="install-command" tabIndex={0} aria-label={t("安装命令")}><code>{selectedCommand?.command}</code></pre>
        </div>
        <p className="setup-hint">{t("安装文件来自客户端官方源。")}</p>
        {target === "codex-cli" && <p className="setup-hint">{t("需要先安装 Node.js 和 npm。")}</p>}
        {!guide.canRun && <button className="setup-terminal" disabled={locked} onClick={() => void run("external", async () => {
          await api.openSetupTerminal(); returnPending.current = true;
          if (active.current) setNotice(api.isSetupPreview ? "预览模式：未打开真实终端" : "在终端粘贴执行，完成后返回检测。");
        })}><UiIcon name="terminal"/>{t("打开 {shell}", { shell: guide.shell })}</button>}
      </details> : null}
      {installStarted && <button className="setup-docs" disabled={locked} onClick={() => { setInstallStarted(false); setNotice(""); }}>{t("安装未成功？允许重新运行")}</button>}
    </>}
    {progress && <div className="setup-progress" role="status"><p>{t(progress.phase === "resolve" ? "正在获取安装信息…" : progress.phase === "verify" ? "正在校验安装包…" : progress.phase === "install" ? "正在安装，请留意系统提示…" : progress.phase === "relay" ? "正在通过中继下载…" : "正在下载安装包…")}</p><progress max={progress.total ?? 1} value={progress.total ? progress.downloaded : undefined}/>{progress.total && <span>{Math.min(100, Math.round(progress.downloaded / progress.total * 100))}%</span>}</div>}
    {(missing || failed) && <div className="setup-recovery">
      <p>{t(failed ? "检测出错，请重试；这不代表程序未安装。" : "已安装？请选择程序路径。")}</p>
      {missing && <button disabled={locked} onClick={() => void run("path", async () => {
        if (api.isSetupPreview) { setNotice("预览模式：未打开文件选择器"); return; }
        const result = await chooseProgram(); if (result !== undefined) await acceptScan(result);
      })}><UiIcon name="folder"/>{t("选择程序路径")}</button>}
    </div>}
    {notice && <p className="setup-feedback" role="status">{t(notice)}</p>}
    {error && <p className="setup-error" role="alert">{t(error)}</p>}
    <div className="setup-footer">
      <button className="setup-docs" disabled={locked} onClick={() => void openDocs()}><UiIcon name="external"/>{t(installed ? "官方更新说明" : "官方安装说明")}</button>
      <div className="setup-actions">
        {installed && <button disabled={locked || readingVersion} onClick={() => void run("version", async () => {
          await readVersion(true);
        })}>{t(readingVersion || phase === "version" ? "检查中…" : "检查更新")}</button>}
        {installed && !missing && !failed ? <button className="primary" disabled={locked} onClick={onDone}>{t("完成")}</button>
          : <button className={installStarted ? "primary" : ""} disabled={locked} onClick={() => { returnPending.current = false; void scan(); }}>{t(phase === "scan" ? "检测中…" : missing || failed ? "再次检测" : "检测安装结果")}</button>}
      </div>
    </div>
  </section>;
}
