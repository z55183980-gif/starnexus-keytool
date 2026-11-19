import { useEffect, useId, useRef, useState } from "react";
import { UiIcon } from "./UiIcon";
import { LANGS, setLang, t, useLang, type Lang } from "./i18n";
import "./LangPicker.css";

/**
 * 轻量语言选择：平时只显示当前语言，点开是自绘菜单（原生下拉框的样式改不了）。
 * 键盘：上下键移动，Enter / 空格选择，Esc / Tab 关闭并把焦点还给按钮。
 */
export function LangPicker() {
  const lang = useLang();
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const button = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLUListElement>(null);
  const id = useId();
  const current = LANGS.findIndex(item => item.id === lang);

  const show = () => { setActive(Math.max(0, current)); setOpen(true); };
  const close = (refocus = true) => { setOpen(false); if (refocus) button.current?.focus(); };
  const choose = (next: Lang) => { setLang(next); close(); };

  // 打开后焦点进菜单；点菜单外面关闭
  useEffect(() => {
    if (!open) return;
    list.current?.focus();
    const onDown = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) close(false); };
    document.addEventListener("pointerdown", onDown);
    return () => document.removeEventListener("pointerdown", onDown);
  }, [open]);

  const onKey = (event: React.KeyboardEvent) => {
    if (event.key === "ArrowDown") { event.preventDefault(); setActive(i => (i + 1) % LANGS.length); }
    else if (event.key === "ArrowUp") { event.preventDefault(); setActive(i => (i - 1 + LANGS.length) % LANGS.length); }
    else if (event.key === "Home") { event.preventDefault(); setActive(0); }
    else if (event.key === "End") { event.preventDefault(); setActive(LANGS.length - 1); }
    else if (event.key === "Enter" || event.key === " ") { event.preventDefault(); choose(LANGS[active].id); }
    else if (event.key === "Escape") { event.preventDefault(); close(); }
    else if (event.key === "Tab") close(false);
  };

  return <div className="lang-picker" ref={root}>
    <button type="button" ref={button} className={`lang-picker-button ${open ? "open" : ""}`} aria-haspopup="listbox" aria-expanded={open} aria-controls={open ? id : undefined}
      aria-label={`${t("界面语言")}: ${LANGS[current]?.label ?? ""}`}
      onClick={() => open ? close() : show()}
      onKeyDown={event => { if (event.key === "ArrowDown" || event.key === "ArrowUp") { event.preventDefault(); show(); } }}>
      <UiIcon name="globe"/>
      <span>{LANGS[current]?.label}</span>
      <svg className="lang-picker-chevron" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M2 3.5 5 6.5 8 3.5" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"/></svg>
    </button>
    {open && <ul id={id} ref={list} className="lang-picker-menu" role="listbox" tabIndex={-1} aria-label={t("界面语言")}
      aria-activedescendant={`${id}-${active}`} onKeyDown={onKey}>
      {LANGS.map((item, index) => <li key={item.id} id={`${id}-${index}`} role="option" aria-selected={item.id === lang}
        className={index === active ? "active" : undefined} lang={item.id}
        onPointerEnter={() => setActive(index)} onClick={() => choose(item.id)}>
        <span>{item.label}</span>
        {item.id === lang && <UiIcon name="check"/>}
      </li>)}
    </ul>}
  </div>;
}
