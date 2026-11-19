import { useSyncExternalStore } from "react";
import { DICT } from "./locales";
import { RU } from "./locales.ru";

/**
 * 四语：简体中文 / 繁體中文（香港用语）/ English / Русский。
 * 源码里直接写简体原文，t() 按当前语言查 locales.ts（俄语在 locales.ru.ts）里的译文。英文兜底：繁體、俄语缺译文时用英文；
 * 整条都查不到（多半是后端返回的动态内容）才显示原文，不会空白。
 * 变量用 {name} 占位：t("写入「{name}」的配置后", { name })。
 */
export type Lang = "zh-CN" | "zh-HK" | "en" | "ru";
export const LANGS: { id: Lang; label: string }[] = [
  { id: "zh-CN", label: "简体中文" },
  { id: "zh-HK", label: "繁體中文" },
  { id: "en", label: "English" },
  { id: "ru", label: "Русский" },
];

const STORE_KEY = "starnexus.lang";

/** 没选过语言时跟随系统：香港、澳门、台湾或明确的繁体 → 繁體（香港用语）；其他中文 → 简体；俄语 → Русский；其余以及认不出来 → English（英文兜底） */
export function detectLang(languages: readonly string[] = typeof navigator === "undefined" ? [] : navigator.languages ?? [navigator.language]): Lang {
  for (const raw of languages) {
    const l = raw.toLowerCase();
    if (/^zh-(hk|mo|tw)|^zh-hant/.test(l)) return "zh-HK";
    if (l.startsWith("zh")) return "zh-CN";
    if (l.startsWith("ru")) return "ru";
    if (l) return "en";
  }
  return "en";
}

function initial(): Lang {
  try {
    const saved = localStorage.getItem(STORE_KEY);
    if (saved === "zh-CN" || saved === "zh-HK" || saved === "en" || saved === "ru") return saved;
    if (saved === "zh-TW") return "zh-HK"; // 早期版本存的繁体代码
  } catch { /* 读不到就跟随系统 */ }
  return detectLang();
}

let current: Lang = initial();
const listeners = new Set<() => void>();
syncDocument();

function syncDocument() {
  if (typeof document !== "undefined") document.documentElement.lang = current === "zh-HK" ? "zh-HK" : current === "zh-CN" ? "zh-Hans" : current;
}

export function getLang(): Lang {
  return current;
}

export function setLang(lang: Lang) {
  if (lang === current) return;
  current = lang;
  try { localStorage.setItem(STORE_KEY, lang); } catch { /* 存不了就只在本次生效 */ }
  syncDocument();
  listeners.forEach(fn => fn());
}

/** 组件里订阅当前语言：切换后重新渲染 */
export function useLang(): Lang {
  return useSyncExternalStore(
    fn => { listeners.add(fn); return () => listeners.delete(fn); },
    () => current,
    () => current,
  );
}

/**
 * 带占位符的原文 → 正则。后端返回的提示常带数字或名字（如「12 行多余的空行」），
 * 整句查不到时按这些模板匹配，把匹配到的值填进译文。
 */
let templates: { re: RegExp; key: string; names: string[] }[] | null = null;
function matchTemplate(text: string): string | null {
  templates ??= Object.keys(DICT).filter(k => /\{\w+\}/.test(k)).map(key => {
    const names: string[] = [];
    const body = key.split(/(\{\w+\})/).map(part => {
      const m = /^\{(\w+)\}$/.exec(part);
      if (m) { names.push(m[1]); return "(.+?)"; }
      return part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    }).join("");
    return { re: new RegExp(`^${body}$`, "s"), key, names };
  });
  for (const tpl of templates) {
    const m = tpl.re.exec(text);
    if (m) return t(tpl.key, Object.fromEntries(tpl.names.map((n, i) => [n, m[i + 1]])));
  }
  return null;
}

/** 翻译一句简体原文；vars 填进 {占位}。整句查不到时再按模板匹配（主要用于后端返回的提示） */
export function t(source: string, vars?: Record<string, string | number | null | undefined>): string {
  if (current !== "zh-CN" && !vars && !DICT[source] && !RU[source] && /[一-鿿]/.test(source)) {
    const hit = matchTemplate(source);
    if (hit !== null) return hit;
  }
  const entry = current === "zh-CN" ? undefined : DICT[source];
  const text = current === "zh-CN" ? source
    : current === "ru" ? RU[source] || entry?.[1] || source
    : !entry ? source : current === "zh-HK" ? entry[0] || entry[1] || source : entry[1] || source;
  return vars ? text.replace(/\{(\w+)\}/g, (m: string, k: string) => (k in vars ? String(vars[k] ?? "") : m)) : text;
}
