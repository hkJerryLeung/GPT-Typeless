//! Native file drag for the setup permission UI (macOS only).
//!
//! Integration: register the command in `generate_handler!`. While the
//! dedicated drag target is visible, send its `getBoundingClientRect()` and
//! `window.innerWidth/innerHeight` to `set_setup_drag_region`; send `null` on
//! teardown. Serialize updates so an old update cannot recreate a removed view.
//! The HTML element underneath is presentation only: this NSView receives the
//! actual mouse events. Keep a separate, keyboard-accessible Finder fallback.
//!
//! `setup-drag-finished` reports `accepted`, `not-accepted` (also cancellation),
//! or `unavailable`. Acceptance is by whichever destination received the drop;
//! it does not mean a macOS permission was enabled.

#![cfg(target_os = "macos")]
use std::{cell::Cell, ffi::CStr, os::unix::ffi::OsStrExt, path::PathBuf};

use objc2::{
    class, define_class, msg_send, rc::Retained, runtime::AnyObject, DefinedClass,
    MainThreadMarker, MainThreadOnly, Message,
};
use objc2_app_kit::{NSDraggingSource, NSEvent, NSEventType, NSView, NSWorkspace};
use objc2_foundation::{NSArray, NSBundle, NSObjectProtocol, NSPoint, NSRect, NSSize, NSURL};
use security_framework::os::macos::code_signing::{Flags, SecCode, SecRequirement};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, WebviewWindow};

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupDragRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub viewport_width: f64,
    pub viewport_height: f64,
}

#[derive(Clone, Serialize)]
struct DragFinished {
    outcome: &'static str,
}

struct MouseDown {
    window_number: isize,
    point: NSPoint,
    event: Retained<NSEvent>,
}

struct SurfaceIvars {
    down: Cell<Option<MouseDown>>,
    dragging: Cell<bool>,
    remove_when_finished: Cell<bool>,
    app: tauri::AppHandle,
}

define_class!(
    // SAFETY: AppKit owns the superview relationship and all methods below
    // are invoked on the main thread. There is no Rust Drop implementation.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "GPTTypelessSetupDragSurface"]
    #[ivars = SurfaceIvars]
    struct SetupDragSurface;

    // SAFETY: No additional NSObjectProtocol requirements.
    unsafe impl NSObjectProtocol for SetupDragSurface {}
    // SAFETY: The required selector is implemented below with AppKit's
    // ABI; NSDraggingSession is not enabled in the existing crate features.
    unsafe impl NSDraggingSource for SetupDragSurface {}

    impl SetupDragSurface {
        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool { false }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool { true }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.ivars().down.set(None);
            if self.ivars().dragging.get()
                || event.r#type() != NSEventType::LeftMouseDown
                || event.buttonNumber() != 0
                || NSEvent::pressedMouseButtons() & 1 == 0
            {
                return;
            }
            let Some(window) = self.window() else { return };
            if event.windowNumber() != window.windowNumber() { return; }
            self.ivars().down.set(Some(MouseDown {
                window_number: event.windowNumber(),
                point: event.locationInWindow(),
                event: event.retain(),
            }));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            self.ivars().down.set(None);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let Some(down) = self.ivars().down.take() else { return };
            if self.ivars().dragging.get()
                || event.r#type() != NSEventType::LeftMouseDragged
                || event.buttonNumber() != 0
                || NSEvent::pressedMouseButtons() & 1 == 0
                || event.windowNumber() != down.window_number
                || self.window().map(|w| w.windowNumber()) != Some(down.window_number)
            {
                self.ivars().down.set(None);
                return;
            }
            let current = event.locationInWindow();
            let dx = current.x - down.point.x;
            let dy = current.y - down.point.y;
            if dx * dx + dy * dy < 16.0 {
                self.ivars().down.set(Some(down));
                return;
            }
            self.ivars().dragging.set(true);
            if self.begin_file_drag(&down.event).is_err() {
                self.ivars().dragging.set(false);
                self.finished("unavailable");
            }
        }

        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn drag_operations(&self, _session: &AnyObject, _context: isize) -> usize {
            // NSDragOperationCopy. Never offer move/delete.
            1
        }

        #[unsafe(method(draggingSession:endedAtPoint:operation:))]
        fn dragging_ended(&self, _session: &AnyObject, _point: NSPoint, operation: usize) {
            self.ivars().down.set(None);
            self.ivars().dragging.set(false);
            self.finished(if operation == 0 { "not-accepted" } else { "accepted" });
        }
    }
);

impl SetupDragSurface {
    fn finished(&self, outcome: &'static str) {
        // Keep the view alive if removing it drops the superview's last retain.
        let keep_alive = self.retain();
        let _ = self
            .ivars()
            .app
            .emit_to("main", "setup-drag-finished", DragFinished { outcome });
        if self.ivars().remove_when_finished.replace(false) {
            self.removeFromSuperview();
        }
        drop(keep_alive);
    }

    fn begin_file_drag(&self, event: &NSEvent) -> Result<(), String> {
        let url = running_bundle_url()?;
        let path = url.path().ok_or("The app URL has no filesystem path")?;
        let icon = NSWorkspace::sharedWorkspace().iconForFile(&path);
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        let frame = NSRect::new(
            NSPoint::new(point.x - 16.0, point.y - 16.0),
            NSSize::new(32.0, 32.0),
        );
        // Signature/path validation can take time. Do not start a session after
        // the user has released the mouse button while validation was running.
        if NSEvent::pressedMouseButtons() & 1 == 0 {
            return Err("The drag was released before it started".into());
        }
        // SAFETY: AppKit's NSDraggingItem takes an NSURL conforming to
        // NSPasteboardWriting. Its array, icon, event and source remain alive
        // through the call; AppKit owns the resulting session. The event is
        // the original left-mouse-down NSEvent supplied directly by NSView.
        unsafe {
            let allocated: *mut AnyObject = msg_send![class!(NSDraggingItem), alloc];
            if allocated.is_null() {
                return Err("Could not allocate a native drag item".into());
            }
            let item: *mut AnyObject = msg_send![allocated, initWithPasteboardWriter: &*url];
            let item = Retained::from_raw(item).ok_or("Could not create a native drag item")?;
            let _: () = msg_send![&*item, setDraggingFrame: frame, contents: &*icon];
            let items = NSArray::from_slice(&[&*item]);
            let _: Retained<AnyObject> =
                msg_send![self, beginDraggingSessionWithItems: &*items, event: event, source: self];
        }
        Ok(())
    }
}

// Return only the current, signed app bundle; never accept a path from JS.
fn running_bundle_url() -> Result<Retained<NSURL>, String> {
    if MainThreadMarker::new().is_none() {
        return Err("AppKit requires the main thread".into());
    }
    let bundle = NSBundle::mainBundle();
    let url = bundle.bundleURL();
    let bundle_path = url_path(&url)?
        .canonicalize()
        .map_err(|_| "Cannot resolve the running app bundle")?;
    if bundle_path.extension().and_then(|s| s.to_str()) != Some("app") || !bundle_path.is_dir() {
        return Err("The running process is not inside an app bundle".into());
    }
    let executable = bundle
        .executableURL()
        .ok_or("The app has no executable URL")?;
    let executable_path = url_path(&executable)?
        .canonicalize()
        .map_err(|_| "Cannot resolve the app executable")?;
    let running_executable = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|_| "Cannot resolve the running executable")?;
    if executable_path != running_executable
        || executable_path.parent() != Some(bundle_path.join("Contents/MacOS").as_path())
    {
        return Err("The bundle does not contain the running executable".into());
    }
    let code =
        SecCode::for_self(Flags::NONE).map_err(|_| "Cannot identify the running signed app")?;
    let requirement: SecRequirement = "true"
        .parse()
        .map_err(|_| "Cannot create the code-signing requirement")?;
    code.check_validity(Flags::NO_NETWORK_ACCESS, &requirement)
        .map_err(|_| "The running app signature is invalid")?;
    let signed_path = code
        .path(Flags::NONE)
        .ok()
        .and_then(|url| url.to_path())
        .ok_or("Cannot resolve the signed app path")?;
    if signed_path.canonicalize().ok().as_deref() != Some(bundle_path.as_path()) {
        return Err("The signed code path does not match the running app bundle".into());
    }
    Ok(url)
}

fn url_path(url: &NSURL) -> Result<PathBuf, String> {
    if !url.isFileURL() {
        return Err("The app bundle URL is not a file URL".into());
    }
    // SAFETY: NSURL owns a NUL-terminated filesystem representation for the
    // lifetime of this borrow. OsStr preserves non-UTF-8 path bytes.
    let bytes = unsafe { CStr::from_ptr(url.fileSystemRepresentation().as_ptr()) }.to_bytes();
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

fn native_frame(
    rect: SetupDragRect,
    bounds: NSRect,
    flipped: bool,
) -> Result<Option<NSRect>, String> {
    let values = [
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        rect.viewport_width,
        rect.viewport_height,
    ];
    if values.iter().any(|n| !n.is_finite())
        || rect.width < 0.0
        || rect.height < 0.0
        || rect.viewport_width < 1.0
        || rect.viewport_height < 1.0
        || !bounds.origin.x.is_finite()
        || !bounds.origin.y.is_finite()
        || !bounds.size.width.is_finite()
        || !bounds.size.height.is_finite()
        || bounds.size.width <= 0.0
        || bounds.size.height <= 0.0
    {
        return Err("Invalid drag target geometry".into());
    }
    let left = rect.x.clamp(0.0, rect.viewport_width);
    let top = rect.y.clamp(0.0, rect.viewport_height);
    let right = (rect.x + rect.width).clamp(0.0, rect.viewport_width);
    let bottom = (rect.y + rect.height).clamp(0.0, rect.viewport_height);
    if right <= left || bottom <= top {
        return Ok(None);
    }
    let sx = bounds.size.width / rect.viewport_width;
    let sy = bounds.size.height / rect.viewport_height;
    let y = if flipped {
        top * sy
    } else {
        (rect.viewport_height - bottom) * sy
    };
    Ok(Some(NSRect::new(
        NSPoint::new(bounds.origin.x + left * sx, bounds.origin.y + y),
        NSSize::new((right - left) * sx, (bottom - top) * sy),
    )))
}

fn update_surface(
    webview: &NSView,
    rect: Option<SetupDragRect>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("AppKit requires the main thread")?;
    let frame = match rect {
        Some(rect) => native_frame(rect, webview.bounds(), webview.isFlipped())?,
        None => None,
    };
    let subviews = webview.subviews();
    let existing = subviews
        .iter()
        .find_map(|v| v.downcast::<SetupDragSurface>().ok());
    if let Some(view) = existing {
        if let Some(frame) = frame {
            view.ivars().remove_when_finished.set(false);
            view.setHidden(false);
            view.setFrame(frame);
        } else if view.ivars().dragging.get() {
            view.ivars().down.set(None);
            view.ivars().remove_when_finished.set(true);
            view.setHidden(true);
        } else {
            view.removeFromSuperview();
        }
    } else if let Some(frame) = frame {
        // Fail early if this is an unbundled dev build or an invalid signature.
        running_bundle_url()?;
        let allocated = mtm.alloc().set_ivars(SurfaceIvars {
            down: Cell::new(None),
            dragging: Cell::new(false),
            remove_when_finished: Cell::new(false),
            app,
        });
        // SAFETY: NSView is initialized with its own valid frame, on the
        // main thread. The superview retains the returned native view.
        unsafe {
            let view: Retained<SetupDragSurface> =
                msg_send![super(allocated), initWithFrame: frame];
            webview.addSubview(&view);
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn set_setup_drag_region(
    window: WebviewWindow,
    rect: Option<SetupDragRect>,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("The setup drag target belongs to the main window".into());
    }
    let app = window.app_handle().clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    window
        .with_webview(move |webview| {
            let pointer = webview.inner();
            let result = if pointer.is_null() || MainThreadMarker::new().is_none() {
                Err("Native webview is unavailable".into())
            } else {
                // SAFETY: Tauri supplies a live WKWebView (an NSView) on the
                // main thread, borrowed only within this callback.
                let view = unsafe { &*pointer.cast::<NSView>() };
                update_surface(view, rect, app)
            };
            let _ = tx.send(result);
        })
        .map_err(|_| "Cannot access the native webview")?;
    rx.await
        .map_err(|_| "Native webview closed before updating the drag target")?
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect() -> SetupDragRect {
        SetupDragRect {
            x: 10.0,
            y: 20.0,
            width: 30.0,
            height: 40.0,
            viewport_width: 100.0,
            viewport_height: 200.0,
        }
    }
    #[test]
    fn converts_and_clips_both_coordinate_systems() {
        let bounds = NSRect::new(NSPoint::new(2.0, 3.0), NSSize::new(200.0, 400.0));
        let flipped = native_frame(rect(), bounds, true).unwrap().unwrap();
        assert_eq!(
            (
                flipped.origin.x,
                flipped.origin.y,
                flipped.size.width,
                flipped.size.height
            ),
            (22.0, 43.0, 60.0, 80.0)
        );
        let unflipped = native_frame(rect(), bounds, false).unwrap().unwrap();
        assert_eq!(unflipped.origin.y, 283.0);
        let mut clipped = rect();
        clipped.x = -20.0;
        assert_eq!(
            native_frame(clipped, bounds, true)
                .unwrap()
                .unwrap()
                .size
                .width,
            20.0
        );
        clipped.x = 200.0;
        assert!(native_frame(clipped, bounds, true).unwrap().is_none());
    }
    #[test]
    fn rejects_invalid_geometry() {
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 200.0));
        let mut bad = rect();
        bad.width = f64::NAN;
        assert!(native_frame(bad, bounds, true).is_err());
        bad = rect();
        bad.viewport_height = 0.0;
        assert!(native_frame(bad, bounds, true).is_err());
    }
}
