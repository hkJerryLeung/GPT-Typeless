import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type DragRect = { x: number; y: number; width: number; height: number; viewportWidth: number; viewportHeight: number };

// Native surface updates must remain ordered even when a wizard unmounts and
// another one opens before an IPC request finishes.
let dragUpdates: Promise<void> = Promise.resolve();

export function SetupDrag() {
  const ref = useRef<HTMLDivElement>(null);
  const [dragError, setDragError] = useState<"unavailable" | "not-accepted" | null>(null);

  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const enqueue = (rect: DragRect | null) => {
      dragUpdates = dragUpdates.catch(() => {}).then(async () => {
        if (disposed && rect) return;
        await invoke("set_setup_drag_region", { rect });
      }).catch(() => { if (!disposed) setDragError("unavailable"); });
    };
    const update = () => {
      const box = element.getBoundingClientRect();
      const clip = element.closest(".native-dialog")?.getBoundingClientRect();
      // Clip to the visible scroll container, so the native view never steals
      // clicks from controls when the icon has scrolled out of view.
      const left = Math.max(box.left, clip?.left ?? 0, 0);
      const top = Math.max(box.top, clip?.top ?? 0, 0);
      const right = Math.min(box.right, clip?.right ?? window.innerWidth, window.innerWidth);
      const bottom = Math.min(box.bottom, clip?.bottom ?? window.innerHeight, window.innerHeight);
      enqueue(right > left && bottom > top ? { x: left, y: top, width: right - left, height: bottom - top, viewportWidth: window.innerWidth, viewportHeight: window.innerHeight } : null);
    };
    void listen<{ outcome: string }>("setup-drag-finished", event => {
      if (!disposed) setDragError(event.payload.outcome === "unavailable" || event.payload.outcome === "not-accepted" ? event.payload.outcome : null);
    }).then(cleanup => { if (disposed) cleanup(); else unlisten = cleanup; }).catch(() => { if (!disposed) setDragError("unavailable"); });
    const observer = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(update);
    observer?.observe(element);
    // The centered dialog can move the icon when surrounding status text
    // changes, even when the icon itself stays the same size.
    const dialog = element.closest(".native-dialog");
    const content = element.closest(".setup-content");
    if (dialog) observer?.observe(dialog);
    if (content) observer?.observe(content);
    window.addEventListener("resize", update);
    window.addEventListener("scroll", update, true);
    update();
    return () => {
      disposed = true;
      observer?.disconnect();
      window.removeEventListener("resize", update);
      window.removeEventListener("scroll", update, true);
      unlisten?.();
      enqueue(null);
    };
  }, []);

  return <div className="setup-drag-wrap">
    <div ref={ref} className="setup-drag-source" role="img" aria-label="按住 GPT-Typeless 圖示拖到權限清單">
      <img src="/setup-app-icon.png" alt="" draggable={false} />
      <div><strong>GPT-Typeless</strong><span>按住圖示，拖到系統權限清單</span></div>
    </div>
    {dragError && <p className="hint" role="alert">{dragError === "unavailable" ? "未能直接拖曳，請用「在 Finder 顯示」再拖入清單。" : "未完成拖曳，可以再試一次，或按「在 Finder 顯示」。"}</p>}
  </div>;
}
