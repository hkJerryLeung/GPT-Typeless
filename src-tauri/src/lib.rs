mod api;
mod audio;
mod dictation;
mod fn_shortcut;
mod macos_input;
mod microphone;
mod setup_drag;
mod storage;
mod text_ai;
mod updater;

use api::TranscribeClient;
use audio::{AudioDevice, AudioManager, StopRecordingResult};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use storage::{AppConfig, Storage, TranscriptionHistory};
use tauri::{Emitter, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

// Read-only local permission diagnostics. Never prompts, captures text or loads keys.
pub fn diagnose_permissions() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let accessibility = macos_input::accessibility_trusted().map_err(|e| e.message)?;
        let post_event = objc2_core_graphics::CGPreflightPostEventAccess();
        println!(
            "{}",
            serde_json::json!({"accessibility":accessibility,"post_event":post_event})
        );
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    Err("Permission diagnostics require macOS.".into())
}

// Explicit local acceptance test, with a fixed harmless string and no model call.
pub fn diagnose_insertion() -> Result<(), String> {
    let target = macos_input::capture_target().map_err(|e| e.message)?;
    let app_name = macos_input::target_app_name(target).unwrap_or_default();
    macos_input::insert_text(target, "GPT-Typeless 自動輸入測試").map_err(|e| e.error.message)?;
    println!(
        "{}",
        serde_json::json!({"inserted":true,"target_app":app_name})
    );
    Ok(())
}

// Local-only reproduction path. Never reads credentials or contacts a server.
pub fn diagnose_audio(hold_ms: u64, directory: std::path::PathBuf) -> Result<(), String> {
    microphone::ensure_permission()?;
    let manager = AudioManager::new(directory)?;
    let start = std::time::Instant::now();
    manager.start_recording(None)?;
    let startup_ms = start.elapsed().as_millis();
    let monitor = manager.level_monitor();
    let hold = std::time::Duration::from_millis(hold_ms.min(5000));
    let capture_start = std::time::Instant::now();
    let mut meter_readings = 0;
    let mut max_rms = 0.0_f32;
    while capture_start.elapsed() < hold {
        let level = monitor.snapshot();
        meter_readings += usize::from(level.recording);
        max_rms = max_rms.max(level.rms);
        std::thread::sleep(std::time::Duration::from_millis(33));
    }
    let result = manager.stop_recording()?;
    let stopped_level = monitor.snapshot();
    let reader = hound::WavReader::open(result.audio_path).map_err(|e| e.to_string())?;
    println!(
        "{}",
        serde_json::json!({"startup_ms":startup_ms,"hold_ms":hold_ms,"frames":reader.duration(),"sample_rate":reader.spec().sample_rate,"meter_readings":meter_readings,"max_rms":max_rms,"meter_stopped":!stopped_level.recording && stopped_level.rms == 0.0})
    );
    Ok(())
}

pub struct AppState {
    audio_manager: AudioManager,
    storage: Storage,
    shortcut: Mutex<Option<Shortcut>>,
    config_lock: tokio::sync::Mutex<()>,
    target: Mutex<Option<TargetContext>>,
    fn_shortcut: Mutex<Option<fn_shortcut::FnShortcut>>,
    fn_error: Mutex<Option<String>>,
    update_gate: updater::ActivityGate,
}

#[derive(Serialize)]
pub struct RecordingStatus {
    is_recording: bool,
    recording_id: Option<String>,
}

#[tauri::command]
async fn start_recording(
    device_name: Option<String>,
    mode: Option<String>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let start_guard = state.update_gate.begin_recording()?;
    let config = state.storage.load_settings()?;
    let needs_selection = matches!(mode.as_deref(), Some("ask" | "edit"));
    if config.auto_insert || needs_selection {
        let target = on_main(&app, move || {
            let selection = if needs_selection {
                macos_input::capture_selected_text().unwrap_or_default()
            } else {
                String::new()
            };
            match macos_input::capture_target() {
                Ok(token) => TargetContext {
                    app_name: macos_input::target_app_name(token).unwrap_or_default(),
                    token: Some(token),
                    selection: if needs_selection {
                        macos_input::selected_text(token).unwrap_or(selection)
                    } else {
                        String::new()
                    },
                    warning: None,
                },
                Err(error) => TargetContext {
                    selection,
                    warning: Some(error.message),
                    ..Default::default()
                },
            }
        })
        .await?;
        *state.target.lock().map_err(|e| e.to_string())? = Some(target);
    }
    let manager = state.audio_manager.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        microphone::ensure_permission()?;
        let recording_id = manager.start_recording(device_name)?;
        start_guard.recording_started()?;
        Ok(recording_id)
    })
    .await
    .map_err(|e| e.to_string())?;
    if result.is_ok() && config.sound_enabled {
        let _ = on_main(&app, || play_feedback("Tink")).await;
    }
    if result.is_err() {
        let target = state.target.lock().map_err(|e| e.to_string())?.take();
        if let Some(token) = target.and_then(|target| target.token) {
            let _ = on_main(&app, move || macos_input::cancel_target(token)).await;
        }
    }
    result
}

#[tauri::command]
async fn stop_recording(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<StopRecordingResult, String> {
    let stop_guard = state.update_gate.begin_stop()?;
    let manager = state.audio_manager.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let result = manager.stop_recording();
        if result.is_ok() {
            stop_guard.finished();
        } else if matches!(manager.get_recording_status(), Ok(None)) {
            stop_guard.no_recording();
        }
        result
    })
    .await
    .map_err(|e| e.to_string())?;
    if result.is_ok()
        && state
            .storage
            .load_settings()
            .map(|config| config.sound_enabled)
            .unwrap_or(false)
    {
        let _ = on_main(&app, || play_feedback("Pop")).await;
    }
    result
}

#[tauri::command]
async fn test_microphone(
    device_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let test_guard = state.update_gate.begin_test()?;
    let manager = state.audio_manager.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _test_guard = test_guard;
        microphone::ensure_permission()?;
        manager.start_recording(device_name)?;
        std::thread::sleep(std::time::Duration::from_secs(2));
        let result = manager.stop_recording()?;
        let mut reader = hound::WavReader::open(result.audio_path).map_err(|e| e.to_string())?;
        let seconds = f64::from(reader.duration()) / f64::from(reader.spec().sample_rate);
        let mut peak = 0_i32;
        for sample in reader.samples::<i16>() {
            peak = peak.max(i32::from(sample.map_err(|e| e.to_string())?).abs());
        }
        if peak == 0 {
            return Err("已收到音訊資料但完全無聲，請選擇實際嘅麥克風再測試".into());
        }
        Ok(format!(
            "麥克風正常：收到 {seconds:.1} 秒音訊。測試錄音只存喺本機，冇上載。"
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_recording_status(state: State<'_, AppState>) -> Result<RecordingStatus, String> {
    let manager = state.audio_manager.clone();
    let recording_id = tauri::async_runtime::spawn_blocking(move || manager.get_recording_status())
        .await
        .map_err(|e| e.to_string())??;
    Ok(RecordingStatus {
        is_recording: recording_id.is_some(),
        recording_id,
    })
}

#[tauri::command]
async fn list_audio_devices(state: State<'_, AppState>) -> Result<Vec<AudioDevice>, String> {
    let manager = state.audio_manager.clone();
    tauri::async_runtime::spawn_blocking(move || manager.list_devices())
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn transcribe_audio(
    audio_path: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let processing_guard = state.update_gate.begin_processing()?;
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _processing_guard = processing_guard;
        let (text, duration) = transcribe_file(audio_path, &storage)?;
        if storage.load_settings()?.history_enabled {
            storage.add_history(text.clone(), duration)?;
        }
        Ok(text)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn transcribe_file(audio_path: String, storage: &Storage) -> Result<(String, u64), String> {
    let path = std::fs::canonicalize(&audio_path).map_err(|e| e.to_string())?;
    let recordings = std::fs::canonicalize(storage.recordings_dir()).map_err(|e| e.to_string())?;
    if !path.starts_with(recordings) || path.extension().and_then(|s| s.to_str()) != Some("wav") {
        return Err("只可以轉錄 GPT-Typeless 嘅錄音檔案".into());
    }
    let config = storage.load_config()?;
    if config.chatgpt_token.is_empty() {
        return Err("請先喺設定輸入 ChatGPT Token".into());
    }
    let mut reader = hound::WavReader::open(&path).map_err(|e| format!("錄音檔案無效: {e}"))?;
    let duration = reader.duration() as u64 / reader.spec().sample_rate as u64;
    let first = reader
        .samples::<i16>()
        .next()
        .transpose()
        .map_err(|e| e.to_string())?;
    if first.is_none() {
        return Err("錄音冇收到聲音，請檢查麥克風權限".into());
    }
    let client = TranscribeClient::from_fields(
        config.chatgpt_token,
        config.oai_client_build_number,
        config.oai_client_version,
        config.oai_device_id,
    )?;
    Ok((client.transcribe(&path)?.text, duration))
}

#[tauri::command]
fn copy_to_clipboard(text: String) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard
        .set_text(text)
        .map_err(|e| format!("無法複製到剪貼簿: {e}"))
}

#[tauri::command]
async fn get_config_command(state: State<'_, AppState>) -> Result<AppConfig, String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || storage.load_config())
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn set_config(
    mut config: AppConfig,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let save_guard = state.update_gate.begin_settings_save()?;
    let _guard = state.config_lock.lock().await;
    config.chatgpt_token = storage::normalize_token(&config.chatgpt_token)?;
    let shortcut: Shortcut = config
        .hotkey
        .parse()
        .map_err(|e| format!("快捷鍵格式無效: {e}"))?;
    let previous = *state.shortcut.lock().map_err(|e| e.to_string())?;
    if previous != Some(shortcut) {
        app.global_shortcut()
            .register(shortcut)
            .map_err(|e| format!("快捷鍵無法註冊，可能已被其他 App 使用: {e}"))?;
    }
    let fn_enabled = config.fn_shortcut_enabled;
    let disable_updates = !config.auto_update_enabled;
    let storage = state.storage.clone();
    let update_gate = state.update_gate.clone();
    let worker_guard = save_guard.clone();
    let saved = tauri::async_runtime::spawn_blocking(move || {
        let _worker_guard = worker_guard;
        storage.save_config(&config)?;
        if disable_updates {
            update_gate.cancel_update()?;
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| e.to_string())?;
    if let Err(error) = saved {
        if previous != Some(shortcut) {
            let _ = app.global_shortcut().unregister(shortcut);
        }
        return Err(error);
    }
    if previous != Some(shortcut) {
        if let Some(old) = previous {
            let _ = app.global_shortcut().unregister(old);
        }
        *state.shortcut.lock().map_err(|e| e.to_string())? = Some(shortcut);
    }
    update_fn_shortcut(&app, &state, fn_enabled);
    Ok(())
}

#[tauri::command]
async fn set_translation_language(
    language: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let _guard = state.config_lock.lock().await;
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || storage.save_translation_language(&language))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
fn check_hotkey(state: State<'_, AppState>) -> Result<(), String> {
    if state.shortcut.lock().map_err(|e| e.to_string())?.is_some() {
        Ok(())
    } else {
        Err("未能啟用全域快捷鍵，可能已被其他程式使用。請喺設定更改快捷鍵再儲存；亦可以使用錄音按鈕。".into())
    }
}

#[tauri::command]
async fn cleanup_old_recordings(
    days: Option<u64>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let manager = state.audio_manager.clone();
    let count = tauri::async_runtime::spawn_blocking(move || {
        manager.cleanup_old_recordings(days.unwrap_or(7))
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(format!("已清理 {count} 個舊錄音檔案"))
}

#[tauri::command]
fn get_transcription_history(
    state: State<'_, AppState>,
) -> Result<Vec<TranscriptionHistory>, String> {
    state.storage.history()
}

#[tauri::command]
fn delete_history_item(id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.storage.delete_history(&id)
}

#[tauri::command]
fn clear_all_history(state: State<'_, AppState>) -> Result<(), String> {
    state.storage.clear_history()
}

#[derive(Default)]
struct TargetContext {
    token: Option<macos_input::TargetToken>,
    selection: String,
    app_name: String,
    warning: Option<String>,
}

fn play_feedback(name: &str) {
    if let Some(sound) =
        objc2_app_kit::NSSound::soundNamed(&objc2_foundation::NSString::from_str(name))
    {
        sound.setVolume(0.2);
        sound.play();
    }
}

async fn on_main<R: Send + 'static>(
    app: &tauri::AppHandle,
    work: impl FnOnce() -> R + Send + 'static,
) -> Result<R, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = sender.send(work());
    })
    .map_err(|e| e.to_string())?;
    receiver.await.map_err(|e| e.to_string())
}

// Surface answers and failures when the main window was hidden for dictation.
#[tauri::command]
fn show_main_window(app: tauri::AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("搵唔到主視窗")?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

#[derive(Serialize)]
struct DictationResult {
    text: String,
    raw_text: String,
    inserted: bool,
    paste_posted: bool,
    warning: Option<String>,
    insertion_detail: Option<String>,
    app_name: String,
    mode: String,
}

#[tauri::command]
async fn complete_dictation(
    audio_path: String,
    mode: Option<String>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<DictationResult, String> {
    let processing_guard = state.update_gate.begin_processing()?;
    let mode = mode.unwrap_or_else(|| "dictate".into());
    if !["dictate", "translate", "edit", "ask"].contains(&mode.as_str()) {
        return Err("未知嘅錄音模式".into());
    }
    let context = state
        .target
        .lock()
        .map_err(|e| e.to_string())?
        .take()
        .unwrap_or_default();
    let token = context.token;
    let storage = state.storage.clone();
    let request_mode = mode.clone();
    let worker_guard = processing_guard.clone();
    let processed = tauri::async_runtime::spawn_blocking(move || {
        let _worker_guard = worker_guard;
        let (raw_text, duration) = transcribe_file(audio_path, &storage)?;
        let config = storage.load_settings()?;
        let prepared = dictation::prepare(
            &raw_text,
            &request_mode,
            &context.selection,
            &context.app_name,
            &config,
            &storage,
        );
        let mut warning = prepared.warning;
        if config.history_enabled && !prepared.text.trim().is_empty() {
            if let Err(error) = storage.add_dictation_history(
                prepared.text.clone(),
                raw_text.clone(),
                request_mode.clone(),
                context.app_name.clone(),
                duration,
            ) {
                warning = Some(format!(
                    "{} 歷史記錄未能儲存：{error}",
                    warning.unwrap_or_default()
                ));
            }
        }
        Ok::<_, String>((
            DictationResult {
                text: prepared.text,
                raw_text,
                inserted: false,
                paste_posted: false,
                warning,
                insertion_detail: None,
                app_name: context.app_name,
                mode: request_mode,
            },
            config.auto_insert && prepared.may_insert,
            context.warning,
        ))
    })
    .await
    .map_err(|e| e.to_string())?;
    let (mut result, auto_insert, capture_warning) = match processed {
        Ok(processed) => processed,
        Err(error) => {
            if let Some(token) = token {
                let _ = on_main(&app, move || macos_input::cancel_target(token)).await;
            }
            return Err(error);
        }
    };
    if auto_insert && !result.text.trim().is_empty() {
        if let Some(token) = token {
            let text = result.text.clone();
            let insertion_guard = processing_guard.clone();
            match on_main(&app, move || {
                let _insertion_guard = insertion_guard;
                macos_input::insert_text(token, &text)
            })
            .await?
            {
                Ok(()) => {
                    result.inserted = true;
                    result.paste_posted = true;
                }
                Err(error) => {
                    result.paste_posted = error.may_have_inserted;
                    result.insertion_detail = Some(error.error.message);
                    let message = if error.may_have_inserted {
                        "貼上已送出一次，但目標 App 未回報確認。請先檢查原本輸入位置；已有完整文字就毋須再貼上。"
                    } else {
                        "未能自動輸入；轉錄已保留，可以手動複製。"
                    };
                    result.warning = Some(match result.warning.take() {
                        Some(previous) => format!("{previous} {message}"),
                        None => message.into(),
                    });
                }
            }
        } else {
            result.warning = Some(format!(
                "未能自動輸入：{}。轉錄已保留，可以手動複製。",
                capture_warning
                    .unwrap_or_else(|| "請先啟用輔助使用，再喺目標輸入欄開始錄音".into())
            ));
        }
    } else if let Some(token) = token {
        let _ = on_main(&app, move || macos_input::cancel_target(token)).await;
    }
    Ok(result)
}

#[tauri::command]
async fn cancel_recording(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let Some(cancel_guard) = state.update_gate.begin_cancel()? else {
        return Ok(());
    };
    let worker_guard = cancel_guard.clone();
    let manager = state.audio_manager.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _worker_guard = worker_guard;
        if manager.get_recording_status()?.is_some() {
            let _ = manager.stop_recording();
        }
        if manager.get_recording_status()?.is_some() {
            return Err("未能停止錄音".to_string());
        }
        _worker_guard.stopped();
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| e.to_string())??;
    let context = state.target.lock().map_err(|e| e.to_string())?.take();
    if let Some(token) = context.and_then(|c| c.token) {
        let _ = on_main(&app, move || macos_input::cancel_target(token)).await;
    }
    if let Some(window) = app.get_webview_window("voice-bar") {
        let _ = window.hide();
    }
    Ok(())
}

#[tauri::command]
async fn get_insertion_status(
    app: tauri::AppHandle,
) -> Result<macos_input::InsertionStatus, String> {
    on_main(&app, macos_input::get_insertion_status)
        .await?
        .map_err(|e| e.message)
}

#[tauri::command]
async fn test_insertion(app: tauri::AppHandle) -> Result<String, String> {
    on_main(&app, || {
        let token = macos_input::capture_target().map_err(|error| error.message)?;
        let selected = macos_input::selected_text(token).unwrap_or_default();
        if !selected.is_empty() {
            let _ = macos_input::cancel_target(token);
            return Err("請先喺空白測試文件放低游標，唔好選取現有文字，再試一次。".into());
        }
        let app_name = macos_input::target_app_name(token).unwrap_or_default();
        match macos_input::insert_text(token, "GPT-Typeless 自動輸入測試") {
            Ok(()) => Ok(format!(
                "已確認插入 {app_name} 嘅原來游標位置。測試冇使用語音辨識或 AI。"
            )),
            Err(failure) => {
                let advice = if failure.may_have_inserted {
                    "請先檢查測試文件，避免重複輸入。"
                } else {
                    "未能插入文字，請檢查權限同測試文件嘅輸入位置。"
                };
                Err(format!("{} {advice}", failure.error.message))
            }
        }
    })
    .await?
}

#[tauri::command]
async fn request_accessibility(app: tauri::AppHandle) -> Result<bool, String> {
    on_main(&app, macos_input::request_accessibility)
        .await?
        .map_err(|e| e.message)
}

#[tauri::command]
async fn request_post_event_access(app: tauri::AppHandle) -> Result<bool, String> {
    on_main(&app, macos_input::request_post_event_access)
        .await?
        .map_err(|e| e.message)
}

/// Opens the system pane only in response to the setup button; it does not grant permission.
#[tauri::command]
async fn open_accessibility_settings(app: tauri::AppHandle) -> Result<(), String> {
    on_main(&app, macos_input::open_accessibility_settings)
        .await?
        .map_err(|e| e.message)
}

/// Reveal this running bundle, never a guessed install path or arbitrary frontend path.
#[tauri::command]
async fn reveal_running_app() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let bundle = executable
        .ancestors()
        .find(|p| p.extension().is_some_and(|ext| ext == "app"))
        .ok_or_else(|| "搵唔到執行中嘅 App，請喺 Finder 開啟應用程式資料夾。".to_string())?;
    tauri_plugin_opener::reveal_item_in_dir(bundle).map_err(|e| e.to_string())
}

#[tauri::command]
async fn clear_provider_key(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || storage.clear_provider_key(&id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn test_provider(
    provider: text_ai::ProviderConfig,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let key = if provider.api_key.trim().is_empty() {
            storage.provider_key(&provider.id)?
        } else {
            provider.api_key.trim().into()
        };
        text_ai::test_connection(&provider, &key).map(|_| "連線測試成功".into())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn process_text(
    request: text_ai::TextRequest,
    provider_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let processing_guard = state.update_gate.begin_standalone_processing()?;
    let storage = state.storage.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _processing_guard = processing_guard;
        let config = storage.load_settings()?;
        let id = provider_id
            .or(config.active_provider_id)
            .ok_or("請先選擇 AI 供應商")?;
        let provider = config
            .providers
            .iter()
            .find(|provider| provider.id == id)
            .ok_or("搵唔到所選供應商")?;
        text_ai::process(provider, &storage.provider_key(&id)?, &request)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn update_history_item(id: String, text: String, state: State<'_, AppState>) -> Result<(), String> {
    state.storage.update_history(&id, text)
}

#[derive(Clone, Serialize, Deserialize)]
struct VoiceStatus {
    status: String,
    mode: String,
    translation_language: Option<String>,
}

#[tauri::command]
fn set_voice_status(
    status: String,
    mode: Option<String>,
    picker_open: Option<bool>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if ![
        "idle",
        "starting",
        "recording",
        "processing",
        "done",
        "error",
    ]
    .contains(&status.as_str())
    {
        return Err("未知嘅錄音狀態".into());
    }
    // Only the selected language leaves the main process, never the config.
    let settings = state.storage.load_settings()?;
    let mode = mode.unwrap_or_else(|| "dictate".into());
    let translating = mode == "translate" && ["starting", "recording"].contains(&status.as_str());
    let expanded = translating && picker_open.unwrap_or(false);
    let payload = VoiceStatus {
        status: status.clone(),
        mode,
        translation_language: translating.then_some(settings.translation_language),
    };
    app.emit("dictation-status", payload)
        .map_err(|e| e.to_string())?;
    if let Some(window) = app.get_webview_window("voice-bar") {
        if !settings.voice_bar_enabled || ["idle", "done", "error"].contains(&status.as_str()) {
            let _ = window.hide();
        } else {
            // Keep the original 196x44 bar in place while leaving room above it
            // for the menu. This Tauri NSWindow is not a non-activating NSPanel.
            let width = if translating { 240.0 } else { 196.0 };
            let height = if expanded {
                304.0
            } else if translating {
                76.0
            } else {
                44.0
            };
            let _ = window.set_size(tauri::LogicalSize::new(width, height));
            if let Some(main) = app.get_webview_window("main") {
                if let Ok(Some(monitor)) = main.current_monitor() {
                    let scale = monitor.scale_factor();
                    let size = monitor.size();
                    let position = monitor.position();
                    let x =
                        (f64::from(position.x) + f64::from(size.width) / 2.0) / scale - width / 2.0;
                    let y = (f64::from(position.y) + f64::from(size.height)) / scale
                        - 110.0
                        - (height - 44.0);
                    let _ = window.set_position(tauri::LogicalPosition::new(x, y));
                }
            }
            let _ = window.show();
        }
    }
    Ok(())
}

fn update_fn_shortcut(app: &tauri::AppHandle, state: &AppState, enabled: bool) {
    let Ok(mut handle) = state.fn_shortcut.lock() else {
        return;
    };
    let Ok(mut error) = state.fn_error.lock() else {
        return;
    };
    if !enabled {
        if let Some(mut listener) = handle.take() {
            listener.uninstall();
        }
        *error = None;
    } else if handle.is_none() {
        let app = app.clone();
        match fn_shortcut::install(true, move |mode| {
            let _ = app.emit("fn-shortcut", mode.as_str());
        }) {
            Ok(listener) => {
                *handle = Some(listener);
                *error = None;
            }
            Err(problem) => *error = Some(problem.to_string()),
        }
    }
}

#[derive(Serialize)]
struct FnStatus {
    enabled: bool,
    available: bool,
    message: String,
}

#[tauri::command]
fn get_fn_status(state: State<'_, AppState>) -> Result<FnStatus, String> {
    let enabled = state.storage.load_settings()?.fn_shortcut_enabled;
    let handle = state.fn_shortcut.lock().map_err(|e| e.to_string())?;
    let available = handle
        .as_ref()
        .is_some_and(|h| h.status() == fn_shortcut::ShortcutStatus::Running);
    let problem = handle
        .as_ref()
        .and_then(|listener| match listener.status() {
            fn_shortcut::ShortcutStatus::Unavailable(error) => Some(error.to_string()),
            _ => None,
        });
    let message = if available {
        "Fn 監聽已啟用，請用鍵盤實際測試。".into()
    } else if enabled {
        let error = problem.or(state.fn_error.lock().map_err(|e| e.to_string())?.clone());
        match error.as_deref() {
            Some("Fn shortcuts unavailable: Accessibility trust is missing.") => {
                "請先允許控制其他 App 嘅權限。".into()
            }
            Some("Fn shortcuts unavailable: Input Monitoring access is missing.") => {
                "請喺系統設定允許輸入監控，然後重新檢查。".into()
            }
            Some(error) => format!("Fn 快捷鍵未能啟用：{error}"),
            None => "Fn 快捷鍵未運作，請重新檢查。".into(),
        }
    } else {
        "未啟用 Fn，仍可使用已設定嘅 Mac 快捷鍵".into()
    };
    Ok(FnStatus {
        enabled,
        available,
        message,
    })
}

// This never grants permission or changes the user's saved choice. It retries
// the listener only after the user previously opted into Fn shortcuts.
#[tauri::command]
fn retry_fn_shortcut(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<FnStatus, String> {
    if state.storage.load_settings()?.fn_shortcut_enabled {
        let stale = {
            let mut handle = state.fn_shortcut.lock().map_err(|e| e.to_string())?;
            if handle
                .as_ref()
                .is_some_and(|listener| listener.status() != fn_shortcut::ShortcutStatus::Running)
            {
                handle.take()
            } else {
                None
            }
        };
        drop(stale);
        update_fn_shortcut(&app, &state, true);
    }
    get_fn_status(state)
}

#[tauri::command]
async fn request_input_monitoring(app: tauri::AppHandle) -> Result<(), String> {
    on_main(&app, || {
        // Only invoked by the user's explicit request. This registers the app
        // with macOS and may show the system permission prompt.
        if objc2_core_graphics::CGRequestListenEventAccess() {
            return Ok::<_, String>(());
        }
        let url = objc2_foundation::NSURL::URLWithString(&objc2_foundation::NSString::from_str(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
        ))
        .ok_or("無法開啟輸入監控設定")?;
        if objc2_app_kit::NSWorkspace::sharedWorkspace().openURL(&url) {
            Ok::<_, String>(())
        } else {
            Err("請喺系統設定 → 私隱與保安 → 輸入監控，啟用 GPT-Typeless".into())
        }
    })
    .await?
}

pub fn run() {
    let context = tauri::generate_context!();
    let builder = tauri::Builder::default();
    // Do not prevent the rest of the app from starting before the parent
    // supplies a complete, signed-version updater configuration.
    let builder = if updater::configured(context.config()) {
        builder.plugin(tauri_plugin_updater::Builder::new().build())
    } else {
        builder
    };
    let app = builder
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _, event| {
                    let _ = app.emit("hotkey-triggered", event.state() == ShortcutState::Pressed);
                })
                .build(),
        )
        .setup(|app| {
            let _ = macos_input::start_target_tracking();
            let voice_bar = tauri::WebviewWindowBuilder::new(
                app,
                "voice-bar",
                tauri::WebviewUrl::App("index.html?overlay=1".into()),
            )
            .title("GPT-Typeless Voice")
            .inner_size(196.0, 44.0)
            .decorations(false)
            .resizable(false)
            .focused(false)
            .focusable(false)
            .accept_first_mouse(true)
            .always_on_top(true)
            .visible_on_all_workspaces(true)
            .skip_taskbar(true)
            .shadow(false)
            .visible(false)
            .background_color(tauri::window::Color(0, 0, 0, 0))
            .build();
            if let Ok(window) = voice_bar {
                // The menu sits above the rounded HTML pill on a clear surface.
                if let Ok(pointer) = window.ns_window() {
                    // SAFETY: setup runs on the main thread, and Tauri owns this
                    // live NSWindow for the duration of the borrowed reference.
                    let native = unsafe { &*pointer.cast::<objc2_app_kit::NSWindow>() };
                    native.setOpaque(false);
                    native.setBackgroundColor(Some(&objc2_app_kit::NSColor::clearColor()));
                    if let Some(view) = native.contentView() {
                        view.setWantsLayer(true);
                        if let Some(layer) = view.layer() {
                            layer.setCornerRadius(0.0);
                            layer.setMasksToBounds(false);
                        }
                    }
                }
            }
            let storage = Storage::new(app.path().app_data_dir()?)?;
            let audio_manager = AudioManager::new(storage.recordings_dir())?;
            let levels = audio_manager.level_monitor();
            let meter_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_millis(33));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut was_recording = false;
                loop {
                    ticker.tick().await;
                    let reading = levels.snapshot();
                    if reading.recording || was_recording {
                        let _ = meter_app.emit_to("voice-bar", "audio-level", reading);
                    }
                    was_recording = reading.recording;
                }
            });
            // Old recordings are removed locally; they are never uploaded on startup.
            audio_manager.cleanup_old_recordings(7)?;
            let config = storage.load_settings()?;
            let shortcut: Shortcut = config.hotkey.parse()?;
            let active = match app.global_shortcut().register(shortcut) {
                Ok(()) => Some(shortcut),
                Err(e) => {
                    eprintln!("快捷鍵註冊失敗，請喺設定更改: {e}");
                    None
                }
            };
            app.manage(AppState {
                audio_manager,
                storage,
                shortcut: Mutex::new(active),
                config_lock: tokio::sync::Mutex::new(()),
                target: Mutex::new(None),
                fn_shortcut: Mutex::new(None),
                fn_error: Mutex::new(None),
                update_gate: updater::ActivityGate::default(),
            });
            update_fn_shortcut(
                app.handle(),
                &app.state::<AppState>(),
                config.fn_shortcut_enabled,
            );
            Ok(())
        })
        .on_window_event(|window, event| {
            #[cfg(target_os = "macos")]
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Keep the global recording shortcut available after closing the window.
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            start_recording,
            stop_recording,
            test_microphone,
            complete_dictation,
            cancel_recording,
            get_insertion_status,
            test_insertion,
            get_fn_status,
            retry_fn_shortcut,
            request_input_monitoring,
            request_accessibility,
            request_post_event_access,
            open_accessibility_settings,
            reveal_running_app,
            setup_drag::set_setup_drag_region,
            clear_provider_key,
            test_provider,
            process_text,
            update_history_item,
            set_voice_status,
            show_main_window,
            get_recording_status,
            list_audio_devices,
            transcribe_audio,
            copy_to_clipboard,
            get_config_command,
            set_config,
            set_translation_language,
            cleanup_old_recordings,
            get_transcription_history,
            delete_history_item,
            clear_all_history,
            check_hotkey,
            updater::check_update,
            updater::download_update,
            updater::install_update,
            updater::cancel_update,
            updater::update_system_idle,
        ])
        .build(context)
        .expect("無法啟動 GPT-Typeless");
    app.run(|app, event| {
        if let tauri::RunEvent::ExitRequested { .. } = &event {
            let _ = macos_input::stop_target_tracking();
            if let Some(state) = app.try_state::<AppState>() {
                if state
                    .audio_manager
                    .get_recording_status()
                    .ok()
                    .flatten()
                    .is_some()
                {
                    let _ = state.audio_manager.stop_recording();
                }
            }
        }
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
    });
}
