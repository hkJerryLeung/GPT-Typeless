import { useEffect, useRef, type ReactNode } from "react";
import { X } from "lucide-react";
import { shortcutKeys } from "../types";

export function Keycaps({ shortcut }: { shortcut: string }) {
  return <span className="keycaps" aria-label={shortcut}>{shortcutKeys(shortcut).map((key, i) => <kbd key={i}>{key}</kbd>)}</span>;
}
export function Toggle({ checked, onChange, label, disabled = false }: { checked: boolean; onChange: (value: boolean) => void; label: string; disabled?: boolean }) {
  return <button type="button" className="toggle" role="switch" aria-checked={checked} aria-label={label} disabled={disabled} onClick={() => onChange(!checked)}><span /></button>;
}
export function SettingRow({ title, description, children }: { title: string; description?: string; children: ReactNode }) {
  return <div className="setting-row"><div className="setting-label"><h4>{title}</h4>{description && <p>{description}</p>}</div><div className="setting-control">{children}</div></div>;
}
export function EmptyState({ icon, title, description, children }: { icon: ReactNode; title: string; description: string; children?: ReactNode }) {
  return <div className="empty-state"><span className="empty-icon">{icon}</span><h3>{title}</h3><p>{description}</p>{children}</div>;
}
export function Dialog({ title, onClose, children, className = "", busy = false }: { title: string; onClose: () => void; children: ReactNode; className?: string; busy?: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  useEffect(() => {
    const previous = document.activeElement;
    ref.current?.focus();
    return () => { if (previous instanceof HTMLElement && previous.isConnected) previous.focus(); };
  }, []);
  return <div className="modal-backdrop" onMouseDown={event => { if (event.target === event.currentTarget && !busy) onClose(); }}>
    <div ref={ref} className={`native-dialog ${className}`} role="dialog" aria-modal="true" aria-label={title} tabIndex={-1} onKeyDown={event => {
      if (event.key === "Escape") { event.stopPropagation(); if (!busy) closeRef.current(); }
      if (event.key === "Tab") {
        const focusable = Array.from(ref.current?.querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), a[href]') ?? []).filter(el => el.getClientRects().length > 0);
        const first = focusable[0], last = focusable[focusable.length - 1];
        if (!first) { event.preventDefault(); return; }
        if (event.shiftKey && (document.activeElement === first || document.activeElement === ref.current)) { event.preventDefault(); last?.focus(); }
        else if (!event.shiftKey && (document.activeElement === last || document.activeElement === ref.current)) { event.preventDefault(); first.focus(); }
      }
    }}>
      <button className="icon-button dialog-close" aria-label="關閉視窗" onClick={onClose} disabled={busy}><X size={18} /></button>
      {children}
    </div>
  </div>;
}
