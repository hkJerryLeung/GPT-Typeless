use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, MutexGuard,
};
use std::time::Duration;
use tauri::{State, WebviewWindow};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::watch;

const UPDATE_BUSY: &str = "UPDATE_BUSY";
const RECORDING_BUSY: &str = "RECORDING_BUSY";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Starting,
    Recording,
    Stopping,
    AwaitingProcessing,
    Processing,
    Cancelling,
    Testing,
    Saving,
    Installing,
}

struct Activity {
    id: u64,
    phase: Phase,
}

struct NetworkOperation {
    id: u64,
    cancel: watch::Sender<bool>,
}

#[derive(Clone, Copy)]
enum LeaseKind {
    Activity,
    Network,
}

enum PreparedUpdate {
    Checked(Update),
    // Only Update::download can populate this variant. The bytes are never
    // exposed to the webview or modified between verification and installation.
    Downloaded(Update, Vec<u8>),
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    activity: Option<Activity>,
    network: Option<NetworkOperation>,
    prepared: Option<PreparedUpdate>,
}

#[derive(Clone, Default)]
pub(crate) struct ActivityGate(Arc<Mutex<Inner>>);

impl ActivityGate {
    fn lock(&self) -> Result<MutexGuard<'_, Inner>, String> {
        self.0.lock().map_err(|_| UPDATE_BUSY.to_string())
    }

    fn next_id(inner: &mut Inner) -> u64 {
        inner.next_id = inner.next_id.wrapping_add(1);
        inner.next_id
    }

    fn reserve(inner: &mut Inner, phase: Phase) -> u64 {
        let id = Self::next_id(inner);
        inner.activity = Some(Activity { id, phase });
        id
    }

    fn lease(&self, id: u64, kind: LeaseKind) -> Lease {
        Lease(Arc::new(LeaseOwner {
            gate: self.clone(),
            id,
            kind,
            release: AtomicBool::new(true),
        }))
    }

    fn begin(&self, phase: Phase) -> Result<Lease, String> {
        let mut inner = self.lock()?;
        if let Some(activity) = &inner.activity {
            return Err(if activity.phase == Phase::Installing {
                UPDATE_BUSY
            } else {
                RECORDING_BUSY
            }
            .into());
        }
        let id = Self::reserve(&mut inner, phase);
        Ok(self.lease(id, LeaseKind::Activity))
    }

    pub(crate) fn begin_recording(&self) -> Result<Lease, String> {
        self.begin(Phase::Starting)
    }

    pub(crate) fn begin_test(&self) -> Result<Lease, String> {
        self.begin(Phase::Testing)
    }

    pub(crate) fn begin_standalone_processing(&self) -> Result<Lease, String> {
        self.begin(Phase::Processing)
    }

    pub(crate) fn begin_settings_save(&self) -> Result<Lease, String> {
        self.begin(Phase::Saving)
    }

    // The stopped recording remains reserved across the separate frontend
    // stop_recording and complete_dictation calls.
    pub(crate) fn begin_processing(&self) -> Result<Lease, String> {
        let mut inner = self.lock()?;
        let id = match &mut inner.activity {
            Some(activity) if activity.phase == Phase::AwaitingProcessing => {
                activity.phase = Phase::Processing;
                activity.id
            }
            Some(activity) => {
                return Err(if activity.phase == Phase::Installing {
                    UPDATE_BUSY
                } else {
                    RECORDING_BUSY
                }
                .into());
            }
            None => Self::reserve(&mut inner, Phase::Processing),
        };
        Ok(self.lease(id, LeaseKind::Activity))
    }

    pub(crate) fn begin_stop(&self) -> Result<StopGuard, String> {
        let mut inner = self.lock()?;
        match &mut inner.activity {
            Some(activity) if activity.phase == Phase::Recording => {
                activity.phase = Phase::Stopping;
                Ok(StopGuard {
                    gate: self.clone(),
                    id: activity.id,
                })
            }
            Some(activity) if activity.phase == Phase::Installing => Err(UPDATE_BUSY.into()),
            _ => Err(RECORDING_BUSY.into()),
        }
    }

    pub(crate) fn begin_cancel(&self) -> Result<Option<CancelLease>, String> {
        let mut inner = self.lock()?;
        match &mut inner.activity {
            None => Ok(None),
            Some(activity)
                if matches!(activity.phase, Phase::Recording | Phase::AwaitingProcessing) =>
            {
                let previous = activity.phase;
                activity.phase = Phase::Cancelling;
                Ok(Some(CancelLease(Arc::new(CancelOwner {
                    gate: self.clone(),
                    id: activity.id,
                    previous,
                    stopped: AtomicBool::new(false),
                }))))
            }
            Some(activity) if activity.phase == Phase::Installing => Err(UPDATE_BUSY.into()),
            _ => Err(RECORDING_BUSY.into()),
        }
    }

    fn network_allowed(inner: &Inner) -> bool {
        inner.network.is_none()
            && !inner
                .activity
                .as_ref()
                .is_some_and(|activity| activity.phase == Phase::Installing)
    }

    fn reserve_network(&self, inner: &mut Inner) -> (Lease, watch::Receiver<bool>) {
        let id = Self::next_id(inner);
        let (cancel, receiver) = watch::channel(false);
        inner.network = Some(NetworkOperation { id, cancel });
        (self.lease(id, LeaseKind::Network), receiver)
    }

    fn begin_update(&self) -> Result<(Lease, watch::Receiver<bool>), String> {
        let mut inner = self.lock()?;
        if !Self::network_allowed(&inner) {
            return Err(UPDATE_BUSY.into());
        }
        Ok(self.reserve_network(&mut inner))
    }

    fn begin_download(&self) -> Result<(Lease, watch::Receiver<bool>, Update), String> {
        let mut inner = self.lock()?;
        if !Self::network_allowed(&inner) {
            return Err(UPDATE_BUSY.into());
        }
        let update = match &inner.prepared {
            Some(PreparedUpdate::Checked(update)) | Some(PreparedUpdate::Downloaded(update, _)) => {
                update.clone()
            }
            None => return Err("UPDATE_NOT_AVAILABLE".into()),
        };
        // Drop old bytes before downloading again.
        inner.prepared = Some(PreparedUpdate::Checked(update.clone()));
        let (lease, receiver) = self.reserve_network(&mut inner);
        Ok((lease, receiver, update))
    }

    fn begin_install(&self) -> Result<(Lease, Update, Vec<u8>), String> {
        let mut inner = self.lock()?;
        if inner.activity.is_some() || inner.network.is_some() {
            return Err(UPDATE_BUSY.into());
        }
        if !matches!(inner.prepared, Some(PreparedUpdate::Downloaded(_, _))) {
            return Err("UPDATE_NOT_DOWNLOADED".into());
        }
        let Some(PreparedUpdate::Downloaded(update, bytes)) = inner.prepared.take() else {
            unreachable!();
        };
        let id = Self::reserve(&mut inner, Phase::Installing);
        Ok((self.lease(id, LeaseKind::Activity), update, bytes))
    }

    fn store_prepared(
        &self,
        lease: &Lease,
        prepared: Option<PreparedUpdate>,
    ) -> Result<(), String> {
        let mut inner = self.lock()?;
        if !inner
            .network
            .as_ref()
            .is_some_and(|network| network.id == lease.0.id && !*network.cancel.borrow())
        {
            return Err("UPDATE_CANCELLED".into());
        }
        inner.prepared = prepared;
        Ok(())
    }

    pub(crate) fn cancel_update(&self) -> Result<(), String> {
        let mut inner = self.lock()?;
        if inner
            .activity
            .as_ref()
            .is_some_and(|activity| activity.phase == Phase::Installing)
        {
            return Err("UPDATE_INSTALLING".into());
        }
        if let Some(network) = &inner.network {
            let _ = network.cancel.send(true);
        }
        inner.prepared = None;
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct Lease(Arc<LeaseOwner>);

struct LeaseOwner {
    gate: ActivityGate,
    id: u64,
    kind: LeaseKind,
    release: AtomicBool,
}

impl Lease {
    pub(crate) fn recording_started(self) -> Result<(), String> {
        let mut inner = self.0.gate.lock()?;
        let activity = inner
            .activity
            .as_mut()
            .filter(|activity| activity.id == self.0.id)
            .ok_or_else(|| RECORDING_BUSY.to_string())?;
        activity.phase = Phase::Recording;
        self.0.release.store(false, Ordering::SeqCst);
        Ok(())
    }
}

impl Drop for LeaseOwner {
    fn drop(&mut self) {
        if !self.release.load(Ordering::SeqCst) {
            return;
        }
        if let Ok(mut inner) = self.gate.0.lock() {
            match self.kind {
                LeaseKind::Activity
                    if inner
                        .activity
                        .as_ref()
                        .is_some_and(|activity| activity.id == self.id) =>
                {
                    inner.activity = None;
                }
                LeaseKind::Network
                    if inner
                        .network
                        .as_ref()
                        .is_some_and(|network| network.id == self.id) =>
                {
                    inner.network = None;
                }
                _ => {}
            }
        }
    }
}

pub(crate) struct StopGuard {
    gate: ActivityGate,
    id: u64,
}

impl StopGuard {
    pub(crate) fn finished(&self) {
        self.transition(Some(Phase::AwaitingProcessing));
    }

    pub(crate) fn no_recording(&self) {
        self.transition(None);
    }

    fn transition(&self, phase: Option<Phase>) {
        if let Ok(mut inner) = self.gate.0.lock() {
            if inner
                .activity
                .as_ref()
                .is_some_and(|activity| activity.id == self.id && activity.phase == Phase::Stopping)
            {
                inner.activity = phase.map(|phase| Activity { id: self.id, phase });
            }
        }
    }
}

impl Drop for StopGuard {
    fn drop(&mut self) {
        // Unknown stop state fails closed; cancellation can inspect the audio worker.
        self.transition(Some(Phase::Recording));
    }
}

#[derive(Clone)]
pub(crate) struct CancelLease(Arc<CancelOwner>);

struct CancelOwner {
    gate: ActivityGate,
    id: u64,
    previous: Phase,
    stopped: AtomicBool,
}

impl CancelLease {
    pub(crate) fn stopped(&self) {
        self.0.stopped.store(true, Ordering::SeqCst);
    }
}

impl Drop for CancelOwner {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.gate.0.lock() {
            if inner.activity.as_ref().is_some_and(|activity| {
                activity.id == self.id && activity.phase == Phase::Cancelling
            }) {
                inner.activity = if self.stopped.load(Ordering::SeqCst) {
                    None
                } else {
                    Some(Activity {
                        id: self.id,
                        phase: self.previous,
                    })
                };
            }
        }
    }
}

fn secure_plugin_config(value: Option<&serde_json::Value>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let Ok(config) = serde_json::from_value::<tauri_plugin_updater::Config>(value.clone()) else {
        return false;
    };
    config.require_signed_version
        && !config.allow_downgrades
        && !config.dangerous_insecure_transport_protocol
        && !config.dangerous_accept_invalid_certs
        && !config.dangerous_accept_invalid_hostnames
        && !config.pubkey.trim().is_empty()
        && !config.endpoints.is_empty()
        && config
            .endpoints
            .iter()
            .all(|endpoint| endpoint.scheme() == "https")
}

pub(crate) fn configured(config: &tauri::Config) -> bool {
    secure_plugin_config(config.plugins.0.get("updater"))
}

fn ensure_main(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("UPDATE_NOT_ALLOWED".into())
    }
}

// Query only a system idle duration; no keys or input contents are captured.
fn system_is_idle() -> bool {
    #[cfg(target_os = "macos")]
    {
        use objc2_core_graphics::{CGEventSource, CGEventSourceStateID, CGEventType};
        let seconds = CGEventSource::seconds_since_last_event_type(
            CGEventSourceStateID::CombinedSessionState,
            CGEventType(u32::MAX),
        );
        seconds.is_finite() && seconds >= 300.0
    }
    #[cfg(not(target_os = "macos"))]
    false
}

#[tauri::command]
pub(crate) fn update_system_idle(window: WebviewWindow) -> Result<bool, String> {
    ensure_main(&window)?;
    Ok(system_is_idle())
}

#[derive(Serialize)]
pub(crate) struct UpdateInfo {
    version: String,
}

#[tauri::command]
pub(crate) async fn check_update(
    app: tauri::AppHandle,
    window: WebviewWindow,
    state: State<'_, crate::AppState>,
) -> Result<Option<UpdateInfo>, String> {
    ensure_main(&window)?;
    if !configured(app.config()) {
        return Err("UPDATE_NOT_CONFIGURED".into());
    }
    let (lease, mut cancel) = state.update_gate.begin_update()?;
    state.update_gate.store_prepared(&lease, None)?;
    let updater = app
        .updater_builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|_| "UPDATE_CHECK_FAILED".to_string())?;
    let update = tokio::select! {
        biased;
        _ = cancel.changed() => return Err("UPDATE_CANCELLED".into()),
        result = updater.check() => result.map_err(|_| "UPDATE_CHECK_FAILED".to_string())?,
    };
    if update
        .as_ref()
        .is_some_and(|update| update.download_url.scheme() != "https")
    {
        return Err("UPDATE_CHECK_FAILED".into());
    }
    let info = update.as_ref().map(|update| UpdateInfo {
        version: update.version.clone(),
    });
    state
        .update_gate
        .store_prepared(&lease, update.map(PreparedUpdate::Checked))?;
    Ok(info)
}

#[tauri::command]
pub(crate) async fn download_update(
    window: WebviewWindow,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    ensure_main(&window)?;
    let (lease, mut cancel, update) = state.update_gate.begin_download()?;
    let bytes = tokio::select! {
        biased;
        _ = cancel.changed() => return Err("UPDATE_CANCELLED".into()),
        result = update.download(|_, _| {}, || {}) => result.map_err(|_| "UPDATE_DOWNLOAD_FAILED".to_string())?,
    };
    state
        .update_gate
        .store_prepared(&lease, Some(PreparedUpdate::Downloaded(update, bytes)))
}

#[tauri::command]
pub(crate) async fn install_update(
    automatic: bool,
    app: tauri::AppHandle,
    window: WebviewWindow,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    ensure_main(&window)?;
    if automatic && !system_is_idle() {
        return Err("UPDATE_NOT_IDLE".into());
    }
    let (lease, update, bytes) = state.update_gate.begin_install()?;
    // The blocking task owns the gate even if the invoking webview goes away.
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        update
            .install(&bytes)
            .map_err(|_| "UPDATE_INSTALL_FAILED".to_string())?;
        app.restart();
    })
    .await
    .map_err(|_| "UPDATE_INSTALL_FAILED".to_string())?
}

#[tauri::command]
pub(crate) fn cancel_update(
    window: WebviewWindow,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    ensure_main(&window)?;
    state.update_gate.cancel_update()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_update() -> Update {
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().plugins.0.insert(
            "updater".into(),
            serde_json::json!({
                "pubkey": "fixture-public-key",
                "endpoints": ["https://example.invalid/latest.json"],
                "requireSignedVersion": true
            }),
        );
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context)
            .unwrap();
        app.updater_builder()
            .target("test-target")
            .build()
            .unwrap()
            .restore_update(serde_json::json!({
                "version": "2.0.0",
                "platforms": { "test-target": {
                    "url": "https://example.invalid/package",
                    "signature": "fixture-signature"
                }}
            }))
            .unwrap()
            .unwrap()
    }

    fn prepare_fixture(gate: &ActivityGate) {
        let (checking, _cancelled) = gate.begin_update().unwrap();
        gate.store_prepared(
            &checking,
            Some(PreparedUpdate::Downloaded(fixture_update(), vec![1, 2, 3])),
        )
        .unwrap();
    }

    #[test]
    fn network_work_can_overlap_recording_but_install_is_blocked_until_processing_finishes() {
        let gate = ActivityGate::default();
        let start = gate.begin_recording().unwrap();
        let (checking, _) = gate.begin_update().unwrap();
        assert!(matches!(gate.begin_install(), Err(ref error) if error == UPDATE_BUSY));
        start.recording_started().unwrap();
        let stop = gate.begin_stop().unwrap();
        assert!(matches!(gate.begin_install(), Err(ref error) if error == UPDATE_BUSY));
        stop.finished();
        drop(stop);
        drop(checking);
        assert!(matches!(gate.begin_install(), Err(ref error) if error == UPDATE_BUSY));
        let processing = gate.begin_processing().unwrap();
        assert!(matches!(gate.begin_install(), Err(ref error) if error == UPDATE_BUSY));
        drop(processing);
        assert!(matches!(gate.begin_install(), Err(ref error) if error == "UPDATE_NOT_DOWNLOADED"));
    }

    #[test]
    fn network_work_and_microphone_test_do_not_exclude_each_other() {
        let gate = ActivityGate::default();
        let (update, _) = gate.begin_update().unwrap();
        let test = gate.begin_test().unwrap();
        assert!(matches!(gate.begin_install(), Err(ref error) if error == UPDATE_BUSY));
        drop(test);
        let recording = gate.begin_recording().unwrap();
        assert!(matches!(gate.begin_install(), Err(ref error) if error == UPDATE_BUSY));
        drop(recording);
        assert!(matches!(gate.begin_install(), Err(ref error) if error == UPDATE_BUSY));
        drop(update);
        assert!(gate.begin_recording().is_ok());
    }

    #[test]
    fn cancel_and_stop_fail_closed_until_recording_is_confirmed_stopped() {
        let gate = ActivityGate::default();
        gate.begin_recording().unwrap().recording_started().unwrap();
        drop(gate.begin_stop().unwrap());
        drop(gate.begin_cancel().unwrap().unwrap());
        assert!(matches!(gate.begin_install(), Err(ref error) if error == UPDATE_BUSY));
        let cancel = gate.begin_cancel().unwrap().unwrap();
        let worker = cancel.clone();
        worker.stopped();
        drop(cancel);
        assert!(matches!(gate.begin_install(), Err(ref error) if error == UPDATE_BUSY));
        drop(worker);
        assert!(matches!(gate.begin_install(), Err(ref error) if error == "UPDATE_NOT_DOWNLOADED"));
    }

    #[tokio::test]
    async fn cancellation_invalidates_inflight_results_and_install_cannot_be_cancelled() {
        let gate = ActivityGate::default();
        let (checking, mut cancelled) = gate.begin_update().unwrap();
        gate.cancel_update().unwrap();
        cancelled.changed().await.unwrap();
        assert!(gate.store_prepared(&checking, None).is_err());
        drop(checking);
        prepare_fixture(&gate);
        let (installing, _, _) = gate.begin_install().unwrap();
        assert!(gate.begin_recording().is_err());
        assert!(gate.begin_update().is_err());
        assert_eq!(gate.cancel_update().unwrap_err(), "UPDATE_INSTALLING");
        drop(installing);
    }

    #[test]
    fn cancelling_discards_prepared_bytes() {
        let gate = ActivityGate::default();
        prepare_fixture(&gate);
        let (checking, cancelled) = gate.begin_update().unwrap();
        gate.cancel_update().unwrap();
        assert!(*cancelled.borrow());
        assert!(gate.lock().unwrap().prepared.is_none());
        assert!(gate.store_prepared(&checking, None).is_err());
        drop(checking);
        assert!(matches!(gate.begin_install(), Err(ref error) if error == "UPDATE_NOT_DOWNLOADED"));
    }

    #[test]
    fn simultaneous_recording_and_installation_cannot_both_start() {
        use std::sync::{mpsc, Barrier};
        let gate = ActivityGate::default();
        prepare_fixture(&gate);
        let start = Arc::new(Barrier::new(3));
        let finish = Arc::new(Barrier::new(3));
        let (sender, receiver) = mpsc::channel();
        let mut threads = Vec::new();
        for installing in [false, true] {
            let gate = gate.clone();
            let start = start.clone();
            let finish = finish.clone();
            let sender = sender.clone();
            threads.push(std::thread::spawn(move || {
                start.wait();
                let lease = if installing {
                    gate.begin_install().map(|(lease, _, _)| lease)
                } else {
                    gate.begin_recording()
                };
                sender.send(lease.is_ok()).unwrap();
                finish.wait();
                drop(lease);
            }));
        }
        start.wait();
        let first = receiver.recv().unwrap();
        let second = receiver.recv().unwrap();
        finish.wait();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_ne!(first, second);
    }

    #[test]
    fn updater_configuration_requires_https_and_signed_versions() {
        let value = serde_json::json!({"pubkey":"test-public-key", "endpoints":["https://example.invalid/latest.json"], "requireSignedVersion":true});
        assert!(secure_plugin_config(Some(&value)));
        assert!(!secure_plugin_config(None));
        let mut unsafe_value = value.clone();
        unsafe_value["requireSignedVersion"] = false.into();
        assert!(!secure_plugin_config(Some(&unsafe_value)));
        unsafe_value = value.clone();
        unsafe_value["allowDowngrades"] = true.into();
        assert!(!secure_plugin_config(Some(&unsafe_value)));
        unsafe_value = value;
        unsafe_value["endpoints"] = serde_json::json!(["http://example.invalid/latest.json"]);
        assert!(!secure_plugin_config(Some(&unsafe_value)));
    }
}
