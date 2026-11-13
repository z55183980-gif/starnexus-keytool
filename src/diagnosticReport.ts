import type { Environment, HealthIssue, LaunchMode, ToolId } from "./types";
import packageInfo from "../package.json" with { type: "json" };

// Export an allowlisted summary, never raw logs/configuration or error text.
// Arbitrary fields may contain credentials, URLs with tokens, or local paths.
const SUMMARIES: Record<string, string> = {
  C1: "Claude 配置文件格式损坏，需手动修复",
  C2: "模型别名不全，切换模型会报 404",
  C3: "Claude 服务地址不对", C4: "同时设置了两种密钥，可能冲突",
  C5: "有其他认证方式，可能不生效",
  D1: "Claude 桌面版配置文件格式损坏，需手动修复",
  D2: "Claude 桌面版服务地址不对", D3: "Claude 桌面版用了其他认证方式，可能不生效",
  D4: "Claude 桌面版需要重新启动", D5: "Claude 桌面版配置暂不可用",
  X1: "Codex 配置文件格式损坏", X2: "Codex 服务地址不对",
  X3: "启用了 Codex profile，可能不生效", E1: "系统环境变量会覆盖这里的配置",
  K1: "密钥无效或已停用", K2: "连不上服务器", K3: "所选模型不可用，换一个",
  K4: "服务暂时不可用，稍后再试", K5: "部分模型别名不可用", K6: "模型接口不兼容",
};
const allowed = (value: unknown, values: string[]) => typeof value === "string" && values.includes(value) ? value : "unknown";
const knownIssue = (id: string) => Object.prototype.hasOwnProperty.call(SUMMARIES, id);

export function diagnosticReport(tool: ToolId, mode: LaunchMode, env: Environment | null,
  issues: HealthIssue[], translate: (text: string) => string = text => text): string {
  const cfg = tool === "codex" ? env?.codex : mode === "desktop" ? env?.claudeDesktop : env?.claude;
  const fileState = cfg && ("configState" in cfg ? cfg.configState : cfg.settingsState);
  return JSON.stringify({
    product: "StarNexus Key Tool", version: packageInfo.version, reportSchema: 1,
    tool: allowed(tool, ["claude", "codex"]), mode: allowed(mode, ["cli", "desktop"]),
    configuration: cfg ? {
      status: allowed(cfg.configurationStatus, ["configured", "official", "custom", "unknown", "broken", "unconfigured"]),
      fileState: fileState && typeof fileState === "object" ? "broken" : allowed(fileState, ["ok", "missing"]),
      authState: "authState" in cfg ? allowed(cfg.authState, ["Missing", "OfficialLogin", "ApiKeyOnly", "Unparsable", "ExternalStore"]) : "not-exported",
      directoryExists: cfg.homeExists === true,
      managedByThisApp: cfg.managedByUs === true,
      modelConfigured: !!cfg.currentModel,
      serviceUrlConfigured: !!cfg.currentBaseUrl,
    } : null,
    checks: issues.map(issue => ({
      id: knownIssue(issue.id) ? issue.id : "unknown",
      level: allowed(issue.level, ["error", "warn"]),
      summary: translate(knownIssue(issue.id) ? SUMMARIES[issue.id] : "未识别的体检问题"),
      action: allowed(issue.fix, ["rewrite", "rebuild", "keys", "settings", "model", "none"]),
    })),
    privacy: translate("已省略密钥、账号信息、本机路径、服务地址和原始日志。"),
  }, null, 2);
}
