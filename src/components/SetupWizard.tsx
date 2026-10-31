import { useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ArrowRight, Check, ExternalLink, Mic, ShieldCheck } from "lucide-react";
import { Dialog } from "./NativeControls";
import { SetupDrag } from "./SetupDrag";
import type { AppConfig, FnStatus, InsertionStatus } from "../types";

export type SetupStep = "token" | "microphone" | "permission";

interface Props {
  initialStep: SetupStep;
  config: AppConfig;
  permission: InsertionStatus | null;
  fnStatus: FnStatus | null;
  onClose: () => void;
  onSaveToken: (token: string) => Promise<void>;
  onRequestPermission: () => Promise<void>;
  onRequestPostEvent: () => Promise<void>;
  onRefreshPermission: () => Promise<void>;
  onRevealApp: () => Promise<void>;
  onEnableFn: () => Promise<void>;
  onRequestFnPermission: () => Promise<void>;
  onRefreshFn: () => Promise<void>;
  testMicrophone: (device: string | null) => Promise<void>;
  testing: boolean;
  microphoneMessage: string;
  microphoneError: boolean;
}

// Session pages contain account metadata too: keep only the access token.
function extractToken(input: string): { token: string; reason?: "missing" | "format" } {
  const value = input.trim();
  if (!value || value.startsWith("<")) return { token: "", reason: "missing" };
  const clean = (token: string) => token.trim().replace(/^Bearer(?:\s+|$)/i, "").trim();
  if (value.startsWith("{") || value.startsWith("[") || value.startsWith('"')) {
    try {
      const session: unknown = JSON.parse(value);
      if (typeof session === "string") return { token: clean(session) };
      if (session && !Array.isArray(session) && typeof session === "object") {
        const fields = session as Record<string, unknown>;
        const token = fields.accessToken ?? fields.access_token;
        if (typeof token === "string" && clean(token)) return { token: clean(token) };
      }
      return { token: "", reason: "missing" };
    } catch { /* A browser's formatted view may include extra text or omit JSON quotes. */ }
  }
  // Only accept one complete, specifically named value from a formatted browser view.
  // Do not silently truncate an unclosed or ambiguous value, or use other session credentials.
  const labels = /(?:^|[\s,{])(?:"accessToken"|'accessToken'|accessToken|"access_token"|'access_token'|access_token)\s*[:=]/gi;
  const pattern = /(?:^|[\s,{])(?:"accessToken"|'accessToken'|accessToken|"access_token"|'access_token'|access_token)\s*[:=]\s*(?:"([^"\r\n]+)"|'([^'\r\n]+)'|((?:Bearer\s+)?[A-Za-z0-9._~+\/=-]+))(?=\s|[,}\]]|$)/gi;
  const matches = Array.from(value.matchAll(pattern));
  if (Array.from(value.matchAll(labels)).length === 1 && matches.length === 1) {
    const candidate = clean(matches[0][1] ?? matches[0][2] ?? matches[0][3]);
    if (candidate && !/\s/.test(candidate)) return { token: candidate };
  }
  if (/accessToken|access_token/i.test(value)) return { token: "", reason: "format" };
  const direct = clean(value);
  if (!direct || /[\s{}\[\]]/.test(direct)) return { token: "", reason: "missing" };
  return { token: direct };
}

export function SetupWizard(props: Props) {
  const [step, setStep] = useState<SetupStep>(props.initialStep);
  const [input, setInput] = useState("");
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const [permissionBusy, setPermissionBusy] = useState(false);
  const requestingPostEvent = useRef(false);
  const busy = saving || props.testing || permissionBusy;

  const open = async (url: string) => {
    setError("");
    try { await openUrl(url); }
    catch { setError("未能開啟瀏覽器，請用瀏覽器手動開啟 chatgpt.com。"); }
  };
  const save = async () => {
    const { token, reason } = extractToken(input);
    if (!token) {
      setError(reason === "format" ? "搵到 accessToken，但讀唔到內容。請重新複製整個登入資料頁，或只複製 accessToken 後面引號內嘅長字串。" : "貼上嘅內容冇 accessToken。請按「開啟登入資料」，確認同一個瀏覽器已登入 ChatGPT；如果頁面只顯示 {} 或警告，請先重新登入再刷新。");
      return;
    }
    if (token.startsWith("sk-")) {
      setError("呢度需要 ChatGPT 登入 Token，唔係 OpenAI API 金鑰。請按上面步驟重新複製。");
      return;
    }
    if (/\s/.test(token)) {
      setError("貼上嘅資料格式唔正確，請重新複製登入資料嘅全部內容。");
      return;
    }
    setSaving(true); setError("");
    try {
      await props.onSaveToken(token);
      setInput("");
      setStep("microphone");
    } catch {
      setError("儲存失敗，請重試；如仍未成功，請檢查 macOS 鑰匙圈同 App 設定。");
    } finally { setSaving(false); }
  };
  const requestPermission = async () => {
    setPermissionBusy(true); setError("");
    try { await props.onRequestPermission(); }
    catch { setError("未能開啟權限設定，可稍後在設定中再試。"); }
    finally { setPermissionBusy(false); }
  };
  const requestPostEvent = async () => {
    if (requestingPostEvent.current) return;
    requestingPostEvent.current = true; setPermissionBusy(true); setError("");
    try { await props.onRequestPostEvent(); }
    catch { setError("未能請求自動貼入權限，請稍後再試或手動複製文字。"); }
    finally { requestingPostEvent.current = false; setPermissionBusy(false); }
  };
  const revealApp = async () => {
    setPermissionBusy(true); setError("");
    try { await props.onRevealApp(); }
    catch { setError("未能開啟 Finder。請手動喺應用程式資料夾搵 GPT-Typeless.app。"); }
    finally { setPermissionBusy(false); }
  };
  const refreshPermission = async () => {
    setPermissionBusy(true); setError("");
    try { await props.onRefreshPermission(); }
    catch { setError("未能檢查權限，請稍後再試。"); }
    finally { setPermissionBusy(false); }
  };
  const setupFn = async (action: () => Promise<void>) => {
    setPermissionBusy(true); setError("");
    try { await action(); }
    catch { setError("未能完成 Fn 設定。可喺「設定」重新啟用，原有快捷鍵仍可使用。"); }
    finally { setPermissionBusy(false); }
  };

  return <Dialog title="首次設定" className="setup-dialog" onClose={props.onClose} busy={busy}>
    <div className="setup-content">
      <div className="setup-progress" aria-label={`第 ${step === "token" ? 1 : step === "microphone" ? 2 : 3} 步，共 3 步`}>
        <span className={`setup-step-number ${step === "token" ? "current" : ""}`}>1</span><span>ChatGPT</span><span className="setup-progress-line" /><span className={`setup-step-number ${step === "microphone" ? "current" : ""}`}>2</span><span>麥克風</span><span className="setup-progress-line" /><span className={`setup-step-number ${step === "permission" ? "current" : ""}`}>3</span><span>自動輸入</span>
      </div>
      {step === "token" ? <>
        <h2>先連接 ChatGPT</h2>
        <p className="setup-intro">跟住以下步驟，就可以開始設定語音轉文字。</p>
        <ol className="setup-steps">
          <li><span>1</span><div><strong>登入 ChatGPT</strong><p>喺瀏覽器登入你嘅 ChatGPT 帳戶。</p><button type="button" className="button" onClick={() => void open("https://chatgpt.com/")}>開啟 ChatGPT <ExternalLink size={13} /></button></div></li>
          <li><span>2</span><div><strong>複製登入資料</strong><p>按下面按鈕開啟登入資料頁（唔係對話頁），確認見到 accessToken，再按 ⌘A 全選、⌘C 複製。</p><button type="button" className="button" onClick={() => void open("https://chatgpt.com/api/auth/session")}>開啟登入資料 <ExternalLink size={13} /></button></div></li>
          <li><span>3</span><div><label htmlFor="setup-token-input"><strong>貼上登入資料</strong></label><p>返回呢度，按 ⌘V 貼上；亦可以直接貼上 Token。</p><input id="setup-token-input" type="password" autoComplete="off" spellCheck={false} value={input} onChange={event => { setInput(event.target.value); setError(""); }} onPaste={event => {
            const extracted = extractToken(event.clipboardData.getData("text"));
            if (extracted.token) { event.preventDefault(); setInput(extracted.token); setError(""); }
          }} placeholder="貼上 Token 或完整內容" /></div></li>
        </ol>
        <p className="setup-security"><ShieldCheck size={16} />登入資料如同密碼，唔好分享或截圖。App 只會儲存 Token 到呢部 Mac 嘅鑰匙圈；貼上後可複製其他文字覆蓋剪貼簿。</p>
        {error && <p className="inline-error" role="alert">{error}</p>}
        <footer><button type="button" className="button subtle" disabled={busy} onClick={props.onClose}>稍後設定</button>{props.config.chatgpt_token.trim() && <button type="button" className="button" disabled={busy} onClick={() => { setError(""); setStep("microphone"); }}>已設定，繼續</button>}<button type="button" className="button primary" disabled={busy || !input.trim()} onClick={() => void save()}>{saving ? "儲存中…" : "儲存並繼續"}<ArrowRight size={15} /></button></footer>
      </> : step === "microphone" ? <>
        <h2>準備好講嘢</h2>
        <p className="setup-intro">{props.config.chatgpt_token.trim() ? "Token 已儲存。" : "可稍後設定 ChatGPT Token。"}首次使用時，請允許麥克風權限。</p>
        <div className="setup-check"><span className="setup-check-icon"><Mic size={18} /></span><div><strong>測試麥克風</strong><p>可以先講兩秒；測試錄音只會留喺本機，唔會上載。麥克風可喺設定更改。</p><button type="button" className="button" disabled={busy} onClick={() => void props.testMicrophone(props.config.audio_device)}>{props.testing ? "測試中…" : "測試麥克風"}</button>{props.microphoneMessage && <p className={props.microphoneError ? "inline-error" : "hint"} role={props.microphoneError ? "alert" : "status"}>{props.microphoneMessage}</p>}</div></div>
        <p className="setup-next"><Mic size={15} />下一步設定自動輸入，令文字回到原來嘅輸入位置。</p>
        <p className="hint">轉錄時錄音會傳送到 ChatGPT。Token 已儲存不代表連線已驗證；如過期或轉錄失敗，可重新設定。</p>
        {error && <p className="inline-error" role="alert">{error}</p>}
        <footer><button type="button" className="button" disabled={busy} onClick={() => setStep("token")}>上一步</button><button type="button" className="button primary" disabled={busy} onClick={() => { setError(""); setStep("permission"); }}>下一步 <ArrowRight size={15} /></button></footer>
      </> : <>
        <h2>開啟自動輸入</h2>
        <p className="setup-intro">允許 GPT-Typeless 控制其他 App，講完嘅文字就可以輸入到原來嘅位置。</p>
        <div className="setup-check"><span className="setup-check-icon"><ShieldCheck size={18} /></span><div><strong>1. 開啟系統設定</strong><p>按下面按鈕，開啟「私隱與安全性」嘅權限清單。</p><button type="button" className="button" disabled={busy} onClick={() => void requestPermission()}>開啟權限設定</button></div></div>
        <div className="setup-check"><span className="setup-check-icon">2</span><div><strong>2. 啟用 GPT-Typeless</strong><p>喺「輔助使用」清單搵 GPT-Typeless，開啟佢旁邊嘅開關。</p><p>如果清單冇，將下面圖示拖入清單，再開啟開關。</p><SetupDrag /><p>亦可以喺 Finder 顯示 App 再拖入，或按清單下方「＋」選擇。</p><button type="button" className="button" disabled={busy} onClick={() => void revealApp()}>在 Finder 顯示</button></div></div>
        <p className="setup-next" role="status"><Check size={15} />{props.permission?.trusted && props.permission.post_event ? "系統顯示已取得權限，可以試用自動輸入。" : props.permission?.trusted ? "已開啟輔助使用，但 macOS 尚未允許自動貼入；可請求權限或重新檢查系統設定。" : props.permission ? "尚未取得權限。完成後返回呢度，按「重新檢查」。" : "未能確認權限狀態，請按「重新檢查」。"}</p>
        {props.permission?.trusted && !props.permission.post_event && <button type="button" className="button" disabled={busy} onClick={() => void requestPostEvent()}>請求自動貼入權限</button>}
        <p className="hint">呢項權限需要你喺 macOS 親自啟用；稍後設定亦可以手動複製文字。</p>
        <details>
          <summary>使用 Typeless 式 Fn 快捷鍵（可選）</summary>
          <p className="hint">Fn 聽寫；Fn＋Shift 翻譯；Fn＋Space 提問。按一下開始，再按 Fn 完成。翻譯同提問需要另外設定 AI 服務。</p>
          <p className="hint">需要你喺系統設定允許「輸入監控」同控制其他 App。啟用後 Fn 可能取代 macOS 原本嘅 Fn 操作；原有快捷鍵仍然保留。</p>
          <p className="hint">部分 Logitech 鍵盤嘅 Fn 不會傳送給 macOS，可以使用全域快捷鍵。</p>
          <p className="hint">{props.fnStatus?.message || "尚未確認 Fn 快捷鍵狀態。"}</p>
          {!props.config.fn_shortcut_enabled
            ? <button type="button" className="button" disabled={busy} onClick={() => void setupFn(props.onEnableFn)}>啟用 Fn 並設定權限</button>
            : !props.fnStatus?.available && <><button type="button" className="button" disabled={busy} onClick={() => void setupFn(props.onRequestFnPermission)}>開啟輸入監控設定</button>{" "}<button type="button" className="button" disabled={busy} onClick={() => void setupFn(props.onRefreshFn)}>重新檢查 Fn</button></>}
        </details>
        {error && <p className="inline-error" role="alert">{error}</p>}
        <footer><button type="button" className="button" disabled={busy} onClick={() => { setError(""); setStep("microphone"); }}>上一步</button><button type="button" className="button" disabled={busy} onClick={() => void refreshPermission()}>重新檢查</button><button type="button" className="button primary" disabled={busy} onClick={props.onClose}>{props.permission?.trusted && props.permission.post_event ? "開始使用" : "稍後設定"}</button></footer>
      </>}
    </div>
  </Dialog>;
}
