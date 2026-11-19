import { useRef, useState } from "react";
import { ClaudeIcon, CodexIcon } from "./BrandIcons";
import { PickerDialog } from "./Launcher";
import { UiIcon } from "./UiIcon";
import { t } from "./i18n";
import * as api from "./api";
import { normalizeServiceUrl, serviceUrlFields } from "./serviceUrl";
import type { LaunchMode, Profile, ToolId } from "./types";
import type { ImportDialog } from "./ConnectionGuide";

export type ApplyOutcome = { tool: ToolId; ok: boolean; error?: string };
/** 把一把密钥写进某个工具的若干形态（桌面版 / 命令行）的配置 */
export type ApplyKey = (targets: { tool: ToolId; modes: LaunchMode[] }[], profileId: string) => Promise<ApplyOutcome[]>;

const META: Record<ToolId, { name: string; Icon: typeof ClaudeIcon }> = {
  claude: { name: "Claude Code", Icon: ClaudeIcon },
  codex: { name: "Codex", Icon: CodexIcon },
};

type Phase = "choose" | "login" | "manual" | "confirm" | "applying" | "done";

/**
 * 引导第四步：选密钥来源（AI 账号 / API Key / 已存密钥），确认后写入所选工具的配置。
 * 写入前后端会自动备份；部分工具失败不阻塞完成，提示去工具页重试。
 */
export function ConnectStep({ targets, profiles, defaultUrl, ignoreProxy, renderImport, applyKey, onWorking, onFinish, onSkip, onDone }: {
  targets: { tool: ToolId; modes: LaunchMode[] }[]; profiles: Profile[]; defaultUrl: string; ignoreProxy: boolean;
  renderImport: ImportDialog; applyKey: ApplyKey; onWorking: (busy: boolean) => void; onFinish: () => void; onSkip: () => void; onDone: () => void;
}) {
  const [phase, setPhase] = useState<Phase>("choose");
  const [profileId, setProfileId] = useState("");
  const [profileName, setProfileName] = useState("");
  const [url, setUrl] = useState(defaultUrl);
  const [key, setKey] = useState("");
  const [tested, setTested] = useState(false);
  const [error, setError] = useState("");
  const [pending, setPending] = useState(false);
  const [results, setResults] = useState<ApplyOutcome[]>([]);
  const lock = useRef(false);
  const tools = targets.map(item => item.tool);
  // 登录导入时新密钥可能还没进 profiles，名字按 id 现查
  const keyName = profileName || profiles.find(item => item.id === profileId)?.name || "";

  const run = async (action: () => Promise<void>) => {
    if (lock.current) return;
    lock.current = true; setPending(true); onWorking(true); setError("");
    try { await action(); } catch (cause) { setError(t(String(cause).replace(/^Error:\s*/, ""))); }
    finally { lock.current = false; setPending(false); onWorking(false); }
  };
  const pick = (id: string, name: string) => { setProfileId(id); setProfileName(name); setError(""); setPhase("confirm"); };

  const manual = () => run(async () => {
    const address = normalizeServiceUrl(url);
    const fields = serviceUrlFields(address, defaultUrl);
    if (!tested) {
      const result = await api.testConnection(fields.codexBaseUrl || defaultUrl, key.trim(), ignoreProxy);
      if (!result.ok) throw new Error(result.message);
      if (!result.models.length) throw new Error(t("未找到可用模型"));
      setTested(true); return;
    }
    const name = new URL(address).host;
    const id = await api.saveProfile({ id: "", name, apiKey: key.trim(), codexModel: "", claudeModel: "", codexEffort: "medium", ...fields });
    setKey(""); setTested(false); pick(id, name);
  });

  const apply = () => run(async () => {
    setPhase("applying");
    const outcome = await applyKey(targets, profileId);
    setResults(outcome); setPhase("done"); onDone();
  });

  const targetText = (tool: ToolId) => {
    const modes = targets.find(item => item.tool === tool)?.modes ?? [];
    return modes.map(mode => t(mode === "desktop" ? "桌面版" : "命令行")).join(t("、"));
  };

  if (phase === "done") {
    const failed = results.filter(item => !item.ok);
    return <>
      <div className={`connect-result ${failed.length ? "partial" : ""}`}>
        <span className="connect-result-mark"><UiIcon name={failed.length ? "alert" : "check"}/></span>
        <h1>{t(failed.length ? "部分工具未完成" : "一切就绪")}</h1>
        <p className="onboarding-lead">{failed.length ? t("其余工具已配置好。未完成的可以稍后在工具页重试。") : t("密钥已接入，现在就可以启动了。")}</p>
      </div>
      <ul className="connect-targets">{results.map(item => { const { name, Icon } = META[item.tool]; return <li key={item.tool} className={item.ok ? "ok" : "err"}>
        <Icon className={`brand brand-${item.tool}`}/><span><strong>{name}</strong><small>{item.ok ? t("已写入 · {kinds}", { kinds: targetText(item.tool) }) : item.error || t("写入失败")}</small></span>
        <UiIcon name={item.ok ? "check" : "alert"}/>
      </li>; })}</ul>
      <p className="install-hint connect-undo"><UiIcon name="shield"/>{t("写入前已自动备份原有配置。")}</p>
      <button type="button" className="primary onboarding-start" onClick={onFinish}>{t("进入主界面")}<UiIcon name="arrow"/></button>
    </>;
  }

  if (phase === "confirm" || phase === "applying") return <>
    <h1>{t("确认写入")}</h1>
    <p className="onboarding-lead">{t("将密钥「{name}」写入以下工具的配置：", { name: keyName })}</p>
    <ul className="connect-targets">{tools.map(tool => { const { name, Icon } = META[tool]; return <li key={tool}>
      <Icon className={`brand brand-${tool}`}/><span><strong>{name}</strong><small>{targetText(tool)}</small></span>
    </li>; })}</ul>
    <p className="install-hint connect-undo"><UiIcon name="shield"/>{t("写入前自动备份现有配置，可随时撤销。")}</p>
    {error && <p className="install-error" role="alert">{error}</p>}
    <button type="button" className="primary onboarding-start" disabled={pending} onClick={() => void apply()}>{t(phase === "applying" ? "正在写入…" : "写入配置")}{phase !== "applying" && <UiIcon name="arrow"/>}</button>
    <button type="button" className="onboarding-skip" disabled={pending} onClick={() => { setPhase("choose"); setError(""); }}>{t("换一把密钥")}</button>
  </>;

  return <>
    <h1>{t("接入密钥")}</h1>
    <p className="onboarding-lead">{t("选择密钥来源，接入后写入所选工具的配置。")}</p>
    <div className="connect-options">
      <button type="button" className="connect-option recommended" disabled={pending} onClick={() => setPhase("login")}>
        <span className="onboarding-icon"><UiIcon name="globe"/></span>
        <span><strong>{t("登录账号")}<em>{t("推荐")}</em></strong><small>{t("在浏览器登录，自动导入密钥")}</small></span><UiIcon name="arrow"/>
      </button>
      <button type="button" className="connect-option" disabled={pending} onClick={() => { setPhase("manual"); setUrl(defaultUrl); setTested(false); }}>
        <span className="onboarding-icon"><UiIcon name="key"/></span>
        <span><strong>{t("填写 API Key")}</strong><small>{t("已有密钥，或使用其他服务")}</small></span><UiIcon name="arrow"/>
      </button>
    </div>
    <button type="button" className="onboarding-skip" disabled={pending} onClick={onSkip}>{t("使用官方账号登录？跳过这一步")}</button>

    {phase === "login" && renderImport(id => pick(id, ""), () => setPhase("choose"), () => setPhase("manual"))}
    {phase === "manual" && <PickerDialog title={t("填写 API Key")} onClose={() => { if (!lock.current) { setPhase("choose"); setError(""); setKey(""); setTested(false); } }} keepOnOverlay
      footer={<button className="primary" disabled={pending || !key.trim() || !url.trim()} onClick={() => void manual()}>{pending ? t("处理中…") : tested ? t("保存密钥") : t("测试连接")}</button>}>
      <label className="field">{t("服务地址")}<input type="url" placeholder="https://…" spellCheck={false} disabled={pending} value={url} onChange={event => { setUrl(event.target.value); setTested(false); setError(""); }}/></label>
      <p className="install-hint">{t("默认是 AI 服务；用其他服务时改成对应地址。")}</p>
      <label className="field">API Key<input type="password" autoComplete="off" spellCheck={false} disabled={pending} value={key} onChange={event => { setKey(event.target.value); setTested(false); setError(""); }}/></label>
      {tested && <p role="status" className="connect-ok"><UiIcon name="check"/>{t("连接成功")}</p>}
      {error && <p className="install-error" role="alert">{error}</p>}
    </PickerDialog>}
  </>;
}
