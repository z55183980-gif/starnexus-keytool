import { useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as api from "./api";
import { UiIcon } from "./UiIcon";
import wordmarkUrl from "./assets/wordmark.png";
import type { GatewayToken } from "./types";
import { t } from "./i18n";
import "./LoginPanel.css";

async function openExternal(url: string) {
  try { await openUrl(url); }
  catch (error) {
    if (import.meta.env.DEV && window.open(url, "_blank", "noopener,noreferrer")) return;
    throw error;
  }
}

type Working = null | "login" | "list" | "import" | "logout" | "browser";

export function LoginPanel(props: {
  loggedAs: string | null;
  tokens: GatewayToken[] | null;
  ignoreProxy: boolean;
  busy: boolean;
  onLoggedIn: (user: string, tokens: GatewayToken[]) => void;
  onLogout: () => void | Promise<void>;
  onImported: (created: string[], updated: string[], unchanged: string[], skipped: string[]) => void | Promise<void>;
  onManualKey: () => void;
  onClose: () => void;
}) {
  const { loggedAs, tokens, ignoreProxy, busy, onLoggedIn, onLogout, onImported, onManualKey, onClose } = props;
  const dialog = useRef<HTMLDialogElement>(null);
  const requestInFlight = useRef(false);
  const id = useId();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");
  const [require2fa, setRequire2fa] = useState(false);
  const [browserWaiting, setBrowserWaiting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [authenticatedAs, setAuthenticatedAs] = useState<string | null>(null);
  const [picked, setPicked] = useState<Set<number>>(new Set());
  const [working, setWorking] = useState<Working>(null);
  const signedIn = loggedAs || authenticatedAs;
  const disabled = busy || working !== null;
  const fresh = tokens?.filter(x => x.usable && !x.linkedProfile) ?? [];

  useEffect(() => {
    const element = dialog.current!;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const overflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    element.showModal();
    return () => {
      element.close();
      document.body.style.overflow = overflow;
      if (previous?.isConnected) previous.focus({ preventScroll: true });
    };
  }, []);

  useEffect(() => {
    setPicked(new Set(tokens?.filter(x => x.usable && !x.linkedProfile).map(x => x.id) ?? []));
  }, [tokens]);

  async function run(kind: NonNullable<Working>, action: () => Promise<void>) {
    if (busy || requestInFlight.current) return;
    requestInFlight.current = true;
    setWorking(kind); setError(null);
    try { await action(); }
    catch (e) { setError(t(String(e).replace(/^Error:\s*/, ""))); }
    finally { requestInFlight.current = false; setWorking(null); }
  }

  async function loadTokens(name: string) {
    const list = await api.gatewayListTokens();
    onLoggedIn(name, list);
  }

  const doLogin = () => run("login", async () => {
    await api.gatewayBrowserLoginCancel(); setBrowserWaiting(false);
    let result: api.LoginResult;
    try {
      result = require2fa ? await api.gatewayLogin2fa(code) : await api.gatewayLogin(username.trim(), password, ignoreProxy);
    } finally { setPassword(""); setCode(""); }
    if (result.require2fa) { setRequire2fa(true); return; }
    setRequire2fa(false);
    setAuthenticatedAs(result.username);
    await loadTokens(result.username);
  });
  const startBrowserLogin = () => run("browser", async () => {
    const url = await api.gatewayBrowserLoginStart(ignoreProxy);
    try { await openExternal(url); setBrowserWaiting(true); }
    catch (error) { await api.gatewayBrowserLoginCancel(); throw error; }
  });
  type Callback = {code: string; state: string; error: string};
  const callback = useRef<(value: Callback) => void>(() => {});
  callback.current = value => {
    if (!browserWaiting) return;
    void run("login", async () => {
      if (value.error) throw new Error(t("官网登录未完成，请重试"));
      const result = await api.gatewayBrowserLoginFinish(value.code, value.state, ignoreProxy);
      setBrowserWaiting(false); setRequire2fa(false); setPassword(""); setCode("");
      setAuthenticatedAs(result.username);
      await loadTokens(result.username);
    });
  };
  useEffect(() => {
    let gone = false; let stop: (() => void) | undefined;
    listen<Callback>("browser-login-callback", event => callback.current(event.payload))
      .then(unlisten => { if(gone) unlisten(); else stop=unlisten; }).catch(() => {});
    return () => { gone=true; stop?.(); void api.gatewayBrowserLoginCancel().catch(() => {}); };
  }, []);

  const switchAccount = () => run("logout", async () => {
    await onLogout(); setAuthenticatedAs(null); setRequire2fa(false); setPassword(""); setCode("");
  });

  const importKeys = (ids: number[]) => run("import", async () => {
    const result = await api.gatewayImportTokens(ids);
    await onImported(result.created, result.updated, result.unchanged, result.skipped);
  });

  const title = t(signedIn ? "选择要导入的密钥" : "登录账号");
  const lede = t(signedIn ? "导入后，即可在启动页和密钥页选择使用。" : "使用账号密码登录，或前往官网登录后自动返回并导入密钥。");
  const alert = error && <p className="cauth-error" role="alert">{error}</p>;
  const spinner = <span className="account-spinner" />;
  const manual = <button type="button" className="cauth-link" disabled={disabled} onClick={onManualKey}>{t("手动粘贴 API Key")}</button>;

  return createPortal(
    <dialog ref={dialog} className="account-dialog" aria-labelledby={`${id}-title`} aria-describedby={`${id}-lede`}
      onCancel={event => { event.preventDefault(); if (!disabled && !requestInFlight.current) onClose(); }}>
      <div className="cauth">
        <button className="cauth-close" aria-label={t("关闭登录导入")} title={t("关闭")} disabled={disabled} onClick={onClose}><UiIcon name="close"/></button>
        <header className="cauth-head">
          <div className="cauth-heading-row"><img className="cauth-logo" src={wordmarkUrl} alt="StarNexus" draggable={false}/><h1 className="cauth-title" id={`${id}-title`}>{title}</h1></div>
          <p className="cauth-lede" id={`${id}-lede`}>{lede}</p>
        </header>

        {signedIn ? <div className="cauth-body">
          <div className="account-identity"><span className="account-avatar">{signedIn.slice(0, 1).toUpperCase()}</span><div><strong>{signedIn}</strong><span>{t("已连接 AI 账号")}</span></div><button className="cauth-link" disabled={disabled} onClick={() => void switchAccount()}>{t("切换账号")}</button></div>
          {tokens === null ? <div className="account-empty"><UiIcon name="refresh"/><strong>{working === "login" || working === "list" ? t("正在读取密钥…") : t("密钥列表尚未加载")}</strong><button className="cauth-link" disabled={disabled} onClick={() => void run("list", () => loadTokens(signedIn))}>{t("重新加载")}</button></div> : tokens.length === 0 ? <div className="account-empty"><UiIcon name="key"/><strong>{t("还没有可导入的密钥")}</strong><p>{t("在开发者控制台创建后，回来刷新列表。")}</p><button className="cauth-link" disabled={disabled} onClick={() => void run("list", () => loadTokens(signedIn))}>{t("刷新列表")}</button></div> : <>
            <div className="account-list-heading"><span>{t("{n} 个密钥 · 已选 {picked} 个", { n: tokens.length, picked: picked.size })}</span><button className="cauth-link" disabled={disabled || fresh.length === 0} onClick={() => setPicked(picked.size === fresh.length ? new Set() : new Set(fresh.map(x => x.id)))}>{picked.size === fresh.length && fresh.length > 0 ? t("取消全选") : t("全选可导入项")}</button></div>
            <div className="account-token-list">{tokens.map(token => token.linkedProfile ? <div className="account-token linked" key={token.id}><UiIcon name="check"/><div><strong>{token.name}</strong><span>{token.usable ? t("已关联「{name}」", { name: token.linkedProfile }) : t(token.note || "当前不可用")}</span></div><button className="cauth-link" disabled={disabled || !token.usable} onClick={() => void importKeys([token.id])}>{t("更新")}</button></div> : <label className={`account-token ${picked.has(token.id) ? "selected" : ""} ${token.usable ? "" : "unavailable"}`} key={token.id}><input type="checkbox" checked={picked.has(token.id)} disabled={disabled || !token.usable} onChange={() => setPicked(previous => { const next = new Set(previous); if (next.has(token.id)) next.delete(token.id); else next.add(token.id); return next; })}/><div><strong>{token.name}</strong><span>{t(token.note || "可导入")}</span></div></label>)}</div>
          </>}
          {alert}
          <button className="cauth-submit primary" disabled={disabled || picked.size === 0} onClick={() => void importKeys([...picked])}>{working === "import" ? <>{spinner}{t("正在导入…")}</> : t("导入选中的 {n} 个密钥", { n: picked.size })}</button>
        </div>

        : <form className="cauth-body" onSubmit={event => { event.preventDefault(); void doLogin(); }}>
          {require2fa ? <label className="cauth-field">{t("验证码或备用码")}<input autoFocus autoComplete="one-time-code" value={code} onChange={event => setCode(event.target.value)} disabled={disabled} required /></label> : <>
            <label className="cauth-field">{t("用户名或电子邮件")}<input autoFocus autoComplete="username" value={username} onChange={event => setUsername(event.target.value)} disabled={disabled} required /></label>
            <label className="cauth-field">{t("密码")}<input type="password" autoComplete="current-password" value={password} onChange={event => setPassword(event.target.value)} disabled={disabled} required /></label>
          </>}
          {browserWaiting && <p role="status">{t("请在官网完成登录，成功后将自动返回工具。")}</p>}
          {alert}
          <button type="submit" className="cauth-submit primary" disabled={disabled}>{working === "login" ? <>{spinner}{t("正在登录…")}</> : t(require2fa ? "验证并登录" : "登录账号")}</button>
          {require2fa && <button type="button" className="cauth-link cauth-foot" disabled={disabled} onClick={() => setRequire2fa(false)}>{t("返回账号登录")}</button>}
          <div className="cauth-row"><button type="button" className="cauth-link" disabled={disabled} onClick={() => void startBrowserLogin()}>{t("前往官网登录")}</button>{manual}</div>
        </form>}
        {!signedIn && <p className="cauth-trust"><UiIcon name="shield"/>{t("仅连接 dkby.com，密码不保存，登录会话仅保留在内存中")}</p>}
      </div>
    </dialog>, document.body,
  );
}
