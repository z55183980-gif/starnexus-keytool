import { useEffect, useRef, useState } from "react";
import { t } from "./i18n";

/**
 * 输出面板：仿 PowerShell 的深色终端，按时间顺序记录每一次操作的结果。
 * - 保存、写入配置、启动、体检都写在这里，可以回看。
 * - 行尾可以挂按钮（撤销、修复、换模型……）。同一组（group）里只有最新一批（batch）的按钮可点，
 *   避免点到已经过时的那条。
 * - 放在设置页的「诊断日志」里，工具页不常驻；status、prompt 都是可选的，不传就不显示。
 */
export type LogKind = "cmd" | "ok" | "err" | "warn" | "info" | "path";
export interface LogAction {
  label: string;
  run: () => void;
}
export interface LogLine {
  id: number;
  time: string;
  kind: LogKind;
  text: string;
  group?: string;
  batch?: number;
  actions?: LogAction[];
  /** 只给工具页底部的提示条用的撤销（日志里已有「撤销这次改动」，不重复显示） */
  undo?: () => void;
}
export interface ConsoleStatus {
  level: "ok" | "warn" | "error" | "idle";
  text: string;
  title?: string;
  onClick?: () => void;
}
/** 输出最多保留多少行：超出后丢掉最早的，避免长时间运行无限累积 */
export const MAX_LOG_LINES = 200;
/** 单行最多多少字：报错原文、路径之类偶尔很长，超出部分截掉 */
export const MAX_LOG_TEXT = 500;
export const clipLogText = (text: string) => text.length > MAX_LOG_TEXT ? text.slice(0, MAX_LOG_TEXT) + t("…（已截断）") : text;

const MARK: Record<LogKind, string> = { cmd: ">", ok: "✓", err: "✗", warn: "!", info: "·", path: " " };

export function OutputConsole(props: {
  lines: LogLine[];
  busy: boolean;
  onClear: () => void;
  /** 因为超出上限被丢掉的更早记录条数 */
  dropped?: number;
  status?: ConsoleStatus;
}) {
  const bodyRef = useRef<HTMLDivElement>(null);
  const [expanded, setExpanded] = useState(false);
  const errorLines = props.lines.filter(l => l.kind === "err");
  const latestErrorId = errorLines[errorLines.length - 1]?.id;
  useEffect(() => { if (latestErrorId !== undefined) setExpanded(true); }, [latestErrorId]);
  // 有新输出就滚到最底下，和终端一样。看最后一行的 id 而不是行数：记录满 200 行后行数不再变化
  const lastId = props.lines[props.lines.length - 1]?.id;
  /** 用户是不是停在最底部；往上翻看历史时，窗口变化不把他拉回去 */
  const atBottom = useRef(true);
  useEffect(() => {
    const el = bodyRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    atBottom.current = true;
  }, [lastId]);
  // 终端高度会随窗口变化（工具页里撑满剩余空间）：停在底部时继续贴着最新一行
  useEffect(() => {
    const el = bodyRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => { if (atBottom.current) el.scrollTop = el.scrollHeight; });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // 每组只保留最新一批的按钮
  const latest = new Map<string, number>();
  for (const l of props.lines) if (l.group && l.batch !== undefined) latest.set(l.group, Math.max(latest.get(l.group) ?? 0, l.batch));
  const live = (l: LogLine) => !l.group || l.batch === latest.get(l.group);

  return (
    <section className={`console ${expanded ? "is-expanded" : "is-compact"}`} aria-label={t("输出")}>
      <div className="console-bar">
        <span className="console-tab"><span className="console-icon" aria-hidden="true">&gt;_</span>{t("诊断日志")}</span>
        <span className="console-tools">
          {props.status && (
            <button type="button" className={`console-btn console-status ${props.status.level}`} title={props.status.title} disabled={!props.status.onClick} onClick={props.status.onClick}>
              <span className="console-dot" aria-hidden="true" />{props.status.text}
            </button>
          )}
          {props.lines.length > 0 && <button type="button" className="console-btn console-clear" onClick={props.onClear}>{t("清空")}</button>}
          <button type="button" className="console-btn console-toggle" aria-expanded={expanded} onClick={() => setExpanded(x => !x)}>{t(expanded ? "收起日志" : "展开日志")}</button>
        </span>
      </div>
      <div className="console-body" ref={bodyRef}
        onScroll={e => { const el = e.currentTarget; atBottom.current = el.scrollHeight - el.clientHeight - el.scrollTop < 24; }}
        >
        <div role="log" aria-live="polite">
          {!!props.dropped && <div className="console-line info"><span className="console-mark" aria-hidden="true">{MARK.info}</span><span className="console-text">{t("已丢弃更早的 {n} 条记录（最多保留 {max} 条）", { n: props.dropped, max: MAX_LOG_LINES })}</span></div>}
          {props.lines.length === 0 && <div className="console-line info"><span className="console-mark" aria-hidden="true">{MARK.info}</span><span className="console-text">{t("还没有记录。")}</span></div>}
          {(expanded ? props.lines : props.lines.slice(-3)).map(l => (
            <div key={l.id} className={`console-line ${l.kind}`}>
              {l.kind !== "path" && <span className="console-time">{l.time}</span>}
              <span className="console-mark" aria-hidden="true">{MARK[l.kind]}</span>
              <span className="console-text">{l.text}</span>
              {live(l) && l.actions?.map(a => (
                <button key={a.label} type="button" className="console-btn console-action" disabled={props.busy} onClick={a.run}>{a.label}</button>
              ))}
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
