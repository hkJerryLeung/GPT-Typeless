import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AudioLines, BookOpen, Check, ChevronRight, Clock3, Copy, CornerDownLeft, FilePenLine, Home, Languages, MessageCircle, Mic, Settings2, ShieldCheck, SlidersHorizontal, Square, TextQuote, X } from "lucide-react";
import { activeProvider, defaultConfig, formatDuration, languageOptions, normalizeConfig, prepareConfig, redactProviderKeys, type AppConfig, type AudioDevice, type DictationMode, type DictationResult, type FnStatus, type InsertionStatus, type TranscriptionHistory } from "./types";
import { useDictation } from "./hooks/useDictation";
import { useUpdater } from "./hooks/useUpdater";
import { AnswerText } from "./components/AnswerText";
import { Keycaps, Toggle } from "./components/NativeControls";
import { SettingsDialog, type SettingsTab } from "./components/Settings";
import { SetupWizard, type SetupStep } from "./components/SetupWizard";
import { HistoryPage } from "./components/HistoryPage";
import { VocabularyPage } from "./components/VocabularyPage";
import "./App.css";

type Page = "home" | "history" | "dictionary" | "snippets";
const navigation = [["home", Home, "首頁"], ["history", Clock3, "歷史記錄"], ["dictionary", BookOpen, "詞庫"], ["snippets", TextQuote, "常用短語"]] as const;
const modes = [
  { id: "dictate", icon: Mic, title: "聽寫", description: "把想法說出來，變成文字。" },
  { id: "translate", icon: Languages, title: "翻譯", description: "用你習慣的語言說，轉成另一種語言。" },
  { id: "edit", icon: FilePenLine, title: "編輯選取文字", description: "先選取文字，再說出修改方式。" },
  { id: "ask", icon: MessageCircle, title: "問 AI", description: "用聲音提問，在回答卡片查看結果。" },
] as const;

export default function App() {
  const [config, setConfig] = useState<AppConfig>(defaultConfig);
  const configRef = useRef(config);
  const [ready, setReady] = useState(false);
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [permission, setPermission] = useState<InsertionStatus | null>(null);
  const [fnStatus, setFnStatus] = useState<FnStatus | null>(null);
  const [history, setHistory] = useState<TranscriptionHistory[]>([]);
  const [page, setPage] = useState<Page>("home");
  const [mode, setMode] = useState<DictationMode>("dictate");
  const [settingsTab, setSettingsTab] = useState<SettingsTab | null>(null);
  const [setupOpen, setSetupOpen] = useState(false);
  const [setupStep, setSetupStep] = useState<SetupStep>("token");
  const setupChecked = useRef(false);
  const [result, setResult] = useState<DictationResult | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [saving, setSaving] = useState(false);
  const saveRef = useRef(false);
  const [reload, setReload] = useState(0);
  const showMain = () => { void invoke("show_main_window").catch(() => {}); };
  const refreshHistory = async () => setHistory(await invoke<TranscriptionHistory[]>("get_transcription_history"));
  const dictation = useDictation({ config, ready, mode, recordingBlocked: settingsTab !== null || setupOpen || saving, fnAvailable: !!fnStatus?.enabled && fnStatus.available, onModeChange: setMode,
    onTranslationLanguageChange: async language => { await invoke("set_translation_language", { language }); setCurrentConfig({ ...configRef.current, translation_language: language }); },
    onError: message => { setError(message); if (message) showMain(); }, onMissingToken: () => { setSetupStep("token"); setSetupOpen(true); showMain(); },
    onResult: value => { setResult(value); setPage("home"); if (value.mode === "ask" || value.warning) showMain(); }, onHistoryRefresh: refreshHistory,
  });
  // An uninserted result remains in memory until the user dismisses it.
  const updateIdle = ready && !dictation.busy && !saving && settingsTab === null && !setupOpen && (!result || result.inserted);
  const updater = useUpdater({ enabled: config.auto_update_enabled, ready, idle: updateIdle,
    acquire: () => updateIdle && !saveRef.current && dictation.lockForUpdate(), release: dictation.unlockAfterUpdate });
  const setCurrentConfig = (value: AppConfig) => { configRef.current = value; setConfig(value); };

  useEffect(() => {
    let disposed = false;
    const report = (message: string, error: unknown) => { if (!disposed) setError(message + String(error)); };
    // These independent requests never delay hotkey listener registration in useDictation.
    void invoke("check_hotkey").catch(error => report("快捷鍵尚未啟用：", error));
    void invoke<AppConfig>("get_config_command").then(value => {
      if (!disposed) {
        const loaded = normalizeConfig(value);
        setCurrentConfig(loaded); setReady(true);
        if (!setupChecked.current) { setupChecked.current = true; if (!loaded.chatgpt_token.trim()) setSetupOpen(true); }
      }
    }).catch(error => report("無法載入設定：", error));
    void invoke<AudioDevice[]>("list_audio_devices").then(value => { if (!disposed) setDevices(value); }).catch(error => report("無法載入麥克風：", error));
    void invoke<TranscriptionHistory[]>("get_transcription_history").then(value => { if (!disposed) setHistory(value); }).catch(error => report("無法載入歷史記錄：", error));
    return () => { disposed = true; };
  }, [reload]);

  useEffect(() => {
    let disposed = false;
    const recheck = () => { void invoke<InsertionStatus>("get_insertion_status").then(value => { if (!disposed) setPermission(value); }).catch(() => { if (!disposed) setPermission(null); }); };
    recheck(); window.addEventListener("focus", recheck);
    return () => { disposed = true; window.removeEventListener("focus", recheck); };
  }, []);
  useEffect(() => {
    let disposed = false;
    // Once the user has enabled Fn, retry the listener when they return from
    // System Settings. This does not request or grant system permissions.
    const recheck = () => { void invoke<FnStatus>(config.fn_shortcut_enabled ? "retry_fn_shortcut" : "get_fn_status").then(value => { if (!disposed) setFnStatus(value); }).catch(() => { if (!disposed) setFnStatus(null); }); };
    recheck(); window.addEventListener("focus", recheck);
    return () => { disposed = true; window.removeEventListener("focus", recheck); };
  }, [config.fn_shortcut_enabled]);
  useEffect(() => { document.documentElement.dataset.appearance = config.appearance; return () => { delete document.documentElement.dataset.appearance; }; }, [config.appearance]);
  useEffect(() => {
    if (!notice) return;
    const timer = window.setTimeout(() => setNotice(""), 3500);
    return () => window.clearTimeout(timer);
  }, [notice]);

  const persistConfig = async (value: AppConfig) => {
    if (!ready) throw new Error("設定尚未載入，請稍後再試。");
    if (saveRef.current || dictation.busy || dictation.updateLocked()) throw new Error("請等候目前操作完成。");
    saveRef.current = true; setSaving(true);
    try {
      const next = prepareConfig(value);
      await invoke("set_config", { config: next });
      setCurrentConfig(redactProviderKeys(next));
      setError(""); setNotice("設定已儲存");
    } finally { saveRef.current = false; setSaving(false); }
  };
  const patchConfig = (patch: Partial<AppConfig>) => persistConfig({ ...configRef.current, ...patch });
  const change = async (patch: Partial<AppConfig>) => { try { await patchConfig(patch); } catch (error) { setError(String(error)); } };
  const copyText = async (text: string) => {
    try { await invoke("copy_to_clipboard", { text }); setNotice("已複製，可按 ⌘V 貼上"); }
    catch { setError("未能複製，請選取下方文字後按 ⌘C。"); }
  };
  const requestPermission = async () => {
    if (!permission?.trusted) await invoke("request_accessibility");
    else if (!permission.post_event) await invoke("open_accessibility_settings");
    setPermission(await invoke<InsertionStatus>("get_insertion_status"));
  };
  const refreshPermission = async () => {
    try { setPermission(await invoke<InsertionStatus>("get_insertion_status")); }
    catch (error) { setPermission(null); throw error; }
  };
  const requestPostEvent = async () => {
    try { await invoke<boolean>("request_post_event_access"); }
    finally { await refreshPermission(); }
  };
  const refreshFn = async () => { setFnStatus(await invoke<FnStatus>("retry_fn_shortcut")); };
  const requestFnPermission = async () => { await invoke("request_input_monitoring"); await refreshFn(); };
  const enableFn = async () => { await patchConfig({ fn_shortcut_enabled: true }); await requestFnPermission(); };
  const openPermissionSettings = async () => { await invoke("open_accessibility_settings"); };
  const revealRunningApp = async () => { await invoke("reveal_running_app"); };
  const deleteHistory = async (id: string) => { await invoke("delete_history_item", { id }); setHistory(previous => previous.filter(item => item.id !== id)); };
  const updateHistory = async (id: string, text: string) => { await invoke("update_history_item", { id, text }); setHistory(previous => previous.map(item => item.id === id ? { ...item, text } : item)); };
  const openSettings = (tab: SettingsTab = "general") => { if (ready && !dictation.busy && !saveRef.current && !dictation.updateLocked()) setSettingsTab(tab); };
  const openSetup = (step: SetupStep = "token") => { if (ready && !dictation.busy && !saveRef.current && !dictation.updateLocked()) { setSettingsTab(null); setSetupStep(step); setSetupOpen(true); } };
  const provider = activeProvider(config);
  const fnReady = config.fn_shortcut_enabled && fnStatus?.enabled && fnStatus.available;
  const fnShortcuts: Partial<Record<DictationMode, string>> = { dictate: "Fn", translate: "Fn+Shift", ask: "Fn+Space" };
  const needsAI = mode !== "dictate" || config.text_mode === "polish";
  const wordCount = history.reduce((sum, item) => sum + Array.from(item.text.replace(/\s/g, "")).length, 0);
  const duration = history.reduce((sum, item) => sum + item.duration, 0);
  const disabled = !ready || saving || dictation.busy || updater.phase === "installing";
  const statusLabels = { idle: "就緒", starting: "啟動中", recording: "錄音中", processing: "處理中", done: "已完成", error: "需要留意" };

  return <>
    <div className="app-shell" inert={settingsTab !== null || setupOpen || updater.phase === "installing"}>
      <aside className="app-sidebar"><div className="sidebar-drag" data-tauri-drag-region /><div className="brand"><AudioLines size={24} strokeWidth={2.4} /><span>GPT-Typeless</span></div>
        <nav className="main-navigation" aria-label="主要導覽">{navigation.map(([id, Icon, label]) => <button key={id} className={page === id ? "selected" : ""} aria-current={page === id ? "page" : undefined} onClick={() => setPage(id)}><Icon size={18} />{label}</button>)}</nav>
        <div className="sidebar-bottom"><div className="sidebar-state"><span className={`status-dot ${dictation.status}`} /><span className="status-text">{dictation.testing ? "麥克風測試中" : ready && !config.chatgpt_token.trim() ? "尚未設定" : statusLabels[dictation.status]}</span></div><button className="sidebar-settings" id="open-settings" disabled={!ready || dictation.busy || saving} onClick={() => openSettings()}><Settings2 size={18} /><span>設定</span></button><button className="sidebar-setup" disabled={!ready || dictation.busy || saving} onClick={() => openSetup()}>設定教學 <ChevronRight size={13} /></button><small>GPT-Typeless v1.3.0</small></div>
      </aside>
      <main className={`app-content ${page === "home" ? "home-page" : ""}`} id="main-content">
        <div className="content-drag" data-tauri-drag-region />
        {error && <div className="error-banner" role="alert"><p className="error-message">{error}</p>{/(401|403)/.test(error) && <button className="text-button" disabled={disabled} onClick={() => openSetup()}>更新 Token</button>}<button className="icon-button" aria-label="關閉錯誤訊息" onClick={() => setError("")}><X size={16} /></button></div>}
        {!ready && <div className="loading-panel" role="status">正在載入本機設定… <button className="text-button" onClick={() => setReload(value => value + 1)}>重新載入</button></div>}
        {page === "home" && <>
          <header className="home-heading"><h1>動口，就能完成。</h1><p>讓想法直接成為文字。</p></header>
          {updater.phase === "ready" && <div className="setup-needed" role="status"><span>{!updateIdle ? "更新已下載，請先完成目前工作。" : config.auto_update_enabled ? "更新已下載，閒置時會自動安裝。" : "更新已下載。"}</span><button className="button" disabled={!updateIdle} onClick={() => void updater.install()}>立即安裝</button></div>}
          <div className="home-layout"><div className="home-primary">
            {ready && !config.chatgpt_token.trim() && <div className="setup-needed"><span>先完成設定，就可以開始語音輸入。</span><button className="button" disabled={disabled} onClick={() => openSetup()}>開始設定 <ChevronRight size={14} /></button></div>}
            <section className="dictation-panel" aria-label="語音輸入模式"><div className="mode-list">{modes.map(item => <button key={item.id} className={`mode-row ${mode === item.id ? "active" : ""}`} aria-pressed={mode === item.id} disabled={disabled} onClick={() => setMode(item.id)}><span className="mode-icon"><item.icon size={21} /></span><span className="mode-description"><strong>{item.title}</strong><small>{item.description}</small></span>{fnReady && fnShortcuts[item.id] ? <Keycaps shortcut={fnShortcuts[item.id]!} /> : mode === item.id ? <Keycaps shortcut={config.hotkey} /> : <ChevronRight size={17} className="mode-chevron" />}</button>)}</div>
              <div className="mode-options">{mode === "dictate" ? <div className="segmented"><button aria-pressed={config.text_mode === "raw"} disabled={disabled} onClick={() => void change({ text_mode: "raw" })}>辨識原文</button><button aria-pressed={config.text_mode === "polish"} disabled={disabled} onClick={() => void change({ text_mode: "polish" })}>AI 潤飾</button></div> : mode === "translate" ? <label>翻譯為 <select aria-label="翻譯目標語言" value={config.translation_language} disabled={disabled} onChange={e => void change({ translation_language: e.target.value })}>{!languageOptions.some(([value]) => value === config.translation_language) && <option value={config.translation_language}>{config.translation_language}</option>}{languageOptions.map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></label> : mode === "edit" ? <span>請先選取要修改的文字；選取內容會傳送到你設定的 AI 服務。</span> : <div className="ask-options"><div><span>網上搜尋（Gemini）</span><Toggle label="網上搜尋（Gemini）" checked={config.web_search_enabled} disabled={disabled || provider?.kind !== "gemini"} onChange={value => void change({ web_search_enabled: value })} /></div><small>提問時，如有選取文字，內容會傳送到你設定的 AI 服務。</small><small>{provider?.kind === "gemini" ? config.web_search_enabled ? "僅問 AI 會使用 Google Search 並附上來源。" : "按需要啟用 Google Search；只在提問後發送請求。" : "需要在 AI 服務選取已配置的 Gemini。"}</small></div>}</div>
              {needsAI && !provider && <div className="dependency-note">此模式需要 AI 服務與金鑰。未設定時會保留原文。<button disabled={disabled} className="text-button" onClick={() => openSettings("providers")}>設定 AI <ChevronRight size={13} /></button></div>}
              <div className="recording-actions"><div><p className="status-message" role="status">{dictation.message || (fnReady && mode !== "edit" ? "Fn 快捷鍵按一下開始，再按一下完成。" : config.shortcut_behavior === "hold" ? "按住快捷鍵說話，鬆開完成。" : "按一下快捷鍵開始，再按一下完成。")}</p>{(dictation.status === "recording" || dictation.status === "processing") && <span className="recording-time">{formatDuration(dictation.elapsed)}</span>}</div><div className="record-buttons">{(dictation.status === "starting" || dictation.recording) && <button className="icon-button cancel-recording" aria-label="取消錄音" onClick={() => void dictation.cancelRecording()}><X size={18} /></button>}<button className={`button primary record-btn ${dictation.recording ? "recording" : ""}`} disabled={dictation.recordDisabled || saving} aria-pressed={dictation.recording} onClick={() => { setNotice(""); dictation.recordClick(); }}>{dictation.recording ? <Square size={14} fill="currentColor" /> : <Mic size={16} />}{dictation.recording ? "完成" : dictation.status === "starting" ? "啟動中…" : dictation.status === "processing" ? "處理中…" : "開始說話"}</button></div></div>
            </section>
            {result && <section className={`result-card ${result.mode === "ask" ? "answer-card" : ""}`} aria-label={result.mode === "ask" ? "AI 回答" : "語音輸入結果"}><header><div>{result.mode === "ask" ? <MessageCircle size={17} /> : result.inserted ? <Check size={17} /> : <CornerDownLeft size={17} />}<h2>{result.mode === "ask" ? "AI 回答" : result.inserted ? "已插入文字" : result.paste_posted ? "已送出文字" : "文字已準備好"}</h2></div><button className="button subtle copy-result-btn" onClick={() => void copyText(result.text)}><Copy size={15} />複製</button><button className="icon-button" aria-label="關閉結果" onClick={() => setResult(null)}><X size={16} /></button></header><p className="transcription-content" tabIndex={0}>{result.mode === "ask" ? <AnswerText text={result.text} onError={setError} /> : result.text}</p>{result.warning && <p className={`result-warning ${result.paste_posted && !result.inserted ? "paste-unconfirmed" : ""}`}>{result.warning}</p>}{result.insertion_detail && <details><summary>自動輸入詳情</summary><p>{result.insertion_detail}</p></details>}{!result.inserted && !result.paste_posted && result.mode !== "ask" && <p className="hint">可複製後切換到輸入位置，按 ⌘V 貼上。</p>}{result.raw_text && result.raw_text !== result.text && <details><summary>辨識原文</summary><p>{result.raw_text}</p></details>}</section>}
            {result?.warning && !result.inserted && !result.paste_posted && result.mode !== "ask" && config.auto_insert && permission && (!permission.trusted || !permission.post_event) && <div className="result-permission"><span>開啟權限後，可以自動輸入到其他 App。</span><button className="button" disabled={disabled} onClick={() => openSetup("permission")}>設定自動輸入權限 <ChevronRight size={14} /></button></div>}
            <section className="make-it-yours"><h2>照你的方式書寫</h2><div className="usecase-grid"><button disabled={!ready || dictation.busy} onClick={() => setPage("dictionary")}><BookOpen /><strong>專有名詞</strong><span>設定名稱與常見辨識別名。</span><ChevronRight size={14} /></button><button disabled={!ready || dictation.busy} onClick={() => setPage("snippets")}><TextQuote /><strong>常用短語</strong><span>用一句話展開一段文字。</span><ChevronRight size={14} /></button><button disabled={disabled} onClick={() => openSettings("personalization")}><SlidersHorizontal /><strong>你的語氣</strong><span>為不同應用程式調整寫法。</span><ChevronRight size={14} /></button></div></section>
          </div><aside className="home-secondary"><section className="usage-panel"><h2>本機記錄</h2><div><Clock3 size={18} /><strong>{formatDuration(duration)}</strong><span>錄音時長</span></div><div><AudioLines size={18} /><strong>{wordCount.toLocaleString("zh-Hant")}</strong><span>文字字元</span></div><div><TextQuote size={18} /><strong>{history.length}</strong><span>筆記錄</span></div><p>依目前保留的歷史計算</p></section><section className="insertion-panel"><div><ShieldCheck size={18} /><h2>自動插入</h2><Toggle checked={config.auto_insert} disabled={disabled} label="自動插入文字" onChange={value => void change({ auto_insert: value })} /></div><p>{!config.auto_insert ? "完成後手動複製文字。" : permission?.trusted && permission.post_event ? "文字會回到原來的輸入位置。" : permission ? "需要控制其他 App 權限。" : "尚未確認控制其他 App 權限。"}</p>{config.auto_insert && <button className="text-button" disabled={disabled} onClick={() => openSetup("permission")}>{permission?.trusted && permission.post_event ? "查看權限" : "設定權限"} <ChevronRight size={13} /></button>}</section><section className="ai-summary"><AudioLines size={16} /><span>{provider ? provider.name : "AI 尚未啟用"}</span><button className="icon-button" aria-label="設定 AI 服務" disabled={disabled} onClick={() => openSettings("providers")}><Settings2 size={15} /></button></section></aside></div>
        </>}
        {page === "history" && <HistoryPage history={history} enabled={config.history_enabled} busy={disabled} onEnabled={value => patchConfig({ history_enabled: value })} onCopy={copyText} onDelete={deleteHistory} onUpdate={updateHistory} onClear={async () => { await invoke("clear_all_history"); setHistory([]); }} />}
        {(page === "dictionary" || page === "snippets") && <VocabularyPage key={page} kind={page} dictionary={config.dictionary} snippets={config.snippets} busy={disabled} onDictionary={dictionary => patchConfig({ dictionary })} onSnippets={snippets => patchConfig({ snippets })} />}
      </main>
    </div>
    {notice && <div className="toast" role="status"><Check size={15} />{notice}</div>}
    {setupOpen && <SetupWizard initialStep={setupStep} config={config} permission={permission} fnStatus={fnStatus} onClose={() => setSetupOpen(false)} onSaveToken={token => patchConfig({ chatgpt_token: token })} onRequestPermission={openPermissionSettings} onRequestPostEvent={requestPostEvent} onRefreshPermission={refreshPermission} onRevealApp={revealRunningApp} onEnableFn={enableFn} onRequestFnPermission={requestFnPermission} onRefreshFn={refreshFn} testMicrophone={dictation.testMicrophone} testing={dictation.testing} microphoneMessage={dictation.microphoneMessage} microphoneError={dictation.microphoneError} />}
    {settingsTab && <SettingsDialog config={config} devices={devices} permission={permission} fnStatus={fnStatus} onRequestFnPermission={requestFnPermission} onRefreshFn={refreshFn} initialTab={settingsTab} onClose={() => setSettingsTab(null)} onSave={persistConfig} onRefreshDevices={async () => setDevices(await invoke<AudioDevice[]>("list_audio_devices"))} onRequestPermission={requestPermission} onKeyCleared={id => setCurrentConfig({ ...configRef.current, providers: configRef.current.providers.map(p => p.id === id ? { ...p, has_key: false, api_key: "" } : p) })} testMicrophone={dictation.testMicrophone} testing={dictation.testing} microphoneMessage={dictation.microphoneMessage} microphoneError={dictation.microphoneError} updateMessage={updater.message} onCheckUpdate={updater.check} />}
  </>;
}
