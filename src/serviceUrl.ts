type ServiceProfile = { custom: boolean; codexBaseUrl: string; claudeBaseUrl: string };

export function normalizeServiceUrl(raw: string): string {
  const value = raw.trim().replace(/\/+$/, "");
  let parsed: URL;
  try { parsed = new URL(value); } catch { throw new Error("请输入有效的服务 URL"); }
  if (!["http:", "https:"].includes(parsed.protocol) || parsed.username || parsed.password || parsed.search || parsed.hash) {
    throw new Error("服务 URL 需使用 http 或 https，且不能包含账号、密码、查询参数或片段");
  }
  return parsed.toString().replace(/\/+$/, "");
}

/** One visible base URL; existing per-client endpoints remain intact until edited. */
export function profileServiceUrl(profile: ServiceProfile, defaultUrl: string): string {
  return profile.custom ? profile.codexBaseUrl || profile.claudeBaseUrl || defaultUrl : defaultUrl;
}

export function serviceUrlFields(raw: string, defaultUrl: string, original?: ServiceProfile) {
  const url = normalizeServiceUrl(raw);
  if (original && url === normalizeServiceUrl(profileServiceUrl(original, defaultUrl))) {
    return { codexBaseUrl: original.codexBaseUrl, claudeBaseUrl: original.claudeBaseUrl };
  }
  const root = (value: string) => value.replace(/\/v1$/, "");
  if (root(url) === root(normalizeServiceUrl(defaultUrl))) {
    return { codexBaseUrl: "", claudeBaseUrl: "" };
  }
  // Claude appends /v1/messages; Codex appends /responses to its API base.
  const codexBaseUrl = new URL(url).pathname === "/" ? `${url}/v1` : url;
  return { codexBaseUrl, claudeBaseUrl: root(url) };
}
