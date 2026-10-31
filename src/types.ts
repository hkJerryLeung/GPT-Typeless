export type DictationMode = "dictate" | "translate" | "edit" | "ask";
export type VoiceStatus = "idle" | "starting" | "recording" | "processing" | "done" | "error";
export interface AudioDevice { name: string; is_default: boolean }
export interface RecordingStatus { is_recording: boolean; recording_id: string | null }
export interface ProviderConfig {
  id: string; name: string; kind: "gemini" | "openai" | "anthropic" | "openai-compatible";
  base_url: string; model: string; api_key: string; has_key: boolean;
}
export interface DictionaryEntry { id: string; term: string; aliases: string[] }
export interface Snippet { id: string; trigger: string; content: string }
export interface AppRule { id: string; app_name: string; tone: string; instructions: string }
export interface AppConfig {
  chatgpt_token: string; oai_client_build_number: string; oai_client_version: string;
  oai_device_id: string; hotkey: string; recording_format: string; audio_device: string | null;
  auto_insert: boolean; fn_shortcut_enabled: boolean; shortcut_behavior: "hold" | "toggle"; text_mode: "raw" | "polish";
  translation_language: string; language_variant: string; tone: string; custom_instructions: string;
  active_provider_id: string | null; providers: ProviderConfig[]; web_search_enabled: boolean; dictionary: DictionaryEntry[];
  snippets: Snippet[]; app_rules: AppRule[]; history_enabled: boolean; sound_enabled: boolean;
  voice_bar_enabled: boolean; auto_update_enabled: boolean; appearance: "system" | "light" | "dark";
}
export interface TranscriptionHistory {
  id: string; text: string; duration: number; timestamp: number;
  raw_text: string; mode: string; app_name: string;
}
export interface DictationResult {
  text: string; raw_text: string; inserted: boolean; paste_posted?: boolean;
  warning?: string; insertion_detail?: string;
  app_name: string; mode: string;
}
export interface InsertionStatus { trusted: boolean; post_event: boolean; target_app?: string }
export interface FnStatus { enabled: boolean; available: boolean; message: string }

export const defaultConfig: AppConfig = {
  chatgpt_token: "", oai_client_build_number: "", oai_client_version: "", oai_device_id: "",
  hotkey: "Cmd+Shift+R", recording_format: "wav", audio_device: null,
  auto_insert: true, fn_shortcut_enabled: false, shortcut_behavior: "hold", text_mode: "raw", translation_language: "English",
  language_variant: "zh-Hant-HK", tone: "natural", custom_instructions: "", active_provider_id: null,
  providers: [], web_search_enabled: false, dictionary: [], snippets: [], app_rules: [], history_enabled: true,
  sound_enabled: true, voice_bar_enabled: true, auto_update_enabled: true, appearance: "system",
};

// Spread the native object so fields added by the backend survive every frontend save.
export function normalizeConfig(config: Partial<AppConfig>): AppConfig {
  return { ...defaultConfig, ...config, providers: (config.providers ?? []).map(p => ({ ...p, api_key: p.api_key ?? "" })) };
}
export function prepareConfig(config: AppConfig): AppConfig {
  return { ...config,
    // Search is opt-in and never follows the user to an unsupported provider.
    web_search_enabled: config.web_search_enabled && config.providers.some(p => p.id === config.active_provider_id && p.kind === "gemini"),
    chatgpt_token: config.chatgpt_token.trim().replace(/^Bearer(?:\s+|$)/i, "").trim(),
    hotkey: config.hotkey.trim(),
    providers: config.providers.map(p => ({ ...p, name: p.name.trim(), base_url: p.base_url.trim(), model: p.model.trim(), api_key: p.api_key.trim() })),
  };
}
export function redactProviderKeys(config: AppConfig): AppConfig {
  return { ...config, providers: config.providers.map(p => ({ ...p, has_key: p.has_key || !!p.api_key.trim(), api_key: "" })) };
}
export function activeProvider(config: AppConfig): ProviderConfig | undefined {
  return config.providers.find(p => p.id === config.active_provider_id && (p.has_key || !!p.api_key.trim()) && p.model.trim() && p.base_url.trim());
}
export const modeLabels: Record<DictationMode, string> = { dictate: "聽寫", translate: "翻譯", edit: "編輯選取文字", ask: "問 AI" };
export const toneOptions = [["natural", "自然"], ["casual", "輕鬆"], ["formal", "正式"], ["concise", "精簡"]] as const;
export const languageOptions = [["English", "英文"], ["繁體中文（香港）", "繁體中文（香港）"], ["繁體中文（台灣）", "繁體中文（台灣）"], ["日本語", "日文"], ["한국어", "韓文"], ["Français", "法文"], ["Deutsch", "德文"], ["Español", "西班牙文"]];
export function shortcutKeys(shortcut: string): string[] {
  const symbols: Record<string, string> = { cmd: "⌘", command: "⌘", super: "⌘", meta: "⌘", shift: "⇧", alt: "⌥", option: "⌥", ctrl: "⌃", control: "⌃", space: "Space", fn: "Fn" };
  return shortcut.split("+").map(key => symbols[key.trim().toLowerCase()] ?? key.trim().toUpperCase());
}
export function formatDuration(seconds: number): string {
  const safe = Math.max(0, Math.floor(seconds));
  return `${Math.floor(safe / 60)}:${String(safe % 60).padStart(2, "0")}`;
}
