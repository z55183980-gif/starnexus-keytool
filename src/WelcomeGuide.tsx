import { useEffect, useState } from "react";
import { ClaudeIcon, CodexIcon } from "./BrandIcons";
import { UiIcon } from "./UiIcon";
import { t, useLang } from "./i18n";
import { LangPicker } from "./LangPicker";
import type { Environment, HealthIssue, LaunchMode, Profile, ToolId } from "./types";
import { InstallStep, installedKinds } from "./InstallStep";
import { CheckPanel, useEnvCheck } from "./EnvCheck";
import { ConnectStep, type ApplyKey } from "./ConnectStep";
import type { ImportDialog } from "./ConnectionGuide";
import "./WelcomeGuide.css";

const STEPS = ["welcome", "install", "connect"] as const;
type Step = typeof STEPS[number];

const ALL_TOOLS: ToolId[] = ["claude", "codex"];

export type GuideConnect = {
  profiles: Profile[]; defaultUrl: string; ignoreProxy: boolean; renderImport: ImportDialog; applyKey: ApplyKey;
  /** 记下同意读取两个工具的配置，并读回本机配置 */
  readEnvironment: () => Promise<Environment | null>;
  checkHealth: (tool: ToolId, service: string, mode: LaunchMode) => Promise<HealthIssue[]>;
};
const TOOL_NAME: Record<ToolId, string> = { claude: "Claude Code", codex: "Codex" };

/**
 * 首次打开的引导：欢迎 → 检查本机环境（进度 → 四张应用卡片和问题）→ 接入密钥。
 * 两个工具都已配好且没有严重问题时，可以跳过接入密钥直接完成。
 * onFinish 带上装了的工具；onSkip 跳过引导直接进主界面。
 */
export function WelcomeGuide({ onFinish, onSkip, connect }: { onFinish: (tools: ToolId[]) => void; onSkip: () => void; connect: GuideConnect }) {
  useLang(); // 切换语言时整页重新渲染
  const [step, setStep] = useState<Step>("welcome");
  /** 安装进行中：锁住返回、跳过和继续 */
  const [working, setWorking] = useState(false);
  const check = useEnvCheck({ ignoreProxy: connect.ignoreProxy, readEnvironment: connect.readEnvironment, checkHealth: connect.checkHealth });
  const { scan, env, issues, phase } = check;
  // 点「开始设置」进入检查页时开始检查；返回后再进来不重复跑
  useEffect(() => { if (step === "install" && phase === "idle") void check.run(); }, [step, phase]);

  // 装了任意一个版本的工具才算在用；一个都没装时两个都算（配置先就位，装好即可用）
  const used = ALL_TOOLS.filter(tool => installedKinds(scan, tool)?.length);
  const selected = used.length ? used : ALL_TOOLS;
  const nothingInstalled = !!scan && !used.length;
  // 在用的工具都已配好、也没有严重问题：可以直接完成
  const allReady = phase === "done" && used.length > 0 && !!env
    && used.every(tool => ["configured", "custom", "official"].includes(env[tool].configurationStatus))
    && !issues.some(issue => issue.level === "error");

  const index = STEPS.indexOf(step);
  const back: Step = step === "connect" ? "install" : "welcome";
  // 写配置的目标：装了哪种就写哪种；还没装的按命令行写（配置文件先就位，装好即可用）
  const targets = selected.map(tool => { const kinds = installedKinds(scan, tool); return { tool, modes: kinds?.length ? kinds : ["cli" as const] }; });
  const [connectDone, setConnectDone] = useState(false);
  return <div className="onboarding">
    <header className="onboarding-top">
      {step === "welcome" || connectDone
        ? <span/>
        : <button type="button" className="onboarding-back" disabled={working} onClick={() => setStep(back)}><UiIcon name="arrow"/>{t("返回")}</button>}
      <LangPicker/>
    </header>

    {step === "welcome" && <section className="onboarding-card">
      <div className="onboarding-tools" aria-hidden="true">
        <span className="onboarding-tool"><ClaudeIcon className="brand brand-claude"/></span>
        <span className="onboarding-link"/>
        <span className="onboarding-tool"><CodexIcon className="brand brand-codex"/></span>
      </div>
      <h1>{t("欢迎使用密钥管理工具")}</h1>
      <p className="onboarding-lead">{t("为 Claude Code 和 Codex 接入密钥，配好后一键启动。")}</p>

      <ul className="onboarding-points">
        <li><span className="onboarding-icon"><UiIcon name="terminal"/></span><span><strong>{t("一键启动")}</strong></span></li>
        <li><span className="onboarding-icon"><UiIcon name="key"/></span><span><strong>{t("集中管理密钥")}</strong></span></li>
        <li><span className="onboarding-icon"><UiIcon name="shield"/></span><span><strong>{t("改动可撤销")}</strong></span></li>
      </ul>

      <button type="button" className="primary onboarding-start" onClick={() => setStep("install")}>{t("开始设置")}<UiIcon name="arrow"/></button>
      <p className="onboarding-consent"><UiIcon name="shield"/>{t("将检查本机的安装和配置，不做任何修改")}</p>
      <button type="button" className="onboarding-skip" onClick={onSkip}>{t("跳过，直接进入")}</button>
    </section>}

    {step === "install" && <section className="onboarding-card align-start">
      <p className="onboarding-step">{t("第 {n} 步，共 {total} 步", { n: index + 1, total: STEPS.length })}</p>
      <div className="onboarding-title-row">
        <h1>{t(phase === "done" ? "本机环境" : "正在检查本机环境")}</h1>
        {phase === "done" && <button type="button" className="onboarding-refresh" disabled={working} onClick={() => void check.run()} title={t("重新检查")} aria-label={t("重新检查")}><UiIcon name="refresh"/></button>}
      </div>

      <CheckPanel check={check} busy={working}/>

      {phase === "done" && <>
        <InstallStep tools={ALL_TOOLS} scan={scan} scanFailed={check.scanFailed} versionOf={check.versionOf} afterInstall={check.afterInstall} busy={setWorking}/>
        {issues.length > 0 && <div className="check-issues">
          <h2>{t("需要注意")}</h2>
          <ul>{issues.map(issue => <li key={`${issue.tool}-${issue.text}`} className={issue.level}><UiIcon name="alert"/><span>{issue.tool && !issue.text.includes(issue.tool === "claude" ? "Claude" : "Codex") && <strong>{TOOL_NAME[issue.tool]}</strong>}{issue.text}</span></li>)}</ul>
          <p>{t("可以稍后在工具页修复。")}</p>
        </div>}
      </>}

      {allReady
        ? <>
          <button type="button" className="primary onboarding-start" disabled={working} onClick={() => onFinish(used)}>{t("完成，进入主界面")}<UiIcon name="arrow"/></button>
          <button type="button" className="onboarding-skip" disabled={working} onClick={() => setStep("connect")}>{t("仍要接入新密钥")}</button>
        </>
        : <button type="button" className="primary onboarding-start" disabled={working || phase !== "done"} onClick={() => setStep("connect")}>{t("下一步：接入密钥")}<UiIcon name="arrow"/></button>}
      {phase === "done" && nothingInstalled && <p className="onboarding-foot">{t("之后也可以在工具页里安装。")}</p>}
    </section>}

    {step === "connect" && <section className="onboarding-card align-start">
      {!connectDone && <p className="onboarding-step">{t("第 {n} 步，共 {total} 步", { n: index + 1, total: STEPS.length })}</p>}
      <ConnectStep targets={targets} {...connect} onWorking={setWorking} onFinish={() => onFinish(selected)} onSkip={() => onFinish(selected)} onDone={() => setConnectDone(true)}/>
    </section>}

    <ol className="onboarding-dots" aria-hidden="true">{STEPS.map((item, i) => <li key={item} className={i === index && !connectDone ? "on" : i <= index ? "done" : ""}/>)}</ol>
  </div>;
}
