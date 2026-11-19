import type { ClientVersion } from "./api";

/** Keep local version probes across page switches, including failed attempts. */
export class ClientVersionCache {
  private entries = new Map<string, Promise<ClientVersion | null>>();
  private values = new Map<string, ClientVersion | null>();
  private key(target: string, path?: string | null) { return JSON.stringify([target, path ?? ""]); }
  peek(target: string, path?: string | null) { return this.values.get(this.key(target, path)) ?? null; }
  read(target: string, path: string | null | undefined, probe: () => Promise<ClientVersion>) {
    const key = this.key(target, path);
    let pending = this.entries.get(key);
    if (!pending) {
      pending = probe().catch(() => null).then(value => {
        if (this.entries.get(key) === pending) this.values.set(key, value);
        return this.values.get(key) ?? null;
      });
      this.entries.set(key, pending);
    }
    return pending;
  }
  set(target: string, path: string | null | undefined, value: ClientVersion) {
    const key = this.key(target, path);
    this.values.set(key, value); this.entries.set(key, Promise.resolve(value));
  }
}

export function shortClientVersion(version: string) {
  return /^\d+\.\d+\.\d+\.0$/.test(version) ? version.slice(0, -2) : version;
}
