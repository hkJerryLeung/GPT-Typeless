//! Recording shortcut parsing and event routing. Single keys use a macOS
//! active tap; combinations continue to use Tauri's global hotkeys.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShortcutSpec {
    pub shortcut: Shortcut,
    pub single_key: Option<SingleKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SingleKey {
    pub code: u16,
    pub allow_fn_flag: bool,
}

pub(crate) fn parse(input: &str) -> Result<ShortcutSpec, String> {
    let input = input.trim();
    if input.eq_ignore_ascii_case("fn") || input.to_ascii_lowercase().starts_with("fn+") {
        return Err("Fn 請使用獨立的 Fn 快捷鍵設定；並非所有鍵盤都會向 macOS 傳送 Fn。".into());
    }
    let shortcut: Shortcut = input
        .parse()
        .map_err(|e| format!("快捷鍵格式或按鍵不受支援: {e}"))?;
    let single_key = if shortcut.mods.is_empty() {
        Some(mac_single_key(shortcut.key).ok_or_else(|| {
            format!(
                "macOS 暫不支援把 {} 設為單鍵錄音快捷鍵；請使用一般按鍵、F1–F20 或組合鍵。",
                shortcut.key
            )
        })?)
    } else {
        None
    };
    Ok(ShortcutSpec {
        shortcut,
        single_key,
    })
}

// Virtual key positions from HIToolbox/Events.h, matching global-hotkey's
// macOS mapping. These are key positions, not layout-independent characters.
// Fn, modifiers, Caps Lock, media keys and F21–F24 are deliberately excluded:
// they are not ordinary KeyDown/KeyUp events on every Mac/keyboard.
fn mac_single_key(key: Code) -> Option<SingleKey> {
    use Code::*;
    let code = match key {
        KeyA => 0x00,
        KeyS => 0x01,
        KeyD => 0x02,
        KeyF => 0x03,
        KeyH => 0x04,
        KeyG => 0x05,
        KeyZ => 0x06,
        KeyX => 0x07,
        KeyC => 0x08,
        KeyV => 0x09,
        KeyB => 0x0b,
        KeyQ => 0x0c,
        KeyW => 0x0d,
        KeyE => 0x0e,
        KeyR => 0x0f,
        KeyY => 0x10,
        KeyT => 0x11,
        Digit1 => 0x12,
        Digit2 => 0x13,
        Digit3 => 0x14,
        Digit4 => 0x15,
        Digit6 => 0x16,
        Digit5 => 0x17,
        Equal => 0x18,
        Digit9 => 0x19,
        Digit7 => 0x1a,
        Minus => 0x1b,
        Digit8 => 0x1c,
        Digit0 => 0x1d,
        BracketRight => 0x1e,
        KeyO => 0x1f,
        KeyU => 0x20,
        BracketLeft => 0x21,
        KeyI => 0x22,
        KeyP => 0x23,
        Enter => 0x24,
        KeyL => 0x25,
        KeyJ => 0x26,
        Quote => 0x27,
        KeyK => 0x28,
        Semicolon => 0x29,
        Backslash => 0x2a,
        Comma => 0x2b,
        Slash => 0x2c,
        KeyN => 0x2d,
        KeyM => 0x2e,
        Period => 0x2f,
        Tab => 0x30,
        Space => 0x31,
        Backquote => 0x32,
        Backspace => 0x33,
        Escape => 0x35,
        F17 => 0x40,
        NumpadDecimal => 0x41,
        NumpadMultiply => 0x43,
        NumpadAdd => 0x45,
        NumpadDivide => 0x4b,
        NumpadEnter => 0x4c,
        NumpadSubtract => 0x4e,
        F18 => 0x4f,
        F19 => 0x50,
        NumpadEqual => 0x51,
        Numpad0 => 0x52,
        Numpad1 => 0x53,
        Numpad2 => 0x54,
        Numpad3 => 0x55,
        Numpad4 => 0x56,
        Numpad5 => 0x57,
        Numpad6 => 0x58,
        Numpad7 => 0x59,
        F20 => 0x5a,
        Numpad8 => 0x5b,
        Numpad9 => 0x5c,
        F5 => 0x60,
        F6 => 0x61,
        F7 => 0x62,
        F3 => 0x63,
        F8 => 0x64,
        F9 => 0x65,
        F11 => 0x67,
        F13 => 0x69,
        F16 => 0x6a,
        F14 => 0x6b,
        F10 => 0x6d,
        F12 => 0x6f,
        F15 => 0x71,
        Insert => 0x72,
        Home => 0x73,
        PageUp => 0x74,
        Delete => 0x75,
        F4 => 0x76,
        End => 0x77,
        F2 => 0x78,
        PageDown => 0x79,
        F1 => 0x7a,
        ArrowLeft => 0x7b,
        ArrowRight => 0x7c,
        ArrowDown => 0x7d,
        ArrowUp => 0x7e,
        _ => return None,
    };
    // The function flag can accompany navigation keys as well as F keys; it
    // does not by itself prove that a physical Fn key was pressed.
    let allow_fn_flag = matches!(
        key,
        F1 | F2
            | F3
            | F4
            | F5
            | F6
            | F7
            | F8
            | F9
            | F10
            | F11
            | F12
            | F13
            | F14
            | F15
            | F16
            | F17
            | F18
            | F19
            | F20
            | Insert
            | Home
            | PageUp
            | Delete
            | End
            | PageDown
            | ArrowLeft
            | ArrowRight
            | ArrowDown
            | ArrowUp
    );
    Some(SingleKey {
        code,
        allow_fn_flag,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RouteId {
    Global(u32),
    Single(u64),
}

#[derive(Default)]
pub(crate) struct Router {
    current: Option<RouteId>,
    down: bool,
}

impl Router {
    fn switch(&mut self, current: Option<RouteId>) -> Option<bool> {
        let release = self.down.then_some(false);
        self.current = current;
        self.down = false;
        release
    }

    fn event(&mut self, id: RouteId, pressed: bool) -> Option<bool> {
        if self.current != Some(id) || self.down == pressed {
            return None;
        }
        self.down = pressed;
        Some(pressed)
    }
}

pub(crate) type SharedRouter = Arc<Mutex<Router>>;
pub(crate) type CleanupWarning = Arc<Mutex<Option<String>>>;

enum Backend {
    Global(AppHandle),
    Single(crate::single_key_shortcut::SingleKeyShortcut),
}

pub(crate) struct Registration {
    pub spec: ShortcutSpec,
    id: RouteId,
    backend: Backend,
    cleanup_warning: CleanupWarning,
}

impl Registration {
    pub(crate) fn prepare(
        app: &AppHandle,
        spec: ShortcutSpec,
        router: &SharedRouter,
        cleanup_warning: &CleanupWarning,
        main_focused: &Arc<AtomicBool>,
    ) -> Result<Self, String> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let (id, backend) = if let Some(key) = spec.single_key {
            let id = RouteId::Single(NEXT_ID.fetch_add(1, Ordering::Relaxed));
            let app = app.clone();
            let router = router.clone();
            let listener =
                crate::single_key_shortcut::install(key, main_focused.clone(), move |pressed| {
                    dispatch(&router, &app, id, pressed)
                })?;
            (id, Backend::Single(listener))
        } else {
            app.global_shortcut()
                .register(spec.shortcut)
                .map_err(|error| {
                    format!("組合鍵無法註冊，可能不受 macOS 支援或已被其他 App 使用: {error}")
                })?;
            (
                RouteId::Global(spec.shortcut.id()),
                Backend::Global(app.clone()),
            )
        };
        Ok(Self {
            spec,
            id,
            backend,
            cleanup_warning: cleanup_warning.clone(),
        })
    }

    pub(crate) fn activate(&self, router: &SharedRouter, app: &AppHandle) {
        switch(router, app, Some(self.id));
        if let Backend::Single(listener) = &self.backend {
            listener.enable();
        }
    }

    pub(crate) fn status(&self) -> Result<(), String> {
        match &self.backend {
            Backend::Global(_) => Ok(()),
            Backend::Single(listener) => listener.status(),
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Backend::Global(app) = &self.backend {
            if let Err(error) = app.global_shortcut().unregister(self.spec.shortcut) {
                let warning =
                    format!("舊組合鍵未能解除註冊，可能仍被佔用；請重新啟動應用程式: {error}");
                *self
                    .cleanup_warning
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(warning.clone());
                eprintln!("{warning}");
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct RegistrationState {
    pub active: Option<Registration>,
    pub error: Option<String>,
}

pub(crate) fn switch(router: &SharedRouter, app: &AppHandle, id: Option<RouteId>) {
    let mut router = router.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(value) = router.switch(id) {
        let _ = app.emit("hotkey-triggered", value);
    }
}

pub(crate) fn dispatch(router: &SharedRouter, app: &AppHandle, id: RouteId, pressed: bool) {
    // Keep the transition and emit ordered across the global and native workers.
    let mut router = router.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(value) = router.event(id, pressed) {
        let _ = app.emit("hotkey-triggered", value);
    }
}

// A prepared registration owns its cleanup. Failure drops only the candidate;
// success activates it before retiring the previous registration.
pub(crate) fn finish_change<T>(
    candidate: Option<T>,
    saved: Result<(), String>,
    activate: impl FnOnce(&T),
    replace: impl FnOnce(T) -> Option<T>,
) -> Result<(), String> {
    saved?;
    if let Some(candidate) = candidate {
        activate(&candidate);
        // The caller must release its state lock before this old registration
        // is dropped: unregistering a Tauri global shortcut needs the main thread.
        let previous = replace(candidate);
        drop(previous);
    }
    Ok(())
}

pub(crate) fn initial_registration<T>(
    prepared: Result<T, String>,
    activate: impl FnOnce(&T),
) -> (Option<T>, Option<String>) {
    match prepared {
        Ok(registration) => {
            activate(&registration);
            (Some(registration), None)
        }
        Err(error) => (None, Some(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn parses_existing_combination_and_safe_single_keys() {
        let combination = parse("Cmd+Shift+R").unwrap();
        assert!(combination.single_key.is_none());
        assert_eq!(parse(" A ").unwrap().single_key.unwrap().code, 0x00);
        assert_eq!(parse("Space").unwrap().single_key.unwrap().code, 0x31);
        let f20 = parse("F20").unwrap().single_key.unwrap();
        assert_eq!(f20.code, 0x5a);
        assert!(f20.allow_fn_flag);
        assert!(!parse("A").unwrap().single_key.unwrap().allow_fn_flag);
        // macOS may mark navigation keys as function keys even without a
        // physical Fn edge, so that flag must not reject them.
        assert!(
            parse("ArrowLeft")
                .unwrap()
                .single_key
                .unwrap()
                .allow_fn_flag
        );
        assert!(parse("Delete").unwrap().single_key.unwrap().allow_fn_flag);
    }

    #[test]
    fn shortcuts_exposed_by_settings_are_accepted_by_the_native_parser() {
        // These names are produced by the shortcut field, including physical
        // punctuation and keypad keys whose characters can vary by layout.
        let keys = [
            "A",
            "Z",
            "0",
            "9",
            "`",
            "\\",
            "[",
            "]",
            ",",
            "=",
            "-",
            ".",
            "'",
            ";",
            "/",
            "Backspace",
            "Delete",
            "Enter",
            "Space",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
            "ArrowUp",
            "Home",
            "End",
            "Insert",
            "PageUp",
            "PageDown",
            "Numpad0",
            "Numpad9",
            "NumpadAdd",
            "NumpadDecimal",
            "NumpadDivide",
            "NumpadEnter",
            "NumpadEqual",
            "NumpadMultiply",
            "NumpadSubtract",
            "F1",
            "F20",
        ];
        for key in keys {
            assert!(parse(key)
                .unwrap_or_else(|error| panic!("{key}: {error}"))
                .single_key
                .is_some());
            let combo = format!("Cmd+Shift+{key}");
            assert!(parse(&combo)
                .unwrap_or_else(|error| panic!("{combo}: {error}"))
                .single_key
                .is_none());
        }
    }

    #[test]
    fn rejects_unexposed_or_non_keyboard_single_keys_with_diagnostics() {
        assert!(parse("Fn").unwrap_err().contains("Fn"));
        assert!(parse("Fn+Space").unwrap_err().contains("Fn"));
        for key in ["F21", "CapsLock", "AudioVolumeUp", "MediaPlayPause"] {
            assert!(parse(key).unwrap_err().contains("單鍵"), "{key}");
        }
        assert!(parse("Cmd+Shift").is_err());
        assert!(parse("Cmd+R+Shift").is_err());
        assert!(parse("Shift").is_err());
    }

    #[test]
    fn router_filters_staged_and_stale_events_and_releases_held_key() {
        let mut router = Router::default();
        let old = RouteId::Global(1);
        let new = RouteId::Single(2);
        assert_eq!(router.switch(Some(old)), None);
        assert_eq!(router.event(new, true), None);
        assert_eq!(router.event(old, true), Some(true));
        assert_eq!(router.event(old, true), None);
        assert_eq!(router.switch(Some(new)), Some(false));
        assert_eq!(router.event(old, false), None);
        assert_eq!(router.event(new, false), None);
        assert_eq!(router.event(new, true), Some(true));
        assert_eq!(router.event(new, false), Some(false));
    }

    struct Resource(Arc<AtomicUsize>);
    impl Drop for Resource {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn save_failure_discards_candidate_and_success_retires_old_registration() {
        let old_drops = Arc::new(AtomicUsize::new(0));
        let new_drops = Arc::new(AtomicUsize::new(0));
        let current = Mutex::new(Some(Resource(old_drops.clone())));
        let failed = Some(Resource(new_drops.clone()));
        assert!(finish_change(
            failed,
            Err("save failed".into()),
            |_| panic!("must not activate"),
            |resource| { current.lock().unwrap().replace(resource) }
        )
        .is_err());
        assert_eq!(old_drops.load(Ordering::SeqCst), 0);
        assert_eq!(new_drops.load(Ordering::SeqCst), 1);
        let replacement = Some(Resource(new_drops.clone()));
        finish_change(
            replacement,
            Ok(()),
            |_| {},
            |resource| current.lock().unwrap().replace(resource),
        )
        .unwrap();
        assert_eq!(old_drops.load(Ordering::SeqCst), 1);
        assert_eq!(new_drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn old_registration_is_dropped_after_state_lock_is_released() {
        use std::sync::Weak;
        struct LockCheckingResource(Weak<Mutex<Option<LockCheckingResource>>>);
        impl Drop for LockCheckingResource {
            fn drop(&mut self) {
                if let Some(state) = self.0.upgrade() {
                    assert!(
                        state.try_lock().is_ok(),
                        "registration dropped while state is locked"
                    );
                }
            }
        }
        let state = Arc::new(Mutex::new(None));
        let resource = || LockCheckingResource(Arc::downgrade(&state));
        *state.lock().unwrap() = Some(resource());
        finish_change(
            Some(resource()),
            Ok(()),
            |_| {},
            |new| state.lock().unwrap().replace(new),
        )
        .unwrap();
    }

    #[test]
    fn invalid_or_unavailable_startup_shortcut_keeps_application_recoverable() {
        for error in [
            parse("Fn").unwrap_err(),
            "input monitoring unavailable".into(),
        ] {
            let (active, reported) =
                initial_registration::<()>(Err(error.clone()), |_| panic!("must not activate"));
            assert!(active.is_none());
            assert_eq!(reported.as_deref(), Some(error.as_str()));
        }
    }

    #[test]
    fn settings_save_keeps_recording_blocked_while_cancelling_updates() {
        let gate = crate::updater::ActivityGate::default();
        let saving = gate.begin_settings_save().unwrap();
        assert!(gate.begin_recording().is_err());
        gate.cancel_update().unwrap();
        assert!(gate.begin_recording().is_err());
        drop(saving);
        assert!(gate.begin_recording().is_ok());
    }
}
