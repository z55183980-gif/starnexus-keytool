import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import * as api from "./api";
import * as mock from "./mock";
import { LANGS, setLang, t, useLang } from "./i18n";
import { UiIcon } from "./UiIcon";
import { UpdateController, type AvailableUpdate, type ClientPolicy, type UpdateProgress, type UpdateView } from "./updateFlow";
import "./Updates.css";

const DOWNLOAD = "https://dkby.com/keys";
const preview = import.meta.env.DEV && mock.activeScenario();
const Context = createContext<{
  state: UpdateView; controller: UpdateController; busy: boolean; ignoreProxy: boolean;
  setBusy: (value: boolean) => void; setIgnoreProxy: (value: boolean) => void;
} | null>(null);
export function useUpdates() {
  const context = useContext(Context);
  if (!context) throw Error("Missing update provider");
  return context;
}

export function UpdateProvider({ children }: { children: ReactNode }) {
  const lang = useLang();
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  busyRef.current = busy;
  const ignoreProxy = useRef(false);
  const [proxyChoice, setProxyChoice] = useState(false);
  const setIgnoreProxy = useCallback((value: boolean) => { ignoreProxy.current = value; setProxyChoice(value); }, []);
  const [view, setView] = useState<UpdateView | null>(null);
  const [controller] = useState(() => new UpdateController({
    busy: () => busyRef.current,
    policy: async () => {
      if (preview) {
        const required = new URLSearchParams(location.search).get("update") === "required";
        return { config: { ...mock.mockRemote(), min_client_version: required ? "0.3.0" : "" }, currentVersion: "0.2.9", required, source: "remote", warning: null };
      }
      return invoke<ClientPolicy>("get_client_policy", { ignoreProxy: ignoreProxy.current });
    },
    check: async () => {
      if (preview) return new URLSearchParams(location.search).has("update") ? { version: "0.3.0", notes: "修复兼容性问题，改善启动体验。" } : null;
      return invoke<AvailableUpdate | null>("check_for_updates", { ignoreProxy: ignoreProxy.current });
    },
    install: async progress => {
      if (preview) throw Error("预览模式不会安装更新");
      const channel = new Channel<UpdateProgress>();
      channel.onmessage = progress;
      await invoke("install_update", { onProgress: channel });
    },
  }, setView));
  useEffect(() => {
    let active = true;
    void api.loadAppState().catch(() => null).then(state => {
      if (!active) return;
      setIgnoreProxy(state?.ignoreProxy ?? false);
      void controller.check();
    });
    const timer = window.setInterval(() => void controller.check(), 6 * 60 * 60 * 1000);
    return () => { active = false; clearInterval(timer); };
  }, [controller, setIgnoreProxy]);
  const state = view ?? controller.state;
  return <Context.Provider value={{ state, controller, busy, setBusy, ignoreProxy: proxyChoice, setIgnoreProxy }}>
    {!state.policy || state.policy.required ? <main className="update-screen">
      <section className="update-gate" aria-labelledby="update-title">
        <label className="update-language">
          <span>{t("语言")}</span>
          <select aria-label={t("界面语言")} value={lang} onChange={event => setLang(event.target.value as typeof LANGS[number]["id"])}>
            {LANGS.map(option => <option key={option.id} value={option.id}>{option.label}</option>)}
          </select>
        </label>
        <h1 id="update-title">{t(state.policy?.required ? "需要更新才能继续" : state.error ? "无法检查版本" : "正在检查版本…")}</h1>
        {state.policy?.required && <p>{t("当前 {current}，请更新至 {minimum} 或更高版本。", { current: state.policy.currentVersion, minimum: state.policy.config.min_client_version })}</p>}
        <UpdatePanel mandatory />
      </section>
    </main> : children}
  </Context.Provider>;
}

async function openDownload() {
  if (preview) { window.open(DOWNLOAD, "_blank", "noopener,noreferrer"); return; }
  await invoke("open_update_download");
}

export function UpdatePanel({ mandatory = false, onDismiss }: { mandatory?: boolean; onDismiss?: () => void }) {
  useLang();
  const { state, controller, busy } = useUpdates();
  const [linkError, setLinkError] = useState("");
  const active = state.phase !== "idle";
  const progress = state.progress;
  const percent = progress?.total ? Math.min(100, Math.round(progress.downloaded / progress.total * 100)) : null;
  const versionTitle = state.update && t("新版 {version}", { version: state.update.version });
  const phaseLabel = t(state.phase === "installing" ? "正在安装更新…" : state.phase === "restarting" ? "正在重启应用…" : "正在下载更新…");
  return <section className="update-panel" aria-label={t("应用更新")}>
    {!mandatory && <h2 className="update-heading">{onDismiss && <span className="update-mark"><UiIcon name="refresh" /></span>}{onDismiss ? versionTitle : t("应用更新")}</h2>}
    {!onDismiss && !mandatory && state.policy && <p className="update-version">{t("当前版本 {version}", { version: state.policy.currentVersion })}</p>}
    {state.policy?.config.notice && <p>{state.policy.config.notice}</p>}
    {state.update && <>
      {!onDismiss && (!mandatory || state.update.version !== state.policy?.config.min_client_version) && <strong>{versionTitle}</strong>}
      {state.update.notes && <pre className="update-notes">{state.update.notes}</pre>}
      {/Win/i.test(navigator.platform) && <p className="update-version">{t("便携版将转为安装版。")}</p>}
    </>}
    {!state.update && state.checked && !state.checking && !state.error && <p>{t(mandatory ? "暂无可用更新，请重试或手动下载。" : "已是最新版本")}</p>}
    {state.policy?.warning && <p className="update-warning">{t(state.policy.warning)}</p>}
    {(state.error || linkError) && <p className="update-error" role="alert">{t(state.error || linkError)}</p>}
    {active && <div className="update-progress" role="status">
      <p>{phaseLabel}{state.phase === "downloading" && percent !== null && <span>{percent}%</span>}</p>
      <progress aria-label={phaseLabel} max={100} value={state.phase === "downloading" ? percent ?? undefined : undefined} />
    </div>}
    {busy && <p>{t("当前操作完成后即可安装更新")}</p>}
    {!active && <div className="update-actions">
      <button className="update-download" onClick={() => { setLinkError(""); void openDownload().catch(() => setLinkError("无法打开下载页面，请检查默认浏览器")); }}>{t("手动下载")}<UiIcon name="external" /></button>
      {((!onDismiss && !mandatory) || !state.update || state.error) && <button disabled={state.checking || busy} onClick={() => void controller.check()}>{t(state.checking ? "检查中…" : "检查更新")}</button>}
      {onDismiss && <button className="update-later" onClick={onDismiss}>{t("稍后")}</button>}
      {state.update && <button className="primary" disabled={state.checking || busy} onClick={() => void controller.install()}>{t("更新并重启")}</button>}
    </div>}
    {mandatory && <label className="update-proxy"><ProxyChoice /></label>}
  </section>;
}

function ProxyChoice() {
  const { ignoreProxy, setIgnoreProxy, state } = useUpdates();
  return <><input type="checkbox" checked={ignoreProxy} disabled={state.checking || state.phase !== "idle"} onChange={event => setIgnoreProxy(event.target.checked)} />{t("更新检查不走系统代理")}</>;
}

export function UpdateBanner() {
  const { state } = useUpdates();
  const [dismissed, setDismissed] = useState<string | null>(null);
  if (!state.update || state.policy?.required || dismissed === state.update.version) return null;
  const { version } = state.update;
  return <aside className="update-banner" aria-label={t("应用更新")}>
    <UpdatePanel onDismiss={() => setDismissed(version)} />
  </aside>;
}
