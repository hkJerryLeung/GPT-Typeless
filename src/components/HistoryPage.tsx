import { useRef, useState } from "react";
import { Clock3, Copy, Pencil, Search, Trash2 } from "lucide-react";
import { formatDuration, modeLabels, type DictationMode, type TranscriptionHistory } from "../types";
import { AnswerText } from "./AnswerText";
import { Dialog, EmptyState } from "./NativeControls";

interface Props {
  history: TranscriptionHistory[]; enabled: boolean; busy: boolean;
  onEnabled: (enabled: boolean) => Promise<void>; onCopy: (text: string) => Promise<void>;
  onDelete: (id: string) => Promise<void>; onClear: () => Promise<void>;
  onUpdate: (id: string, text: string) => Promise<void>;
}
export function HistoryPage(props: Props) {
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState("all");
  const [editing, setEditing] = useState<TranscriptionHistory | null>(null);
  const [text, setText] = useState("");
  const [clearing, setClearing] = useState(false);
  const [working, setWorking] = useState(false);
  const workingRef = useRef(false);
  const [error, setError] = useState("");
  const query = search.trim().toLocaleLowerCase();
  const items = props.history.filter(item => (filter === "all" || (filter === "ask" ? item.mode === "ask" : item.mode !== "ask")) && `${item.text} ${item.raw_text || ""} ${item.app_name || ""}`.toLocaleLowerCase().includes(query)).sort((a, b) => b.timestamp - a.timestamp);
  const grouped = new Map<string, TranscriptionHistory[]>();
  const today = new Date().toLocaleDateString("zh-Hant");
  for (const item of items) { const date = new Date(item.timestamp).toLocaleDateString("zh-Hant"); const key = date === today ? "今天" : date; grouped.set(key, [...(grouped.get(key) ?? []), item]); }
  const run = async (action: () => Promise<void>) => {
    if (workingRef.current) return;
    workingRef.current = true; setWorking(true); setError("");
    try { await action(); } catch (error) { setError(String(error)); } finally { workingRef.current = false; setWorking(false); }
  };
  return <>
    <header className="page-heading"><h1>歷史記錄</h1><button className="button subtle danger" disabled={!props.history.length || working} onClick={() => setClearing(true)}><Trash2 size={15} />清空記錄</button></header>
    <div className="privacy-panel"><div><h3>保留歷史記錄</h3><p>{props.enabled ? "最近 1,000 筆文字保存在此 Mac。" : "新的轉錄不會加入歷史；既有記錄仍保留。"}</p></div><select aria-label="保留歷史記錄" disabled={props.busy || working} value={props.enabled ? "on" : "off"} onChange={e => void run(() => props.onEnabled(e.target.value === "on"))}><option value="on">保留</option><option value="off">不儲存</option></select></div>
    <div className="collection-toolbar"><div className="segmented" aria-label="歷史篩選">{[["all", "全部"], ["dictate", "語音輸入"], ["ask", "問 AI"]].map(([value, label]) => <button key={value} aria-pressed={filter === value} onClick={() => setFilter(value)}>{label}</button>)}</div><label className="search-field"><Search size={16} /><input aria-label="搜尋歷史記錄" placeholder="搜尋記錄" value={search} onChange={e => setSearch(e.target.value)} /></label></div>
    {error && <p className="inline-error" role="alert">{error}</p>}
    {!items.length ? <EmptyState icon={<Clock3 />} title={query || filter !== "all" ? "沒有符合的記錄" : "你的文字，隨時找得到"} description={query ? "試試其他關鍵字。" : "完成語音輸入後，文字會出現在這裡。"} /> : <div className="history-groups">{Array.from(grouped).map(([day, entries]) => <section key={day}><h2 className="date-label">{day}</h2><div className="history-list">{entries.map(item => <article key={item.id} className="history-item"><time dateTime={new Date(item.timestamp).toISOString()}>{new Date(item.timestamp).toLocaleTimeString("zh-Hant", { hour: "2-digit", minute: "2-digit", hour12: false })}</time><div className="history-body"><p>{item.mode === "ask" ? <AnswerText text={item.text} onError={setError} /> : item.text}</p><div className="history-meta"><span>{modeLabels[item.mode as DictationMode] || "聽寫"}</span>{item.app_name && <span>{item.app_name}</span>}<span>{formatDuration(item.duration)}</span></div>{item.raw_text && item.raw_text !== item.text && <details><summary>辨識原文</summary><p>{item.raw_text}</p></details>}</div><div className="row-actions"><button className="icon-button history-copy-btn" aria-label="複製記錄" onClick={() => void props.onCopy(item.text)}><Copy size={15} /></button><button className="icon-button" aria-label="編輯記錄" disabled={working} onClick={() => { setEditing(item); setText(item.text); }}><Pencil size={15} /></button><button className="icon-button danger history-delete-btn" aria-label="刪除記錄" disabled={working} onClick={() => void run(() => props.onDelete(item.id))}><Trash2 size={15} /></button></div></article>)}</div></section>)}</div>}
    {editing && <Dialog title="編輯記錄" onClose={() => setEditing(null)} busy={working}><form className="editor-form" onSubmit={e => { e.preventDefault(); void run(async () => { await props.onUpdate(editing.id, text.trim()); setEditing(null); }); }}><h2>編輯記錄</h2><label className="field">文字內容<textarea autoFocus rows={8} value={text} onChange={e => setText(e.target.value)} /></label><p className="hint">只更新歷史文字，不會再次插入。</p>{error && <p className="inline-error" role="alert">{error}</p>}<footer><button type="button" className="button" disabled={working} onClick={() => setEditing(null)}>取消</button><button className="button primary" disabled={working || !text.trim()}>儲存</button></footer></form></Dialog>}
    {clearing && <Dialog title="清空歷史記錄" onClose={() => setClearing(false)} busy={working}><div className="editor-form"><h2>清空歷史記錄？</h2><p>此 Mac 上的 {props.history.length} 筆文字記錄將被刪除。</p>{error && <p className="inline-error" role="alert">{error}</p>}<footer><button className="button" disabled={working} onClick={() => setClearing(false)}>保留</button><button className="button destructive" disabled={working} onClick={() => void run(async () => { await props.onClear(); setClearing(false); })}>清空記錄</button></footer></div></Dialog>}
  </>;
}
