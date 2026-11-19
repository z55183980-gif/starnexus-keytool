import type { ApplyRequest, ApplyResult, Environment, LaunchMode, LaunchRequest, ToolId } from "./types";

interface Dependencies {
  validateLaunch: (req: LaunchRequest) => Promise<void>;
  checkClientVersion?: (target: "codex-cli", ignoreProxy: boolean, latest: boolean) => Promise<{ verifiedBuiltin?: boolean }>;
  scanEnvironment: (baseUrl: string, tools?: (ToolId | "claude-cli" | "claude-desktop")[], preferredProfileId?: string) => Promise<Environment>;
  applyConfig: (req: ApplyRequest) => Promise<ApplyResult>;
  revertToOfficial: (codex: boolean, claude: boolean, claudeDesktop: boolean) => Promise<string>;
  launchClient: (req: LaunchRequest) => Promise<{ message: string }>;
}

/** Shared by the button state and the fresh check immediately before launch. */
export function configurationMatches(
  tool: ToolId, mode: LaunchMode, service: string, env: Environment | null,
  model: string, reasoningEffort: string,
): boolean {
  if (service === "current") return true;
  if (!env) return false;
  const current = env[tool === "claude" && mode === "desktop" ? "claudeDesktop" : tool];
  if (service === "official") return ["official", "unconfigured"].includes(current.configurationStatus);
  return current.configurationStatus === "configured" && current.activeProfileId === service
    && (current.currentModel ?? "") === model
    && (tool !== "codex" || env.codex.currentReasoningEffort === reasoningEffort);
}

/** Launch is read-only, including tray launches. Changes require a separate save. */
export async function launchWithConfiguration(
  tool: ToolId, mode: LaunchMode, service: string, directory: string,
  applyRequest: ApplyRequest,
  dependencies: Dependencies,
): Promise<{ message: string }> {
  const req: LaunchRequest = { target: `${tool}-${mode}`, directory, service };
  await dependencies.validateLaunch(req);
  if (mode === "desktop" && service === "current") return dependencies.launchClient(req);
  await prepareConfiguration(tool, mode, service, applyRequest, dependencies, false, () => {});
  return dependencies.launchClient(req);
}

/** Save and verify only. Installation and working directory are launch concerns. */
export async function saveLaunchConfiguration(
  tool: ToolId, mode: LaunchMode, service: string, applyRequest: ApplyRequest,
  dependencies: Dependencies, onConfigured: (result: ApplyResult) => void,
): Promise<void> {
  if (service === "current") return;
  await prepareConfiguration(tool, mode, service, applyRequest, dependencies, true, onConfigured);
}

async function prepareConfiguration(
  tool: ToolId, mode: LaunchMode, service: string, applyRequest: ApplyRequest,
  dependencies: Dependencies, allowWrite: boolean, onConfigured: (result: ApplyResult) => void,
): Promise<void> {
  const claudeDesktop = tool === "claude" && mode === "desktop";
  const slot = claudeDesktop ? "claudeDesktop" : tool;
  const request: ApplyRequest = {
    ...applyRequest,
    configureCodex: tool === "codex",
    codexDesktop: tool === "codex" && mode === "desktop",
    configureClaude: tool === "claude" && !claudeDesktop,
    configureClaudeDesktop: claudeDesktop,
  };
  const scan = () => dependencies.scanEnvironment(request.baseUrl, [tool === "claude" ? `claude-${mode}` : tool], service);
  const env = await scan();
  const current = env[slot];
  if (current.configurationStatus === "broken") throw new Error("配置文件需要修复，请到密钥页查看详情后再启动");
  if (service === "current") return;
  const expectedModel = tool === "codex" ? request.model : claudeDesktop
    ? [request.claudeModel, request.claudeOpusModel, request.claudeSonnetModel, request.claudeHaikuModel].find(name => name?.trim())?.trim() ?? "" : request.claudeModel;
  let independent = request.conservative;
  if (tool === "codex" && mode === "cli" && service !== "official" && env.codex.builtinRelay && !independent) {
    const version = await dependencies.checkClientVersion?.("codex-cli", request.ignoreProxy, false).catch(() => null);
    independent = !version?.verifiedBuiltin;
  }
  const matches = (value: Environment) => {
    if (!configurationMatches(tool, mode, service, value, expectedModel, request.reasoningEffort)) return false;
    if (service === "official") return true;
    if (tool === "codex") return !(independent && value.codex.builtinRelay);
    const actual = value[claudeDesktop ? "claudeDesktop" : "claude"].configuredModels;
    const aliases = [request.claudeOpusModel, request.claudeSonnetModel, request.claudeHaikuModel].map(name => name || "");
    const expected = claudeDesktop ? [...new Set([request.claudeModel, ...aliases].map(name => name?.trim()).filter(Boolean))] : aliases;
    return !actual || JSON.stringify(actual) === JSON.stringify(expected);
  };
  if (matches(env)) return;
  if (!allowWrite) throw new Error("配置尚未保存或已发生变化，请先保存并写入配置，再启动");

  if (service === "official") {
    if (!["official", "unconfigured"].includes(current.configurationStatus)) {
      const backupId = await dependencies.revertToOfficial(tool === "codex", request.configureClaude, claudeDesktop);
      if (backupId) onConfigured({ backupId, paths: [], warnings: [], wroteCodexConfig: tool === "codex", wroteCodexAuth: false, wroteClaudeSettings: request.configureClaude, wroteClaudeDesktop: claudeDesktop });
    }
    const verified = (await scan())[slot];
    if (!["official", "unconfigured"].includes(verified.configurationStatus)) throw new Error("仍检测到自定义服务或外部认证覆盖，已停止启动。请检查配置详情，或明确选择沿用当前配置。");
  } else {
    onConfigured(await dependencies.applyConfig(request));
    const verifiedEnv = await scan();
    if (!matches(verifiedEnv)) {
      throw new Error("配置已写入，但仍有外部配置或认证覆盖，已停止启动。请查看配置详情；本次修改可撤销。");
    }
  }
}
