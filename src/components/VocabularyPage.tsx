import { useRef, useState } from "react";
import { BookOpen, FileUp, Pencil, Plus, Search, TextQuote, Trash2 } from "lucide-react";
import type { DictionaryEntry, Snippet } from "../types";
import { Dialog, EmptyState } from "./NativeControls";
import { mergeDictionaryTerms, parseDictionaryCsv } from "./dictionaryCsv";

interface Props {
  kind: "dictionary" | "snippets"; dictionary: DictionaryEntry[]; snippets: Snippet[]; busy: boolean;
  onDictionary: (entries: DictionaryEntry[]) => Promise<void>; onSnippets: (snippets: Snippet[]) => Promise<void>;
}
export function VocabularyPage(props: Props) {
  const dictionary = props.kind === "dictionary";
  const latestProps = useRef(props);
  latestProps.current = props;
  const [search, setSearch] = useState("");
  const [editor, setEditor] = useState<{ id: string; term: string; details: string } | null>(null);
  const [importOpen, setImportOpen] = useState(false);
  const [importHasHeader, setImportHasHeader] = useState(false);
  const [importError, setImportError] = useState("");
  const [importResult, setImportResult] = useState("");
  const [selected, setSelected] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const [error, setError] = useState("");
  const entries = dictionary ? props.dictionary.map(e => ({ id: e.id, term: e.term, details: e.aliases.join("、") })) : props.snippets.map(e => ({ id: e.id, term: e.trigger, details: e.content }));
  const visible = entries.filter(e => `${e.term} ${e.details}`.toLocaleLowerCase().includes(search.toLocaleLowerCase().trim()));
  const locked = props.busy || saving;
  const run = async (action: () => Promise<void>) => {
    if (props.busy || savingRef.current) return;
    savingRef.current = true; setSaving(true); setError("");
    try { await action(); } catch (error) { setError(String(error)); } finally { savingRef.current = false; setSaving(false); }
  };
  const remove = (ids: string[]) => run(async () => {
    if (dictionary) await props.onDictionary(props.dictionary.filter(e => !ids.includes(e.id)));
    else await props.onSnippets(props.snippets.filter(e => !ids.includes(e.id)));
    setSelected(previous => previous.filter(id => !ids.includes(id)));
  });
  const save = () => run(async () => {
    if (!editor?.term.trim()) return;
    if (entries.some(e => e.id !== editor.id && e.term.trim().toLocaleLowerCase() === editor.term.trim().toLocaleLowerCase())) throw new Error(dictionary ? "此詞彙已存在。" : "此觸發短語已存在。");
    if (dictionary) {
      const next = { id: editor.id, term: editor.term.trim(), aliases: [...new Set(editor.details.split(/[、,，\n]/).map(s => s.trim()).filter(Boolean))] };
      await props.onDictionary(props.dictionary.some(e => e.id === next.id) ? props.dictionary.map(e => e.id === next.id ? next : e) : [...props.dictionary, next]);
    } else {
      if (!editor.details.trim()) throw new Error("請填寫展開文字。");
      const next = { id: editor.id, trigger: editor.term.trim(), content: editor.details.trim() };
      await props.onSnippets(props.snippets.some(e => e.id === next.id) ? props.snippets.map(e => e.id === next.id ? next : e) : [...props.snippets, next]);
    }
    setEditor(null);
  });
  const importFile = async (file: File, skipHeader: boolean) => {
    if (latestProps.current.busy || savingRef.current) { setImportError("詞庫正在更新，請稍後再試。"); return; }
    savingRef.current = true; setSaving(true); setImportError(""); setImportResult("");
    let writeAttempted = false;
    try {
      if (file.size > 5 * 1024 * 1024) throw new Error("CSV 不能大於 5 MB。");
      const bytes = await file.arrayBuffer();
      let text: string;
      try { text = new TextDecoder("utf-8", { fatal: true }).decode(bytes); }
      catch { throw new Error("檔案不是有效的 UTF-8 編碼。"); }
      const parsed = parseDictionaryCsv(text, skipHeader);
      const current = latestProps.current;
      if (current.busy || current.kind !== "dictionary") throw new Error("詞庫正在更新或頁面已切換，請稍後再試。");
      const merged = mergeDictionaryTerms(current.dictionary, parsed.terms, () => crypto.randomUUID());
      if (merged.added) {
        writeAttempted = true;
        await current.onDictionary(merged.entries);
      }
      const details = [`已新增 ${merged.added} 個詞彙`, `略過 ${merged.skippedExisting} 個已存在詞彙`, `略過 ${merged.skippedRepeated} 個檔案內重複詞彙`, `略過 ${parsed.emptyRows} 個空白列`];
      if (parsed.headerSkipped) details.push("已依選項略過標題列");
      setImportResult(`${details.join("；")}。`);
      setImportOpen(false);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setImportError(`${writeAttempted ? "儲存時發生問題，請先檢查詞庫再重試。" : "未匯入任何詞彙。"}${message}`);
    } finally {
      savingRef.current = false; setSaving(false);
    }
  };
  return <>
    <header className="page-heading"><h1>{dictionary ? "詞庫" : "常用短語"}</h1><div style={{ display: "flex", gap: 8, flexWrap: "wrap", justifyContent: "flex-end" }}>{dictionary && <button type="button" className="button" disabled={locked} onClick={() => { setError(""); setImportError(""); setImportResult(""); setImportHasHeader(false); setImportOpen(true); }}><FileUp size={16} />匯入 CSV</button>}<button className="button primary" disabled={locked} onClick={() => { setError(""); setEditor({ id: crypto.randomUUID(), term: "", details: "" }); }}><Plus size={16} />{dictionary ? "新增詞彙" : "新增短語"}</button></div></header>
    <p className="page-intro">{dictionary ? "為人名、品牌與專有名詞設定正確寫法。別名會在辨識後替換為此詞彙。" : "說出完整觸發短語，便會展開為你儲存的文字。只比對完整一句。"}</p>
    <div className="collection-toolbar"><div className="collection-count">全部 <span>{entries.length}</span></div><label className="search-field"><Search size={16} /><input aria-label={dictionary ? "搜尋詞庫" : "搜尋短語"} placeholder={dictionary ? "搜尋詞彙或別名" : "搜尋短語"} value={search} onChange={e => setSearch(e.target.value)} /></label></div>
    {visible.length > 0 && <div className="selection-toolbar"><label><input type="checkbox" aria-label="選取目前顯示的全部項目" checked={visible.every(e => selected.includes(e.id))} onChange={e => setSelected(e.target.checked ? [...new Set([...selected, ...visible.map(item => item.id)])] : selected.filter(id => !visible.some(item => item.id === id)))} />選取全部</label>{selected.length > 0 && <button className="text-button danger" disabled={locked} onClick={() => void remove(selected)}>刪除已選 {selected.length} 項</button>}</div>}
    {error && <p className="inline-error" role="alert">{error}</p>}
    {dictionary && importResult && <p className="hint" role="status">{importResult}</p>}
    {!visible.length ? <EmptyState icon={dictionary ? <BookOpen /> : <TextQuote />} title={search ? "沒有符合的項目" : dictionary ? "用你習慣的名字" : "常說的話，一句完成"} description={search ? "試試其他關鍵字。" : dictionary ? "加入專有名詞及常見辨識別名。" : "例如將「我的聯絡資料」展開為完整聯絡方式。"} /> : <div className={dictionary ? "dictionary-grid" : "snippet-list"}>{visible.map(entry => <article key={entry.id} className="vocabulary-item"><input type="checkbox" aria-label={`選取 ${entry.term}`} checked={selected.includes(entry.id)} onChange={e => setSelected(e.target.checked ? [...selected, entry.id] : selected.filter(id => id !== entry.id))} /><button className="vocabulary-content" disabled={locked} onClick={() => { setError(""); setEditor(entry); }}><span>{entry.term}</span>{entry.details && <small>{entry.details}</small>}</button><div className="row-actions"><button className="icon-button" disabled={locked} aria-label={`編輯 ${entry.term}`} onClick={() => { setError(""); setEditor(entry); }}><Pencil size={14} /></button><button className="icon-button danger" disabled={locked} aria-label={`刪除 ${entry.term}`} onClick={() => void remove([entry.id])}><Trash2 size={14} /></button></div></article>)}</div>}
    {editor && <Dialog title={dictionary ? "編輯詞彙" : "編輯短語"} onClose={() => setEditor(null)} busy={saving}><form className="editor-form" onSubmit={e => { e.preventDefault(); void save(); }}><h2>{dictionary ? "詞彙" : "常用短語"}</h2><label className="field">{dictionary ? "正確寫法" : "完整觸發短語"}<input autoFocus value={editor.term} onChange={e => setEditor({ ...editor, term: e.target.value })} required /></label><label className="field">{dictionary ? "辨識別名" : "展開文字"}<textarea rows={dictionary ? 3 : 6} value={editor.details} onChange={e => setEditor({ ...editor, details: e.target.value })} placeholder={dictionary ? "以逗號或換行分隔" : "輸入完整文字"} required={!dictionary} /></label>{dictionary && <p className="hint">保留詞彙本身，將上方別名替換成正確寫法。</p>}{error && <p className="inline-error" role="alert">{error}</p>}<footer><button type="button" className="button" disabled={saving} onClick={() => setEditor(null)}>取消</button><button className="button primary" disabled={locked || !editor.term.trim() || (!dictionary && !editor.details.trim())}>{saving ? "儲存中…" : "儲存"}</button></footer></form></Dialog>}
    {dictionary && importOpen && <Dialog title="匯入詞彙" onClose={() => setImportOpen(false)} busy={saving}><div className="editor-form"><h2>匯入詞彙</h2><p className="hint">選擇 UTF-8 CSV（最多 5 MB），每行一個詞彙；含逗號的詞彙請用雙引號包住。現有詞彙及別名不會被覆蓋。</p><label><input type="checkbox" checked={importHasHeader} disabled={locked} onChange={e => setImportHasHeader(e.target.checked)} /> 首個非空列是標題，不匯入</label><label className="field">選擇 CSV 檔案<input type="file" accept=".csv,text/csv" disabled={locked} onChange={e => { const file = e.currentTarget.files?.[0]; e.currentTarget.value = ""; if (file) void importFile(file, importHasHeader); }} /></label>{saving && <p className="hint" role="status">匯入中…</p>}{importError && <p className="inline-error" role="alert">{importError}</p>}<footer><button type="button" className="button" disabled={saving} onClick={() => setImportOpen(false)}>取消</button></footer></div></Dialog>}
  </>;
}
