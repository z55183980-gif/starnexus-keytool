import { useState } from "react";
import { ClaudeIcon, CodexIcon } from "./BrandIcons";
import { UiIcon } from "./UiIcon";
import { t } from "./i18n";
import * as api from "./api";
import { shortClientVersion } from "./clientVersionCache";
import type { LaunchMode, LaunchTarget, LauncherScan, ToolId } from "./types";

const META: Record<ToolId, { name: string; Icon: typeof ClaudeIcon }> = {
  claude: { name: "Claude Code", Icon: ClaudeIcon },
  codex: { name: "Codex", Icon: CodexIcon },
};

/** 某个工具装了哪些版本（桌面版 / 命令行）；还没扫到返回 null */
export function installedKinds(scan: LauncherScan | null, tool: ToolId): LaunchMode[] | null {
  if (!scan) return null;
  return (["desktop", "cli"] as const).filter(kind => scan.installations.some(item => item.target === `${tool}-${kind}` && item.installed));
}

export const kindLabel = (kind: LaunchMode) => kind === "desktop" ? t("桌面版") : t("命令行");

const progressText = (p: api.InstallProgress) => t(p.phase === "resolve" ? "正在获取安装信息…" : p.phase === "verify" ? "正在校验安装包…" : p.phase === "install" ? "正在安装，请留意系统提示…" : p.phase === "relay" ? "正在通过中继下载…" : "正在下载安装包…");

type Card = { progress: api.InstallProgress | null; notice: string; error: string };
const EMPTY: Card = { progress: null, notice: "", error: "" };

/**
 * 引导安装页：两个工具的桌面版 / 命令行各一张应用卡片。
 * 没装的显示「安装」，装了但不是最新版显示「更新」，按钮按下后本身变成进度条。
 * 一次只装一个；安装器需要用户在系统里点完，回到窗口时由外层自动重新检测。
 */
export function InstallStep({ tools, scan, scanFailed, versionOf, afterInstall, busy }: {
  tools: ToolId[]; scan: LauncherScan | null; scanFailed: boolean;
  /** 已安装程序的版本；undefined = 还在读，null = 读不到 */
  versionOf: (target: LaunchTarget) => api.ClientVersion | null | undefined;
  /** 装完或更新完：外层重新检测安装并重读这一项的版本 */
  afterInstall: (target: LaunchTarget) => Promise<void>;
  busy: (working: boolean) => void;
}) {
  const [cards, setCards] = useState<Partial<Record<LaunchTarget, Card>>>({});
  const patch = (target: LaunchTarget, next: Partial<Card>) => setCards(current => ({ ...current, [target]: { ...EMPTY, ...current[target], ...next } }));
  const installing = Object.values(cards).some(card => card?.progress);
  const apps = tools.flatMap(tool => (["desktop", "cli"] as const).map(kind => ({ tool, kind, target: `${tool}-${kind}` as LaunchTarget })));

  const install = async (target: LaunchTarget) => {
    if (installing) return;
    patch(target, { progress: { phase: "resolve", downloaded: 0, total: null }, notice: "", error: "" });
    busy(true);
    try {
      const result = await api.runClientInstall(target, progress => patch(target, { progress }));
      // 预览模式的模拟提示不显示；真实安装没装完时（比如要在系统安装器里点完）才提示
      if (!api.isSetupPreview) patch(target, { notice: result.installed ? "" : result.message });
      await afterInstall(target);
    } catch (cause) {
      patch(target, { error: String(cause).replace(/^Error:\s*/, "") });
    } finally { patch(target, { progress: null }); busy(false); }
  };

  return <>
  <div className="app-grid">
    {apps.map(({ tool, kind, target }) => {
      const { name, Icon } = META[tool];
      const found = scan?.installations.find(item => item.target === target);
      const installed = !!found?.installed;
      const card = cards[target] ?? EMPTY;
      const version = installed ? versionOf(target) : undefined;
      const outdated = installed && version?.updateAvailable === true && !!version.latest;
      return <article key={target} className={`app-card ${installed ? "installed" : ""}`}>
        <div className="app-card-head">
          <span className="app-card-icon"><Icon className={`brand brand-${tool}`}/><span className="app-card-kind" aria-hidden="true"><UiIcon name={kind === "desktop" ? "desktop" : "terminal"}/></span></span>
          <button type="button" className="app-card-docs" disabled={installing} title={t("官方安装说明")} aria-label={t("{name} 官方安装说明", { name: `${name} ${kindLabel(kind)}` })}
            onClick={() => void api.openClientDownload(target)}><UiIcon name="external"/></button>
        </div>
        <strong className="app-card-name">{name}</strong>
        <span className="app-card-sub">{kindLabel(kind)}{version && <span className="app-card-version"> · {shortClientVersion(version.current)}</span>}</span>

        {card.progress || outdated || (!installed && scan)
          ? (() => {
            // 按钮本身就是进度条：有总大小时从左往右填充并显示百分比；没有时一道光来回扫
            const ratio = card.progress?.total ? Math.min(1, card.progress.downloaded / card.progress.total) : null;
            const label = outdated ? t("更新") : t("安装");
            return <button type="button" className={`app-card-install ${outdated && !card.progress ? "update" : ""} ${card.progress ? "loading" : ""} ${card.progress && ratio === null ? "indeterminate" : ""} ${ratio !== null && ratio >= .5 ? "past-half" : ""}`}
              disabled={installing} title={outdated ? t("更新至 {version}", { version: shortClientVersion(version!.latest!) }) : undefined}
              aria-label={card.progress ? progressText(card.progress) : outdated ? t("更新至 {version}", { version: shortClientVersion(version!.latest!) }) : label} onClick={() => void install(target)}>
              {card.progress && <span className="app-card-fill" style={{ width: `${(ratio ?? 0) * 100}%` }} aria-hidden="true"/>}
              <span className="app-card-install-label">{card.progress ? ratio !== null ? `${Math.round(ratio * 100)}%` : "" : label}</span>
            </button>;
          })()
          : installed
            // 本次在这里装好的，打勾时带一下弹出动画；本来就装好的不动
            ? <span className={`app-card-state ok ${cards[target] ? "pop" : ""}`}><UiIcon name="check"/>{t("已安装")}</span>
            : <span className="app-card-state">{t(scanFailed ? "暂时无法检测" : "检测中…")}</span>}
        {card.notice && <p className="install-note" role="status">{t(card.notice)}</p>}
        {card.error && <p className="install-error" role="alert">{t(card.error)}</p>}
      </article>;
    })}
  </div>
  </>;
}
