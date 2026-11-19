import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { t } from "./i18n";

export function ServiceKeyRow({ className, title, disabled, actions, children }: {
  className: string; title: string; disabled: boolean; children: ReactNode;
  actions: { label: string; disabled?: boolean; run: () => void }[];
}) {
  const [anchor, setAnchor] = useState<{ x: number; y: number } | null>(null);
  const menu = useRef<HTMLDivElement>(null);
  const row = useRef<HTMLDivElement>(null);
  const close = () => setAnchor(null);
  const focusTitle = () => row.current?.querySelector<HTMLButtonElement>(".service-key-title")?.focus({ preventScroll: true });
  useLayoutEffect(() => {
    const element = menu.current;
    if (!anchor || !element) return;
    const rect = element.getBoundingClientRect();
    element.style.left = Math.max(4, Math.min(anchor.x, window.innerWidth - rect.width - 4)) + "px";
    element.style.top = Math.max(4, Math.min(anchor.y, window.innerHeight - rect.height - 4)) + "px";
    element.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus({ preventScroll: true });
  }, [anchor]);
  useEffect(() => {
    if (!anchor) return;
    if (disabled) { close(); return; }
    const outside = (event: Event) => { if (!menu.current?.contains(event.target as Node)) close(); };
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("focusin", outside);
    window.addEventListener("scroll", outside, true);
    window.addEventListener("resize", close);
    window.addEventListener("blur", close);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("focusin", outside);
      window.removeEventListener("scroll", outside, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("blur", close);
    };
  }, [anchor, disabled]);
  return <><div ref={row} className={className} role="listitem" onContextMenu={event => {
    event.preventDefault();
    if (!disabled) setAnchor({ x: event.clientX, y: event.clientY });
  }} onKeyDown={event => {
    if (!disabled && (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10"))) {
      event.preventDefault();
      const rect = event.target instanceof HTMLElement ? event.target.getBoundingClientRect() : event.currentTarget.getBoundingClientRect();
      setAnchor({ x: rect.left, y: rect.bottom });
    }
  }}>{children}</div>
    {anchor && !disabled && createPortal(<div ref={menu} role="menu" aria-label={title} className="service-key-menu" onContextMenu={event => event.preventDefault()} onKeyDown={event => {
      if (event.key === "Escape" || event.key === "Tab") {
        if (event.key === "Escape") event.preventDefault();
        close(); focusTitle(); return;
      }
      const items = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)"));
      const index = items.indexOf(document.activeElement as HTMLButtonElement);
      const next = event.key === "ArrowDown" ? index + 1 : event.key === "ArrowUp" ? index - 1 : event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : null;
      if (next !== null && items.length) { event.preventDefault(); items[(next + items.length) % items.length].focus(); }
    }}>
      {actions.map(action => <button key={action.label} type="button" role="menuitem" tabIndex={-1} disabled={action.disabled}
        onClick={() => { close(); focusTitle(); action.run(); }}>{t(action.label)}</button>)}
    </div>, document.body)}
  </>;
}
