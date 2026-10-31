import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { languageOptions, type AppConfig, type DictationMode, type DictationResult, type RecordingStatus, type VoiceStatus } from "../types";

interface Options {
  config: AppConfig; ready: boolean; mode: DictationMode; recordingBlocked?: boolean;
  fnAvailable?: boolean; onModeChange?: (mode: DictationMode) => void;
  // Persists one change and synchronizes the main window's config/ref on success.
  onTranslationLanguageChange: (language: string) => Promise<void>;
  onResult: (result: DictationResult) => void;
  onHistoryRefresh: () => Promise<void>;
  onError: (message: string) => void;
  onMissingToken: () => void;
}

export function useDictation(options: Options) {
  const [status, setStatus] = useState<VoiceStatus>("idle");
  const [message, setMessage] = useState("");
  const [elapsed, setElapsed] = useState(0);
  const [testing, setTesting] = useState(false);
  const [microphoneMessage, setMicrophoneMessage] = useState("");
  const [microphoneError, setMicrophoneError] = useState(false);
  const latest = useRef(options);
  const recordingRef = useRef(false);
  const ownerRef = useRef<"button" | "hotkey" | "fn" | null>(null);
  const startPromiseRef = useRef<Promise<void> | null>(null);
  const stopPromiseRef = useRef<Promise<void> | null>(null);
  const cancelPromiseRef = useRef<Promise<void> | null>(null);
  const languageSaveRef = useRef<Promise<boolean> | null>(null);
  const languageSaveFailedRef = useRef(false);
  const pickerResizeRef = useRef<Promise<void>>(Promise.resolve());
  const microphoneTestRef = useRef(false);
  const updateLockRef = useRef(false);
  const cancelRequestedRef = useRef(false);
  const stopStageRef = useRef<"waiting" | "stopping" | "processing" | null>(null);
  const sessionRef = useRef({ mode: options.mode, behavior: options.config.shortcut_behavior });
  const startedAtRef = useRef<number | null>(null);
  const hotkeyDownRef = useRef(false);
  const handlersRef = useRef({ press: () => {}, release: () => {}, stop: () => {}, cancel: () => {},
    selectLanguage: (_language: string) => {}, resizePicker: (_open: boolean) => {} });
  const fnHandlerRef = useRef<(mode: DictationMode) => void>(() => {});
  useLayoutEffect(() => { latest.current = options; });

  // Register before any device/config requests. Late async registrations must clean up too.
  useEffect(() => {
    let disposed = false;
    const cleanups: (() => void)[] = [];
    const register = async (event: string, handler: (payload: unknown) => void) => {
      try {
        const cleanup = await listen(event, e => { if (!disposed) handler(e.payload); });
        if (disposed) cleanup(); else cleanups.push(cleanup);
      } catch { if (!disposed) latest.current.onError("無法監聽錄音操作，請重新開啟應用程式。"); }
    };
    void register("hotkey-triggered", payload => {
      if (payload === true) {
        if (hotkeyDownRef.current) return;
        hotkeyDownRef.current = true;
        handlersRef.current.press();
      } else if (payload === false && hotkeyDownRef.current) {
        hotkeyDownRef.current = false;
        handlersRef.current.release();
      }
    });
    void register("overlay-action", payload => {
      if (payload === "stop") handlersRef.current.stop();
      if (payload === "cancel") handlersRef.current.cancel();
      if (payload && typeof payload === "object" && "type" in payload) {
        if (payload.type === "language-select" && "language" in payload && typeof payload.language === "string") handlersRef.current.selectLanguage(payload.language);
        if (payload.type === "language-picker" && "open" in payload && typeof payload.open === "boolean") handlersRef.current.resizePicker(payload.open);
      }
    });
    void register("fn-shortcut", payload => {
      if (payload === "dictate" || payload === "translate" || payload === "ask") fnHandlerRef.current(payload);
    });
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && (recordingRef.current || startPromiseRef.current)) {
        event.preventDefault(); handlersRef.current.cancel();
      }
    };
    window.addEventListener("keydown", escape);
    return () => { disposed = true; cleanups.forEach(fn => fn()); hotkeyDownRef.current = false; window.removeEventListener("keydown", escape); };
  }, []);

  useEffect(() => {
    if (!options.ready) return;
    // Serialize resizing and status updates, so a late open cannot re-show a finished bar.
    const mode = sessionRef.current.mode;
    pickerResizeRef.current = pickerResizeRef.current.then(async () => {
      await invoke("set_voice_status", { status, mode, pickerOpen: false });
    }).catch(() => {});
  }, [status, options.ready]);

  useEffect(() => {
    if (status !== "recording") return;
    const tick = () => setElapsed(Math.floor((Date.now() - (startedAtRef.current ?? Date.now())) / 1000));
    tick(); const timer = window.setInterval(tick, 200);
    return () => window.clearInterval(timer);
  }, [status]);

  const beginRecording = (source: "button" | "hotkey" | "fn", requestedMode = latest.current.mode): boolean => {
    if (!latest.current.ready || latest.current.recordingBlocked || updateLockRef.current || startPromiseRef.current || stopPromiseRef.current || cancelPromiseRef.current || recordingRef.current || microphoneTestRef.current) return false;
    if (!latest.current.config.chatgpt_token.trim()) { latest.current.onMissingToken(); return false; }
    ownerRef.current = source;
    cancelRequestedRef.current = false;
    languageSaveFailedRef.current = false;
    sessionRef.current = { mode: requestedMode, behavior: source === "fn" ? "toggle" : latest.current.config.shortcut_behavior };
    if (source === "fn") latest.current.onModeChange?.(requestedMode);
    setStatus("starting"); setMessage("正在啟動麥克風…"); setElapsed(0); latest.current.onError("");
    const start = async () => {
      try {
        await invoke<string>("start_recording", { deviceName: latest.current.config.audio_device || undefined, mode: sessionRef.current.mode });
        recordingRef.current = true;
      } catch (error) {
        try { recordingRef.current = (await invoke<RecordingStatus>("get_recording_status")).is_recording; }
        catch { recordingRef.current = false; }
        if (!recordingRef.current) {
          ownerRef.current = null; setStatus("error"); setMessage("未能啟動錄音"); latest.current.onError("無法開始錄音：" + String(error));
        }
      } finally {
        if (recordingRef.current) {
          startedAtRef.current = Date.now(); setStatus("recording");
          setMessage(source === "hotkey" && sessionRef.current.behavior === "hold" ? "錄音中，鬆開停止" : "錄音中，再點一下停止");
        }
        startPromiseRef.current = null;
      }
    };
    startPromiseRef.current = start();
    return true;
  };

  const finishRecording = (): Promise<void> => {
    if (microphoneTestRef.current || cancelRequestedRef.current || cancelPromiseRef.current) return Promise.resolve();
    if (stopPromiseRef.current) return stopPromiseRef.current;
    if (!startPromiseRef.current && !recordingRef.current) return Promise.resolve();
    stopStageRef.current = "waiting";
    const finish = async () => {
      try {
        await startPromiseRef.current;
        if (!recordingRef.current || cancelRequestedRef.current) return;
        const pendingLanguage = languageSaveRef.current;
        const languageSaved = pendingLanguage ? await pendingLanguage : !languageSaveFailedRef.current;
        if (!languageSaved) {
          languageSaveFailedRef.current = false;
          if (recordingRef.current && !cancelRequestedRef.current) setMessage("未能儲存語言，錄音仍在繼續；請重試或再次按完成。");
          return;
        }
        await pickerResizeRef.current;
        if (!recordingRef.current || cancelRequestedRef.current) return;
        stopStageRef.current = "stopping"; setStatus("processing"); setMessage("正在結束錄音…");
        let audio: { audio_path: string; duration: number };
        try { audio = await invoke("stop_recording"); }
        catch (error) {
          try { recordingRef.current = (await invoke<RecordingStatus>("get_recording_status")).is_recording; } catch { /* Keep retry available if native status is unknown. */ }
          if (!recordingRef.current) ownerRef.current = null;
          setStatus(recordingRef.current ? "recording" : "error"); setMessage(recordingRef.current ? "停止失敗，可再按停止或取消" : "錄音已結束，請重新錄音");
          latest.current.onError("無法停止錄音：" + String(error)); return;
        }
        recordingRef.current = false; ownerRef.current = null; startedAtRef.current = null;
        setElapsed(audio.duration); stopStageRef.current = "processing"; setMessage("正在辨識及處理文字…");
        const result = await invoke<DictationResult>("complete_dictation", { audioPath: audio.audio_path, mode: sessionRef.current.mode });
        if (!result.text.trim()) { setStatus("error"); setMessage("沒有辨識到文字"); latest.current.onError(result.warning || "請確認麥克風及錄音內容。"); return; }
        latest.current.onResult(result);
        // Successful native insertion preserves the user's clipboard. Copy is always explicit.
        setStatus("done"); setMessage(result.inserted ? "已插入原來的輸入位置" : result.mode === "ask" ? "回答已準備好" : result.paste_posted ? "已送出文字，請確認原本輸入位置" : "文字已準備好，可複製貼上");
        try { await latest.current.onHistoryRefresh(); } catch { latest.current.onError("文字已完成，暫時無法重新載入歷史記錄。"); }
      } catch (error) { setStatus("error"); setMessage("處理失敗，錄音已保存在本機"); latest.current.onError("轉錄失敗：" + String(error)); }
      finally { stopPromiseRef.current = null; stopStageRef.current = null; }
    };
    stopPromiseRef.current = finish();
    return stopPromiseRef.current;
  };

  const cancelRecording = (): Promise<void> => {
    if (cancelPromiseRef.current) return cancelPromiseRef.current;
    if ((!recordingRef.current && !startPromiseRef.current) || (stopPromiseRef.current && stopStageRef.current !== "waiting")) return Promise.resolve();
    cancelRequestedRef.current = true;
    const cancel = async () => {
      try {
        await startPromiseRef.current;
        if (recordingRef.current) await invoke("cancel_recording");
        recordingRef.current = false; ownerRef.current = null; startedAtRef.current = null;
        setStatus("idle"); setElapsed(0); setMessage("錄音已取消，沒有轉錄或插入文字");
        // Keep another session from starting until any selected language finishes saving.
        await languageSaveRef.current;
      } catch (error) { setStatus(recordingRef.current ? "recording" : "error"); latest.current.onError("未能取消錄音，請重試：" + String(error)); }
      finally { cancelPromiseRef.current = null; cancelRequestedRef.current = false; }
    };
    cancelPromiseRef.current = cancel(); return cancelPromiseRef.current;
  };

  const recordClick = () => {
    if (startPromiseRef.current || stopPromiseRef.current || cancelPromiseRef.current || microphoneTestRef.current) return;
    if (recordingRef.current) void finishRecording(); else beginRecording("button");
  };
  useLayoutEffect(() => {
    fnHandlerRef.current = requestedMode => {
      if (!latest.current.ready || latest.current.recordingBlocked || !latest.current.config.fn_shortcut_enabled || !latest.current.fnAvailable) return;
      if (recordingRef.current || startPromiseRef.current) void finishRecording();
      else beginRecording("fn", requestedMode);
    };
    handlersRef.current = {
      press: () => {
        if (ownerRef.current === "hotkey" && sessionRef.current.behavior === "toggle") void finishRecording();
        else beginRecording("hotkey");
      },
      release: () => { if (ownerRef.current === "hotkey" && sessionRef.current.behavior === "hold") void finishRecording(); },
      stop: () => { void finishRecording(); }, cancel: () => { void cancelRecording(); },
      resizePicker: open => {
        if (sessionRef.current.mode !== "translate" || (!recordingRef.current && !startPromiseRef.current) || stopStageRef.current || cancelRequestedRef.current) return;
        const recordingStatus = recordingRef.current ? "recording" : "starting";
        pickerResizeRef.current = pickerResizeRef.current.then(async () => {
          if (stopStageRef.current || cancelRequestedRef.current) return;
          await invoke("set_voice_status", { status: recordingStatus, mode: "translate", pickerOpen: open });
        }).catch(() => {});
      },
      selectLanguage: language => {
        const feedback = (payload: { translation_language?: string; language_error?: string }) => {
          // This event contains only a language or a fixed message, never config or keys.
          void emit("dictation-status", payload).catch(() => {});
        };
        if (sessionRef.current.mode !== "translate" || (!recordingRef.current && !startPromiseRef.current) || stopStageRef.current || cancelRequestedRef.current || languageSaveRef.current || !languageOptions.some(([value]) => value === language)) {
          feedback({ language_error: "未能變更語言，請再試。" }); return;
        }
        languageSaveFailedRef.current = false;
        const save = async () => {
          try {
            if (!latest.current.onTranslationLanguageChange) throw new Error("language save unavailable");
            await latest.current.onTranslationLanguageChange(language);
            feedback({ translation_language: language, language_error: "" });
            return true;
          } catch {
            languageSaveFailedRef.current = true;
            feedback({ language_error: "未能儲存語言，請再試。" });
            return false;
          }
        };
        const pending = save();
        languageSaveRef.current = pending;
        void pending.then(() => { if (languageSaveRef.current === pending) languageSaveRef.current = null; });
      },
    };
  });

  const testMicrophone = async (deviceName: string | null) => {
    if (!latest.current.ready || updateLockRef.current || microphoneTestRef.current || startPromiseRef.current || stopPromiseRef.current || cancelPromiseRef.current || recordingRef.current || ownerRef.current) return;
    microphoneTestRef.current = true; setTesting(true); setMicrophoneError(false); setMicrophoneMessage("測試中，請說話…");
    try { setMicrophoneMessage(await invoke<string>("test_microphone", { deviceName: deviceName || undefined })); }
    catch (error) { setMicrophoneError(true); setMicrophoneMessage("麥克風測試失敗：" + String(error)); }
    finally { microphoneTestRef.current = false; setTesting(false); }
  };
  // Synchronous guard closes the gap between deciding to install and React's next render.
  const lockForUpdate = () => {
    if (!latest.current.ready || updateLockRef.current || microphoneTestRef.current || startPromiseRef.current || stopPromiseRef.current || cancelPromiseRef.current || recordingRef.current || ownerRef.current) return false;
    updateLockRef.current = true;
    return true;
  };
  const unlockAfterUpdate = () => { updateLockRef.current = false; };
  const updateLocked = () => updateLockRef.current;
  return { status, message, elapsed, testing, microphoneMessage, microphoneError, recordClick, cancelRecording, testMicrophone, lockForUpdate, unlockAfterUpdate, updateLocked,
    recording: status === "recording", busy: status === "starting" || status === "recording" || status === "processing" || testing,
    recordDisabled: !options.ready || options.recordingBlocked || status === "starting" || status === "processing" || testing,
  };
}
