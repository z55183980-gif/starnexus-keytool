import { useState } from "react";
import { t } from "./i18n";
import { UiIcon } from "./UiIcon";

/**
 * 一键修复的卡片：占住启动卡片的正文区域，只有两种样子。
 * - busy：正在分析 / 写入 / 复检，逐步显示进度，不能操作。
 * - ask：需要用户拿主意。只有选项，没有输入框；默认选中第一个，另有一个「先不处理 / 取消」。
 */
export interface RepairChoice {
  label: string;
  hint?: string;
  /** 标「推荐」 */
  recommended?: boolean;
  run: () => void;
}
export type RepairView =
  | { phase: "busy"; title: string; steps: string[]; current: number }
  | { phase: "ask"; title: string; question: string; cause?: string; choices: RepairChoice[]; confirmLabel: string; cancelLabel: string; footnote?: string; onCancel: () => void };

export function RepairCard({ view, disabled }: { view: RepairView; disabled?: boolean }) {
  const [picked, setPicked] = useState(0);
  if (view.phase === "busy") {
    return <div className="repair-card" role="status" aria-live="polite">
      <div className="repair-kicker"><UiIcon name="sparkle"/>{view.title}</div>
      <ol className="repair-steps">
        {view.steps.map((step, i) => <li key={step} className={i < view.current ? "done" : i === view.current ? "active" : ""}>
          <span className="repair-step-mark" aria-hidden="true">{i < view.current ? <UiIcon name="check"/> : null}</span>{step}
        </li>)}
      </ol>
    </div>;
  }
  const choice = view.choices[picked] ?? view.choices[0];
  return <div className="repair-card">
    <div className="repair-kicker"><UiIcon name="sparkle"/>{view.title}</div>
    <strong className="repair-question">{view.question}</strong>
    {view.cause && <p className="repair-cause">{view.cause}</p>}
    <div className="repair-options" role="radiogroup" aria-label={view.question}>
      {view.choices.map((c, i) => <button key={c.label} type="button" role="radio" aria-checked={i === picked} className="repair-option" disabled={disabled} onClick={() => setPicked(i)}>
        <span className="service-radio" aria-hidden="true"/>
        <span className="service-text"><strong>{c.label}{c.recommended && <span className="repair-tag">{t("推荐")}</span>}</strong>{c.hint && <small>{c.hint}</small>}</span>
      </button>)}
    </div>
    {view.footnote && <p className="repair-footnote"><UiIcon name="shield"/>{view.footnote}</p>}
    <div className="repair-actions">
      <button type="button" className="primary" disabled={disabled || !choice} onClick={() => choice?.run()}>{view.confirmLabel}</button>
      <button type="button" className="ghost" disabled={disabled} onClick={view.onCancel}>{view.cancelLabel}</button>
    </div>
  </div>;
}
