import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type UpdateState = { phase: "idle" | "checking" | "downloading" | "ready" | "installing" | "error"; message: string; version?: string };
interface Options {
  enabled: boolean;
  ready: boolean;
  idle: boolean;
  acquire: () => boolean;
  release: () => void;
}

const quietPeriod = 5 * 60_000;
const checkInterval = 4 * 60 * 60_000;

export function useUpdater(options: Options) {
  const [state, setState] = useState<UpdateState>({ phase: "idle", message: "尚未檢查更新。" });
  const latest = useRef(options);
  const running = useRef(false);
  const automatic = useRef(false);
  useLayoutEffect(() => { latest.current = options; });

  const check = async (manual = false) => {
    if (!latest.current.ready || running.current) return;
    running.current = true; automatic.current = !manual;
    setState({ phase: "checking", message: "正在檢查更新…" });
    try {
      const update = await invoke<{ version: string } | null>("check_update");
      if (!manual && !latest.current.enabled) return;
      if (!update) { setState({ phase: "idle", message: "已是最新版本。" }); return; }
      setState({ phase: "downloading", message: `正在下載 ${update.version}…`, version: update.version });
      await invoke("download_update");
      if (!manual && !latest.current.enabled) return;
      setState({ phase: "ready", message: `已下載 ${update.version}，可在閒置時安裝。`, version: update.version });
    } catch {
      setState({ phase: "error", message: "暫時未能檢查或下載更新，可稍後再試。" });
    } finally { running.current = false; automatic.current = false; }
  };

  const install = async (automatic = false) => {
    if (state.phase !== "ready" || running.current || !latest.current.idle || !latest.current.acquire()) return;
    running.current = true;
    setState({ phase: "installing", message: "正在安裝更新並重新啟動…", version: state.version });
    try {
      await invoke("install_update", { automatic });
      // Normally the process restarts and this line is not reached.
      setState({ phase: "idle", message: "更新已安裝。" });
    } catch (error) {
      if (automatic && String(error).includes("UPDATE_NOT_IDLE")) setState({ phase: "ready", message: "已下載更新，等候電腦閒置。", version: state.version });
      else setState({ phase: "error", message: "未能安裝更新，原有版本仍可使用。請稍後重新檢查。" });
    } finally { latest.current.release(); running.current = false; }
  };

  const checkRef = useRef(check);
  const installRef = useRef(install);
  useLayoutEffect(() => { checkRef.current = check; installRef.current = install; });

  useEffect(() => {
    if (!options.ready || !options.enabled) return;
    const first = window.setTimeout(() => void checkRef.current(), 10_000);
    const repeat = window.setInterval(() => void checkRef.current(), checkInterval);
    return () => { window.clearTimeout(first); window.clearInterval(repeat); };
  }, [options.ready, options.enabled]);

  useEffect(() => {
    if (!options.enabled && automatic.current) void invoke("cancel_update").catch(() => {});
  }, [options.enabled]);

  useEffect(() => {
    if (!options.enabled || !options.idle || state.phase !== "ready") return;
    let timer: number, generation = 0, disposed = false;
    const attempt = async () => {
      const current = generation;
      let idle = false;
      try { idle = await invoke<boolean>("update_system_idle"); } catch { /* Retry without interrupting the user. */ }
      if (disposed || current !== generation || !latest.current.enabled || !latest.current.idle) return;
      if (idle) void installRef.current(true);
      else timer = window.setTimeout(() => void attempt(), 60_000);
    };
    const arm = () => { generation++; window.clearTimeout(timer); timer = window.setTimeout(() => void attempt(), quietPeriod); };
    arm();
    // Only activity in this app is observed; no keystrokes are recorded.
    window.addEventListener("pointerdown", arm); window.addEventListener("keydown", arm); window.addEventListener("focus", arm);
    return () => { disposed = true; window.clearTimeout(timer); window.removeEventListener("pointerdown", arm); window.removeEventListener("keydown", arm); window.removeEventListener("focus", arm); };
  }, [options.enabled, options.idle, state.phase]);

  return { ...state, check: () => check(true), install };
}
