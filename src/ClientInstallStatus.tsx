import { useEffect, useState } from "react";
import * as api from "./api";
import { clientUpdateState } from "./clientUpdateState";
import { t } from "./i18n";
import type { LauncherInstallation, LaunchTarget } from "./types";
import { ClientVersionCache, shortClientVersion } from "./clientVersionCache";

/** The header follows the selected desktop/CLI target; installation stays in its manager. */
export function ClientInstallStatus({ target, installation, disabled, scanning, managerOpen, onOpen, cache }: {
  target: LaunchTarget; installation?: LauncherInstallation; disabled: boolean;
  scanning: boolean; managerOpen: boolean; onOpen: () => void;
  cache: ClientVersionCache;
}) {
  const [version, setVersion] = useState<api.ClientVersion | null>(() => cache.peek(target, installation?.path));
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let cancelled = false;
    setVersion(cache.peek(target, installation?.path)); setFailed(false);
    if (!installation?.installed || managerOpen || scanning) return;
    void cache.read(target, installation?.path, () => api.checkClientVersion(target, false, false))
      .then(result => { if (!cancelled) { setVersion(result); setFailed(!result || !!result.warning); } });
    return () => { cancelled = true; };
  }, [target, installation?.installed, installation?.path, managerOpen, scanning, cache]);
  const state = clientUpdateState(!!installation?.installed, version, false, false);
  const label = scanning ? "检测中…" : !installation ? "检测安装"
    : !installation.installed ? "安装" : state === "available" ? "更新" : failed ? "重试检查" : "检查更新";
  const status = !installation ? "待检测" : !installation.installed ? "未检测到"
    : "已安装";
  return <div className="install-status-row">
    <span className={`install-state ${installation?.installed ? "found" : ""}`} title={version ? t("本机版本：{version}", { version: version.current }) : undefined}>{t(status)}{installation?.installed && version && <span className="install-version"> · {shortClientVersion(version.current)}</span>}</span>
    <button type="button" className={`install-inline-action ${state === "available" ? "has-update" : ""}`}
      disabled={disabled || scanning} aria-haspopup="dialog"
      title={state === "available" ? t("更新至 {version}", { version: version!.latest! }) : undefined}
      onClick={onOpen}>{t(label)}</button>
  </div>;
}
