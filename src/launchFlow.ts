import type { ApplyRequest, ApplyResult, Environment, LaunchMode, LaunchRequest, ToolId } from "./types";

interface Dependencies {
  validateLaunch: (req: LaunchRequest) => Promise<void>;
  scanEnvironment: (baseUrl: string) => Promise<Environment>;
  applyConfig: (req: ApplyRequest) => Promise<ApplyResult>;
  revertToOfficial: (codex: boolean, claude: boolean, claudeDesktop: boolean) => Promise<string>;
  launchClient: (req: LaunchRequest) => Promise<{ message: string }>;
}

/**
 * Validate installation/directory before touching configuration; never launch after a failed write.
 * Codex desktop and CLI share ~/.codex. Claude does not: the CLI reads ~/.claude/settings.json while
 * Claude desktop reads its own third-party inference config, so each mode writes and checks its own file.
 * The one exception: desktop + "current" just opens the app without inspecting anything.
 */
export async function launchWithConfiguration(
  tool: ToolId, mode: LaunchMode, service: string, directory: string,
  applyRequest: ApplyRequest,
  dependencies: Dependencies,
  onConfigured: (result: ApplyResult) => void,
): Promise<{ message: string }> {
  const req: LaunchRequest = { target: `${tool}-${mode}`, directory };
  await dependencies.validateLaunch(req);
  if (mode === "desktop" && service === "current") return dependencies.launchClient(req);

  const claudeDesktop = tool === "claude" && mode === "desktop";
  const slot = claudeDesktop ? "claudeDesktop" : tool;
  const request: ApplyRequest = {
    ...applyRequest,
    configureCodex: tool === "codex",
    configureClaude: tool === "claude" && !claudeDesktop,
    configureClaudeDesktop: claudeDesktop,
  };
  const env = await dependencies.scanEnvironment(request.baseUrl);
  const current = env[slot];
  if (current.configurationStatus === "broken") throw new Error("配置文件需要修复，请到密钥页查看详情后再启动");
  if (service === "current") return dependencies.launchClient(req);

  if (service === "official") {
    if (!["official", "unconfigured"].includes(current.configurationStatus)) {
      const backupId = await dependencies.revertToOfficial(tool === "codex", request.configureClaude, claudeDesktop);
      if (backupId) onConfigured({ backupId, paths: [], warnings: [], wroteCodexConfig: tool === "codex", wroteCodexAuth: false, wroteClaudeSettings: request.configureClaude, wroteClaudeDesktop: claudeDesktop });
    }
    const verified = (await dependencies.scanEnvironment(request.baseUrl))[slot];
    if (!["official", "unconfigured"].includes(verified.configurationStatus)) throw new Error("仍检测到自定义服务或外部认证覆盖，已停止启动。请检查配置详情，或明确选择沿用当前配置。");
  } else {
    const expectedModel = tool === "codex" ? request.model : request.claudeModel;
    const matches = current.configurationStatus === "configured" && current.activeProfileId === service
      // Custom Claude services may intentionally omit a model; detect reports that as null.
      && (current.currentModel ?? "") === expectedModel
      && (tool !== "codex" || env.codex.currentReasoningEffort === request.reasoningEffort);
    if (!matches) {
      onConfigured(await dependencies.applyConfig(request));
      const verified = (await dependencies.scanEnvironment(request.baseUrl))[slot];
      if (verified.configurationStatus !== "configured" || verified.activeProfileId !== service) {
        throw new Error("配置已写入，但仍有外部配置或认证覆盖，已停止启动。请查看配置详情；本次修改可撤销。");
      }
    }
  }
  return dependencies.launchClient(req);
}
