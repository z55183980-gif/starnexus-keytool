import type { CodexEnv } from "./types";

type LocalConfig = Pick<CodexEnv, "configurationStatus" | "currentBaseUrl" | "currentKeyMasked" | "currentModel">;

export function needsConnectionGuide(config?: LocalConfig) {
  if (!config) return false;
  return config.configurationStatus === "unconfigured" || (config.configurationStatus === "unknown" && !config.currentBaseUrl && !config.currentKeyMasked && !config.currentModel);
}

export const ONBOARDED_KEY = "starnexus.onboarded";

type Store = Pick<Storage, "getItem">;

/** 首次打开才走引导：没完成过引导、也没用过主界面（老用户升级上来不打扰）。预览可用 ?onboarding=1 强制显示。 */
export function needsOnboarding(store: Store | null, search = "") {
  if (new URLSearchParams(search).get("onboarding") === "1") return true;
  try { return !!store && !store.getItem(ONBOARDED_KEY) && !store.getItem("starnexus.lastToolPage"); }
  catch { return false; }
}
