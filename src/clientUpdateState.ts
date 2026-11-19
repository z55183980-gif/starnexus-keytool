export type VersionSnapshot = { current: string; latest: string | null; updateAvailable: boolean | null; warning: string | null };
export function clientUpdateState(installed: boolean, version: VersionSnapshot | null, checking: boolean, pending: boolean) {
  if (!installed) return "missing";
  if (checking) return "checking";
  if (pending) return "pending";
  if (!version || !version.latest || version.updateAvailable === null || version.warning) return "unknown";
  return version.updateAvailable ? "available" : "current";
}
