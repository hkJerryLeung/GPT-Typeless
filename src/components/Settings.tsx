import { useRef, useState, type KeyboardEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AudioLines, Check, Cpu, Globe2, Info, Keyboard, Mic, Plus, RotateCcw, Settings2, SlidersHorizontal, Trash2 } from "lucide-react";
import { activeProvider, languageOptions, prepareConfig, toneOptions, type AppConfig, type AudioDevice, type FnStatus, type InsertionStatus, type ProviderConfig } from "../types";
import { Dialog, Keycaps, SettingRow, Toggle } from "./NativeControls";

export type SettingsTab = "general" | "providers" | "personalization" | "about";
interface Props {
  config: AppConfig; devices: AudioDevice[]; permission: InsertionStatus | null;
  fnStatus: FnStatus | null; onRequestFnPermission: () => Promise<void>; onRefreshFn: () => Promise<void>;
  initialTab: SettingsTab; onClose: () => void; onSave: (config: AppConfig) => Promise<void>;
  onRefreshDevices: () => Promise<void>; onRequestPermission: () => Promise<void>;
  onKeyCleared: (id: string) => void;
  testMicrophone: (device: string | null) => Promise<void>; testing: boolean;
  microphoneMessage: string; microphoneError: boolean;
  updateMessage: string; onCheckUpdate: () => Promise<void>;
}
const providerPresets: Record<ProviderConfig["kind"], { name: string; base_url: string; model: string }> = {
  gemini: { name: "Gemini", base_url: "https://generativelanguage.googleapis.com/v1beta", model: "gemini-3.8-flash" },
  openai: { name: "OpenAI", base_url: "https://api.openai.com/v1", model: "" },
  anthropic: { name: "Anthropic", base_url: "https://api.anthropic.com/v1", model: "" },
  "openai-compatible": { name: "OpenAI 相容", base_url: "", model: "" },
};
function captureShortcut(event: KeyboardEvent<HTMLInputElement>, update: (shortcut: string) => void) {
  if (["Tab", "Escape"].includes(event.key)) return;
  event.preventDefault();
  if (["Meta", "Shift", "Alt", "Control"].includes(event.key)) return;
  const keys = [event.metaKey ? "Cmd" : "", event.ctrlKey ? "Ctrl" : "", event.altKey ? "Alt" : "", event.shiftKey ? "Shift" : "", event.code === "Space" ? "Space" : event.key.length === 1 ? event.key.toUpperCase() : event.key].filter(Boolean);
  if (keys.length > 1) update(keys.join("+"));
}

export function SettingsDialog(props: Props) {
  const [draft, setDraft] = useState(props.config);
  const [tab, setTab] = useState<SettingsTab>(props.initialTab);
  const [saving, setSaving] = useState(false);
  const [working, setWorking] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [testResult, setTestResult] = useState<{ id: string; text: string } | null>(null);
  const [insertionResult, setInsertionResult] = useState<{ text: string; failed: boolean } | null>(null);
  const workRef = useRef(false);
  const update = <K extends keyof AppConfig>(key: K, value: AppConfig[K]) => setDraft(previous => {
    const next = { ...previous, [key]: value };
    return { ...next, web_search_enabled: next.web_search_enabled && next.providers.some(p => p.id === next.active_provider_id && p.kind === "gemini") };
  });
  const updateProvider = (id: string, patch: Partial<ProviderConfig>) => {
    update("providers", draft.providers.map(p => p.id === id ? { ...p, ...patch } : p));
    setTestResult(null);
  };
  const dirty = JSON.stringify(draft) !== JSON.stringify(props.config);
  const locked = saving || !!working || props.testing;
  const close = () => { if (!locked) props.onClose(); };
  const save = async () => {
    if (workRef.current) return;
    const next = prepareConfig(draft);
    if (!next.hotkey) { setError("請設定全域快捷鍵。"); return; }
    const invalid = next.providers.find(p => !p.name || !p.model || !p.base_url);
    if (invalid) { setError(`請填寫「${invalid.name || "AI 設定檔"}」的名稱、模型及 API 網址。`); setTab("providers"); return; }
    if (next.app_rules.some(r => !r.app_name.trim())) { setError("請填寫每項應用程式規則的名稱。"); setTab("personalization"); return; }
    if (!next.translation_language.trim()) { setError("請填寫翻譯目標語言。"); setTab("general"); return; }
    workRef.current = true; setSaving(true); setError("");
    try { await props.onSave(next); props.onClose(); }
    catch (error) { setError("無法儲存設定：" + String(error)); }
    finally { workRef.current = false; setSaving(false); }
  };
  const run = async (id: string, fn: () => Promise<void>) => {
    if (workRef.current) return;
    workRef.current = true; setWorking(id); setError("");
    try { await fn(); } catch (error) { setError(String(error)); }
    finally { workRef.current = false; setWorking(null); }
  };
  const provider = activeProvider(draft);
  const aiReady = !!provider;
  const menus = [["general", Settings2, "設定"], ["providers", Cpu, "AI 服務"], ["personalization", SlidersHorizontal, "個人化"], ["about", Info, "關於"]] as const;
  return <Dialog title="設定" className="settings-dialog" onClose={close} busy={locked}>
    <aside className="settings-sidebar"><div className="settings-brand">GPT-Typeless</div><nav aria-label="設定分類">{menus.map(([id, Icon, label]) => <button key={id} className={tab === id ? "selected" : ""} aria-current={tab === id ? "page" : undefined} onClick={() => setTab(id)}><Icon size={18} />{label}</button>)}</nav><p>偏好設定儲存在此 Mac</p></aside>
    <div className="settings-body"><header className="settings-heading"><h2>{menus.find(([id]) => id === tab)?.[2]}</h2></header>
      <div className="settings-scroll">
        <fieldset className="settings-fieldset" disabled={locked}>
        {tab === "general" && <>
          <section className="settings-section"><h3><Keyboard />鍵盤快捷鍵</h3>
            <SettingRow title="全域錄音" description="使用首頁選取的模式。在其他應用程式也能啟動。"><div className="shortcut-editor"><Keycaps shortcut={draft.hotkey} /><input id="hotkey-input" aria-label="按下新的快捷鍵" value={draft.hotkey} readOnly onKeyDown={e => captureShortcut(e, value => update("hotkey", value))} title="點選後按下新的組合鍵" /><button className="icon-button" title="重設為 ⌘⇧R" aria-label="重設快捷鍵" onClick={() => update("hotkey", "Cmd+Shift+R")}><RotateCcw size={15} /></button></div></SettingRow>
            <SettingRow title="快捷鍵操作" description="錄音按鈕始終是點一下開始，再點一下完成。"><select id="shortcut-behavior" value={draft.shortcut_behavior} onChange={e => update("shortcut_behavior", e.target.value as AppConfig["shortcut_behavior"])}><option value="hold">按住說話，鬆開完成</option><option value="toggle">按一下開始，再按一下完成</option></select></SettingRow>
            <SettingRow title="使用 Fn 快捷鍵" description="Fn 聽寫、Fn⇧ 翻譯、Fn Space 問 AI；按一下開始，再按一下完成。"><Toggle label="使用 Fn 快捷鍵" checked={draft.fn_shortcut_enabled} onChange={value => update("fn_shortcut_enabled", value)} /></SettingRow>
            <p className="hint" role="status">{draft.fn_shortcut_enabled !== props.config.fn_shortcut_enabled ? "儲存後套用 Fn 設定。" : props.fnStatus?.message || "尚未確認 Fn 快捷鍵狀態。"}</p>
            <p className="hint">部分 Logitech 鍵盤嘅 Fn 不會傳送給 macOS；可使用上方全域快捷鍵。</p>
            {!props.fnStatus?.available && <SettingRow title="Fn 權限" description="需要你允許控制其他 App 與輸入監控；Fn 可能取代 macOS 原本的 Fn 操作。原有全域快捷鍵仍可使用。"><div className="control-stack"><button className="button" onClick={() => void run("fn-permission", props.onRequestFnPermission)}>開啟輸入監控設定</button>{props.config.fn_shortcut_enabled && <button className="text-button" onClick={() => void run("fn-retry", props.onRefreshFn)}>重新檢查 Fn</button>}</div></SettingRow>}
          </section>
          <section className="settings-section"><h3><Globe2 />語言</h3>
            <SettingRow title="介面語言"><span className="secondary">繁體中文</span></SettingRow>
            <SettingRow title="語言慣用形式" description="AI 潤飾使用此語言與地區偏好。"><select value={draft.language_variant} onChange={e => update("language_variant", e.target.value)}><option value="zh-Hant-HK">繁體中文（香港）</option><option value="zh-Hant-TW">繁體中文（台灣）</option><option value="zh-Hans-CN">简体中文</option><option value="en-US">English (US)</option><option value="en-GB">English (UK)</option>{!["zh-Hant-HK", "zh-Hant-TW", "zh-Hans-CN", "en-US", "en-GB"].includes(draft.language_variant) && <option value={draft.language_variant}>{draft.language_variant}</option>}</select></SettingRow>
            <SettingRow title="翻譯目標語言" description="選擇或輸入任意語言。"><input list="settings-languages" value={draft.translation_language} onChange={e => update("translation_language", e.target.value)} /><datalist id="settings-languages">{languageOptions.map(([value, label]) => <option key={value} value={value}>{label}</option>)}</datalist></SettingRow>
          </section>
          <section className="settings-section"><h3><Mic />麥克風</h3>
            <SettingRow title="輸入裝置"><div className="control-stack"><select id="device-select" value={draft.audio_device || ""} onChange={e => update("audio_device", e.target.value || null)}><option value="">自動偵測（系統預設）</option>{draft.audio_device && !props.devices.some(d => d.name === draft.audio_device) && <option value={draft.audio_device}>{draft.audio_device}（未連接）</option>}{props.devices.map(device => <option key={device.name} value={device.name}>{device.name}{device.is_default ? "（預設）" : ""}</option>)}</select><button className="text-button" onClick={() => void run("devices", props.onRefreshDevices)}>重新偵測</button></div></SettingRow>
            <SettingRow title="測試麥克風" description="本機錄音 2 秒，不會上傳或讀取 API 金鑰。"><button id="test-microphone-btn" className="button" disabled={locked} onClick={() => void props.testMicrophone(draft.audio_device)}>{props.testing ? "測試中…" : "測試麥克風"}</button></SettingRow>
            <p id="microphone-test-result" className={props.microphoneError ? "inline-error" : "hint"} role={props.microphoneError ? "alert" : "status"}>{props.microphoneMessage}</p>
          </section>
          <section className="settings-section"><h3><Settings2 />輸入與外觀</h3>
            <SettingRow title="自動插入文字" description="完成後回到原來的輸入位置，不會自動傳送訊息。"><Toggle label="自動插入文字" checked={draft.auto_insert} onChange={value => update("auto_insert", value)} /></SettingRow>
            <SettingRow title="控制其他 App 權限" description={props.permission?.trusted && props.permission.post_event ? "已取得權限；個別應用程式仍可能不支援插入。" : props.permission ? "在私隱與安全性 → 輔助使用（或 Device Control and Data Access）啟用 GPT-Typeless。" : "尚未確認權限狀態。"}><button className="button" onClick={() => void run("permission", props.onRequestPermission)}>{props.permission?.trusted && props.permission.post_event ? "重新檢查" : "開啟權限設定"}</button></SettingRow>
            <SettingRow title="測試自動輸入" description="先在空白測試文件中點選文字插入位置，再返回此處按下測試。只會插入「GPT-Typeless 自動輸入測試」，不使用語音辨識、AI、網絡或金鑰。"><button id="test-insertion-btn" className="button" disabled={locked} onClick={() => void run("insertion", async () => {
              setInsertionResult({ text: "正在測試自動輸入…", failed: false });
              try { setInsertionResult({ text: await invoke<string>("test_insertion"), failed: false }); }
              catch (error) { setInsertionResult({ text: "自動輸入測試失敗：" + String(error), failed: true }); }
            })}>{working === "insertion" ? "測試中…" : "測試自動輸入"}</button></SettingRow>
            <p id="insertion-test-result" className={insertionResult?.failed ? "inline-error" : "hint"} role={insertionResult?.failed ? "alert" : "status"}>{insertionResult?.text}</p>
            <SettingRow title="浮動錄音列"><Toggle label="浮動錄音列" checked={draft.voice_bar_enabled} onChange={value => update("voice_bar_enabled", value)} /></SettingRow>
            <SettingRow title="操作音效"><Toggle label="操作音效" checked={draft.sound_enabled} onChange={value => update("sound_enabled", value)} /></SettingRow>
            <SettingRow title="保留歷史記錄" description="最多保留最近 1,000 筆文字；關閉不會刪除既有記錄。"><Toggle label="保留歷史記錄" checked={draft.history_enabled} onChange={value => update("history_enabled", value)} /></SettingRow>
            <SettingRow title="外觀"><select value={draft.appearance} onChange={e => update("appearance", e.target.value as AppConfig["appearance"])}><option value="system">跟隨系統</option><option value="light">淺色</option><option value="dark">深色</option></select></SettingRow>
          </section>
        </>}
        {tab === "providers" && <>
          <p className="settings-intro">語音辨識使用 ChatGPT Token；潤飾、翻譯、編輯與問答使用你選取的 AI 設定檔。</p>
          <section className="settings-section"><h3><AudioLines />語音辨識</h3><label className="field-label" htmlFor="token-input">ChatGPT Token</label><input id="token-input" type="password" autoComplete="off" spellCheck={false} value={draft.chatgpt_token} onChange={e => update("chatgpt_token", e.target.value)} placeholder="貼上 Token，可包含 Bearer 前綴" /><p className="hint">由原生端儲存至 macOS 鑰匙圈。</p>
            <details className="advanced-fields"><summary>進階請求設定</summary>{([ ["oai_client_build_number", "oai-client-build-number"], ["oai_client_version", "oai-client-version"], ["oai_device_id", "oai-device-id"] ] as const).map(([key, label]) => <label className="field" key={key}>{label}<input value={draft[key]} onChange={e => update(key, e.target.value)} autoComplete="off" spellCheck={false} /></label>)}</details>
          </section>
          <section className="settings-section"><h3><Cpu />AI 服務設定檔</h3>
            <SettingRow title="目前使用" description="停用 AI 時保留辨識原文。"><select id="active-provider" value={draft.active_provider_id ?? ""} onChange={e => update("active_provider_id", e.target.value || null)}><option value="">停用 AI</option>{draft.providers.map(provider => <option key={provider.id} value={provider.id}>{provider.name}</option>)}</select></SettingRow>
            <SettingRow title="網上搜尋（Gemini）" description={provider?.kind === "gemini" ? "只用於問 AI。啟用後，問題會交由 Gemini 的 Google Search 搜尋並附上來源。" : "需要選取已設定模型與金鑰的 Gemini；切換其他服務時會關閉。"}><Toggle label="網上搜尋（Gemini）" checked={draft.web_search_enabled} disabled={provider?.kind !== "gemini"} onChange={value => update("web_search_enabled", value)} /></SettingRow>
            {!aiReady && <p className="dependency-note">尚未選取完整的 AI 設定檔與金鑰。相關模式會保留原文並顯示提示。</p>}
            {draft.providers.map(provider => <article className="provider-profile" key={provider.id}>
              <header><h4>{provider.name || "AI 設定檔"}</h4><button className="icon-button danger" aria-label={`刪除 ${provider.name} 設定檔`} onClick={() => { update("providers", draft.providers.filter(p => p.id !== provider.id)); if (draft.active_provider_id === provider.id) update("active_provider_id", null); }}><Trash2 size={16} /></button></header>
              <div className="form-grid"><label className="field">名稱<input value={provider.name} onChange={e => updateProvider(provider.id, { name: e.target.value })} /></label><label className="field">服務<select value={provider.kind} onChange={e => { const kind = e.target.value as ProviderConfig["kind"]; updateProvider(provider.id, { kind, ...providerPresets[kind] }); }}><option value="gemini">Gemini</option><option value="openai">OpenAI</option><option value="anthropic">Anthropic</option><option value="openai-compatible">OpenAI 相容</option></select></label></div>
              <label className="field">API 網址<input type="url" value={provider.base_url} placeholder="https://…" onChange={e => updateProvider(provider.id, { base_url: e.target.value })} spellCheck={false} /></label>
              <label className="field">模型<input value={provider.model} placeholder="輸入供應商提供的模型 ID" onChange={e => updateProvider(provider.id, { model: e.target.value })} spellCheck={false} /></label>
              <label className="field">API 金鑰<input type="password" autoComplete="off" spellCheck={false} value={provider.api_key} placeholder={provider.has_key ? "已儲存，留空即可保留" : "輸入 API 金鑰"} onChange={e => updateProvider(provider.id, { api_key: e.target.value })} /></label>
              <div className="profile-actions"><span className="hint">{provider.has_key ? <><Check size={13} />已儲存金鑰</> : "尚未儲存金鑰"}</span>{provider.has_key && <button className="text-button danger" onClick={() => void run(provider.id, async () => { await invoke("clear_provider_key", { id: provider.id }); props.onKeyCleared(provider.id); updateProvider(provider.id, { has_key: false, api_key: "" }); })}>立即移除金鑰</button>}</div>
              <button className="button" disabled={!provider.base_url.trim() || !provider.model.trim() || (!provider.has_key && !provider.api_key.trim())} onClick={() => void run(provider.id, async () => { setTestResult(null); const text = await invoke<string>("test_provider", { provider }); setTestResult({ id: provider.id, text }); })}>{working === provider.id ? "請求中…" : "測試連線（發送請求）"}</button>
              {testResult?.id === provider.id && <p className="hint" role="status">{testResult.text}</p>}
            </article>)}
            <button className="button" id="add-provider" onClick={() => { const provider: ProviderConfig = { id: crypto.randomUUID(), kind: "gemini", ...providerPresets.gemini, api_key: "", has_key: false }; update("providers", [...draft.providers, provider]); }}><Plus size={15} />新增設定檔</button>
          </section>
        </>}
        {tab === "personalization" && <>
          <p className="settings-intro">設定你偏好的寫法，並為不同應用程式指定語氣。以下偏好用於已配置的 AI 文字處理。</p>
          {!aiReady && <p className="dependency-note">請在「AI 服務」選取模型及金鑰，讓個人化偏好套用至潤飾、翻譯與編輯。</p>}
          <section className="settings-section"><h3><SlidersHorizontal />寫作偏好</h3><SettingRow title="聽寫文字" description="原文仍會套用本機詞庫與完整觸發短語。"><select value={draft.text_mode} onChange={e => update("text_mode", e.target.value as AppConfig["text_mode"])}><option value="raw">辨識原文</option><option value="polish">AI 潤飾</option></select></SettingRow>
            <SettingRow title="預設語氣"><select value={draft.tone} onChange={e => update("tone", e.target.value)}>{toneOptions.map(([value, label]) => <option key={value} value={value}>{label}</option>)}{!toneOptions.some(([value]) => value === draft.tone) && <option value={draft.tone}>{draft.tone}</option>}</select></SettingRow>
            <label className="field">自訂寫作指示<textarea rows={4} value={draft.custom_instructions} placeholder="例如：保留香港廣東話用字；以短段落書寫。" onChange={e => update("custom_instructions", e.target.value)} /></label>
          </section>
          <section className="settings-section"><h3>應用程式規則 <span className="count">{draft.app_rules.length}</span></h3>
            {draft.app_rules.map(rule => <article className="provider-profile" key={rule.id}><header><h4>{rule.app_name || "新增規則"}</h4><button className="icon-button danger" aria-label="刪除應用程式規則" onClick={() => update("app_rules", draft.app_rules.filter(r => r.id !== rule.id))}><Trash2 size={16} /></button></header><div className="form-grid"><label className="field">應用程式名稱<input value={rule.app_name} placeholder="例如：Mail" onChange={e => update("app_rules", draft.app_rules.map(r => r.id === rule.id ? { ...r, app_name: e.target.value } : r))} /></label><label className="field">語氣<select value={rule.tone} onChange={e => update("app_rules", draft.app_rules.map(r => r.id === rule.id ? { ...r, tone: e.target.value } : r))}>{toneOptions.map(([value, label]) => <option key={value} value={value}>{label}</option>)}{!toneOptions.some(([value]) => value === rule.tone) && <option value={rule.tone}>{rule.tone}</option>}</select></label></div><label className="field">寫作指示<textarea rows={2} value={rule.instructions} onChange={e => update("app_rules", draft.app_rules.map(r => r.id === rule.id ? { ...r, instructions: e.target.value } : r))} /></label></article>)}
            <button className="button" onClick={() => update("app_rules", [...draft.app_rules, { id: crypto.randomUUID(), app_name: "", tone: "natural", instructions: "" }])}><Plus size={15} />新增應用程式規則</button>
          </section>
        </>}
        {tab === "about" && <><section className="about-panel"><AudioLines size={38} strokeWidth={1.5} /><h3>GPT-Typeless</h3><p>macOS 語音輸入與 AI 寫作工具</p><p className="secondary">獨立應用程式，並非 Typeless 官方產品。</p><div className="about-details"><p>語音辨識會使用你儲存的 ChatGPT Token。啟用 AI 處理時，文字及相關選取內容會傳送至你設定的服務。</p><p>歷史記錄與寫作偏好保存在此 Mac。API 金鑰由原生端儲存至鑰匙圈。</p><p>Gemini 問答可選擇啟用 Google Search 網上搜尋。跨裝置同步、訂閱服務與自動學習不在此版本範圍。</p></div></section>
          <section className="settings-section"><h3>軟件更新</h3>
            <SettingRow title="自動更新" description="定期向 GitHub 檢查，驗證簽署後於閒置時安裝。"><Toggle label="自動更新" checked={draft.auto_update_enabled} onChange={value => update("auto_update_enabled", value)} /></SettingRow>
            <SettingRow title="檢查更新" description={props.updateMessage}><button className="button" onClick={() => void run("update-check", props.onCheckUpdate)}>{working === "update-check" ? "檢查中…" : "立即檢查"}</button></SettingRow>
          </section>
        </>}
        </fieldset>
      </div>
      {error && <p className="settings-error" role="alert">{error}</p>}
      <footer className="settings-footer"><span>{dirty ? "有尚未儲存的變更" : "所有設定已儲存"}</span><button className="button" disabled={locked} onClick={close}>取消</button><button className="button primary modal-save-btn" disabled={locked || !dirty} onClick={() => void save()}>{saving ? "儲存中…" : "儲存變更"}</button></footer>
    </div>
  </Dialog>;
}
