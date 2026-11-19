import type { RemoteConfig } from "./types";

export interface ClientPolicy {
  config: RemoteConfig;
  currentVersion: string;
  required: boolean;
  source: "remote" | "cache" | "builtin";
  warning: string | null;
}
export interface AvailableUpdate { version: string; notes: string }
export interface UpdateProgress { phase: "downloading" | "installing"; downloaded: number; total: number | null }
export interface UpdateView {
  policy: ClientPolicy | null;
  update: AvailableUpdate | null;
  checking: boolean;
  phase: "idle" | "downloading" | "installing" | "restarting";
  progress: UpdateProgress | null;
  error: string | null;
  checked: boolean;
}
export interface UpdateDependencies {
  policy: () => Promise<ClientPolicy>;
  check: () => Promise<AvailableUpdate | null>;
  install: (progress: (value: UpdateProgress) => void) => Promise<void>;
  busy: () => boolean;
}

/** One in-flight operation; mandatory policy survives update-server and download failures. */
export class UpdateController {
  state: UpdateView = { policy: null, update: null, checking: false, phase: "idle", progress: null, error: null, checked: false };
  private dependencies: UpdateDependencies;
  private changed: (state: UpdateView) => void;
  constructor(dependencies: UpdateDependencies, changed: (state: UpdateView) => void) {
    this.dependencies = dependencies;
    this.changed = changed;
  }
  private patch(patch: Partial<UpdateView>) { this.state = { ...this.state, ...patch }; this.changed(this.state); }
  async check() {
    if (this.state.checking || this.state.phase !== "idle" || this.dependencies.busy()) return;
    this.patch({ checking: true, error: null, update: null });
    try {
      this.patch({ policy: await this.dependencies.policy() });
      this.patch({ update: await this.dependencies.check(), checked: true });
    } catch (error) {
      this.patch({ error: String(error).replace(/^Error:\s*/, "") });
    } finally { this.patch({ checking: false }); }
  }
  async install() {
    if (!this.state.update || this.state.checking || this.state.phase !== "idle" || this.dependencies.busy()) return;
    this.patch({ phase: "downloading", progress: null, error: null });
    try {
      await this.dependencies.install(progress => this.patch({ progress, phase: progress.phase }));
      this.patch({ phase: "restarting" });
    } catch (error) {
      this.patch({ phase: "idle", error: String(error).replace(/^Error:\s*/, "") });
    }
  }
}
