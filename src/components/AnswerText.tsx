import type { ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";

// Keep model output as React text. Only explicit HTTP(S) URLs become links;
// no HTML or Markdown is interpreted, and nothing opens before a user click.
export function AnswerText({ text, onError }: { text: string; onError: (message: string) => void }) {
  const parts: ReactNode[] = [];
  let cursor = 0;
  for (const match of text.matchAll(/https?:\/\/[^\s<>"'`\u0000-\u001f]+/g)) {
    const start = match.index!;
    const candidate = match[0].replace(/[\])},.;:!?，。；：！？、）》」』]+$/, "");
    let url: URL;
    try { url = new URL(candidate); } catch { continue; }
    if (!["https:", "http:"].includes(url.protocol) || url.username || url.password) continue;
    parts.push(text.slice(cursor, start));
    parts.push(<a key={start} href={url.href} rel="noopener noreferrer" onClick={event => {
      event.preventDefault();
      void openUrl(url.href).catch(() => onError("未能開啟來源連結，可複製網址後在瀏覽器開啟。"));
    }}>{candidate}</a>);
    cursor = start + candidate.length;
  }
  parts.push(text.slice(cursor));
  return <>{parts}</>;
}
