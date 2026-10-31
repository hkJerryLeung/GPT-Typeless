import { useEffect, useRef, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { Check, ChevronDown, Languages, X } from "lucide-react";
import type { DictationMode, VoiceStatus } from "../types";

// Keep these choices in step with languageOptions; custom saved values remain visible.
// This isolated window never imports or loads the app's configuration or credentials.
const languages = [
  ["English", "英文"], ["繁體中文（香港）", "繁體中文（香港）"], ["繁體中文（台灣）", "繁體中文（台灣）"],
  ["日本語", "日文"], ["한국어", "韓文"], ["Français", "法文"], ["Deutsch", "德文"], ["Español", "西班牙文"],
] as const;
const quietBars = () => Array<number>(13).fill(0);
function displayLevel(rms: number): number {
  if (!Number.isFinite(rms) || rms <= 0) return 0;
  // Display-only -35..-20 dBFS range; PCM is unchanged.
  return Math.max(0, Math.min(1, (20 * Math.log10(Math.min(1, rms)) + 35) / 15));
}

interface StatusPayload {
  status?: VoiceStatus; mode?: DictationMode; translation_language?: string | null; language_error?: string;
}

export default function VoiceOverlay() {
  const [status, setStatus] = useState<VoiceStatus>("idle");
  const statusRef = useRef<VoiceStatus>("idle");
  const [mode, setMode] = useState<DictationMode>("dictate");
  const [language, setLanguage] = useState("");
  const confirmedLanguageRef = useRef(false);
  const [pickerOpen, setPickerOpen] = useState(false);
  const pickerOpenRef = useRef(false);
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const [languageError, setLanguageError] = useState("");
  const [levels, setLevels] = useState(quietBars);
  const [error, setError] = useState("");
  const closeTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    let disposed = false;
    const cleanups: (() => void)[] = [];
    let staleTimer: number | undefined;
    const reset = () => {
      window.clearTimeout(staleTimer); staleTimer = undefined;
      setLevels(quietBars());
    };
    const register = <T,>(name: string, handle: (payload: T) => void) => {
      void listen<T>(name, event => { if (!disposed) handle(event.payload); })
        .then(cleanup => { if (disposed) cleanup(); else cleanups.push(cleanup); })
        .catch(() => { if (!disposed) setError("無法連接錄音狀態"); });
    };
    register<StatusPayload>("dictation-status", payload => {
      if (!payload) return;
      if (payload.status) {
        if (statusRef.current !== payload.status) {
          if (payload.status === "starting" && statusRef.current !== "recording") confirmedLanguageRef.current = false;
          statusRef.current = payload.status; setStatus(payload.status); setError(""); reset();
        }
        if (payload.status !== "recording" && payload.status !== "starting") {
          window.clearTimeout(closeTimer.current); pickerOpenRef.current = false; setPickerOpen(false);
          savingRef.current = false; setSaving(false); setLanguageError("");
        }
      }
      if (payload.mode) setMode(payload.mode);
      if (typeof payload.language_error === "string") {
        savingRef.current = false; setSaving(false); setLanguageError(payload.language_error);
      }
      if (typeof payload.translation_language === "string" && typeof payload.language_error === "string") {
        confirmedLanguageRef.current = true;
        setLanguage(payload.translation_language);
      } else if (typeof payload.translation_language === "string" && !savingRef.current && !confirmedLanguageRef.current) {
        setLanguage(payload.translation_language);
      }
    });
    register<{ recording: boolean; rms: number }>("audio-level", payload => {
      if (statusRef.current !== "recording") return;
      if (!payload || payload.recording !== true || typeof payload.rms !== "number" || !Number.isFinite(payload.rms)) { reset(); return; }
      setLevels(previous => [...previous.slice(1), displayLevel(payload.rms)]);
      window.clearTimeout(staleTimer);
      staleTimer = window.setTimeout(() => { if (!disposed) setLevels(quietBars()); }, 250);
    });
    return () => {
      disposed = true; window.clearTimeout(staleTimer); window.clearTimeout(closeTimer.current);
      cleanups.forEach(cleanup => cleanup());
    };
  }, []);

  const action = async (value: "stop" | "cancel") => {
    try { await emit("overlay-action", value); } catch { setError("操作未送出，請使用主視窗"); }
  };
  const setPicker = (open: boolean) => {
    window.clearTimeout(closeTimer.current);
    if (pickerOpenRef.current === open) return;
    pickerOpenRef.current = open; setPickerOpen(open);
    void emit("overlay-action", { type: "language-picker", open }).catch(() => {
      pickerOpenRef.current = false; setPickerOpen(false); setLanguageError("無法開啟語言選單");
    });
  };
  const chooseLanguage = (value: string) => {
    if (savingRef.current) return;
    if (value === language) { setLanguageError(""); setPicker(false); return; }
    savingRef.current = true; setSaving(true); setLanguageError("");
    void emit("overlay-action", { type: "language-select", language: value }).catch(() => {
      savingRef.current = false; setSaving(false); setLanguageError("未能送出語言選擇，請再試。");
    });
    setPicker(false);
  };
  if (status === "idle" && !error) return null;
  const active = status === "recording" || status === "starting";
  const current = languages.find(([value]) => value === language);
  const choices: readonly (readonly [string, string])[] = language && !current ? [[language, language], ...languages] : languages;
  return <div className="overlay-stage" onMouseDown={event => event.preventDefault()}>
    <div className="voice-stack">
      {active && mode === "translate" && <div className="voice-language-region"
        onMouseEnter={() => window.clearTimeout(closeTimer.current)}
        onMouseLeave={() => { closeTimer.current = window.setTimeout(() => setPicker(false), 180); }}>
        {pickerOpen && <div className="voice-language-menu" role="listbox" aria-label="翻譯目標語言">
          {choices.map(([value, label]) => <button key={value} type="button" role="option" aria-selected={value === language}
            tabIndex={-1} disabled={saving} onClick={() => chooseLanguage(value)}><span>{label}</span>{value === language && <Check size={15} />}</button>)}
        </div>}
        <button type="button" className={`voice-language-chip ${languageError ? "has-error" : ""}`} tabIndex={-1}
          aria-label={languageError || "選擇翻譯語言"} aria-haspopup="listbox" aria-expanded={pickerOpen} aria-busy={saving}
          onClick={() => setPicker(!pickerOpenRef.current)}>
          <Languages size={13} /><span>{languageError || (saving ? "儲存中…" : current?.[1] || language || "翻譯語言")}</span><ChevronDown size={13} />
        </button>
      </div>}
      <div className={`voice-pill ${active ? "is-recording" : "is-thinking"}`} role="status" aria-label={status === "recording" ? "錄音中；聲波顯示麥克風實際音量" : "錄音狀態"}>
        {active ? <><button tabIndex={-1} aria-label="取消錄音" onClick={() => void action("cancel")}><X size={17} /></button>
          {status === "recording" ? <span className="waveform" aria-hidden="true">{levels.map((level, i) => <i key={i} style={{ height: `${3 + level * 22}px` }} />)}</span> : <span>啟動中</span>}
          <button tabIndex={-1} className="voice-confirm" aria-label="完成錄音" disabled={status === "starting"} onClick={() => void action("stop")}><Check size={19} /></button></>
          : <span>{error || (status === "processing" ? "處理中…" : status === "done" ? "已完成" : "請查看主視窗")}</span>}
      </div>
    </div>
  </div>;
}
