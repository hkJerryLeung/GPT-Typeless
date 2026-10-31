//! Optional macOS Fn shortcuts. No permission prompts, injection, or text reads.
//!
//! Parent wiring: add `#[cfg(target_os = "macos")] mod fn_shortcut;`, retain the
//! handle returned by `install(config.fn_shortcut_enabled, |mode| { /* enqueue */ })`,
//! and call `uninstall()` when the setting turns off. Existing Cmd+Shift+R stays
//! registered independently. The handler runs on a worker: only enqueue/emit an
//! action, dispatching AX/UI work to the main thread. Never record/network here.
//! Recheck the setting when handling an already-enqueued parent action.
//! Status is observable; a failed tap needs explicit uninstall/reinstall.
//!
//! Dependencies for parent to add (macOS target; no manifest edits in this task):
//! - objc2-core-graphics 0.3.2, default-features=false,
//!   features=["std", "CGEvent", "CGEventTypes", "CGEventSource"]
//! - Add ["CFMachPort", "CFRunLoop", "CFDate"] to existing
//!   objc2-core-foundation 0.3.2 features (std/CFString already enabled).
//! - Existing objc2-application-services ["HIServices", "AXUIElement"] suffices.
//!
//! Semantics: Fn alone / Fn+Shift fire ON Fn RELEASE, to disambiguate chords;
//! Fn+Space fires on Space down, once (no autorepeat). Either Shift is supported.
//! Parent interprets each mode event as a toggle, including finish while recording.
//! Only a physical kVK_Function flagsChanged starts a gesture. Other Fn combos
//! cancel the candidate and their key events pass through unchanged. Fn edges
//! reserved for a candidate are suppressed even if a later key cancels it; this
//! avoids forwarding a partial Fn gesture to the system emoji shortcut. No OS
//! preference is changed. Hardware/OS popup suppression needs manual acceptance;
//! public events do not guarantee every keyboard exposes a usable Fn key.
//!
//! Apple references:
//! https://developer.apple.com/documentation/coregraphics/cgevent/tapcreate(tap:place:options:eventsofinterest:callback:userinfo:)
//! https://developer.apple.com/documentation/coregraphics/cgeventtapcallback
//! https://developer.apple.com/documentation/coregraphics/cgeventflags
//! https://developer.apple.com/documentation/coregraphics/cgpreflightlisteneventaccess()
//! https://developer.apple.com/documentation/coregraphics/cgevent/tapenable(tap:enable:)

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutMode {
    Dictate,
    Translate,
    Ask,
}

impl ShortcutMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dictate => "dictate",
            Self::Translate => "translate",
            Self::Ask => "ask",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutError {
    NotEnabled,
    #[cfg(not(target_os = "macos"))]
    UnsupportedPlatform,
    AccessibilityRequired,
    InputMonitoringRequired,
    AlreadyInstalled,
    ThreadUnavailable,
    TapUnavailable,
    RunLoopUnavailable,
    TapDisabled,
    RepeatedTimeout,
    HandlerUnavailable,
    HandlerPanicked,
}

impl fmt::Display for ShortcutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotEnabled => "Optional native Fn shortcuts are not enabled in settings.",
            #[cfg(not(target_os = "macos"))]
            Self::UnsupportedPlatform => "Native Fn shortcuts require macOS.",
            Self::AccessibilityRequired => {
                "Fn shortcuts unavailable: Accessibility trust is missing."
            }
            Self::InputMonitoringRequired => {
                "Fn shortcuts unavailable: Input Monitoring access is missing."
            }
            Self::AlreadyInstalled => "A native Fn shortcut listener is already installed.",
            Self::ThreadUnavailable => "The native Fn shortcut worker could not start.",
            Self::TapUnavailable => "macOS could not create or enable an active Fn event tap.",
            Self::RunLoopUnavailable => "The native Fn shortcut run loop is unavailable.",
            Self::TapDisabled => {
                "macOS disabled the Fn event tap; explicitly re-enable the setting to retry."
            }
            Self::RepeatedTimeout => {
                "The Fn event tap timed out again and was stopped; explicitly re-enable to retry."
            }
            Self::HandlerUnavailable => {
                "The Fn action handler is unavailable or its bounded queue is full."
            }
            Self::HandlerPanicked => "The Fn action handler failed; the listener was stopped.",
        })
    }
}

impl std::error::Error for ShortcutError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutStatus {
    Running,
    Stopped,
    Unavailable(ShortcutError),
}

// Shared status contains no key codes, event objects, or user text.
const RUNNING: u8 = 0;
const STOPPED: u8 = 1;
const DISABLED: u8 = 2;
const TIMED_OUT: u8 = 3;
const HANDLER_GONE: u8 = 4;
const HANDLER_PANICKED: u8 = 5;
const RUN_LOOP_GONE: u8 = 6;
const AX_GONE: u8 = 7;
const INPUT_GONE: u8 = 8;

pub struct FnShortcut {
    stop: Arc<AtomicBool>,
    health: Arc<AtomicU8>,
    thread: Option<JoinHandle<()>>,
}

impl FnShortcut {
    pub fn status(&self) -> ShortcutStatus {
        if self.health.load(Ordering::Acquire) == RUNNING
            && self.thread.as_ref().is_some_and(JoinHandle::is_finished)
        {
            return ShortcutStatus::Unavailable(ShortcutError::ThreadUnavailable);
        }
        match self.health.load(Ordering::Acquire) {
            RUNNING => ShortcutStatus::Running,
            STOPPED => ShortcutStatus::Stopped,
            DISABLED => ShortcutStatus::Unavailable(ShortcutError::TapDisabled),
            TIMED_OUT => ShortcutStatus::Unavailable(ShortcutError::RepeatedTimeout),
            HANDLER_GONE => ShortcutStatus::Unavailable(ShortcutError::HandlerUnavailable),
            HANDLER_PANICKED => ShortcutStatus::Unavailable(ShortcutError::HandlerPanicked),
            AX_GONE => ShortcutStatus::Unavailable(ShortcutError::AccessibilityRequired),
            INPUT_GONE => ShortcutStatus::Unavailable(ShortcutError::InputMonitoringRequired),
            _ => ShortcutStatus::Unavailable(ShortcutError::RunLoopUnavailable),
        }
    }

    /// Stops accepting/dispatching queued actions and joins the owned event loop.
    /// A handler already executing may finish; it must only enqueue parent work.
    /// Safe on any thread. No main-thread join or AppKit objects are involved.
    pub fn uninstall(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for FnShortcut {
    fn drop(&mut self) {
        self.uninstall();
    }
}

/// The explicit setting must be true. Permission preflights never show prompts.
/// Does not install a passive fallback if the active/filter tap is unavailable.
pub fn install(
    user_enabled: bool,
    handler: impl Fn(ShortcutMode) + Send + 'static,
) -> Result<FnShortcut, ShortcutError> {
    if !user_enabled {
        return Err(ShortcutError::NotEnabled);
    }
    #[cfg(target_os = "macos")]
    {
        native::install(handler)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = handler;
        Err(ShortcutError::UnsupportedPlatform)
    }
}

#[derive(Clone, Copy)]
enum Input {
    Fn(bool),
    Shift,
    SpaceDown { repeat: bool },
    SpaceUp,
    Other,
}

#[derive(Clone, Copy, Default)]
struct Modifiers {
    function: bool,
    shift: bool,
    forbidden: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Decision {
    suppress: bool,
    mode: Option<ShortcutMode>,
}

// Only gesture state is retained. Unrelated keys collapse immediately to Other.
#[derive(Default)]
struct Gesture {
    owned_fn: bool,
    pending: Option<ShortcutMode>,
    owned_space: bool,
    wait_for_release: bool,
}

impl Gesture {
    fn interrupt(&mut self, fn_held: bool) {
        self.pending = None;
        self.wait_for_release = fn_held;
        self.owned_fn &= fn_held;
        // Keep ownership of suppressed edges until their releases arrive.
    }

    fn step(&mut self, input: Input, flags: Modifiers) -> Decision {
        let pass = Decision::default();
        let consume = || Decision {
            suppress: true,
            mode: None,
        };
        // A timeout can hide the Fn release after interrupt() sampled it as held.
        // A later non-Fn event without the Fn flag indicates the old gesture ended.
        // Reconcile before consuming an owned Space edge, but keep an actual Fn
        // release paired with its suppressed down edge in the branch below.
        if self.wait_for_release && !flags.function && !matches!(input, Input::Fn(_)) {
            self.wait_for_release = false;
            self.owned_fn = false;
        }
        if matches!(input, Input::SpaceUp) && self.owned_space {
            self.owned_space = false;
            return consume();
        }
        if matches!(input, Input::SpaceDown { .. }) && self.owned_space {
            return consume();
        }
        if let Input::Fn(down) = input {
            if !down {
                let suppress = std::mem::take(&mut self.owned_fn);
                let mode = self
                    .pending
                    .take()
                    .filter(|_| !flags.forbidden && !self.wait_for_release);
                self.wait_for_release = false;
                return Decision {
                    suppress,
                    mode: if suppress { mode } else { None },
                };
            }
            if self.owned_fn {
                return consume();
            }
            if self.wait_for_release || flags.forbidden || self.owned_space {
                return pass;
            }
            self.owned_fn = true;
            self.pending = Some(if flags.shift {
                ShortcutMode::Translate
            } else {
                ShortcutMode::Dictate
            });
            return consume();
        }
        if self.wait_for_release {
            self.wait_for_release = flags.function;
            return pass;
        }
        if !self.owned_fn {
            return pass;
        }
        if !flags.function {
            self.owned_fn = false;
            self.pending = None;
            return pass;
        }
        if flags.forbidden {
            self.pending = None;
            return pass;
        }
        match input {
            Input::Shift if flags.shift => {
                if self.pending.is_some() {
                    self.pending = Some(ShortcutMode::Translate);
                }
            }
            Input::Shift => {}
            Input::SpaceDown { repeat: false }
                if !flags.shift && self.pending == Some(ShortcutMode::Dictate) =>
            {
                self.pending = None;
                self.owned_space = true;
                return Decision {
                    suppress: true,
                    mode: Some(ShortcutMode::Ask),
                };
            }
            _ => self.pending = None,
        }
        pass
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use objc2_application_services::AXIsProcessTrusted;
    use objc2_core_foundation::{kCFRunLoopDefaultMode, CFMachPort, CFRunLoop, CFRunLoopRunResult};
    use objc2_core_graphics::{
        CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID,
        CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType,
        CGPreflightListenEventAccess,
    };
    use std::ffi::c_void;
    use std::ptr::NonNull;

    static INSTALLED: AtomicBool = AtomicBool::new(false);

    struct Installation;
    impl Drop for Installation {
        fn drop(&mut self) {
            INSTALLED.store(false, Ordering::Release);
        }
    }

    fn permissions() -> Result<(), ShortcutError> {
        // AXIsProcessTrusted only reads trust; no prompt/options/request API.
        if !unsafe { AXIsProcessTrusted() } {
            return Err(ShortcutError::AccessibilityRequired);
        }
        if !CGPreflightListenEventAccess() {
            return Err(ShortcutError::InputMonitoringRequired);
        }
        Ok(())
    }

    fn fn_held() -> bool {
        CGEventSource::flags_state(CGEventSourceStateID::CombinedSessionState)
            .contains(CGEventFlags::MaskSecondaryFn)
    }

    struct Context {
        gesture: Gesture,
        actions: mpsc::SyncSender<ShortcutMode>,
        stop: Arc<AtomicBool>,
        health: Arc<AtomicU8>,
        timeout: bool,
    }

    unsafe extern "C-unwind" fn callback(
        _proxy: CGEventTapProxy,
        kind: CGEventType,
        event: NonNull<CGEvent>,
        user: *mut c_void,
    ) -> *mut CGEvent {
        // Box is owned by this run-loop thread and outlives port invalidation.
        let context = unsafe { &mut *user.cast::<Context>() };
        if kind == CGEventType::TapDisabledByTimeout {
            context.timeout = true;
            return event.as_ptr();
        }
        if kind == CGEventType::TapDisabledByUserInput {
            context.health.store(DISABLED, Ordering::Release);
            return event.as_ptr();
        }
        if context.stop.load(Ordering::Acquire) || context.health.load(Ordering::Acquire) != RUNNING
        {
            return event.as_ptr();
        }
        // No Unicode/string access. Only immediate classification of Fn/Shift/Space.
        let event_ref = Some(unsafe { event.as_ref() });
        let code = CGEvent::integer_value_field(event_ref, CGEventField::KeyboardEventKeycode);
        let flags = CGEvent::flags(event_ref);
        let modifiers = Modifiers {
            function: flags.contains(CGEventFlags::MaskSecondaryFn),
            shift: flags.contains(CGEventFlags::MaskShift),
            forbidden: flags.intersects(
                CGEventFlags::MaskCommand
                    | CGEventFlags::MaskControl
                    | CGEventFlags::MaskAlternate
                    | CGEventFlags::MaskHelp,
            ),
        };
        // Public HIToolbox/Events.h virtual key constants: Function=63, Space=49,
        // Shift=56, RightShift=60. A Fn bit on arrow/function keys is not an edge.
        let input = match (kind, code) {
            (CGEventType::FlagsChanged, 63) => Input::Fn(modifiers.function),
            (CGEventType::FlagsChanged, 56 | 60) => Input::Shift,
            (CGEventType::KeyDown, 49) => Input::SpaceDown {
                repeat: CGEvent::integer_value_field(
                    event_ref,
                    CGEventField::KeyboardEventAutorepeat,
                ) != 0,
            },
            (CGEventType::KeyUp, 49) => Input::SpaceUp,
            _ => Input::Other,
        };
        let decision = context.gesture.step(input, modifiers);
        if let Some(mode) = decision.mode {
            if context.actions.try_send(mode).is_err() {
                context.health.store(HANDLER_GONE, Ordering::Release);
            }
        }
        if decision.suppress {
            std::ptr::null_mut()
        } else {
            event.as_ptr()
        }
    }

    pub(super) fn install(
        handler: impl Fn(ShortcutMode) + Send + 'static,
    ) -> Result<FnShortcut, ShortcutError> {
        permissions()?;
        INSTALLED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| ShortcutError::AlreadyInstalled)?;
        let stop = Arc::new(AtomicBool::new(false));
        let health = Arc::new(AtomicU8::new(RUNNING));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker_stop = stop.clone();
        let worker_health = health.clone();
        let thread = thread::Builder::new()
            .name("typeless-fn-tap".into())
            .spawn(move || {
                let _installation = Installation;
                let (actions, receive) = mpsc::sync_channel(8);
                let mut context = Box::new(Context {
                    gesture: Gesture {
                        wait_for_release: fn_held(),
                        ..Gesture::default()
                    },
                    actions,
                    stop: worker_stop.clone(),
                    health: worker_health.clone(),
                    timeout: false,
                });
                let mask = (1 << CGEventType::FlagsChanged.0)
                    | (1 << CGEventType::KeyDown.0)
                    | (1 << CGEventType::KeyUp.0);
                // Active session tap; never root-only HID tap or passive fallback.
                let tap = unsafe {
                    CGEvent::tap_create(
                        CGEventTapLocation::SessionEventTap,
                        CGEventTapPlacement::HeadInsertEventTap,
                        CGEventTapOptions::Default,
                        mask,
                        Some(callback),
                        (&mut *context as *mut Context).cast(),
                    )
                };
                let Some(tap) = tap else {
                    let _ = ready_tx.send(Err(ShortcutError::TapUnavailable));
                    return;
                };
                let Some(source) = CFMachPort::new_run_loop_source(None, Some(&tap), 0) else {
                    tap.invalidate();
                    let _ = ready_tx.send(Err(ShortcutError::RunLoopUnavailable));
                    return;
                };
                let Some(run_loop) = CFRunLoop::current() else {
                    tap.invalidate();
                    let _ = ready_tx.send(Err(ShortcutError::RunLoopUnavailable));
                    return;
                };
                let action_stop = worker_stop.clone();
                let action_health = worker_health.clone();
                // Arbitrary parent code never runs inside the event tap or its loop.
                let dispatch = thread::Builder::new()
                    .name("typeless-fn-actions".into())
                    .spawn(move || {
                        while let Ok(mode) = receive.recv() {
                            if action_stop.load(Ordering::Acquire) {
                                break;
                            }
                            if action_health.load(Ordering::Acquire) != RUNNING {
                                continue;
                            }
                            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                handler(mode)
                            }))
                            .is_err()
                            {
                                action_health.store(HANDLER_PANICKED, Ordering::Release);
                                action_stop.store(true, Ordering::Release);
                                break;
                            }
                        }
                    });
                if dispatch.is_err() {
                    tap.invalidate();
                    let _ = ready_tx.send(Err(ShortcutError::ThreadUnavailable));
                    return;
                }
                // kCFRunLoopDefaultMode is an immutable framework-owned constant.
                let mode = unsafe { kCFRunLoopDefaultMode };
                run_loop.add_source(Some(&source), mode);
                if !CGEvent::tap_is_enabled(&tap) {
                    worker_health.store(DISABLED, Ordering::Release);
                    let _ = ready_tx.send(Err(ShortcutError::TapUnavailable));
                } else if ready_tx.send(Ok(())).is_err() {
                    worker_stop.store(true, Ordering::Release);
                }
                let mut recovered = false;
                while !worker_stop.load(Ordering::Acquire)
                    && worker_health.load(Ordering::Acquire) == RUNNING
                {
                    // Bounded wait allows deterministic uninstall without sending
                    // native pointers across threads. Return after handling a source.
                    let result = CFRunLoop::run_in_mode(mode, 0.1, true);
                    if result == CFRunLoopRunResult::Finished
                        || result == CFRunLoopRunResult::Stopped
                    {
                        worker_health.store(RUN_LOOP_GONE, Ordering::Release);
                        break;
                    }
                    if context.timeout
                        && !worker_stop.load(Ordering::Acquire)
                        && worker_health.load(Ordering::Acquire) == RUNNING
                    {
                        context.timeout = false;
                        context.gesture.interrupt(fn_held());
                        if recovered {
                            worker_health.store(TIMED_OUT, Ordering::Release);
                            break;
                        }
                        if let Err(error) = permissions() {
                            worker_health.store(
                                if error == ShortcutError::AccessibilityRequired {
                                    AX_GONE
                                } else {
                                    INPUT_GONE
                                },
                                Ordering::Release,
                            );
                            break;
                        }
                        recovered = true; // At most ONE recovery per installation.
                        CGEvent::tap_enable(&tap, true);
                        if !CGEvent::tap_is_enabled(&tap) {
                            worker_health.store(DISABLED, Ordering::Release);
                            break;
                        }
                    }
                    // Permission/user disablement may invalidate the tap without
                    // delivering a notification. Never spin or silently reinstall.
                    if !context.timeout && !CGEvent::tap_is_enabled(&tap) {
                        worker_health.store(DISABLED, Ordering::Release);
                        break;
                    }
                }
                CGEvent::tap_enable(&tap, false);
                run_loop.remove_source(Some(&source), mode);
                tap.invalidate();
                // Invalidate/release all callback-bearing resources before Context.
                drop(source);
                drop(tap);
                drop(context);
                let _ = worker_health.compare_exchange(
                    RUNNING,
                    STOPPED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                // Dropping the sender stops the dispatcher; an in-progress parent
                // handler may finish. Do not block uninstall on arbitrary parent code.
                drop(dispatch);
            });
        let thread = match thread {
            Ok(thread) => thread,
            Err(_) => {
                INSTALLED.store(false, Ordering::Release);
                return Err(ShortcutError::ThreadUnavailable);
            }
        };
        match ready_rx
            .recv()
            .unwrap_or(Err(ShortcutError::ThreadUnavailable))
        {
            Ok(()) => Ok(FnShortcut {
                stop,
                health,
                thread: Some(thread),
            }),
            Err(error) => {
                stop.store(true, Ordering::Release);
                let _ = thread.join();
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(function: bool, shift: bool) -> Modifiers {
        Modifiers {
            function,
            shift,
            forbidden: false,
        }
    }

    #[test]
    fn fn_alone_toggles_once_on_release() {
        let mut g = Gesture::default();
        assert_eq!(
            g.step(Input::Fn(true), flags(true, false)),
            Decision {
                suppress: true,
                mode: None
            }
        );
        assert_eq!(
            g.step(Input::Fn(false), flags(false, false)),
            Decision {
                suppress: true,
                mode: Some(ShortcutMode::Dictate)
            }
        );
        assert_eq!(
            g.step(Input::Fn(false), flags(false, false)),
            Decision::default()
        );
    }

    #[test]
    fn shift_can_precede_or_follow_fn_and_release_first() {
        for shift_first in [false, true] {
            let mut g = Gesture::default();
            g.step(Input::Fn(true), flags(true, shift_first));
            if !shift_first {
                assert!(!g.step(Input::Shift, flags(true, true)).suppress);
            }
            assert!(!g.step(Input::Shift, flags(true, false)).suppress);
            assert_eq!(
                g.step(Input::Fn(false), flags(false, false)).mode,
                Some(ShortcutMode::Translate)
            );
        }
    }

    #[test]
    fn space_selects_ask_without_dictate_or_repeat_and_consumes_both_edges() {
        let mut g = Gesture::default();
        g.step(Input::Fn(true), flags(true, false));
        assert_eq!(
            g.step(Input::SpaceDown { repeat: false }, flags(true, false)),
            Decision {
                suppress: true,
                mode: Some(ShortcutMode::Ask)
            }
        );
        assert_eq!(
            g.step(Input::SpaceDown { repeat: true }, flags(true, false)),
            Decision {
                suppress: true,
                mode: None
            }
        );
        assert_eq!(
            g.step(Input::Fn(false), flags(false, false)),
            Decision {
                suppress: true,
                mode: None
            }
        );
        assert!(g.step(Input::SpaceUp, flags(false, false)).suppress);
        assert!(!g.step(Input::SpaceUp, flags(false, false)).suppress);
    }

    #[test]
    fn unrelated_keys_and_unsupported_chords_pass_and_cancel_candidate() {
        for input in [Input::Other, Input::SpaceDown { repeat: true }] {
            let mut g = Gesture::default();
            g.step(Input::Fn(true), flags(true, false));
            assert_eq!(g.step(input, flags(true, false)), Decision::default());
            assert_eq!(g.step(Input::Fn(false), flags(false, false)).mode, None);
        }
        let mut g = Gesture::default();
        g.step(Input::Fn(true), flags(true, true));
        assert_eq!(
            g.step(Input::SpaceDown { repeat: false }, flags(true, true)),
            Decision::default()
        );
        assert_eq!(g.step(Input::Fn(false), flags(false, true)).mode, None);
    }

    #[test]
    fn command_control_option_chords_and_fn_flag_without_physical_edge_pass() {
        let mut g = Gesture::default();
        let forbidden = Modifiers {
            forbidden: true,
            ..flags(true, false)
        };
        assert_eq!(g.step(Input::Fn(true), forbidden), Decision::default());
        assert_eq!(g.step(Input::Other, forbidden), Decision::default());
        assert_eq!(
            g.step(Input::Fn(false), flags(false, false)),
            Decision::default()
        );
        assert_eq!(
            g.step(Input::SpaceDown { repeat: false }, flags(true, false)),
            Decision::default()
        );
        g.step(Input::Fn(true), flags(true, false));
        assert!(!g.step(Input::Other, forbidden).suppress);
        assert_eq!(g.step(Input::Fn(false), flags(false, false)).mode, None);
    }

    #[test]
    fn interruption_cannot_finish_a_partial_gesture_or_rearm_from_shift() {
        let mut g = Gesture::default();
        g.step(Input::Fn(true), flags(true, false));
        g.interrupt(true);
        assert_eq!(g.step(Input::Shift, flags(true, true)), Decision::default());
        assert_eq!(
            g.step(Input::Fn(false), flags(false, false)),
            Decision {
                suppress: true,
                mode: None
            }
        );
        g.step(Input::Fn(true), flags(true, false));
        assert_eq!(
            g.step(Input::Fn(false), flags(false, false)).mode,
            Some(ShortcutMode::Dictate)
        );
    }

    #[test]
    fn installing_mid_gesture_does_not_claim_release_or_space() {
        let mut g = Gesture {
            wait_for_release: true,
            ..Gesture::default()
        };
        assert_eq!(
            g.step(Input::SpaceDown { repeat: false }, flags(true, false)),
            Decision::default()
        );
        assert_eq!(
            g.step(Input::Fn(false), flags(false, false)),
            Decision::default()
        );
        g.step(Input::Fn(true), flags(true, false));
        assert_eq!(
            g.step(Input::Fn(false), flags(false, false)).mode,
            Some(ShortcutMode::Dictate)
        );
    }

    #[test]
    fn interruption_after_missed_fn_release_does_not_swallow_next_toggle() {
        let mut g = Gesture::default();
        g.step(Input::Fn(true), flags(true, false));
        g.interrupt(false);
        g.step(Input::Fn(true), flags(true, false));
        assert_eq!(
            g.step(Input::Fn(false), flags(false, false)).mode,
            Some(ShortcutMode::Dictate)
        );
    }

    #[test]
    fn interrupted_gesture_rearms_when_later_event_shows_fn_was_released() {
        let mut g = Gesture::default();
        g.step(Input::Fn(true), flags(true, false));
        g.interrupt(true);
        // Fn is released while the tap cannot observe its release.
        assert_eq!(
            g.step(Input::Other, flags(false, false)),
            Decision::default()
        );
        assert!(g.step(Input::Fn(true), flags(true, false)).suppress);
        assert_eq!(
            g.step(Input::Fn(false), flags(false, false)),
            Decision {
                suppress: true,
                mode: Some(ShortcutMode::Dictate),
            }
        );
    }

    #[test]
    fn interrupted_ask_rearms_when_owned_space_release_shows_fn_was_released() {
        let mut g = Gesture::default();
        g.step(Input::Fn(true), flags(true, false));
        assert_eq!(
            g.step(Input::SpaceDown { repeat: false }, flags(true, false))
                .mode,
            Some(ShortcutMode::Ask)
        );
        g.interrupt(true);
        assert_eq!(
            g.step(Input::SpaceUp, flags(false, false)),
            Decision {
                suppress: true,
                mode: None,
            }
        );
        assert!(g.step(Input::Fn(true), flags(true, false)).suppress);
        assert_eq!(
            g.step(Input::Fn(false), flags(false, false)).mode,
            Some(ShortcutMode::Dictate)
        );
    }

    #[test]
    fn interruption_preserves_claimed_space_release_without_another_action() {
        let mut g = Gesture::default();
        g.step(Input::Fn(true), flags(true, false));
        g.step(Input::SpaceDown { repeat: false }, flags(true, false));
        g.interrupt(true);
        assert_eq!(
            g.step(Input::SpaceUp, flags(true, false)),
            Decision {
                suppress: true,
                mode: None
            }
        );
        assert_eq!(g.step(Input::Fn(false), flags(false, false)).mode, None);
    }
}
