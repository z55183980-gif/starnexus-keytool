import { useRef, useState, type ReactNode } from "react";
import { PickerDialog } from "./Launcher";
import { UiIcon } from "./UiIcon";
import { t } from "./i18n";
import { normalizeServiceUrl, serviceUrlFields } from "./serviceUrl";
import type { Profile, ToolId } from "./types";
import * as api from "./api";

export type ImportDialog = (onReady: (id: string) => void, onClose: () => void, onManual: () => void) => ReactNode;

export function ConnectionGuide({ tool, profiles, disabled, defaultUrl, ignoreProxy, renderImport, apply, launchOfficial, working }: {
  tool: ToolId; profiles: Profile[]; disabled: boolean; defaultUrl: string; ignoreProxy: boolean; working: (busy: boolean) => void;
  renderImport: ImportDialog; apply: (id: string) => Promise<boolean>; launchOfficial: () => Promise<void>;
}) {
  const [step, setStep] = useState<"choose" | "manual" | "import" | "confirm">("choose");
  const [url, setUrl] = useState("");
  const [key, setKey] = useState("");
  const [tested, setTested] = useState(false);
  const [savedId, setSavedId] = useState("");
  const [error, setError] = useState("");
  const [pending, setPending] = useState(false);
  const lock = useRef(false);
  const name = tool === "codex" ? "Codex" : "Claude";
  const reset = () => { if (lock.current) return; setStep("choose"); setError(""); setSavedId(""); setTested(false); setKey(""); };
  const run = async (action: () => Promise<void>) => {
    if (lock.current || disabled) return;
    lock.current = true; setPending(true); working(true); setError("");
    try { await action(); } catch (cause) { setError(t(String(cause).replace(/^Error:\s*/, ""))); }
    finally { lock.current = false; setPending(false); working(false); }
  };
  const busy = pending || disabled;
  const confirm = (id: string) => { setSavedId(id); setStep("confirm"); setError(""); };
  const applySaved = async (id: string) => { if (!await apply(id)) throw new Error(t("未能应用，请重试")); };
  const manual = async () => {
    const address = normalizeServiceUrl(url);
    const fields = serviceUrlFields(address, defaultUrl);
    if (!tested) {
      const result = await api.testConnection(fields.codexBaseUrl || defaultUrl, key.trim(), ignoreProxy);
      if (!result.ok) throw new Error(result.message);
      if (!result.models.length) throw new Error(t("未找到可用模型"));
      setTested(true); return;
    }
    const id = await api.saveProfile({ id: "", name: new URL(address).host, apiKey: key.trim(), codexModel: "", claudeModel: "", codexEffort: "medium", ...fields });
    setKey(""); confirm(id); await applySaved(id);
  };
  const applyLabel = t("应用到 {tool}", { tool: name });
  return <div className="connection-guide"><h2>{t("选择连接方式")}</h2>
    <button disabled={busy} onClick={() => void run(launchOfficial)}><UiIcon name="external"/><span><strong>{t("官方账号")}</strong><small>{t("打开客户端登录")}</small></span><UiIcon name="arrow"/></button>
    <button disabled={busy} onClick={() => setStep("import")}><UiIcon name="globe"/><span><strong>{t("AI 账号")}</strong><small>{t("登录并导入密钥")}</small></span><UiIcon name="arrow"/></button>
    <button disabled={busy} onClick={() => setStep("manual")}><UiIcon name="key"/><span><strong>API Key</strong><small>{t("填写服务地址和密钥")}</small></span><UiIcon name="arrow"/></button>
    {step === "import" && renderImport(confirm, reset, () => setStep("manual"))}
    {(step === "manual" || step === "confirm") && <PickerDialog title={step === "manual" ? t("连接 API Key") : applyLabel} onClose={reset} keepOnOverlay footer={<button className="primary" disabled={busy || (step === "manual" && (!key.trim() || !url.trim()))} onClick={() => void run(step === "manual" ? manual : () => applySaved(savedId))}>{pending ? t("处理中…") : step === "confirm" ? applyLabel : tested ? t("保存并应用到 {tool}", { tool: name }) : t("测试连接")}</button>}>
      {step === "manual" ? <><label className="field">{t("服务地址")}<input type="url" placeholder="https://…" spellCheck={false} disabled={busy} value={url} onChange={event => { setUrl(event.target.value); setTested(false); setError(""); }}/></label><label className="field">API Key<input type="password" autoComplete="off" spellCheck={false} disabled={busy} value={key} onChange={event => { setKey(event.target.value); setTested(false); setError(""); }}/></label>{tested && <p role="status">{t("连接成功")}</p>}</> : <p>{t("密钥：{name}", { name: profiles.find(profile => profile.id === savedId)?.name || new URL(url || defaultUrl).host })}</p>}
      {error && <p className="warn-text" role="alert">{error}</p>}
    </PickerDialog>}
  </div>;
}
