export interface DictionaryItem {
  id: string;
  term: string;
  aliases: string[];
}

export interface ParsedDictionaryCsv {
  terms: string[];
  emptyRows: number;
  headerSkipped: boolean;
}

// A CSV record contains exactly one word or phrase. Quoted fields may contain
// commas, escaped quotes and line breaks; extra columns are never discarded.
export function parseDictionaryCsv(input: string, skipHeader = false): ParsedDictionaryCsv {
  const text = input.startsWith("\uFEFF") ? input.slice(1) : input;
  const terms: string[] = [];
  let emptyRows = 0;
  let headerSkipped = false;
  let fields: string[] = [];
  let field = "";
  let state: "start" | "plain" | "quoted" | "closed" = "start";
  let line = 1;
  let recordLine = 1;
  let hasRecord = false;

  const finishRecord = () => {
    fields.push(field);
    if (fields.length !== 1) throw new Error(`第 ${recordLine} 行有多於一欄；請每行只放一個詞彙，含逗號的詞彙請用雙引號包住。`);
    const term = fields[0].trim();
    if (!term) emptyRows++;
    else if (skipHeader && !headerSkipped) headerSkipped = true;
    else terms.push(term);
    fields = [];
    field = "";
    state = "start";
    hasRecord = false;
  };

  for (let i = 0; i < text.length; i++) {
    const char = text[i];
    if (char === "\0" || (char < " " && char !== "\t" && char !== "\r" && char !== "\n")) {
      throw new Error(`第 ${line} 行包含不支援的控制字元。`);
    }
    if (state === "quoted") {
      if (char === '"') {
        if (text[i + 1] === '"') { field += '"'; i++; }
        else state = "closed";
      } else if (char === "\r" || char === "\n") {
        field += char;
        if (char === "\r" && text[i + 1] === "\n") { field += "\n"; i++; }
        line++;
      } else field += char;
      continue;
    }
    if (char === ",") {
      fields.push(field);
      field = "";
      state = "start";
      hasRecord = true;
    } else if (char === "\r" || char === "\n") {
      finishRecord();
      if (char === "\r" && text[i + 1] === "\n") i++;
      line++;
      recordLine = line;
    } else if (state === "start" && char === '"') {
      state = "quoted";
      hasRecord = true;
    } else if (char === '"' || state === "closed") {
      throw new Error(`第 ${line} 行的雙引號格式不正確。`);
    } else {
      field += char;
      state = "plain";
      hasRecord = true;
    }
  }

  if (state === "quoted") throw new Error(`第 ${recordLine} 行的雙引號未關閉。`);
  if (hasRecord) finishRecord();
  if (!terms.length) throw new Error(skipHeader && headerSkipped ? "標題之後沒有可匯入的詞彙。" : "CSV 沒有可匯入的詞彙。");
  return { terms, emptyRows, headerSkipped };
}

export function mergeDictionaryTerms(existing: readonly DictionaryItem[], terms: readonly string[], createId: () => string) {
  const keyFor = (term: string) => term.trim().normalize("NFC").toLocaleLowerCase();
  const existingTerms = new Set(existing.map(entry => keyFor(entry.term)));
  const importedTerms = new Set<string>();
  const ids = new Set(existing.map(entry => entry.id));
  const entries = [...existing];
  let skippedExisting = 0;
  let skippedRepeated = 0;
  let added = 0;

  for (const term of terms) {
    const key = keyFor(term);
    if (existingTerms.has(key)) { skippedExisting++; continue; }
    if (importedTerms.has(key)) { skippedRepeated++; continue; }
    let id = "";
    for (let attempt = 0; attempt < 10; attempt++) {
      const candidate = createId();
      if (candidate && !ids.has(candidate)) { id = candidate; break; }
    }
    if (!id) throw new Error("未能建立不重複的詞彙識別碼，請重試。");
    ids.add(id);
    importedTerms.add(key);
    entries.push({ id, term, aliases: [] });
    added++;
  }
  return { entries, added, skippedExisting, skippedRepeated };
}
