//! Bounded, one-shot macOS dictation insertion using a temporary clipboard and Cmd-V.
//!
//! Parent integration (this module does not register Tauri commands):
//! - Add `#[cfg(target_os = "macos")] mod macos_input;` in lib.rs.
//! - Call `start_target_tracking()` once on the Tauri main thread during setup.
//!   It observes NSWorkspace *application deactivation*, snapshots that outgoing
//!   external application's focused field, and keeps only the latest result.
//!   This preserves the caret before a click on our window/record button. It is
//!   event driven: no polling, keystroke hooks, screen capture, or document reads.
//! - At dictation start, BEFORE showing/focusing our window, call `capture_target()`.
//!   A global shortcut uses the frontmost external app. A button start restores
//!   only the recent outgoing app (at most 30 seconds old) before validating its
//!   snapshot. If deactivation had hidden its focus, acquire a fresh initial
//!   snapshot after restoration. Missing/stale/security failures are not guessed.
//! - Carry the opaque `TargetToken` through transcription/polish/translation.
//!   `selected_text(token)` returns only the captured selection for edit prompts.
//!   `capture_selected_text()` supports Ask/read-only selections without an
//!   insertion token or writable field, reading only bounded AXSelectedText.
//!   On completion call `insert_text(token, &completed_text)`; on cancellation call
//!   `cancel_target(token)`. A new capture invalidates the previous token.
//!   If our app is frontmost on completion, insertion restores the captured app,
//!   then validates its field/snapshot before clipboard or text mutation.
//!   If an unrelated app is frontmost, it neither activates nor writes.
//! - Every public function MUST run on the process main thread, e.g. inside
//!   `AppHandle::run_on_main_thread`. Do not block the main thread waiting for that
//!   closure. Only tokens, strings, errors and results cross async/thread boundaries;
//!   AX/AppKit objects stay in thread-local state. Wrong-thread calls fail safely.
//! - On insertion failure, retain/display `failure.transcript` and offer the
//!   parent's explicit Copy action. If `may_have_inserted`, tell the user to check
//!   the original field before pasting. Never auto-retry a consumed token.
//! - Permission checks NEVER prompt. Only invoke the request/settings functions
//!   from an explicit permission UI action. A request does not grant permission.
//!
//! Safety/compatibility bounds: one remembered target and one pending session,
//! 15 minute session TTL, 16,384 UTF-16 units per selection/output, and only the
//! selection plus 64 UTF-16 units of context on each side are read via
//! AXStringForRange, or same-range AXAttributedStringForRange if unavailable.
//! No AXValue/full-document text reads. Each AX IPC has a 150 ms timeout;
//! preflight and paste confirmation each have a 1.5 second budget.
//! Activation confirmation waits at most 250 ms plus one bounded AX query within
//! that same budget; it does not pump/reenter the main run loop. A pending OS
//! activation can still complete after a timeout, but no text is then inserted.
//! Only a single contiguous selection is supported. Role, secure subrole, process
//! identity, focused field, range, length, and bounded context must still match.
//! Edits outside that neighborhood with unchanged length cannot be detected.
//! AX has no compare-and-swap: an external edit racing the final checked write
//! cannot be made atomic. Unsupported/ambiguous providers fail closed; there is
//! no Return, submit action, or whole-field replacement. App activation
//! uses no force/ignore-other-apps flags and requires confirmed original focus.
//! Paste is chosen BEFORE any write; AXSelectedText is never written. Only a
//! Cmd-V down/up pair is posted to the original PID after foreground/focus checks.
//! Clipboard items/formats are materialized as NSData before changing it
//! (limits: 32 items, 64 types/item, 32 MiB combined). Failed snapshots abort.
//! Restore requires our changeCount AND ownership marker; newer clipboard
//! contents are left alone. Pasteboard compare/write is not atomic. Known
//! terminal hosts and secure fields are rejected. Default/Ask clipboard access
//! may show the OS prompt during the user's explicit dictation/test action;
//! AlwaysDeny fails closed. This module never changes accessBehavior. If a prompt
//! exhausts preflight's budget, it aborts before clearContents; retry after grant.
//!
//! References: <https://developer.apple.com/documentation/applicationservices/kaxselectedtextattribute>,
//! <https://developer.apple.com/documentation/appkit/nsworkspace/diddeactivateapplicationnotification>,
//! <https://developer.apple.com/documentation/appkit/nsrunningapplication/activate(options:)>,
//! <https://docs.rs/objc2-application-services/latest/objc2_application_services/>.
//! objc2-application-services 0.3.2 bindings. AXSelectedText is writable only in
//! some providers; writability is checked via AXValue/AXSelectedText metadata.

use serde::Serialize;
use std::fmt;
use std::time::Duration;

const MAX_TEXT_UNITS: usize = 16_384;
const CONTEXT_UNITS: usize = 64;
const REMEMBER_TTL: Duration = Duration::from_secs(30);
const SESSION_TTL: Duration = Duration::from_secs(15 * 60);

/// Opaque, main-thread-owned session ID; safe to move through async provider work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct TargetToken(u64);

#[derive(Clone, Debug, Serialize)]
pub struct InsertionStatus {
    pub trusted: bool,
    /// Read-only check for posting the paste event.
    pub post_event: bool,
    /// App label for a recent remembered/pending target; does not imply that
    /// its caret is still valid. No field text is read by the status function.
    pub target_app: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputErrorCode {
    WrongThread,
    PermissionDenied,
    MissingTarget,
    StaleTarget,
    SecureField,
    UnsupportedField,
    TooLarge,
    Timeout,
    Accessibility,
    VerificationFailed,
    SettingsUnavailable,
}

#[derive(Clone, Debug, Serialize)]
pub struct InputError {
    pub code: InputErrorCode,
    pub message: String,
}

impl InputError {
    fn new(code: InputErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for InputError {}

/// Parent retains this text for its Copy action. Clipboard restoration is attempted
/// only while still owned. `may_have_inserted` becomes true before posting Cmd-V;
/// it remains true on unconfirmed/late delivery. Never automatically retry it.
#[derive(Clone, Serialize)]
pub struct InsertionFailure {
    pub error: InputError,
    pub transcript: String,
    pub may_have_inserted: bool,
}

// Deliberately exclude dictation/selected text from diagnostic formatting.
impl fmt::Debug for InsertionFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InsertionFailure")
            .field("error", &self.error)
            .field("may_have_inserted", &self.may_have_inserted)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TextRange {
    start: usize,
    len: usize,
}

impl TextRange {
    fn checked(location: isize, length: isize, total: usize) -> Result<Self, InputError> {
        let invalid = || {
            InputError::new(
                InputErrorCode::UnsupportedField,
                "The field returned an invalid text selection; use Copy.",
            )
        };
        let start = usize::try_from(location).map_err(|_| invalid())?;
        let len = usize::try_from(length).map_err(|_| invalid())?;
        if start.checked_add(len).is_none_or(|end| end > total) {
            return Err(invalid());
        }
        if len > MAX_TEXT_UNITS {
            return Err(InputError::new(
                InputErrorCode::TooLarge,
                "The selected text exceeds the automatic insertion limit; use Copy.",
            ));
        }
        Ok(Self { start, len })
    }

    fn context(self, total: usize) -> Self {
        let start = self.start.saturating_sub(CONTEXT_UNITS);
        let end = (self.start + self.len)
            .saturating_add(CONTEXT_UNITS)
            .min(total);
        Self {
            start,
            len: end - start,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct TextSnapshot {
    total: usize,
    selection: TextRange,
    context_start: usize,
    context: Vec<u16>,
}

impl TextSnapshot {
    fn selected_text(&self) -> Result<String, InputError> {
        let start = self.selection.start - self.context_start;
        for boundary in [start, start + self.selection.len] {
            if boundary > 0
                && boundary < self.context.len()
                && (0xD800..=0xDBFF).contains(&self.context[boundary - 1])
                && (0xDC00..=0xDFFF).contains(&self.context[boundary])
            {
                return Err(InputError::new(
                    InputErrorCode::UnsupportedField,
                    "The caret or selection splits a Unicode character; use Copy.",
                ));
            }
        }
        String::from_utf16(&self.context[start..start + self.selection.len]).map_err(|_| {
            InputError::new(
                InputErrorCode::UnsupportedField,
                "The selection splits a Unicode character; use Copy.",
            )
        })
    }

    fn after_insertion(&self, text: &str) -> Result<(usize, usize, Vec<u16>), InputError> {
        let units: Vec<u16> = text.encode_utf16().take(MAX_TEXT_UNITS + 1).collect();
        if units.is_empty() || units.len() > MAX_TEXT_UNITS {
            return Err(InputError::new(InputErrorCode::TooLarge, "Automatic insertion requires 1–16,384 UTF-16 units; the transcript is available for Copy."));
        }
        let start = self.selection.start - self.context_start;
        let mut context = self.context[..start].to_vec();
        context.extend_from_slice(&units);
        context.extend_from_slice(&self.context[start + self.selection.len..]);
        let total = self
            .total
            .checked_sub(self.selection.len)
            .and_then(|n| n.checked_add(units.len()))
            .filter(|n| *n <= isize::MAX as usize)
            .ok_or_else(|| {
                InputError::new(
                    InputErrorCode::TooLarge,
                    "The resulting field length is invalid; use Copy.",
                )
            })?;
        Ok((total, self.selection.start + units.len(), context))
    }
}

fn check_selected_length(length: isize) -> Result<(), InputError> {
    if length < 0 || length as usize > MAX_TEXT_UNITS {
        Err(InputError::new(InputErrorCode::TooLarge, "The selection exceeds 16,384 UTF-16 units or has an invalid length. Select a smaller passage for Ask/edit."))
    } else {
        Ok(())
    }
}

fn check_age(age: Duration, limit: Duration) -> Result<(), InputError> {
    if age > limit {
        Err(InputError::new(InputErrorCode::StaleTarget, "The original caret has expired. Focus the desired field and start dictation again; use Copy for this transcript."))
    } else {
        Ok(())
    }
}

// A late result must not consume a newer recording's target. A matching result
// consumes its target before any native write, so timeouts cannot cause retries.
fn take_target<T>(
    pending: &mut Option<(TargetToken, T)>,
    token: TargetToken,
) -> Result<T, InputError> {
    if pending.as_ref().map(|(id, _)| *id) != Some(token) {
        return Err(InputError::new(
            InputErrorCode::StaleTarget,
            "This dictation target was cancelled, replaced, or already used; use Copy.",
        ));
    }
    Ok(pending.take().expect("matching pending target").1)
}

// The same decision gates both activation and stale-focus validation. Never
// reactivate the target over an unrelated app the user has switched to.
fn should_activate_target(
    front_pid: i32,
    own_pid: i32,
    target_pid: i32,
) -> Result<bool, InputError> {
    if own_pid <= 0 || target_pid <= 0 || own_pid == target_pid {
        return Err(InputError::new(
            InputErrorCode::StaleTarget,
            "The original external application is no longer valid; use Copy.",
        ));
    }
    if front_pid == target_pid {
        return Ok(false);
    }
    if front_pid == own_pid {
        return Ok(true);
    }
    Err(InputError::new(
        InputErrorCode::StaleTarget,
        "You switched to another application during dictation; use Copy to choose the destination.",
    ))
}

fn owns_clipboard(expected: isize, current: isize, marker_matches: bool) -> bool {
    expected == current && marker_matches
}

fn clipboard_access_allowed(behavior: isize) -> bool {
    behavior != 3 // NSPasteboardAccessBehaviorAlwaysDeny; Default/Ask may prompt.
}

fn attributed_range_fallback(status: i32) -> bool {
    // NoValue, AttributeUnsupported, ParameterizedAttributeUnsupported only.
    matches!(status, -25212 | -25205 | -25213)
}

fn can_recapture_inactive_focus(code: InputErrorCode) -> bool {
    matches!(
        code,
        InputErrorCode::MissingTarget | InputErrorCode::UnsupportedField
    )
}

fn terminal_host(bundle: &str, name: &str) -> bool {
    let bundle = bundle.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    [
        "com.apple.terminal",
        "com.googlecode.iterm2",
        "com.mitchellh.ghostty",
        "net.kovidgoyal.kitty",
        "org.alacritty",
        "org.alacritty.alacritty",
        "com.github.wez.wezterm",
        "dev.warp.warp-stable",
        "dev.warp.warp",
        "co.zeit.hyper",
        "com.termius-dmg.mac",
        "org.tabby",
        "com.raphaelamorim.rio",
    ]
    .contains(&bundle.as_str())
        || [
            "terminal",
            "iterm",
            "iterm2",
            "ghostty",
            "kitty",
            "alacritty",
            "wezterm",
            "warp",
            "hyper",
            "termius",
            "tabby",
            "rio",
        ]
        .contains(&name.as_str())
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSApplicationActivationOptions, NSPasteboard, NSPasteboardItem, NSPasteboardTypeString,
        NSPasteboardWriting, NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
        NSWorkspaceDidDeactivateApplicationNotification,
    };
    use objc2_application_services::{
        kAXTrustedCheckOptionPrompt, AXError, AXIsProcessTrusted, AXIsProcessTrustedWithOptions,
        AXUIElement, AXValue, AXValueType,
    };
    use objc2_core_foundation::{
        CFArray, CFAttributedString, CFBoolean, CFDictionary, CFNumber, CFRange, CFRetained,
        CFString, CFType, ConcreteType,
    };
    use objc2_core_graphics::{
        CGEvent, CGEventFlags, CGEventSource, CGEventSourceStateID, CGPreflightPostEventAccess,
        CGRequestPostEventAccess,
    };
    use objc2_foundation::{
        NSArray, NSNotification, NSObjectProtocol, NSOperationQueue, NSString, NSURL,
    };
    use std::cell::RefCell;
    use std::ptr::NonNull;
    use std::time::Instant;

    struct Target {
        process: Retained<NSRunningApplication>,
        application: CFRetained<AXUIElement>,
        field: CFRetained<AXUIElement>,
        snapshot: TextSnapshot,
        captured: Instant,
    }

    struct RememberedSelection {
        process: Retained<NSRunningApplication>,
        captured: Instant,
        text: Result<String, InputError>,
    }

    #[derive(Default)]
    struct State {
        observer: Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
        remembered: Option<Result<Target, InputError>>,
        remembered_selection: Option<RememberedSelection>,
        pending: Option<(TargetToken, Target)>,
        next_id: u64,
    }

    thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }

    struct Budget(Instant);
    impl Budget {
        fn new() -> Self {
            Self(Instant::now())
        }
        fn check(&self) -> Result<(), InputError> {
            if self.0.elapsed() > Duration::from_millis(1500) {
                Err(InputError::new(InputErrorCode::Timeout, "The original app took too long to respond; use Copy and check the original field."))
            } else {
                Ok(())
            }
        }
    }

    fn main_thread() -> Result<MainThreadMarker, InputError> {
        MainThreadMarker::new().ok_or_else(|| {
            InputError::new(
                InputErrorCode::WrongThread,
                "macos_input must be called on Tauri's main thread.",
            )
        })
    }

    /// Read-only, non-prompting. Rechecked for every capture and insertion.
    pub fn accessibility_trusted() -> Result<bool, InputError> {
        main_thread()?;
        // SAFETY: AXIsProcessTrusted has no pointer parameters or side effects.
        Ok(unsafe { AXIsProcessTrusted() })
    }

    /// Non-prompting UI status. Does not acquire or refresh a target.
    pub fn get_insertion_status() -> Result<InsertionStatus, InputError> {
        let trusted = accessibility_trusted()?;
        let target_app = if trusted {
            STATE.with(|state| {
                let state = state.borrow();
                let target = state
                    .pending
                    .as_ref()
                    .map(|(_, target)| (target, SESSION_TTL))
                    .or_else(|| {
                        state
                            .remembered
                            .as_ref()
                            .and_then(|result| result.as_ref().ok())
                            .map(|target| (target, REMEMBER_TTL))
                    });
                target
                    .filter(|(target, ttl)| {
                        !target.process.isTerminated() && target.captured.elapsed() <= *ttl
                    })
                    .and_then(|(target, _)| target.process.localizedName())
                    .map(|name| name.to_string())
            })
        } else {
            None
        };
        Ok(InsertionStatus {
            trusted,
            post_event: CGPreflightPostEventAccess(),
            target_app,
        })
    }

    fn require_trust() -> Result<(), InputError> {
        if accessibility_trusted()? {
            Ok(())
        } else {
            Err(InputError::new(InputErrorCode::PermissionDenied, "請喺系統設定 → 私隱與安全性 → 輔助使用（或 Device Control and Data Access）啟用 GPT-Typeless，再喺目標輸入欄開始錄音"))
        }
    }

    /// Explicit permission UI action only. Prompting is asynchronous; false is
    /// not a denial of a just-shown prompt. Check `accessibility_trusted` later.
    pub fn request_accessibility_permission() -> Result<bool, InputError> {
        main_thread()?;
        // SAFETY: immutable framework constant; dictionary has CFString keys and
        // CFBoolean values as required by AXIsProcessTrustedWithOptions.
        let options = CFDictionary::from_slices(
            &[unsafe { kAXTrustedCheckOptionPrompt }],
            &[CFBoolean::new(true)],
        );
        Ok(unsafe { AXIsProcessTrustedWithOptions(Some(options.as_ref())) })
    }

    /// Called only by the explicit permission action; recheck current state first.
    pub fn request_post_event_access() -> Result<bool, InputError> {
        main_thread()?;
        if !accessibility_trusted()? {
            return Ok(false);
        }
        if CGPreflightPostEventAccess() {
            return Ok(true);
        }
        Ok(CGRequestPostEventAccess())
    }

    /// Explicit settings UI action only. Does not grant Accessibility access.
    pub fn open_accessibility_settings() -> Result<(), InputError> {
        main_thread()?;
        let url = NSURL::URLWithString(&NSString::from_str(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        ))
        .ok_or_else(|| {
            InputError::new(
                InputErrorCode::SettingsUnavailable,
                "Cannot create the Accessibility settings URL.",
            )
        })?;
        if NSWorkspace::sharedWorkspace().openURL(&url) {
            Ok(())
        } else {
            Err(InputError::new(
                InputErrorCode::SettingsUnavailable,
                "請開啟系統設定 → Privacy & Security → Device Control and Data Access（舊版 macOS：Accessibility）。",
            ))
        }
    }

    /// Parent permission-button entry point. The user controls the OS grant.
    /// Returns the current trust result, not the outcome of the asynchronous UI.
    pub fn request_accessibility() -> Result<bool, InputError> {
        let trusted = request_accessibility_permission()?;
        if !trusted {
            open_accessibility_settings()?;
        }
        Ok(trusted)
    }

    /// Install once at setup, including before permission is granted. Does not
    /// prompt or read text until an external app deactivates with trust present.
    /// Uses the outgoing app from the notification, never the now-frontmost app.
    pub fn start_target_tracking() -> Result<(), InputError> {
        main_thread()?;
        if STATE.with(|state| state.borrow().observer.is_some()) {
            return Ok(());
        }
        let block = RcBlock::new(|notification: NonNull<NSNotification>| {
            if MainThreadMarker::new().is_none() {
                return;
            }
            if !STATE.with(|state| state.borrow().observer.is_some()) {
                return;
            }
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.remembered = None;
                state.remembered_selection = None;
            });
            // SAFETY: notification is valid for the duration of this callback.
            let app = unsafe { notification.as_ref() }
                .userInfo()
                .and_then(|info| info.objectForKey(unsafe { NSWorkspaceApplicationKey }))
                .and_then(|object| object.downcast::<NSRunningApplication>().ok());
            if let Some(app) = app {
                if app.processIdentifier() == std::process::id() as i32 {
                    return;
                }
                // No permission prompts, logging, or retry loop in the tracker.
                let budget = Budget::new();
                let text = require_trust().and_then(|()| read_selected_from_app(&app, &budget));
                let selection = RememberedSelection {
                    process: app.clone(),
                    captured: Instant::now(),
                    text,
                };
                let target = require_trust().and_then(|()| capture_app(app, &budget));
                STATE.with(|state| {
                    let mut state = state.borrow_mut();
                    state.remembered = Some(target);
                    state.remembered_selection = Some(selection);
                });
            }
        });
        // SAFETY: the block captures no thread-bound objects and is sendable.
        // Main queue confines native work/state to the main thread. NSWorkspace
        // owns this notification name, and the center copies/retains the block.
        let observer = unsafe {
            NSWorkspace::sharedWorkspace()
                .notificationCenter()
                .addObserverForName_object_queue_usingBlock(
                    Some(NSWorkspaceDidDeactivateApplicationNotification),
                    None,
                    Some(&NSOperationQueue::mainQueue()),
                    &block,
                )
        };
        STATE.with(|state| state.borrow_mut().observer = Some(observer));
        Ok(())
    }

    /// Stop the observer and discard all captured text/handles, e.g. on disable.
    pub fn stop_target_tracking() -> Result<(), InputError> {
        main_thread()?;
        let observer = STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.remembered = None;
            state.remembered_selection = None;
            state.pending = None;
            state.observer.take()
        });
        if let Some(observer) = observer {
            // SAFETY: token belongs to this notification center and is retained.
            unsafe {
                NSWorkspace::sharedWorkspace()
                    .notificationCenter()
                    .removeObserver((*observer).as_ref())
            };
        }
        Ok(())
    }

    /// Capture immediately when recording starts. Invalidates any older session
    /// even if this capture fails. Call before any show/set_focus of our window.
    pub fn capture_target() -> Result<TargetToken, InputError> {
        main_thread()?;
        STATE.with(|state| state.borrow_mut().pending = None);
        require_trust()?;
        let budget = Budget::new();
        let front = NSWorkspace::sharedWorkspace().frontmostApplication()
            .ok_or_else(|| InputError::new(InputErrorCode::MissingTarget, "No external application is focused. Focus an editable field and start dictation again."))?;
        let mut target = if front.processIdentifier() == std::process::id() as i32 {
            let (remembered, recent) = STATE.with(|state| {
                let mut state = state.borrow_mut();
                let recent = state
                    .remembered_selection
                    .as_ref()
                    .map(|selection| (selection.process.clone(), selection.captured));
                (state.remembered.take(), recent)
            });
            match remembered {
                Some(Ok(target)) => {
                    check_age(target.captured.elapsed(), REMEMBER_TTL)?;
                    activate_target_if_needed(&target, &budget)?;
                    target
                }
                failed => {
                    if let Some(Err(error)) = failed {
                        if !can_recapture_inactive_focus(error.code) {
                            return Err(error);
                        }
                    }
                    let (process, captured) = recent.ok_or_else(|| InputError::new(InputErrorCode::MissingTarget, "No recent external caret was captured. Focus the destination field, then use the recording shortcut or return to the record button within 30 seconds."))?;
                    check_age(captured.elapsed(), REMEMBER_TTL)?;
                    // Some native apps withdraw AXFocusedUIElement as soon as
                    // they deactivate. Restore only that recent app, then take
                    // the initial snapshot while its field is active again.
                    restore_recent_app(&process, &budget)?;
                    capture_app(process, &budget)?
                }
            }
        } else {
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.remembered = None;
                state.remembered_selection = None;
            });
            capture_app(front, &budget)?
        };
        target.captured = Instant::now();
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.next_id = state.next_id.checked_add(1).ok_or_else(|| {
                InputError::new(
                    InputErrorCode::StaleTarget,
                    "The input session counter is exhausted; restart the app.",
                )
            })?;
            let token = TargetToken(state.next_id);
            state.pending = Some((token, target));
            Ok(token)
        })
    }

    /// The captured selection only (empty at a caret). No new text is read.
    /// Obtain at capture time for provider prompts; insertion still revalidates.
    pub fn selected_text(token: TargetToken) -> Result<String, InputError> {
        main_thread()?;
        require_trust()?;
        STATE.with(|state| {
            let state = state.borrow();
            let (_, target) = state
                .pending
                .as_ref()
                .filter(|(id, _)| *id == token)
                .ok_or_else(|| {
                    InputError::new(
                        InputErrorCode::StaleTarget,
                        "The selected-text session is no longer available.",
                    )
                })?;
            check_age(target.captured.elapsed(), SESSION_TTL)?;
            target.snapshot.selected_text()
        })
    }

    /// Captured app label for the UI; does not read any field text.
    pub fn target_app_name(token: TargetToken) -> Result<String, InputError> {
        main_thread()?;
        STATE.with(|state| {
            let state = state.borrow();
            let (_, target) = state
                .pending
                .as_ref()
                .filter(|(id, _)| *id == token)
                .ok_or_else(|| {
                    InputError::new(
                        InputErrorCode::StaleTarget,
                        "The insertion target is no longer available.",
                    )
                })?;
            check_age(target.captured.elapsed(), SESSION_TTL)?;
            if target.process.isTerminated() {
                return Err(InputError::new(
                    InputErrorCode::StaleTarget,
                    "The original application has exited.",
                ));
            }
            Ok(target
                .process
                .localizedName()
                .map(|name| name.to_string())
                .unwrap_or_else(|| "Original application".into()))
        })
    }

    /// Ask/read-only mode: AXSelectedText only, bounded to 16,384 UTF-16 units.
    /// Does not require settable text, a caret/range, or a successful capture_target.
    /// If our app is frontmost, uses the recent deactivation selection snapshot.
    /// Empty selection is returned as an empty string when the provider exposes it.
    pub fn capture_selected_text() -> Result<String, InputError> {
        main_thread()?;
        require_trust()?;
        let front = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .ok_or_else(|| {
                InputError::new(
                    InputErrorCode::MissingTarget,
                    "Focus and select text in another application first.",
                )
            })?;
        if front.processIdentifier() != std::process::id() as i32 {
            return read_selected_from_app(&front, &Budget::new());
        }
        let (recent, cached) = STATE.with(|state| {
            let state = state.borrow();
            if let Some((_, target)) = state.pending.as_ref() {
                check_age(target.captured.elapsed(), SESSION_TTL)?;
                if !target.process.isTerminated() {
                    return Ok((None, target.snapshot.selected_text()));
                }
            }
            let selection = state.remembered_selection.as_ref().ok_or_else(|| InputError::new(InputErrorCode::MissingTarget, "No recent external selection was captured. Select text in the source app, then return within 30 seconds."))?;
            check_age(selection.captured.elapsed(), REMEMBER_TTL)?;
            if selection.process.isTerminated() {
                return Err(InputError::new(InputErrorCode::StaleTarget, "The source application has exited."));
            }
            Ok((Some(selection.process.clone()), selection.text.clone()))
        })?;
        match cached {
            Ok(text) => Ok(text),
            Err(error) if can_recapture_inactive_focus(error.code) => {
                let process = recent.ok_or(error)?;
                let budget = Budget::new();
                restore_recent_app(&process, &budget)?;
                read_selected_from_app(&process, &budget)
            }
            Err(error) => Err(error),
        }
    }

    pub fn cancel_target(token: TargetToken) -> Result<(), InputError> {
        main_thread()?;
        STATE
            .with(|state| take_target(&mut state.borrow_mut().pending, token))
            .map(drop)
    }

    /// Consume one target; native paste is selected before ANY text write.
    /// If our app is frontmost, return focus to the validated original app first.
    /// Posts one Cmd-V pair; never retries after posting or uses an AX text setter.
    pub fn insert_text(token: TargetToken, text: &str) -> Result<(), InsertionFailure> {
        let mut may_have_inserted = false;
        let result = (|| {
            main_thread()?;
            let target = STATE.with(|state| take_target(&mut state.borrow_mut().pending, token))?;
            require_trust()?;
            let budget = Budget::new();
            check_age(target.captured.elapsed(), SESSION_TTL)?;
            let (expected_total, expected_caret, expected_context) =
                target.snapshot.after_insertion(text)?;
            reject_terminal(&target.process)?;
            activate_target_if_needed(&target, &budget)?;
            if !CGPreflightPostEventAccess() {
                return Err(InputError::new(InputErrorCode::PermissionDenied, "macOS 尚未允許自動貼入。請喺 Privacy & Security → Device Control and Data Access 啟用 GPT-Typeless，再喺目標輸入欄重試。文字已保留。"));
            }
            // Create both events before touching the pasteboard. Private source
            // avoids inheriting held shortcut modifiers. Virtual key 9 is V.
            let source = CGEventSource::new(CGEventSourceStateID::Private)
                .ok_or_else(|| clipboard_error("Cannot create a native paste event source."))?;
            let down = CGEvent::new_keyboard_event(Some(&source), 9, true)
                .ok_or_else(|| clipboard_error("Cannot create the native paste key-down."))?;
            let up = CGEvent::new_keyboard_event(Some(&source), 9, false)
                .ok_or_else(|| clipboard_error("Cannot create the native paste key-up."))?;
            CGEvent::set_flags(Some(&down), CGEventFlags::MaskCommand);
            CGEvent::set_flags(Some(&up), CGEventFlags::MaskCommand);
            let mut clipboard = ClipboardLease::publish(text, token, &budget)?;
            let pasted = (|| {
                // Materializing promised clipboard formats can take time; check
                // the original field/context again immediately before the event.
                require_trust()?;
                validate_target(&target, &budget)?;
                if frontmost_pid(&target, &budget)? != target.process.processIdentifier()
                    || !clipboard.is_owned()
                {
                    return Err(clipboard_error("Focus or clipboard ownership changed before native paste. No text was inserted."));
                }
                may_have_inserted = true;
                CGEvent::post_to_pid(target.process.processIdentifier(), Some(&down));
                CGEvent::post_to_pid(target.process.processIdentifier(), Some(&up));
                // No event retry. Only AX reads poll while the host processes
                // paste/input asynchronously. Keep temporary text until verified
                // or the confirmation budget is exhausted.
                confirm_paste(&target, expected_total, expected_caret, &expected_context)
            })();
            let restored = clipboard.restore();
            match (pasted, restored) {
                (Err(mut error), Err(restore)) => {
                    error.message.push_str(&format!(
                        " Clipboard restoration also failed: {}",
                        restore.message
                    ));
                    Err(error)
                }
                (Err(error), _) | (_, Err(error)) => Err(error),
                (Ok(()), Ok(())) => Ok(()),
            }
        })();
        result.map_err(|error| InsertionFailure {
            error,
            transcript: text.to_owned(),
            may_have_inserted,
        })
    }

    fn clipboard_error(message: &str) -> InputError {
        InputError::new(
            InputErrorCode::Accessibility,
            format!("{message} The transcript remains available for Copy."),
        )
    }

    struct ClipboardLease {
        board: Retained<NSPasteboard>,
        original: Retained<NSArray<ProtocolObject<dyn NSPasteboardWriting>>>,
        owned_count: isize,
        marker: Retained<NSString>,
        finished: bool,
    }

    impl ClipboardLease {
        fn publish(text: &str, token: TargetToken, budget: &Budget) -> Result<Self, InputError> {
            let check_budget = || {
                budget.check().map_err(|_| InputError::new(InputErrorCode::Timeout,
                "Clipboard permission/preservation took too long. The clipboard was left unchanged. After answering the OS prompt, retry dictation; this transcript remains available for Copy."))
            };
            check_budget()?;
            let board = NSPasteboard::generalPasteboard();
            // The selector is available on macOS 15.4+. Default/Ask allow the
            // normal system-controlled prompt from this explicit user action.
            if board.respondsToSelector(objc2::sel!(accessBehavior))
                && !clipboard_access_allowed(board.accessBehavior().0)
            {
                return Err(clipboard_error("Clipboard access is set to Always Deny. Allow GPT-Typeless to paste from other apps in System Settings, then retry automatic insertion."));
            }
            let before = board.changeCount();
            let mut originals: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = Vec::new();
            let mut bytes = 0usize;
            if let Some(items) = board.pasteboardItems() {
                if items.len() > 32 {
                    return Err(clipboard_error(
                        "The clipboard has too many items to preserve safely.",
                    ));
                }
                for item_index in 0..items.len() {
                    let item = items.objectAtIndex(item_index);
                    check_budget()?;
                    let copy = NSPasteboardItem::new();
                    let types = item.types();
                    if types.len() > 64 {
                        return Err(clipboard_error(
                            "The clipboard has too many formats to preserve safely.",
                        ));
                    }
                    for type_index in 0..types.len() {
                        let kind = types.objectAtIndex(type_index);
                        check_budget()?;
                        let data = item.dataForType(&kind).ok_or_else(|| clipboard_error("A clipboard format could not be preserved; the clipboard was left unchanged."))?;
                        bytes = bytes
                            .checked_add(data.length())
                            .filter(|n| *n <= 32 * 1024 * 1024)
                            .ok_or_else(|| {
                                clipboard_error(
                                    "The clipboard exceeds the 32 MiB preservation limit.",
                                )
                            })?;
                        if !copy.setData_forType(&data, &kind) {
                            return Err(clipboard_error("Cannot preserve a clipboard format."));
                        }
                    }
                    originals.push(ProtocolObject::from_retained(copy));
                }
            } else {
                // A nil result may mean access was declined, not an empty board.
                // Only an explicit empty type list is safe to preserve as empty.
                if !board.types().is_some_and(|types| types.is_empty()) {
                    return Err(clipboard_error("The clipboard contents could not be enumerated safely. Allow the OS clipboard request, then retry."));
                }
            }
            if board.respondsToSelector(objc2::sel!(accessBehavior))
                && !clipboard_access_allowed(board.accessBehavior().0)
            {
                return Err(clipboard_error(
                    "Clipboard access was denied; the clipboard was left unchanged.",
                ));
            }
            let original = NSArray::from_retained_slice(&originals);
            let marker = NSString::from_str(&format!("{}:{}", std::process::id(), token.0));
            let temporary = NSPasteboardItem::new();
            if !temporary
                .setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString })
                || !temporary.setString_forType(
                    &marker,
                    &NSString::from_str("com.gpt-typeless.temporary-paste"),
                )
            {
                return Err(clipboard_error("Cannot prepare temporary paste text."));
            }
            check_budget()?;
            if board.changeCount() != before {
                return Err(clipboard_error(
                    "The clipboard changed during preservation; no paste was attempted.",
                ));
            }
            let owned_count = board.clearContents();
            let mut lease = Self {
                board,
                original,
                owned_count,
                marker,
                finished: false,
            };
            // Guard the gap between clear and publish as well as restoration.
            if lease.board.changeCount() != owned_count {
                lease.finished = true;
                return Err(clipboard_error(
                    "The clipboard changed before temporary paste text could be published.",
                ));
            }
            let objects = NSArray::from_slice(&[ProtocolObject::from_ref(&*temporary)]);
            if !lease.board.writeObjects(&objects) {
                // We still own the cleared board. Restore without requiring a
                // marker that a failed write may not have installed.
                if lease.board.changeCount() == owned_count {
                    lease.restore_original()?;
                }
                lease.finished = true;
                return Err(clipboard_error("Cannot publish temporary paste text."));
            }
            lease.owned_count = lease.board.changeCount();
            if !lease.is_owned() {
                return Err(clipboard_error("Clipboard ownership changed before paste."));
            }
            Ok(lease)
        }

        fn is_owned(&self) -> bool {
            let count = self.board.changeCount();
            if count != self.owned_count {
                return false;
            }
            let marker = self
                .board
                .stringForType(&NSString::from_str("com.gpt-typeless.temporary-paste"));
            owns_clipboard(
                self.owned_count,
                self.board.changeCount(),
                marker.is_some_and(|value| *value == *self.marker),
            )
        }

        fn restore_original(&self) -> Result<(), InputError> {
            let cleared = self.board.clearContents();
            if self.board.changeCount() != cleared {
                return Ok(());
            }
            if !self.original.is_empty() && !self.board.writeObjects(&self.original) {
                return Err(clipboard_error(
                    "The original clipboard formats could not be restored.",
                ));
            }
            Ok(())
        }

        fn restore(&mut self) -> Result<(), InputError> {
            if self.finished {
                return Ok(());
            }
            self.finished = true;
            if self.is_owned() {
                self.restore_original()?;
            }
            Ok(())
        }
    }

    impl Drop for ClipboardLease {
        fn drop(&mut self) {
            let _ = self.restore();
        }
    }

    fn confirm_paste(
        target: &Target,
        total: usize,
        caret: usize,
        context: &[u16],
    ) -> Result<(), InputError> {
        let budget = Budget::new();
        loop {
            budget.check()?;
            require_trust()?;
            if frontmost_pid(target, &budget)? != target.process.processIdentifier()
                || *focused_field(&target.application, &budget)? != *target.field
            {
                return Err(InputError::new(InputErrorCode::VerificationFailed, "Focus changed after native paste. Check the original field before using Copy; do not automatically retry."));
            }
            let verified: Result<bool, InputError> = (|| {
                let after = snapshot(&target.field, &budget)?;
                if after.total != total
                    || after.selection
                        != (TextRange {
                            start: caret,
                            len: 0,
                        })
                {
                    return Ok(false);
                }
                Ok(read_text_range(
                    &target.field,
                    TextRange {
                        start: target.snapshot.context_start,
                        len: context.len(),
                    },
                    &budget,
                )? == context)
            })();
            match verified {
                Ok(true) => return Ok(()),
                Err(error)
                    if matches!(
                        error.code,
                        InputErrorCode::PermissionDenied | InputErrorCode::SecureField
                    ) =>
                {
                    return Err(error)
                }
                _ => {}
            }
            if budget.0.elapsed() >= Duration::from_millis(1200) {
                return Err(InputError::new(InputErrorCode::VerificationFailed, "Native paste was posted once but the app did not confirm the complete text in time. Check the original field before using Copy; do not automatically retry."));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn reject_terminal(process: &NSRunningApplication) -> Result<(), InputError> {
        if terminal_host(
            &process
                .bundleIdentifier()
                .map(|s| s.to_string())
                .unwrap_or_default(),
            &process
                .localizedName()
                .map(|s| s.to_string())
                .unwrap_or_default(),
        ) {
            Err(InputError::new(InputErrorCode::UnsupportedField, "Automatic paste into terminal hosts is disabled; use Copy and choose the destination manually."))
        } else {
            Ok(())
        }
    }

    fn ax_result(error: AXError, operation: &str) -> Result<(), InputError> {
        let code = match error {
            AXError::Success => return Ok(()),
            AXError::APIDisabled => InputErrorCode::PermissionDenied,
            AXError::InvalidUIElement => InputErrorCode::StaleTarget,
            AXError::CannotComplete => InputErrorCode::Timeout,
            AXError::AttributeUnsupported
            | AXError::ParameterizedAttributeUnsupported
            | AXError::NotImplemented => InputErrorCode::UnsupportedField,
            AXError::NoValue => InputErrorCode::MissingTarget,
            _ => InputErrorCode::Accessibility,
        };
        Err(InputError::new(code, format!("Cannot {operation} in the original field (AX {}). Focus an editable field and retry dictation, or use Copy for this transcript.", error.0)))
    }

    fn prepare(element: &AXUIElement, budget: &Budget) -> Result<(), InputError> {
        budget.check()?;
        // SAFETY: valid retained element; per-object timeout avoids changing
        // global Accessibility timeouts for the app or unrelated modules.
        ax_result(
            unsafe { element.set_messaging_timeout(0.150) },
            "bound the Accessibility timeout",
        )
    }

    fn attribute(
        element: &AXUIElement,
        name: &'static str,
        budget: &Budget,
    ) -> Result<CFRetained<CFType>, InputError> {
        budget.check()?;
        let mut value = std::ptr::null();
        // SAFETY: initialized out-pointer; Copy returns a +1 retained CF object.
        ax_result(
            unsafe {
                element.copy_attribute_value(
                    &CFString::from_static_str(name),
                    NonNull::from(&mut value),
                )
            },
            name,
        )?;
        let value = NonNull::new(value.cast_mut()).ok_or_else(|| {
            InputError::new(
                InputErrorCode::UnsupportedField,
                format!("The original field returned no {name}; use Copy."),
            )
        })?;
        Ok(unsafe { CFRetained::from_raw(value) })
    }

    fn typed<T: ConcreteType>(value: CFRetained<CFType>) -> Result<CFRetained<T>, InputError> {
        value.downcast().map_err(|_| {
            InputError::new(
                InputErrorCode::UnsupportedField,
                "The original field returned an unsupported Accessibility value; use Copy.",
            )
        })
    }

    fn optional_attribute(
        element: &AXUIElement,
        name: &'static str,
        budget: &Budget,
    ) -> Result<Option<CFRetained<CFType>>, InputError> {
        // Unsupported/missing optional attributes are ordinary for native fields;
        // timeouts and revoked permission must still fail closed.
        match attribute(element, name, budget) {
            Ok(value) => Ok(Some(value)),
            Err(error)
                if matches!(
                    error.code,
                    InputErrorCode::MissingTarget | InputErrorCode::UnsupportedField
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn nonsecure(field: &AXUIElement, budget: &Budget) -> Result<(), InputError> {
        if let Some(subrole) = optional_attribute(field, "AXSubrole", budget)? {
            if *typed::<CFString>(subrole)? == *CFString::from_static_str("AXSecureTextField") {
                return Err(InputError::new(InputErrorCode::SecureField, "Password/secure fields cannot be read or edited automatically. The transcript remains available for Copy."));
            }
        }
        Ok(())
    }

    fn editable(field: &AXUIElement, budget: &Budget) -> Result<(), InputError> {
        nonsecure(field, budget)?;
        let role = typed::<CFString>(attribute(field, "AXRole", budget)?)?;
        if !["AXTextField", "AXTextArea", "AXComboBox"]
            .iter()
            .any(|name| *role == *CFString::from_str(name))
        {
            return Err(InputError::new(
                InputErrorCode::UnsupportedField,
                "The original focus is not a supported editable text field; use Copy.",
            ));
        }
        if let Some(enabled) = optional_attribute(field, "AXEnabled", budget)? {
            if !typed::<CFBoolean>(enabled)?.as_bool() {
                return Err(InputError::new(
                    InputErrorCode::UnsupportedField,
                    "The original text field is disabled; use Copy.",
                ));
            }
        }
        for name in ["AXValue", "AXSelectedText"] {
            budget.check()?;
            let mut settable = 0;
            // SAFETY: initialized Boolean output; this queries writability only,
            // never reads or replaces the field's AXValue/document text.
            let status = unsafe {
                field.is_attribute_settable(
                    &CFString::from_static_str(name),
                    NonNull::from(&mut settable),
                )
            };
            if status == AXError::Success && settable != 0 {
                return Ok(());
            }
            if !matches!(
                status,
                AXError::Success
                    | AXError::AttributeUnsupported
                    | AXError::NoValue
                    | AXError::NotImplemented
            ) {
                ax_result(status, "check text-field writability")?;
            }
        }
        Err(InputError::new(
            InputErrorCode::UnsupportedField,
            "The original field did not advertise writable text; use Copy.",
        ))
    }

    fn read_selected_from_app(
        process: &NSRunningApplication,
        budget: &Budget,
    ) -> Result<String, InputError> {
        if process.isTerminated()
            || process.processIdentifier() <= 0
            || process.processIdentifier() == std::process::id() as i32
        {
            return Err(InputError::new(
                InputErrorCode::MissingTarget,
                "Focus and select text in another application first.",
            ));
        }
        // SAFETY: PID is supplied by the retained running application, on main.
        // SAFETY: retained running process, verified live, on the main thread.
        let application = unsafe { AXUIElement::new_application(process.processIdentifier()) };
        prepare(&application, budget)?;
        let field = focused_field(&application, budget)?;
        nonsecure(&field, budget)?;
        // Only selected text; no AXValue, document content, character count,
        // selected range or writable-attribute query in this read-only path.
        let selected = typed::<CFString>(attribute(&field, "AXSelectedText", budget)?)?;
        check_selected_length(selected.length())?;
        Ok(selected.to_string())
    }

    fn ax_range(value: CFRetained<CFType>) -> Result<CFRange, InputError> {
        let value = typed::<AXValue>(value)?;
        let mut range = CFRange {
            location: 0,
            length: 0,
        };
        // SAFETY: output points to correctly sized/aligned CFRange storage.
        if !unsafe { value.value(AXValueType::CFRange, NonNull::from(&mut range).cast()) } {
            return Err(InputError::new(
                InputErrorCode::UnsupportedField,
                "The original field did not expose a text caret; use Copy.",
            ));
        }
        Ok(range)
    }

    fn read_text_range(
        field: &AXUIElement,
        range: TextRange,
        budget: &Budget,
    ) -> Result<Vec<u16>, InputError> {
        budget.check()?;
        if range.len > MAX_TEXT_UNITS + 2 * CONTEXT_UNITS {
            return Err(InputError::new(
                InputErrorCode::TooLarge,
                "The caret context is too large; use Copy.",
            ));
        }
        if range.len == 0 {
            return Ok(Vec::new());
        }
        let mut native = CFRange {
            location: range.start as isize,
            length: range.len as isize,
        };
        // SAFETY: valid CFRange input; AXValue copies it synchronously.
        let parameter =
            unsafe { AXValue::new(AXValueType::CFRange, NonNull::from(&mut native).cast()) }
                .ok_or_else(|| {
                    InputError::new(
                        InputErrorCode::Accessibility,
                        "Cannot encode the original caret range; use Copy.",
                    )
                })?;
        let mut result = std::ptr::null();
        // SAFETY: both endpoints take the SAME bounded CFRange AXValue and
        // return a +1 CF object. Neither reads the whole field's AXValue.
        let status = unsafe {
            field.copy_parameterized_attribute_value(
                &CFString::from_static_str("AXStringForRange"),
                parameter.as_ref(),
                NonNull::from(&mut result),
            )
        };
        let string = if attributed_range_fallback(status.0) {
            // Release any unexpected result accompanying an error before retry.
            if let Some(value) = NonNull::new(result.cast_mut()) {
                drop(unsafe { CFRetained::from_raw(value) });
            }
            result = std::ptr::null();
            budget.check()?;
            ax_result(
                unsafe {
                    field.copy_parameterized_attribute_value(
                        &CFString::from_static_str("AXAttributedStringForRange"),
                        parameter.as_ref(),
                        NonNull::from(&mut result),
                    )
                },
                "read bounded attributed caret context",
            )?;
            let result = NonNull::new(result.cast_mut()).ok_or_else(|| {
                clipboard_error("The original field returned no attributed caret context.")
            })?;
            let attributed = typed::<CFAttributedString>(unsafe { CFRetained::from_raw(result) })?;
            if attributed.length() != range.len as isize {
                return Err(InputError::new(InputErrorCode::StaleTarget, "The bounded attributed text length did not match the original range; use Copy."));
            }
            attributed.string().ok_or_else(|| {
                clipboard_error("The original field returned no attributed text string.")
            })?
        } else {
            ax_result(status, "read bounded caret context")?;
            let result = NonNull::new(result.cast_mut()).ok_or_else(|| {
                InputError::new(
                    InputErrorCode::UnsupportedField,
                    "The field returned no caret context; use Copy.",
                )
            })?;
            typed::<CFString>(unsafe { CFRetained::from_raw(result) })?
        };
        if string.length() != range.len as isize {
            return Err(InputError::new(InputErrorCode::StaleTarget, "The original text changed while capturing the caret, or the app uses unsupported text offsets; use Copy."));
        }
        let mut units = vec![0; range.len];
        // SAFETY: exact CFString length and writable output allocation checked above.
        unsafe {
            string.characters(
                CFRange {
                    location: 0,
                    length: range.len as isize,
                },
                units.as_mut_ptr(),
            )
        };
        Ok(units)
    }

    fn snapshot(field: &AXUIElement, budget: &Budget) -> Result<TextSnapshot, InputError> {
        // Security checks precede ANY text read, at capture and completion.
        editable(field, budget)?;
        let total = typed::<CFNumber>(attribute(field, "AXNumberOfCharacters", budget)?)?
            .as_isize()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| {
                InputError::new(
                    InputErrorCode::UnsupportedField,
                    "The field did not expose a valid character count; use Copy.",
                )
            })?;
        let native = ax_range(attribute(field, "AXSelectedTextRange", budget)?)?;
        let selection = TextRange::checked(native.location, native.length, total)?;
        if let Some(ranges) = optional_attribute(field, "AXSelectedTextRanges", budget)? {
            if typed::<CFArray>(ranges)?.len() > 1 {
                return Err(InputError::new(
                    InputErrorCode::UnsupportedField,
                    "Multiple text selections cannot be replaced automatically; use Copy.",
                ));
            }
        }
        let context_range = selection.context(total);
        let context = read_text_range(field, context_range, budget)?;
        let snapshot = TextSnapshot {
            total,
            selection,
            context_start: context_range.start,
            context,
        };
        snapshot.selected_text()?; // Reject a selection splitting a surrogate pair.
        Ok(snapshot)
    }

    fn focused_field(
        application: &AXUIElement,
        budget: &Budget,
    ) -> Result<CFRetained<AXUIElement>, InputError> {
        let field = typed(attribute(application, "AXFocusedUIElement", budget)?)?;
        prepare(&field, budget)?;
        Ok(field)
    }

    fn capture_app(
        process: Retained<NSRunningApplication>,
        budget: &Budget,
    ) -> Result<Target, InputError> {
        reject_terminal(&process)?;
        if process.isTerminated()
            || process.processIdentifier() <= 0
            || process.processIdentifier() == std::process::id() as i32
        {
            return Err(InputError::new(
                InputErrorCode::MissingTarget,
                "Focus a text field in another application before starting dictation.",
            ));
        }
        // SAFETY: PID is from the retained NSRunningApplication. Holding that
        // object and checking isTerminated later prevents reuse of an old PID.
        let application = unsafe { AXUIElement::new_application(process.processIdentifier()) };
        prepare(&application, budget)?;
        let field = focused_field(&application, budget)?;
        let mut pid = 0;
        // SAFETY: valid element and initialized pid_t output.
        ax_result(
            unsafe { field.pid(NonNull::from(&mut pid)) },
            "identify the focused field",
        )?;
        if pid != process.processIdentifier() {
            return Err(InputError::new(
                InputErrorCode::UnsupportedField,
                "The focused field belongs to a different process; use Copy.",
            ));
        }
        let snapshot = snapshot(&field, budget)?;
        Ok(Target {
            process,
            application,
            field,
            snapshot,
            captured: Instant::now(),
        })
    }

    // NSRunningApplication.isActive can remain cached until a main-run-loop turn.
    // Confirm with AXFrontmost on the already timeout-bounded original app, and
    // keep NSWorkspace's unrelated-app guard. Do not set a system-wide AX timeout:
    // that would change the timeout for unrelated Accessibility code too.
    fn frontmost_pid(target: &Target, budget: &Budget) -> Result<i32, InputError> {
        frontmost_app_pid(&target.process, &target.application, budget)
    }

    fn frontmost_app_pid(
        process: &NSRunningApplication,
        application: &AXUIElement,
        budget: &Budget,
    ) -> Result<i32, InputError> {
        let front_pid = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .ok_or_else(|| {
                InputError::new(
                    InputErrorCode::StaleTarget,
                    "The active application is no longer available; use Copy.",
                )
            })?
            .processIdentifier();
        let target_pid = process.processIdentifier();
        should_activate_target(front_pid, std::process::id() as i32, target_pid)?;
        let active = typed::<CFBoolean>(attribute(application, "AXFrontmost", budget)?)?.as_bool();
        if active {
            return Ok(target_pid);
        }
        if front_pid == target_pid {
            return Err(InputError::new(
                InputErrorCode::StaleTarget,
                "The original app's active status could not be confirmed; use Copy.",
            ));
        }
        Ok(front_pid)
    }

    // Inactive applications may expose no AXFocusedUIElement. Activate only
    // from our own foreground app, then validate the original snapshot before
    // touching text or the clipboard. An unrelated foreground app still aborts.
    fn activate_target_if_needed(target: &Target, budget: &Budget) -> Result<(), InputError> {
        activate_app_if_needed(&target.process, &target.application, budget)?;
        require_trust()?;
        validate_target(target, budget)
    }

    fn restore_recent_app(
        process: &NSRunningApplication,
        budget: &Budget,
    ) -> Result<(), InputError> {
        reject_terminal(process)?;
        require_trust()?;
        if process.isTerminated() || process.processIdentifier() <= 0 {
            return Err(InputError::new(
                InputErrorCode::StaleTarget,
                "The recent input application has exited; focus the destination again.",
            ));
        }
        let application = unsafe { AXUIElement::new_application(process.processIdentifier()) };
        prepare(&application, budget)?;
        activate_app_if_needed(process, &application, budget)
    }

    fn activate_app_if_needed(
        process: &NSRunningApplication,
        application: &AXUIElement,
        budget: &Budget,
    ) -> Result<(), InputError> {
        let own_pid = std::process::id() as i32;
        let target_pid = process.processIdentifier();
        if !should_activate_target(
            frontmost_app_pid(process, application, budget)?,
            own_pid,
            target_pid,
        )? {
            return Ok(());
        }
        budget.check()?;
        // Empty options cooperate with the OS and never force activation over a
        // newer unrelated foreground app. Only the retained original app is used.
        if !process.activateWithOptions(NSApplicationActivationOptions::empty()) {
            return Err(InputError::new(
                InputErrorCode::Accessibility,
                "macOS declined to activate the original app. No text was inserted; use Copy.",
            ));
        }
        let requested = Instant::now();
        loop {
            budget.check()?;
            if !should_activate_target(
                frontmost_app_pid(process, application, budget)?,
                own_pid,
                target_pid,
            )? {
                return Ok(());
            }
            if requested.elapsed() >= Duration::from_millis(250) {
                return Err(InputError::new(InputErrorCode::Timeout, "The original app did not become active in time. No text was inserted; use Copy."));
            }
            // Brief bounded wait only; no run-loop pumping/reentrant callbacks.
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn validate_target(target: &Target, budget: &Budget) -> Result<(), InputError> {
        reject_terminal(&target.process)?;
        if target.process.isTerminated() {
            return Err(InputError::new(
                InputErrorCode::StaleTarget,
                "The original application has exited; use Copy.",
            ));
        }
        should_activate_target(
            frontmost_pid(target, budget)?,
            std::process::id() as i32,
            target.process.processIdentifier(),
        )?;
        let focused = focused_field(&target.application, budget)?;
        if *focused != *target.field || snapshot(&target.field, budget)? != target.snapshot {
            return Err(InputError::new(InputErrorCode::StaleTarget, "The original field, caret, selection, or nearby text changed; use Copy instead of replacing the new selection."));
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub use platform::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn state(text: &str, start: usize, len: usize) -> TextSnapshot {
        let units: Vec<u16> = text.encode_utf16().collect();
        let selection = TextRange::checked(start as isize, len as isize, units.len()).unwrap();
        let context = selection.context(units.len());
        TextSnapshot {
            total: units.len(),
            selection,
            context_start: context.start,
            context: units[context.start..context.start + context.len].to_vec(),
        }
    }

    #[test]
    fn preserves_unicode_and_replaces_only_original_selection() {
        let captured = state("前😀後", 1, 2);
        assert_eq!(captured.selected_text().unwrap(), "😀");
        let (total, caret, context) = captured.after_insertion("你好🌍").unwrap();
        assert_eq!((total, caret), (6, 5));
        assert_eq!(String::from_utf16(&context).unwrap(), "前你好🌍後");
        let caret = state("a😀b", 3, 0);
        assert_eq!(caret.selected_text().unwrap(), "");
        assert_eq!(
            String::from_utf16(&caret.after_insertion("\nhello").unwrap().2).unwrap(),
            "a😀\nhellob"
        );
    }

    #[test]
    fn rejects_negative_overflow_and_out_of_bounds_ranges() {
        for (start, len, total) in [
            (-1, 0, 10),
            (0, -1, 10),
            (9, 2, 10),
            (isize::MAX, isize::MAX, 10),
        ] {
            assert!(TextRange::checked(start, len, total).is_err());
        }
        assert!(state("😀", 1, 1).selected_text().is_err());
        assert!(state("😀", 1, 0).selected_text().is_err());
    }

    #[test]
    fn bounds_selection_output_and_context_without_reading_document() {
        assert_eq!(
            TextRange {
                start: 100_000,
                len: 0
            }
            .context(1_000_000),
            TextRange {
                start: 99_936,
                len: 128
            }
        );
        assert_eq!(
            TextRange { start: 0, len: 0 }.context(0),
            TextRange { start: 0, len: 0 }
        );
        let captured = state("", 0, 0);
        assert!(captured.after_insertion("").is_err());
        assert!(captured
            .after_insertion(&"a".repeat(MAX_TEXT_UNITS))
            .is_ok());
        assert!(captured
            .after_insertion(&"😀".repeat(MAX_TEXT_UNITS / 2 + 1))
            .is_err());
        assert!(TextRange::checked(0, MAX_TEXT_UNITS as isize + 1, MAX_TEXT_UNITS + 1).is_err());
    }

    #[test]
    fn detects_same_length_nearby_edits_caret_moves_and_selection_changes() {
        let captured = state("hello there", 5, 0);
        assert!(captured != state("jello there", 5, 0));
        assert!(captured != state("hello there", 6, 0));
        assert!(captured != state("hello there", 5, 1));
    }

    #[test]
    fn expired_targets_do_not_silently_retarget() {
        assert!(check_age(REMEMBER_TTL, REMEMBER_TTL).is_ok());
        assert_eq!(
            check_age(REMEMBER_TTL + Duration::from_nanos(1), REMEMBER_TTL)
                .unwrap_err()
                .code,
            InputErrorCode::StaleTarget
        );
        assert!(check_age(SESSION_TTL + Duration::from_secs(1), SESSION_TTL).is_err());
    }

    #[test]
    fn late_completion_preserves_new_session_and_matching_token_is_one_shot() {
        let mut pending = Some((TargetToken(2), "new target"));
        assert!(take_target(&mut pending, TargetToken(1)).is_err());
        assert_eq!(pending, Some((TargetToken(2), "new target")));
        assert_eq!(
            take_target(&mut pending, TargetToken(2)).unwrap(),
            "new target"
        );
        assert!(take_target(&mut pending, TargetToken(2)).is_err());
        assert!(pending.is_none());
    }

    #[test]
    fn inactive_focus_recapture_never_bypasses_security_or_stale_target_failures() {
        assert!(can_recapture_inactive_focus(InputErrorCode::MissingTarget));
        assert!(can_recapture_inactive_focus(
            InputErrorCode::UnsupportedField
        ));
        for code in [
            InputErrorCode::PermissionDenied,
            InputErrorCode::SecureField,
            InputErrorCode::StaleTarget,
            InputErrorCode::TooLarge,
            InputErrorCode::Timeout,
            InputErrorCode::WrongThread,
            InputErrorCode::Accessibility,
        ] {
            assert!(!can_recapture_inactive_focus(code));
        }
    }

    #[test]
    fn activation_only_returns_focus_from_our_app_to_original_target() {
        assert!(should_activate_target(10, 10, 20).unwrap());
        assert!(!should_activate_target(20, 10, 20).unwrap());
        for front in [30, 0, -1] {
            assert_eq!(
                should_activate_target(front, 10, 20).unwrap_err().code,
                InputErrorCode::StaleTarget
            );
        }
        assert!(should_activate_target(10, 10, 10).is_err());
        assert!(should_activate_target(10, 10, 0).is_err());
        // During asynchronous activation, a new unrelated frontmost app aborts;
        // it must not be treated as another activation opportunity.
        let decisions: Vec<_> = [10, 10, 30]
            .into_iter()
            .map(|front| should_activate_target(front, 10, 20))
            .collect();
        assert!(decisions[0].as_ref().unwrap());
        assert!(decisions[1].as_ref().unwrap());
        assert!(decisions[2].is_err());
    }

    #[test]
    fn failure_retains_transcript_without_logging_it() {
        let failure = InsertionFailure {
            error: InputError::new(InputErrorCode::PermissionDenied, "Permission required"),
            transcript: "private transcript".into(),
            may_have_inserted: false,
        };
        assert_eq!(failure.transcript, "private transcript");
        assert!(!format!("{failure:?}").contains("private transcript"));
        fn send<T: Send>() {}
        send::<TargetToken>();
        send::<InsertionFailure>();
    }

    #[test]
    fn restore_requires_unchanged_generation_and_our_marker() {
        assert!(owns_clipboard(42, 42, true));
        assert!(!owns_clipboard(42, 43, true)); // even identical copied text is newer
        assert!(!owns_clipboard(42, 42, false));
        assert!(!owns_clipboard(42, 41, true)); // reset/rollback is not ownership
    }

    #[test]
    fn default_and_ask_clipboard_access_allow_user_controlled_prompt() {
        assert!(clipboard_access_allowed(0));
        assert!(clipboard_access_allowed(1));
        assert!(clipboard_access_allowed(2));
        assert!(!clipboard_access_allowed(3));
    }

    #[test]
    fn bounded_attributed_fallback_is_only_for_unavailable_plain_ranges() {
        for status in [-25212, -25205, -25213] {
            assert!(attributed_range_fallback(status));
        }
        for status in [0, -25211, -25204, -25202, -25201] {
            assert!(!attributed_range_fallback(status));
        }
        // The fallback decision never changes the captured/requested range.
        let range = TextRange::checked(3, 0, 5).unwrap().context(5);
        assert_eq!(range, TextRange { start: 0, len: 5 });
        assert!(range.len <= MAX_TEXT_UNITS + 2 * CONTEXT_UNITS);
    }

    #[test]
    fn terminal_hosts_are_rejected_without_rejecting_browser_editors() {
        assert!(terminal_host("com.apple.Terminal", "終端機"));
        assert!(terminal_host("com.googlecode.iterm2", "iTerm2"));
        assert!(terminal_host("custom.wrapper", "Ghostty"));
        assert!(!terminal_host("com.google.Chrome", "Google Chrome"));
        assert!(!terminal_host("com.apple.TextEdit", "TextEdit"));
    }

    #[test]
    fn read_only_selection_length_accepts_empty_and_rejects_oversized() {
        assert!(check_selected_length(0).is_ok());
        assert!(check_selected_length(MAX_TEXT_UNITS as isize).is_ok());
        assert_eq!(
            check_selected_length(MAX_TEXT_UNITS as isize + 1)
                .unwrap_err()
                .code,
            InputErrorCode::TooLarge
        );
        assert!(check_selected_length(-1).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn off_main_thread_fails_before_any_accessibility_work() {
        std::thread::spawn(|| {
            assert_eq!(
                accessibility_trusted().unwrap_err().code,
                InputErrorCode::WrongThread
            );
            assert_eq!(
                request_post_event_access().unwrap_err().code,
                InputErrorCode::WrongThread
            );
            assert_eq!(
                capture_selected_text().unwrap_err().code,
                InputErrorCode::WrongThread
            );
            let failure = insert_text(TargetToken(1), "retained transcript").unwrap_err();
            assert_eq!(failure.error.code, InputErrorCode::WrongThread);
            assert_eq!(failure.transcript, "retained transcript");
            assert!(!failure.may_have_inserted);
        })
        .join()
        .unwrap();
    }
}
