import { t } from "./i18n";

/**
 * 按厂商给模型分组，组名用灰色标题。
 *
 * 只按名字归类，不判断"能不能用" —— 网关有三十多个渠道做格式转换，
 * DeepSeek、Gemini 这些照样能走 Codex，靠模型名猜兼容性只会误导用户。
 */
const FAMILIES: { name: string; test: (m: string) => boolean }[] = [
  { name: "GPT", test: (m) => /^(gpt|o[1-9]|chatgpt|codex)/.test(m) },
  { name: "Claude", test: (m) => m.startsWith("claude") },
  { name: "Gemini", test: (m) => m.startsWith("gemini") },
  { name: "DeepSeek", test: (m) => m.startsWith("deepseek") },
  { name: "Qwen 通义", test: (m) => /^(qwen|qwq)/.test(m) },
  { name: "GLM 智谱", test: (m) => /^(glm|chatglm)/.test(m) },
  { name: "Kimi 月之暗面", test: (m) => /^(moonshot|kimi)/.test(m) },
  { name: "Grok", test: (m) => m.startsWith("grok") },
  { name: "豆包", test: (m) => /^(doubao|ep-)/.test(m) },
];

export function groupByFamily(models: string[]): { name: string; items: string[] }[] {
  const buckets = FAMILIES.map((f) => ({ name: f.name, items: [] as string[] }));
  const others: string[] = [];

  for (const m of models) {
    const lower = m.toLowerCase();
    const i = FAMILIES.findIndex((f) => f.test(lower));
    if (i >= 0) buckets[i].items.push(m);
    else others.push(m);
  }

  const out = buckets.filter((b) => b.items.length > 0);
  if (others.length) out.push({ name: "其他", items: others });
  return out;
}

/** Codex 思考深度 */
export const EFFORTS: { value: string; label: string }[] = [
  { value: "minimal", label: "最低 — 最快最省" },
  { value: "low", label: "较低" },
  { value: "medium", label: "中等 — 推荐" },
  { value: "high", label: "较高" },
  { value: "xhigh", label: "最高 — 部分模型支持" },
];
/** 思考深度的简称（当前语言）：取「最低 — 最快最省」里破折号前那一段 */
export const effortLabel = (v: string) => { const e = EFFORTS.find(x => x.value === v); return e ? t(e.label).split(" — ")[0] : v; };

/** 工具调用网关用的接口：Codex 走 Responses，Claude Code 走 Messages */
export const REQUIRED_ENDPOINT: Record<"codex" | "claude", string> = { codex: "responses", claude: "messages" };

/** 这个模型能不能给这个工具用；不知道它支持哪些接口时当作能用 */
export function supports(info: { endpoints: string[] } | undefined, tool: "codex" | "claude"): boolean {
  return !info || !info.endpoints.length || info.endpoints.includes(REQUIRED_ENDPOINT[tool]);
}

/**
 * 密钥没单独设置模型时用哪个：默认模型这把密钥能用就用它；
 * 否则按网关目录挑一个能用的，优先同一家（Codex 选 GPT，Claude Code 选 Claude）。没有目录时就用默认值。
 */
export function pickModel(tool: "codex" | "claude", preferred: string, catalog: { id: string; endpoints: string[] }[] | undefined): string {
  if (!catalog?.length) return preferred;
  const fits = (id: string) => { const m = catalog.find(x => x.id === id); return !!m && supports(m, tool); };
  if (preferred && fits(preferred)) return preferred;
  const usable = catalog.filter(m => supports(m, tool));
  const family = tool === "codex" ? /^(gpt|o\d|codex)/i : /^claude/i;
  return (usable.find(m => family.test(m.id)) ?? usable[0])?.id ?? preferred;
}
