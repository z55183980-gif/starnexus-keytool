import type { LauncherPreferences, ToolId, LaunchMode } from "./types";

export type TrayAction = { kind: "launch" | "switchKey"; tool: ToolId } | { kind: "directory"; path: string };
export interface TrayDependencies {
  consented: (tool: ToolId) => boolean;
  showTool: (tool: ToolId, picker: boolean) => void;
  validateDirectory: (path: string) => Promise<void>;
  save: (patch: Partial<LauncherPreferences>) => Promise<boolean>;
  launch: (tool: ToolId, mode: LaunchMode, service: string, directory: string) => Promise<boolean>;
}

export async function runTrayAction(action: TrayAction, prefs: LauncherPreferences, deps: TrayDependencies): Promise<void> {
  if (action.kind === "directory") {
    await deps.validateDirectory(action.path);
    if (!await deps.save({ projectDirectory: action.path, recentDirectories: [action.path, ...prefs.recentDirectories.filter(p => p !== action.path)].slice(0, 8) })) {
      throw Error("项目目录未保存，请重试");
    }
    return;
  }
  const tool = action.tool;
  deps.showTool(tool, action.kind === "switchKey" && deps.consented(tool));
  if (!deps.consented(tool)) throw Error("请先读取本机配置，再使用托盘快捷操作");
  if (action.kind === "switchKey") return;
  const mode = prefs[`${tool}Mode`];
  // 启动只用配置文件里当前生效的服务，不读旧的「所选服务」偏好
  const ok = await deps.launch(tool, mode, "current", prefs.projectDirectory);
  if (ok && mode === "cli") await deps.save({ recentDirectories: [prefs.projectDirectory, ...prefs.recentDirectories.filter(p => p !== prefs.projectDirectory)].slice(0, 8) });
}

/** Serializes startup/event delivery. Pending actions remain native-side while the UI is busy. */
export class TrayConsumer {
  private running = false;
  private ready: () => boolean;
  private take: () => Promise<TrayAction | null>;
  private run: (action: TrayAction) => Promise<void>;
  private error: (error: unknown) => void;
  constructor(ready: () => boolean, take: () => Promise<TrayAction | null>, run: (action: TrayAction) => Promise<void>, error: (error: unknown) => void) {
    this.ready = ready; this.take = take; this.run = run; this.error = error;
  }
  async drain() {
    if (this.running || !this.ready()) return;
    this.running = true;
    try {
      while (this.ready()) {
        const action = await this.take();
        if (!action) break;
        try { await this.run(action); } catch (error) { this.error(error); }
      }
    } catch (error) { this.error(error); }
    finally { this.running = false; }
  }
}
