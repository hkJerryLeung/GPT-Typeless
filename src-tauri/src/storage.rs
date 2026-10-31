use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub chatgpt_token: String,
    pub oai_client_build_number: String,
    pub oai_client_version: String,
    pub oai_device_id: String,
    pub hotkey: String,
    pub recording_format: String,
    pub audio_device: Option<String>,
    pub auto_insert: bool,
    pub shortcut_behavior: String,
    pub fn_shortcut_enabled: bool,
    pub text_mode: String,
    pub translation_language: String,
    pub language_variant: String,
    pub tone: String,
    pub custom_instructions: String,
    pub active_provider_id: Option<String>,
    pub providers: Vec<crate::text_ai::ProviderConfig>,
    pub dictionary: Vec<DictionaryEntry>,
    pub snippets: Vec<Snippet>,
    pub app_rules: Vec<AppRule>,
    pub history_enabled: bool,
    pub auto_update_enabled: bool,
    pub web_search_enabled: bool,
    pub sound_enabled: bool,
    pub voice_bar_enabled: bool,
    pub appearance: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            chatgpt_token: String::new(),
            oai_client_build_number: String::new(),
            oai_client_version: String::new(),
            oai_device_id: String::new(),
            hotkey: if cfg!(target_os = "macos") {
                "Cmd+Shift+R"
            } else {
                "Ctrl+Shift+R"
            }
            .into(),
            recording_format: "wav".into(),
            audio_device: None,
            auto_insert: true,
            shortcut_behavior: "hold".into(),
            fn_shortcut_enabled: false,
            text_mode: "raw".into(),
            translation_language: "English".into(),
            language_variant: "zh-Hant-HK".into(),
            tone: "natural".into(),
            custom_instructions: String::new(),
            active_provider_id: None,
            providers: Vec::new(),
            dictionary: Vec::new(),
            snippets: Vec::new(),
            app_rules: Vec::new(),
            history_enabled: true,
            auto_update_enabled: true,
            web_search_enabled: false,
            sound_enabled: true,
            voice_bar_enabled: true,
            appearance: "system".into(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DictionaryEntry {
    pub id: String,
    pub term: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub id: String,
    pub trigger: String,
    pub content: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AppRule {
    pub id: String,
    pub app_name: String,
    pub tone: String,
    pub instructions: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TranscriptionHistory {
    pub id: String,
    pub text: String,
    pub duration: u64,
    pub timestamp: i64,
    #[serde(default)]
    pub raw_text: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub app_name: String,
}

#[derive(Clone)]
pub struct Storage {
    directory: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl Storage {
    pub fn new(directory: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(directory.join("recordings")).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [&directory, &directory.join("recordings")] {
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(Self {
            directory,
            lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn recordings_dir(&self) -> PathBuf {
        self.directory.join("recordings")
    }

    pub fn load_settings(&self) -> Result<AppConfig, String> {
        let path = self.directory.join("settings.json");
        if !path.exists() {
            return Ok(AppConfig::default());
        }
        serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("無法讀取設定: {e}"))
    }

    pub fn load_config(&self) -> Result<AppConfig, String> {
        let mut config = self.load_settings()?;
        #[cfg(target_os = "macos")]
        {
            config.chatgpt_token = load_token()?;
        }
        for provider in &mut config.providers {
            provider.api_key.clear();
            provider.has_key = !self.provider_key(&provider.id)?.is_empty();
        }
        Ok(config)
    }

    pub fn save_config(&self, config: &AppConfig) -> Result<(), String> {
        let _guard = self.lock.lock().map_err(|e| e.to_string())?;
        let mut ids = std::collections::HashSet::new();
        for provider in &config.providers {
            validate_provider_id(&provider.id)?;
            if !ids.insert(provider.id.clone()) {
                return Err("供應商識別重複".into());
            }
            if !provider.model.trim().is_empty() {
                crate::text_ai::validate_provider(provider)?;
            }
        }
        #[cfg(target_os = "macos")]
        save_token(&config.chatgpt_token)?;
        for provider in &config.providers {
            if !provider.api_key.trim().is_empty() {
                save_provider_key(&provider.id, provider.api_key.trim())?;
            }
        }
        atomic_write(
            &self.directory.join("settings.json"),
            &settings_bytes(config)?,
        )
    }

    // The live voice bar changes only this setting. Do not load or rewrite
    // Keychain credentials, and preserve any fields from future versions.
    pub fn save_translation_language(&self, language: &str) -> Result<(), String> {
        let language = language.trim();
        if language.is_empty() || language.len() > 256 || language.chars().any(char::is_control) {
            return Err("翻譯語言無效".into());
        }
        let _guard = self.lock.lock().map_err(|e| e.to_string())?;
        let path = self.directory.join("settings.json");
        let mut settings: serde_json::Value = if path.exists() {
            serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
                .map_err(|_| "無法讀取設定".to_string())?
        } else {
            serde_json::json!({})
        };
        let object = settings.as_object_mut().ok_or("設定格式無效")?;
        object.insert("translation_language".into(), language.into());
        let bytes = serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?;
        atomic_write(&path, &bytes)
    }

    pub fn history(&self) -> Result<Vec<TranscriptionHistory>, String> {
        let path = self.directory.join("history.json");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let mut history: Vec<TranscriptionHistory> =
            serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
                .map_err(|e| format!("無法讀取歷史記錄: {e}"))?;
        history.sort_by_key(|item| std::cmp::Reverse(item.timestamp));
        history.truncate(1000);
        Ok(history)
    }

    pub fn add_history(&self, text: String, duration: u64) -> Result<(), String> {
        self.add_dictation_history(text, String::new(), "raw".into(), String::new(), duration)
    }

    pub fn add_dictation_history(
        &self,
        text: String,
        raw_text: String,
        mode: String,
        app_name: String,
        duration: u64,
    ) -> Result<(), String> {
        let _guard = self.lock.lock().map_err(|e| e.to_string())?;
        let mut history = self.history()?;
        let timestamp = chrono::Utc::now().timestamp_millis();
        history.insert(
            0,
            TranscriptionHistory {
                id: timestamp.to_string(),
                text,
                duration,
                timestamp,
                raw_text,
                mode,
                app_name,
            },
        );
        history.truncate(1000);
        self.write_history(&history)
    }

    pub fn provider_key(&self, id: &str) -> Result<String, String> {
        validate_provider_id(id)?;
        #[cfg(target_os = "macos")]
        {
            keychain_read(&format!("provider:{id}"))
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("呢個版本只支援 macOS 鑰匙圈".into())
        }
    }

    pub fn clear_provider_key(&self, id: &str) -> Result<(), String> {
        validate_provider_id(id)?;
        #[cfg(target_os = "macos")]
        {
            keychain_write(&format!("provider:{id}"), "")
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("呢個版本只支援 macOS 鑰匙圈".into())
        }
    }

    pub fn update_history(&self, id: &str, text: String) -> Result<(), String> {
        let _guard = self.lock.lock().map_err(|e| e.to_string())?;
        let mut history = self.history()?;
        let item = history
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or("搵唔到記錄")?;
        item.text = text;
        self.write_history(&history)
    }

    pub fn delete_history(&self, id: &str) -> Result<(), String> {
        let _guard = self.lock.lock().map_err(|e| e.to_string())?;
        let mut history = self.history()?;
        history.retain(|item| item.id != id);
        self.write_history(&history)
    }

    pub fn clear_history(&self) -> Result<(), String> {
        let _guard = self.lock.lock().map_err(|e| e.to_string())?;
        self.write_history(&[])
    }

    fn write_history(&self, history: &[TranscriptionHistory]) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(history).map_err(|e| e.to_string())?;
        atomic_write(&self.directory.join("history.json"), &bytes)
    }
}

fn settings_bytes(config: &AppConfig) -> Result<Vec<u8>, String> {
    let mut settings = serde_json::to_value(config).map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    settings.as_object_mut().unwrap().remove("chatgpt_token");
    if let Some(providers) = settings.get_mut("providers").and_then(|p| p.as_array_mut()) {
        for provider in providers {
            if let Some(object) = provider.as_object_mut() {
                object.remove("api_key");
                object.remove("has_key");
            }
        }
    }
    serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp = path.with_extension("json.tmp");
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    fs::rename(temp, path).map_err(|e| e.to_string())
}

pub fn normalize_token(token: &str) -> Result<String, String> {
    let token = token.trim();
    let token = token.strip_prefix("Bearer ").unwrap_or(token).trim();
    if token.chars().any(char::is_whitespace) {
        return Err("Token 內含空白或換行，請只貼上 Authorization 的 Token 部分".into());
    }
    if token.starts_with("sk-") {
        return Err("呢度需要 ChatGPT 登入 Token；一般 OpenAI API key 不適用".into());
    }
    Ok(token.into())
}

#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "com.gpttypeless";
#[cfg(target_os = "macos")]
const KEYCHAIN_ACCOUNT: &str = "chatgpt-token";

#[cfg(target_os = "macos")]
fn load_token() -> Result<String, String> {
    match security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| "鑰匙圈內嘅 Token 格式無效".into()),
        Err(e) if e.code() == -25300 => Ok(String::new()),
        Err(e) => Err(format!("無法讀取 macOS 鑰匙圈: {e}")),
    }
}

#[cfg(target_os = "macos")]
fn save_token(token: &str) -> Result<(), String> {
    use security_framework::passwords::{delete_generic_password, set_generic_password};
    if token.is_empty() {
        match delete_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == -25300 => Ok(()),
            Err(e) => Err(format!("無法移除鑰匙圈 Token: {e}")),
        }
    } else {
        set_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, token.as_bytes())
            .map_err(|e| format!("無法儲存 Token 到 macOS 鑰匙圈: {e}"))
    }
}

fn validate_provider_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 80
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("供應商識別無效".into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn keychain_read(account: &str) -> Result<String, String> {
    match security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, account) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| "鑰匙圈資料無效".into()),
        Err(error) if error.code() == -25300 => Ok(String::new()),
        Err(error) => Err(format!("無法讀取鑰匙圈: {error}")),
    }
}

#[cfg(target_os = "macos")]
fn keychain_write(account: &str, key: &str) -> Result<(), String> {
    use security_framework::passwords::{delete_generic_password, set_generic_password};
    let result = if key.is_empty() {
        delete_generic_password(KEYCHAIN_SERVICE, account)
    } else {
        set_generic_password(KEYCHAIN_SERVICE, account, key.as_bytes())
    };
    match result {
        Ok(()) => Ok(()),
        Err(error) if key.is_empty() && error.code() == -25300 => Ok(()),
        Err(error) => Err(format!("無法儲存鑰匙圈: {error}")),
    }
}

fn save_provider_key(id: &str, key: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        keychain_write(&format!("provider:{id}"), key)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (id, key);
        Err("呢個版本只支援 macOS 鑰匙圈".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_language_save_preserves_unrelated_settings_and_rejects_invalid_values() {
        let dir =
            std::env::temp_dir().join(format!("gpttypeless-language-test-{}", std::process::id()));
        let storage = Storage::new(dir.clone()).unwrap();
        let path = dir.join("settings.json");
        let original = serde_json::json!({"hotkey":"Cmd+Shift+R", "future": {"keep": true}, "providers": [{"id":"unchanged"}], "translation_language":"English"});
        fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
        storage.save_translation_language("日本語").unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["translation_language"], "日本語");
        assert_eq!(saved["future"], original["future"]);
        assert_eq!(saved["providers"], original["providers"]);
        assert_eq!(saved["hotkey"], original["hotkey"]);
        assert!(storage.save_translation_language(" \n ").is_err());
        assert!(storage.save_translation_language(&"x".repeat(257)).is_err());
        assert_eq!(
            fs::read(&path).unwrap(),
            serde_json::to_vec_pretty(&saved).unwrap()
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_settings_keep_working_and_new_secrets_never_enter_settings() {
        let mut config: AppConfig =
            serde_json::from_str(r#"{"hotkey":"Cmd+Shift+R","audio_device":null}"#).unwrap();
        assert!(config.auto_insert);
        assert!(config.auto_update_enabled);
        assert!(config.providers.is_empty());
        config.providers.push(crate::text_ai::ProviderConfig {
            id: "gemini-test".into(),
            name: "Gemini".into(),
            kind: "gemini".into(),
            base_url: "https://generativelanguage.googleapis.com/v1beta".into(),
            model: "fixture-model".into(),
            api_key: "fixture-secret-never-persist".into(),
            has_key: true,
        });
        let encoded = String::from_utf8(settings_bytes(&config).unwrap()).unwrap();
        assert!(!encoded.contains("fixture-secret"));
        assert!(!encoded.contains("api_key"));
        assert!(!encoded.contains("has_key"));
    }

    #[test]
    fn automatic_updates_default_on_and_persist_the_explicit_choice() {
        let old: AppConfig = serde_json::from_str("{}").unwrap();
        assert!(old.auto_update_enabled);

        let disabled: AppConfig = serde_json::from_str(r#"{"auto_update_enabled":false}"#).unwrap();
        assert!(!disabled.auto_update_enabled);
        let saved = settings_bytes(&disabled).unwrap();
        let loaded: AppConfig = serde_json::from_slice(&saved).unwrap();
        assert!(!loaded.auto_update_enabled);
    }

    #[test]
    fn token_normalization_accepts_header_and_rejects_other_key_types() {
        assert_eq!(normalize_token("  Bearer eyJtest  ").unwrap(), "eyJtest");
        assert!(normalize_token("sk-project-test").is_err());
        assert!(normalize_token("token\nother").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_settings_do_not_contain_token() {
        let config = AppConfig {
            chatgpt_token: "test-secret-never-persist".into(),
            ..Default::default()
        };
        let text = String::from_utf8(settings_bytes(&config).unwrap()).unwrap();
        assert!(!text.contains("test-secret"));
        assert!(!text.contains("chatgpt_token"));
    }

    #[test]
    fn history_survives_reload_without_old_five_item_limit() {
        let dir =
            std::env::temp_dir().join(format!("gpttypeless-storage-test-{}", std::process::id()));
        let storage = Storage::new(dir.clone()).unwrap();
        for n in 0..7 {
            storage.add_history(format!("test {n}"), n).unwrap();
        }
        let reloaded = Storage::new(dir.clone()).unwrap();
        assert_eq!(reloaded.history().unwrap().len(), 7);
        assert_eq!(reloaded.history().unwrap()[0].text, "test 6");
        storage.clear_history().unwrap();
        assert!(reloaded.history().unwrap().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
}
