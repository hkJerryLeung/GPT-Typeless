//! macOS single-key filter. Only a selected, unmodified key gesture is consumed;
//! no text, key history, or unrelated key data is stored or logged.

use crate::recording_shortcut::SingleKey;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

const RUNNING: u8 = 1;
const STOPPED: u8 = 2;
const DISABLED: u8 = 3;
const TIMED_OUT: u8 = 4;
const INPUT_GONE: u8 = 5;
const WORKER_FAILED: u8 = 6;
const ACCESSIBILITY_GONE: u8 = 7;

pub(crate) struct SingleKeyShortcut {
    enabled: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    health: Arc<AtomicU8>,
    thread: Option<JoinHandle<()>>,
}

impl SingleKeyShortcut {
    pub(crate) fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    pub(crate) fn status(&self) -> Result<(), String> {
        let status = self.health.load(Ordering::Acquire);
        if status == RUNNING && self.thread.as_ref().is_some_and(JoinHandle::is_finished) {
            return Err("單鍵快捷鍵監聽程序已停止，請重新儲存快捷鍵。".into());
        }
        match status {
            RUNNING => Ok(()),
            INPUT_GONE => Err("單鍵快捷鍵需要 macOS 輸入監控權限；請允許後重新儲存快捷鍵。".into()),
            ACCESSIBILITY_GONE => {
                Err("單鍵快捷鍵需要 macOS 輔助使用權限；請允許後重新儲存快捷鍵。".into())
            }
            TIMED_OUT => Err("macOS 已暫停單鍵快捷鍵監聽；請重新儲存快捷鍵。".into()),
            DISABLED => {
                Err("macOS 已停用單鍵快捷鍵監聽；請檢查輔助使用及輸入監控權限並重新儲存。".into())
            }
            _ => Err("單鍵快捷鍵監聽程序已停止，請重新儲存快捷鍵。".into()),
        }
    }
}

impl Drop for SingleKeyShortcut {
    fn drop(&mut self) {
        self.enabled.store(false, Ordering::Release);
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Default)]
struct Gesture {
    waiting_for_release: bool,
    down: bool,
    reconciled: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct Decision {
    event: Option<bool>,
    suppress: bool,
}

impl Gesture {
    fn step(&mut self, pressed: bool, repeat: bool, block_start: bool) -> Decision {
        let pass = || Decision {
            event: None,
            suppress: false,
        };
        if self.down {
            if !pressed {
                self.down = false;
                let event = (!self.reconciled).then_some(false);
                self.reconciled = false;
                return Decision {
                    event,
                    suppress: true,
                };
            }
            if self.reconciled && !repeat {
                // The previous KeyUp was lost; this is a fresh press.
                self.down = false;
                self.reconciled = false;
            } else {
                // Once a down is consumed, consume repeat and the matching up
                // even if modifiers or focus change before the key is released.
                return Decision {
                    event: None,
                    suppress: true,
                };
            }
        }
        if self.waiting_for_release {
            if !pressed {
                self.waiting_for_release = false;
            }
            return pass();
        }
        if !pressed {
            return pass();
        }
        if repeat || block_start {
            self.waiting_for_release = true;
            return pass();
        }
        self.down = true;
        Decision {
            event: Some(true),
            suppress: true,
        }
    }

    fn reconcile_released(&mut self) -> Option<bool> {
        self.waiting_for_release = false;
        if self.down && !self.reconciled {
            // Continue consuming a late KeyUp even after notifying hold mode.
            self.reconciled = true;
            Some(false)
        } else {
            None
        }
    }

    fn abandon_down(&mut self) {
        self.down = false;
        self.reconciled = false;
        self.waiting_for_release = true;
    }
}

pub(crate) fn install(
    key: SingleKey,
    main_focused: Arc<AtomicBool>,
    handler: impl Fn(bool) + Send + 'static,
) -> Result<SingleKeyShortcut, String> {
    #[cfg(target_os = "macos")]
    {
        native::install(key, main_focused, handler)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (key, main_focused, handler);
        Err("單鍵錄音快捷鍵目前只支援 macOS。".into())
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
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    struct Context {
        key: SingleKey,
        gesture: Gesture,
        down_since: Option<Instant>,
        observed_hardware_down: bool,
        enabled: Arc<AtomicBool>,
        main_focused: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        health: Arc<AtomicU8>,
        events: mpsc::SyncSender<bool>,
    }

    impl Context {
        fn send(&self, event: Option<bool>) -> bool {
            if let Some(event) = event {
                if !self.enabled.load(Ordering::Acquire) {
                    return false;
                }
                if self.events.try_send(event).is_err() {
                    self.health.store(WORKER_FAILED, Ordering::Release);
                    return false;
                }
            }
            true
        }
    }

    unsafe extern "C-unwind" fn callback(
        _proxy: CGEventTapProxy,
        kind: CGEventType,
        event: NonNull<CGEvent>,
        user: *mut c_void,
    ) -> *mut CGEvent {
        // The run-loop thread owns the context until the tap is invalidated.
        let context = unsafe { &mut *user.cast::<Context>() };
        if kind == CGEventType::TapDisabledByTimeout {
            context.health.store(TIMED_OUT, Ordering::Release);
        } else if kind == CGEventType::TapDisabledByUserInput {
            context.health.store(DISABLED, Ordering::Release);
        } else if context.health.load(Ordering::Acquire) == RUNNING
            && (kind == CGEventType::KeyDown || kind == CGEventType::KeyUp)
        {
            let event_ref = Some(unsafe { event.as_ref() });
            if CGEvent::integer_value_field(event_ref, CGEventField::KeyboardEventKeycode)
                == i64::from(context.key.code)
            {
                let flags = CGEvent::flags(event_ref);
                let forbidden = flags.intersects(
                    CGEventFlags::MaskCommand
                        | CGEventFlags::MaskControl
                        | CGEventFlags::MaskAlternate
                        | CGEventFlags::MaskShift
                        | CGEventFlags::MaskHelp,
                ) || (!context.key.allow_fn_flag
                    && flags.contains(CGEventFlags::MaskSecondaryFn));
                let repeat =
                    CGEvent::integer_value_field(event_ref, CGEventField::KeyboardEventAutorepeat)
                        != 0;
                let target_pid =
                    CGEvent::integer_value_field(event_ref, CGEventField::EventTargetUnixProcessID);
                let block_start = forbidden
                    || context.main_focused.load(Ordering::Acquire)
                    || target_pid == i64::from(std::process::id())
                    || !context.enabled.load(Ordering::Acquire)
                    || context.stop.load(Ordering::Acquire);
                let decision =
                    context
                        .gesture
                        .step(kind == CGEventType::KeyDown, repeat, block_start);
                if let Some(pressed) = decision.event {
                    context.down_since = pressed.then(Instant::now);
                    context.observed_hardware_down = false;
                    if !context.send(Some(pressed)) && pressed {
                        // A press that cannot reach the app must remain usable.
                        context.gesture.abandon_down();
                        return event.as_ptr();
                    }
                }
                if decision.suppress {
                    return std::ptr::null_mut();
                }
            }
        }
        event.as_ptr()
    }

    pub(super) fn install(
        key: SingleKey,
        main_focused: Arc<AtomicBool>,
        handler: impl Fn(bool) + Send + 'static,
    ) -> Result<SingleKeyShortcut, String> {
        // Both checks are read-only and never trigger a permissions prompt.
        if !unsafe { AXIsProcessTrusted() } {
            return Err("單鍵快捷鍵需要 macOS 輔助使用權限；請允許後重新儲存快捷鍵。".into());
        }
        if !CGPreflightListenEventAccess() {
            return Err("單鍵快捷鍵需要 macOS 輸入監控權限；請允許後重新儲存快捷鍵。".into());
        }
        let enabled = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let health = Arc::new(AtomicU8::new(RUNNING));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker_enabled = enabled.clone();
        let worker_stop = stop.clone();
        let worker_health = health.clone();
        let thread = thread::Builder::new()
            .name("typeless-single-key".into())
            .spawn(move || {
                let (events, receive) = mpsc::sync_channel(32);
                let mut context = Box::new(Context {
                    key,
                    gesture: Gesture {
                        waiting_for_release: CGEventSource::key_state(
                            CGEventSourceStateID::CombinedSessionState,
                            key.code,
                        ),
                        ..Gesture::default()
                    },
                    down_since: None,
                    observed_hardware_down: false,
                    enabled: worker_enabled,
                    main_focused,
                    stop: worker_stop.clone(),
                    health: worker_health.clone(),
                    events,
                });
                let mask = (1 << CGEventType::KeyDown.0) | (1 << CGEventType::KeyUp.0);
                let tap = unsafe {
                    CGEvent::tap_create(
                        CGEventTapLocation::SessionEventTap,
                        // Fn's active tap is head-inserted. It must observe an
                        // Fn+F-key before this tap consumes the F-key.
                        CGEventTapPlacement::TailAppendEventTap,
                        CGEventTapOptions::Default,
                        mask,
                        Some(callback),
                        (&mut *context as *mut Context).cast(),
                    )
                };
                let Some(tap) = tap else {
                    let _ = ready_tx.send(Err(
                        "macOS 無法建立單鍵監聽；請檢查輔助使用及輸入監控權限。".into(),
                    ));
                    return;
                };
                let Some(source) = CFMachPort::new_run_loop_source(None, Some(&tap), 0) else {
                    tap.invalidate();
                    let _ = ready_tx.send(Err("macOS 無法啟動單鍵監聽。".into()));
                    return;
                };
                let Some(run_loop) = CFRunLoop::current() else {
                    tap.invalidate();
                    let _ = ready_tx.send(Err("macOS 無法啟動單鍵監聽。".into()));
                    return;
                };
                let dispatch_health = worker_health.clone();
                let dispatcher = thread::Builder::new()
                    .name("typeless-single-key-events".into())
                    .spawn(move || {
                        while let Ok(pressed) = receive.recv() {
                            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                handler(pressed)
                            }))
                            .is_err()
                            {
                                dispatch_health.store(WORKER_FAILED, Ordering::Release);
                                break;
                            }
                        }
                        // A disabled tap or a lost release must not leave hold mode down.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            handler(false)
                        }));
                    });
                let Ok(dispatcher) = dispatcher else {
                    tap.invalidate();
                    let _ = ready_tx.send(Err("無法啟動單鍵快捷鍵事件處理。".into()));
                    return;
                };
                let mode = unsafe { kCFRunLoopDefaultMode };
                run_loop.add_source(Some(&source), mode);
                if !CGEvent::tap_is_enabled(&tap) {
                    worker_health.store(DISABLED, Ordering::Release);
                    let _ = ready_tx.send(Err(
                        "macOS 未能啟用單鍵監聽；請檢查輔助使用及輸入監控權限。".into(),
                    ));
                } else if ready_tx.send(Ok(())).is_err() {
                    worker_stop.store(true, Ordering::Release);
                }
                let mut last_permission_check = Instant::now();
                let mut draining_since: Option<Instant> = None;
                while worker_health.load(Ordering::Acquire) == RUNNING {
                    if worker_stop.load(Ordering::Acquire) {
                        let since = draining_since.get_or_insert_with(Instant::now);
                        if !context.gesture.down || since.elapsed() >= Duration::from_secs(1) {
                            break;
                        }
                    }
                    let result = CFRunLoop::run_in_mode(mode, 0.1, true);
                    if result == CFRunLoopRunResult::Finished
                        || result == CFRunLoopRunResult::Stopped
                    {
                        worker_health.store(WORKER_FAILED, Ordering::Release);
                        break;
                    }
                    if worker_health.load(Ordering::Acquire) != RUNNING {
                        break;
                    }
                    if !CGEvent::tap_is_enabled(&tap) {
                        worker_health.store(DISABLED, Ordering::Release);
                        break;
                    }
                    if last_permission_check.elapsed() >= Duration::from_secs(1) {
                        last_permission_check = Instant::now();
                        if !unsafe { AXIsProcessTrusted() } {
                            worker_health.store(ACCESSIBILITY_GONE, Ordering::Release);
                            break;
                        }
                        if !CGPreflightListenEventAccess() {
                            worker_health.store(INPUT_GONE, Ordering::Release);
                            break;
                        }
                    }
                    // Only infer a missing KeyUp after the OS has actually
                    // reported this physical key down. Remapped/synthetic keys
                    // need not appear in the HID state table at all.
                    if context.gesture.down && !context.gesture.reconciled {
                        if CGEventSource::key_state(CGEventSourceStateID::HIDSystemState, key.code)
                        {
                            context.observed_hardware_down = true;
                        } else if context.observed_hardware_down
                            && context
                                .down_since
                                .is_some_and(|since| since.elapsed() >= Duration::from_millis(250))
                        {
                            let release = context.gesture.reconcile_released();
                            context.down_since = None;
                            context.send(release);
                        }
                    }
                }
                CGEvent::tap_enable(&tap, false);
                run_loop.remove_source(Some(&source), mode);
                tap.invalidate();
                drop(source);
                drop(tap);
                drop(context);
                let _ = worker_health.compare_exchange(
                    RUNNING,
                    STOPPED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                let _ = dispatcher.join();
            })
            .map_err(|_| "無法啟動單鍵快捷鍵監聽程序。".to_string())?;
        match ready_rx
            .recv()
            .unwrap_or_else(|_| Err("單鍵快捷鍵監聽程序未能啟動。".into()))
        {
            Ok(()) => Ok(SingleKeyShortcut {
                enabled,
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

    #[cfg(test)]
    mod tests {
        use super::*;

        fn event(context: &mut Context, code: u16, down: bool, flags: CGEventFlags) -> bool {
            event_for_target(context, code, down, flags, 0)
        }

        fn event_for_target(
            context: &mut Context,
            code: u16,
            down: bool,
            flags: CGEventFlags,
            pid: i64,
        ) -> bool {
            // Construct an event in memory only; do not post it to the system.
            let event = CGEvent::new_keyboard_event(None, code, down).unwrap();
            CGEvent::set_flags(Some(&event), flags);
            CGEvent::set_integer_value_field(
                Some(&event),
                CGEventField::EventTargetUnixProcessID,
                pid,
            );
            let pointer = NonNull::from(&*event);
            let kind = if down {
                CGEventType::KeyDown
            } else {
                CGEventType::KeyUp
            };
            let returned = unsafe {
                callback(
                    std::ptr::null_mut(),
                    kind,
                    pointer,
                    (context as *mut Context).cast(),
                )
            };
            if !returned.is_null() {
                assert_eq!(returned, pointer.as_ptr());
            }
            returned.is_null()
        }

        fn context(key: SingleKey) -> (Context, mpsc::Receiver<bool>) {
            let (events, receiver) = mpsc::sync_channel(8);
            (
                Context {
                    key,
                    gesture: Gesture::default(),
                    down_since: None,
                    observed_hardware_down: false,
                    enabled: Arc::new(AtomicBool::new(true)),
                    main_focused: Arc::new(AtomicBool::new(false)),
                    stop: Arc::new(AtomicBool::new(false)),
                    health: Arc::new(AtomicU8::new(RUNNING)),
                    events,
                },
                receiver,
            )
        }

        #[test]
        fn callback_only_suppresses_the_selected_unmodified_gesture() {
            let (mut context, receiver) = context(SingleKey {
                code: 0,
                allow_fn_flag: false,
            });
            assert!(!event(&mut context, 1, true, CGEventFlags::empty()));
            assert!(!event(&mut context, 0, true, CGEventFlags::MaskCommand));
            assert!(!event(&mut context, 0, false, CGEventFlags::MaskCommand));
            assert!(!event(&mut context, 0, true, CGEventFlags::MaskSecondaryFn));
            assert!(!event(
                &mut context,
                0,
                false,
                CGEventFlags::MaskSecondaryFn
            ));
            assert!(receiver.try_recv().is_err());
            assert!(event(&mut context, 0, true, CGEventFlags::empty()));
            assert!(event(&mut context, 0, false, CGEventFlags::MaskShift));
            assert_eq!(receiver.try_recv(), Ok(true));
            assert_eq!(receiver.try_recv(), Ok(false));
            assert!(receiver.try_recv().is_err());
        }

        #[test]
        fn callback_accepts_fn_flag_for_function_keys_and_stages_silently() {
            let (mut context, receiver) = context(SingleKey {
                code: 0x7a,
                allow_fn_flag: true,
            });
            context.enabled.store(false, Ordering::Release);
            assert!(!event(
                &mut context,
                0x7a,
                true,
                CGEventFlags::MaskSecondaryFn
            ));
            assert!(!event(
                &mut context,
                0x7a,
                false,
                CGEventFlags::MaskSecondaryFn
            ));
            assert!(receiver.try_recv().is_err());
            context.enabled.store(true, Ordering::Release);
            assert!(event(
                &mut context,
                0x7a,
                true,
                CGEventFlags::MaskSecondaryFn
            ));
            assert!(event(
                &mut context,
                0x7a,
                false,
                CGEventFlags::MaskSecondaryFn
            ));
            assert_eq!(receiver.try_recv(), Ok(true));
            assert_eq!(receiver.try_recv(), Ok(false));
        }

        #[test]
        fn app_focus_and_focus_changes_never_split_a_key_pair() {
            let (mut context, receiver) = context(SingleKey {
                code: 0,
                allow_fn_flag: false,
            });
            context.main_focused.store(true, Ordering::Release);
            assert!(!event(&mut context, 0, true, CGEventFlags::empty()));
            context.main_focused.store(false, Ordering::Release);
            assert!(!event(&mut context, 0, false, CGEventFlags::empty()));
            assert!(receiver.try_recv().is_err());
            assert!(event(&mut context, 0, true, CGEventFlags::empty()));
            context.main_focused.store(true, Ordering::Release);
            assert!(event(&mut context, 0, false, CGEventFlags::empty()));
            assert_eq!(receiver.try_recv(), Ok(true));
            assert_eq!(receiver.try_recv(), Ok(false));
        }

        #[test]
        fn own_process_target_and_shutdown_do_not_split_key_pairs() {
            let (mut context, receiver) = context(SingleKey {
                code: 0,
                allow_fn_flag: false,
            });
            let own_pid = i64::from(std::process::id());
            assert!(!event_for_target(
                &mut context,
                0,
                true,
                CGEventFlags::empty(),
                own_pid
            ));
            assert!(!event_for_target(
                &mut context,
                0,
                false,
                CGEventFlags::empty(),
                own_pid
            ));
            assert!(receiver.try_recv().is_err());
            assert!(event(&mut context, 0, true, CGEventFlags::empty()));
            context.enabled.store(false, Ordering::Release);
            context.stop.store(true, Ordering::Release);
            assert!(event(&mut context, 0, false, CGEventFlags::empty()));
            assert_eq!(receiver.try_recv(), Ok(true));
            assert!(receiver.try_recv().is_err());
        }

        #[test]
        fn failed_dispatch_never_suppresses_the_initial_key_down() {
            let (mut context, receiver) = context(SingleKey {
                code: 0,
                allow_fn_flag: false,
            });
            drop(receiver);
            assert!(!event(&mut context, 0, true, CGEventFlags::empty()));
            assert_eq!(context.health.load(Ordering::Acquire), WORKER_FAILED);
            assert!(!event(&mut context, 0, false, CGEventFlags::empty()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_key_releases_even_if_modifiers_change_and_repeat_is_ignored() {
        let mut gesture = Gesture::default();
        assert_eq!(
            gesture.step(true, false, false),
            Decision {
                event: Some(true),
                suppress: true
            }
        );
        assert_eq!(
            gesture.step(true, true, true),
            Decision {
                event: None,
                suppress: true
            }
        );
        assert_eq!(
            gesture.step(false, false, true),
            Decision {
                event: Some(false),
                suppress: true
            }
        );
        assert_eq!(
            gesture.step(false, false, false),
            Decision {
                event: None,
                suppress: false
            }
        );
    }

    #[test]
    fn modified_presses_do_not_start_recording() {
        let mut gesture = Gesture::default();
        assert_eq!(
            gesture.step(true, false, true),
            Decision {
                event: None,
                suppress: false
            }
        );
        assert_eq!(
            gesture.step(false, false, true),
            Decision {
                event: None,
                suppress: false
            }
        );
        assert_eq!(
            gesture.step(true, true, false),
            Decision {
                event: None,
                suppress: false
            }
        );
        assert_eq!(
            gesture.step(false, false, false),
            Decision {
                event: None,
                suppress: false
            }
        );
        assert_eq!(
            gesture.step(true, false, false),
            Decision {
                event: Some(true),
                suppress: true
            }
        );
    }

    #[test]
    fn held_at_install_waits_for_release_and_missing_up_is_reconciled() {
        let mut gesture = Gesture {
            waiting_for_release: true,
            ..Gesture::default()
        };
        assert_eq!(
            gesture.step(true, true, false),
            Decision {
                event: None,
                suppress: false
            }
        );
        assert_eq!(gesture.reconcile_released(), None);
        assert_eq!(
            gesture.step(true, false, false),
            Decision {
                event: Some(true),
                suppress: true
            }
        );
        assert_eq!(gesture.reconcile_released(), Some(false));
        assert_eq!(gesture.reconcile_released(), None);
        assert_eq!(
            gesture.step(false, false, false),
            Decision {
                event: None,
                suppress: true
            }
        );
    }
}
