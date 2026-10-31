// GPTService Tauri - Audio Recording Module

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

const AUDIO_QUEUE_CAPACITY: usize = 64;
const AUDIO_POLL_INTERVAL: Duration = Duration::from_millis(10);
const LEVEL_STALE_AFTER: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct AudioLevel {
    pub recording: bool,
    pub rms: f32,
}

#[derive(Default)]
struct LevelState {
    recording: bool,
    rms: f32,
    updated_at: Option<Instant>,
}

impl LevelState {
    fn reading(&self, now: Instant) -> AudioLevel {
        let fresh = self
            .updated_at
            .is_some_and(|updated| now.saturating_duration_since(updated) <= LEVEL_STALE_AFTER);
        AudioLevel {
            recording: self.recording,
            rms: if self.recording && fresh {
                self.rms
            } else {
                0.0
            },
        }
    }
}

// Only scalar level metadata is shared. No audio/device handle or PCM is sent
// to the UI, and the capture callback never waits for a display reader.
#[derive(Clone, Default)]
pub struct AudioLevelMonitor {
    state: Arc<Mutex<LevelState>>,
}

impl AudioLevelMonitor {
    pub fn snapshot(&self) -> AudioLevel {
        self.state
            .lock()
            .map(|state| state.reading(Instant::now()))
            .unwrap_or_default()
    }

    fn publish(&self, samples: &[i16]) {
        let rms = pcm_rms(samples);
        if let Ok(mut state) = self.state.try_lock() {
            state.rms = rms;
            state.updated_at = Some(Instant::now());
        }
    }

    fn activate(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.recording = true;
        }
    }

    fn reset(&self) {
        if let Ok(mut state) = self.state.lock() {
            *state = LevelState::default();
        }
    }
}

fn pcm_rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum = samples
        .iter()
        .map(|sample| {
            let normalized = f64::from(*sample) / 32_768.0;
            normalized * normalized
        })
        .sum::<f64>();
    (sum / samples.len() as f64).sqrt() as f32
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDevice {
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopRecordingResult {
    pub audio_path: String,
    pub duration: u64,
}

type Reply<T> = Sender<Result<T, String>>;

enum AudioRequest {
    ListDevices(Reply<Vec<AudioDevice>>),
    StartRecording {
        device_name: Option<String>,
        reply: Reply<String>,
    },
    StopRecording(Reply<StopRecordingResult>),
    RecordingStatus(Reply<Option<String>>),
    CleanupOldRecordings {
        days: u64,
        reply: Reply<usize>,
    },
}

// CPAL objects and WAV writers stay on the owner thread. Shared level metadata
// contains no native handles, so this remains Send + Sync without unsafe impls.
#[derive(Clone)]
pub struct AudioManager {
    requests: Sender<AudioRequest>,
    level_monitor: AudioLevelMonitor,
}

impl AudioManager {
    pub fn new(recordings_dir: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&recordings_dir).map_err(|e| format!("無法建立錄音目錄: {}", e))?;

        let (requests, receiver) = mpsc::channel();
        let level_monitor = AudioLevelMonitor::default();
        let worker_monitor = level_monitor.clone();
        // The worker exits and finalizes any recording when the final sender
        // handle is dropped. It never owns a clone of the request sender.
        let _ = thread::Builder::new()
            .name("audio-owner".to_string())
            .spawn(move || {
                AudioWorker {
                    recordings_dir,
                    current_session: None,
                    level_monitor: worker_monitor,
                }
                .run(receiver);
            })
            .map_err(|e| format!("無法啟動錄音執行緒: {}", e))?;

        Ok(Self {
            requests,
            level_monitor,
        })
    }

    pub fn level_monitor(&self) -> AudioLevelMonitor {
        self.level_monitor.clone()
    }

    fn request<T>(&self, request: impl FnOnce(Reply<T>) -> AudioRequest) -> Result<T, String> {
        let (reply, receiver) = mpsc::channel();
        self.requests
            .send(request(reply))
            .map_err(|_| "錄音執行緒已停止".to_string())?;
        receiver
            .recv()
            .map_err(|_| "錄音執行緒未能回覆".to_string())?
    }

    pub fn list_devices(&self) -> Result<Vec<AudioDevice>, String> {
        self.request(AudioRequest::ListDevices)
    }

    pub fn start_recording(&self, device_name: Option<String>) -> Result<String, String> {
        self.request(|reply| AudioRequest::StartRecording { device_name, reply })
    }

    pub fn stop_recording(&self) -> Result<StopRecordingResult, String> {
        self.request(AudioRequest::StopRecording)
    }

    pub fn get_recording_status(&self) -> Result<Option<String>, String> {
        self.request(AudioRequest::RecordingStatus)
    }

    pub fn cleanup_old_recordings(&self, days: u64) -> Result<usize, String> {
        self.request(|reply| AudioRequest::CleanupOldRecordings { days, reply })
    }
}

struct AudioWorker {
    recordings_dir: PathBuf,
    current_session: Option<RecordingSession>,
    level_monitor: AudioLevelMonitor,
}

impl AudioWorker {
    fn run(mut self, requests: Receiver<AudioRequest>) {
        loop {
            let request = if let Some(session) = self.current_session.as_mut() {
                session.drain_samples();
                match requests.recv_timeout(AUDIO_POLL_INTERVAL) {
                    Ok(request) => request,
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match requests.recv() {
                    Ok(request) => request,
                    Err(_) => break,
                }
            };

            match request {
                AudioRequest::ListDevices(reply) => {
                    let _ = reply.send(self.list_devices());
                }
                AudioRequest::StartRecording { device_name, reply } => {
                    let _ = reply.send(self.start_recording(device_name));
                }
                AudioRequest::StopRecording(reply) => {
                    let _ = reply.send(self.stop_recording());
                }
                AudioRequest::RecordingStatus(reply) => {
                    let _ = reply.send(Ok(self.current_session.as_ref().map(|s| s.id.clone())));
                }
                AudioRequest::CleanupOldRecordings { days, reply } => {
                    let active_path = self.current_session.as_ref().map(|s| s.file_path.as_path());
                    let _ = reply.send(cleanup_recordings(&self.recordings_dir, active_path, days));
                }
            }
        }

        if self.current_session.is_some() {
            if let Err(error) = self.stop_recording() {
                eprintln!("關閉時無法完成錄音: {}", error);
            }
        }
    }

    fn list_devices(&self) -> Result<Vec<AudioDevice>, String> {
        let host = cpal::default_host();
        let default_name = host
            .default_input_device()
            .and_then(|d| d.description().ok().map(|d| d.name().to_string()));
        let devices = host
            .input_devices()
            .map_err(|e| format!("無法取得輸入設備: {}", e))?;

        Ok(devices
            .map(|device| {
                let name = device
                    .description()
                    .map(|d| d.name().to_string())
                    .unwrap_or_else(|_| "Unknown Device".to_string());
                AudioDevice {
                    is_default: default_name.as_ref() == Some(&name),
                    name,
                }
            })
            .collect())
    }

    fn start_recording(&mut self, device_name: Option<String>) -> Result<String, String> {
        if self.current_session.is_some() {
            return Err("已有錄音會話進行中".to_string());
        }
        self.level_monitor.reset();

        let host = cpal::default_host();
        let device = if let Some(name) = device_name {
            host.input_devices()
                .map_err(|e| format!("無法取得輸入設備: {}", e))?
                .find(|device| {
                    device
                        .description()
                        .ok()
                        .map(|d| d.name().to_string())
                        .as_deref()
                        == Some(name.as_str())
                })
                .ok_or_else(|| format!("找不到設備: {}", name))?
        } else {
            host.default_input_device()
                .ok_or_else(|| "沒有可用的錄音設備".to_string())?
        };

        let config = device
            .default_input_config()
            .map_err(|e| format!("無法取得設備配置: {}", e))?;
        if !matches!(
            config.sample_format(),
            SampleFormat::I16 | SampleFormat::U16 | SampleFormat::F32
        ) {
            return Err("不支援的取樣格式".to_string());
        }

        let id = format!("rec_{}", chrono::Utc::now().timestamp_micros());
        let file_path = self.recordings_dir.join(format!("{}.wav", id));
        let writer = create_wav_writer(&file_path, config.channels(), config.sample_rate())?;

        let result = (|| {
            let (samples, samples_receiver) = mpsc::sync_channel(AUDIO_QUEUE_CAPACITY);
            // Preserve the first capture error without an unbounded error queue.
            let (errors, errors_receiver) = mpsc::sync_channel(1);
            let stream_config = config.config();
            let stream = match config.sample_format() {
                SampleFormat::I16 => build_stream(
                    &device,
                    &stream_config,
                    samples,
                    errors,
                    self.level_monitor.clone(),
                    |s: i16| s,
                ),
                SampleFormat::U16 => build_stream(
                    &device,
                    &stream_config,
                    samples,
                    errors,
                    self.level_monitor.clone(),
                    u16_to_i16,
                ),
                SampleFormat::F32 => build_stream(
                    &device,
                    &stream_config,
                    samples,
                    errors,
                    self.level_monitor.clone(),
                    f32_to_i16,
                ),
                _ => unreachable!("sample format was validated before creating the file"),
            }?;

            let mut session = RecordingSession {
                id: id.clone(),
                file_path: file_path.clone(),
                stream: Some(stream),
                writer,
                samples: samples_receiver,
                errors: errors_receiver,
                recording_error: None,
            };

            if let Err(error) = session.stream.as_ref().unwrap().play() {
                let _ = session.finish();
                return Err(format!("無法啟動錄音串流: {}", error));
            }

            if let Err(error) = session.wait_for_samples() {
                let _ = session.finish();
                return Err(error);
            }

            Ok(session)
        })();

        match result {
            Ok(session) => {
                self.current_session = Some(session);
                self.level_monitor.activate();
                Ok(id)
            }
            Err(error) => {
                self.level_monitor.reset();
                // This file was created with create_new, so existing recordings
                // can never be overwritten or removed on a failed start.
                let _ = std::fs::remove_file(file_path);
                Err(error)
            }
        }
    }

    fn stop_recording(&mut self) -> Result<StopRecordingResult, String> {
        let result = self
            .current_session
            .take()
            .ok_or_else(|| "沒有進行中的錄音會話".to_string())?
            .finish();
        self.level_monitor.reset();
        result
    }
}

struct RecordingSession {
    id: String,
    file_path: PathBuf,
    stream: Option<cpal::Stream>,
    writer: hound::WavWriter<BufWriter<File>>,
    samples: Receiver<Vec<i16>>,
    errors: Receiver<String>,
    recording_error: Option<String>,
}

impl RecordingSession {
    fn wait_for_samples(&mut self) -> Result<(), String> {
        let samples = self
            .samples
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| {
                self.errors
                    .try_recv()
                    .unwrap_or_else(|_| "麥克風啟動後未送出音訊，請檢查輸入設備及授權".into())
            })?;
        self.write_samples(samples);
        self.drain_samples();
        if let Some(error) = &self.recording_error {
            return Err(error.clone());
        }
        if self.writer.duration() == 0 {
            return Err("麥克風未送出完整音訊，請重新選擇輸入設備".into());
        }
        Ok(())
    }

    fn write_samples(&mut self, samples: Vec<i16>) {
        if self.recording_error.is_none() {
            for sample in samples {
                if let Err(error) = self.writer.write_sample(sample) {
                    self.recording_error = Some(format!("無法寫入錄音: {}", error));
                    break;
                }
            }
        }
    }

    fn drain_samples(&mut self) {
        // Bound each batch so commands remain responsive even if capture is
        // faster than disk writes. After stopping, this drains the whole queue.
        for _ in 0..AUDIO_QUEUE_CAPACITY {
            let Ok(samples) = self.samples.try_recv() else {
                break;
            };
            self.write_samples(samples);
        }

        if let Ok(error) = self.errors.try_recv() {
            self.recording_error.get_or_insert(error);
        }
    }

    fn finish(mut self) -> Result<StopRecordingResult, String> {
        if let Some(stream) = self.stream.take() {
            // Explicitly stop callbacks before draining their remaining data.
            // Some backends retain internal stream references after Drop.
            if let Err(error) = stream.pause() {
                self.recording_error
                    .get_or_insert_with(|| format!("無法停止錄音串流: {}", error));
            }
            drop(stream);
        }
        self.drain_samples();
        let duration = finalize_wav(self.writer, self.recording_error)?;

        Ok(StopRecordingResult {
            audio_path: self.file_path.to_string_lossy().into_owned(),
            duration,
        })
    }
}

fn build_stream<T: cpal::SizedSample + 'static>(
    device: &Device,
    config: &cpal::StreamConfig,
    samples: SyncSender<Vec<i16>>,
    errors: SyncSender<String>,
    level_monitor: AudioLevelMonitor,
    convert: fn(T) -> i16,
) -> Result<cpal::Stream, String> {
    let data_errors = errors.clone();
    device
        .build_input_stream(
            *config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let converted: Vec<i16> = data.iter().copied().map(convert).collect();
                level_monitor.publish(&converted);
                if samples.try_send(converted).is_err() {
                    let _ =
                        data_errors.try_send("錄音緩衝區已滿或已關閉，錄音資料不完整".to_string());
                }
            },
            move |error| {
                let _ = errors.try_send(format!("錄音串流錯誤: {}", error));
            },
            None,
        )
        .map_err(|e| format!("無法建立錄音串流: {}", e))
}

fn u16_to_i16(sample: u16) -> i16 {
    (i32::from(sample) - 32_768) as i16
}

fn f32_to_i16(sample: f32) -> i16 {
    if sample.is_nan() {
        return 0;
    }
    // Float-to-integer casts saturate, so +1.0 becomes 32767 rather than
    // wrapping, while -1.0 retains the full negative range of PCM16.
    (sample.clamp(-1.0, 1.0) * 32_768.0).round() as i16
}

fn create_wav_writer(
    file_path: &Path,
    channels: u16,
    sample_rate: u32,
) -> Result<hound::WavWriter<BufWriter<File>>, String> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(file_path)
        .map_err(|e| format!("無法建立 WAV 檔案: {}", e))?;
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    hound::WavWriter::new(BufWriter::new(file), spec)
        .map_err(|e| format!("無法建立 WAV 寫入器: {}", e))
}

fn finalize_wav<W: Write + Seek>(
    writer: hound::WavWriter<W>,
    mut recording_error: Option<String>,
) -> Result<u64, String> {
    if writer.duration() == 0 && recording_error.is_none() {
        recording_error =
            Some("錄音未收到任何音訊，請檢查麥克風授權及輸入設備，再按住錄音鍵重試".into());
    }
    let duration = u64::from(writer.duration()) / u64::from(writer.spec().sample_rate);
    // Always finalize, including after a capture/write error, and report both
    // failures if flushing the WAV header or buffered data also fails.
    let finalized = writer.finalize();
    match (recording_error, finalized) {
        (None, Ok(())) => Ok(duration),
        (Some(error), Ok(())) => Err(error),
        (None, Err(error)) => Err(format!("無法完成 WAV 檔案: {}", error)),
        (Some(error), Err(finalize_error)) => {
            Err(format!("{}；無法完成 WAV 檔案: {}", error, finalize_error))
        }
    }
}

fn cleanup_recordings(
    recordings_dir: &Path,
    active_path: Option<&Path>,
    days: u64,
) -> Result<usize, String> {
    let now = SystemTime::now();
    let max_age = Duration::from_secs(days.saturating_mul(24 * 60 * 60));
    let mut cleaned_count = 0;

    for entry in
        std::fs::read_dir(recordings_dir).map_err(|e| format!("無法讀取錄音目錄: {}", e))?
    {
        let entry = entry.map_err(|e| format!("無法讀取檔案項目: {}", e))?;
        let path = entry.path();
        if active_path == Some(path.as_path())
            || path.extension().and_then(|s| s.to_str()) != Some("wav")
        {
            continue;
        }

        let metadata = entry
            .metadata()
            .map_err(|e| format!("無法讀取錄音檔案資訊: {}", e))?;
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata
            .modified()
            .map_err(|e| format!("無法讀取錄音修改時間: {}", e))?;
        if now.duration_since(modified).unwrap_or_default() > max_age {
            std::fs::remove_file(&path).map_err(|e| format!("無法刪除舊錄音檔案: {}", e))?;
            cleaned_count += 1;
        }
    }

    Ok(cleaned_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_tracks_pcm_amplitude_without_cancelling_opposite_channels() {
        assert_eq!(pcm_rms(&[]), 0.0);
        assert_eq!(pcm_rms(&[0; 128]), 0.0);
        assert_eq!(pcm_rms(&[16_384, -16_384]), 0.5);
        assert_eq!(pcm_rms(&[i16::MIN; 128]), 1.0);
        assert!(pcm_rms(&[1_000; 128]) < pcm_rms(&[10_000; 128]));
        assert!(pcm_rms(&[i16::MAX; 128]) < 1.0);
    }

    #[test]
    fn meter_resets_between_sessions_and_zeros_stale_capture() {
        let meter = AudioLevelMonitor::default();
        assert!(!meter.snapshot().recording);
        meter.publish(&[16_384; 128]);
        assert_eq!(meter.snapshot().rms, 0.0);
        meter.activate();
        assert_eq!(meter.snapshot().rms, 0.5);
        meter.reset();
        assert!(!meter.snapshot().recording);
        meter.activate();
        assert_eq!(meter.snapshot().rms, 0.0);
        let now = Instant::now();
        let state = LevelState {
            recording: true,
            rms: 0.7,
            updated_at: Some(now),
        };
        assert_eq!(state.reading(now).rms, 0.7);
        assert_eq!(
            state
                .reading(now + LEVEL_STALE_AFTER + Duration::from_millis(1))
                .rms,
            0.0
        );
    }

    #[test]
    fn readiness_waits_for_delayed_samples_and_preserves_them_on_immediate_stop() {
        let directory = TestDirectory::new();
        let path = directory.0.join("ready.wav");
        let (sender, receiver) = mpsc::sync_channel(AUDIO_QUEUE_CAPACITY);
        let (_errors, errors_receiver) = mpsc::sync_channel(1);
        let mut session = RecordingSession {
            id: "ready".into(),
            file_path: path.clone(),
            stream: None,
            writer: create_wav_writer(&path, 1, 48_000).unwrap(),
            samples: receiver,
            errors: errors_receiver,
            recording_error: None,
        };
        let capture = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            sender.send(vec![123; 512]).unwrap();
        });
        session.wait_for_samples().unwrap();
        assert_eq!(session.writer.duration(), 512);
        session.finish().unwrap();
        capture.join().unwrap();
        assert_eq!(hound::WavReader::open(path).unwrap().duration(), 512);
    }

    #[test]
    fn empty_capture_is_reported_instead_of_saved_as_success() {
        let directory = TestDirectory::new();
        let path = directory.0.join("empty.wav");
        let writer = create_wav_writer(&path, 1, 48_000).unwrap();
        assert!(finalize_wav(writer, None)
            .unwrap_err()
            .contains("未收到任何音訊"));
        assert_eq!(hound::WavReader::open(path).unwrap().duration(), 0);
    }
    use std::io::{self, Cursor, SeekFrom};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "gpt-typeless-audio-test-{}-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_micros(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn manager_is_send_sync_and_replies_without_audio_hardware() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AudioManager>();

        let directory = TestDirectory::new();
        let manager = AudioManager::new(directory.0.join("recordings")).unwrap();
        let cloned = manager.clone();
        drop(manager);
        assert!(cloned.get_recording_status().unwrap().is_none());
        assert!(cloned.stop_recording().is_err());
        assert_eq!(cloned.cleanup_old_recordings(0).unwrap(), 0);
    }

    #[test]
    fn conversions_preserve_silence_endpoints_and_clamp_floats() {
        assert_eq!(
            [0, 32_767, 32_768, 65_535].map(u16_to_i16),
            [i16::MIN, -1, 0, i16::MAX]
        );
        assert_eq!(
            [
                f32::NEG_INFINITY,
                -2.0,
                -1.0,
                -0.5,
                0.0,
                0.5,
                1.0,
                2.0,
                f32::INFINITY,
                f32::NAN
            ]
            .map(f32_to_i16),
            [
                i16::MIN,
                i16::MIN,
                i16::MIN,
                -16_384,
                0,
                16_384,
                i16::MAX,
                i16::MAX,
                i16::MAX,
                0
            ]
        );
    }

    #[test]
    fn finalized_wav_is_pcm16_interleaved_and_duration_counts_frames() {
        let directory = TestDirectory::new();
        let path = directory.0.join("stereo.wav");
        let expected = [i16::MIN, i16::MAX, 0, 16_384, -16_384, 1];
        let mut writer = create_wav_writer(&path, 2, 48_000).unwrap();
        for sample in expected.into_iter().cycle().take(96_000) {
            writer.write_sample(sample).unwrap();
        }
        assert_eq!(finalize_wav(writer, None).unwrap(), 1);

        let mut reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().sample_format, hound::SampleFormat::Int);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.spec().sample_rate, 48_000);
        assert_eq!(reader.duration(), 48_000);
        assert_eq!(reader.len(), 96_000);
        assert_eq!(
            reader
                .samples::<i16>()
                .take(expected.len())
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            expected
        );
        assert!(create_wav_writer(&path, 1, 48_000).is_err());
    }

    #[test]
    fn stopping_drains_queued_samples_before_finalizing() {
        let directory = TestDirectory::new();
        let path = directory.0.join("queued.wav");
        let (samples, receiver) = mpsc::sync_channel(AUDIO_QUEUE_CAPACITY);
        let (_errors, errors_receiver) = mpsc::sync_channel(1);
        for _ in 0..AUDIO_QUEUE_CAPACITY {
            samples.send(vec![i16::MIN, i16::MAX]).unwrap();
        }
        let mut worker = AudioWorker {
            recordings_dir: directory.0.clone(),
            level_monitor: AudioLevelMonitor::default(),
            current_session: Some(RecordingSession {
                id: "queued".to_string(),
                file_path: path.clone(),
                stream: None,
                writer: create_wav_writer(&path, 2, 48_000).unwrap(),
                samples: receiver,
                errors: errors_receiver,
                recording_error: None,
            }),
        };

        let result = worker.stop_recording().unwrap();
        assert_eq!(Path::new(&result.audio_path), path);
        assert!(worker.current_session.is_none());
        let mut reader = hound::WavReader::open(path).unwrap();
        assert_eq!(reader.duration(), AUDIO_QUEUE_CAPACITY as u32);
        assert_eq!(reader.len(), (AUDIO_QUEUE_CAPACITY * 2) as u32);
        assert!(reader.samples::<i16>().all(|sample| sample.is_ok()));
    }

    #[test]
    fn capture_errors_are_reported_after_finalizing_the_wav() {
        let directory = TestDirectory::new();
        let path = directory.0.join("partial.wav");
        let mut writer = create_wav_writer(&path, 1, 48_000).unwrap();
        writer.write_sample(123_i16).unwrap();
        assert_eq!(
            finalize_wav(writer, Some("capture failed".to_string())).unwrap_err(),
            "capture failed"
        );
        let mut reader = hound::WavReader::open(path).unwrap();
        assert_eq!(reader.duration(), 1);
        assert_eq!(reader.samples::<i16>().next().unwrap().unwrap(), 123);
    }

    struct FailOnFlush(Cursor<Vec<u8>>);

    impl Write for FailOnFlush {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.0.write(data)
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("flush failed"))
        }
    }

    impl Seek for FailOnFlush {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.0.seek(position)
        }
    }

    #[test]
    fn finalization_errors_are_returned() {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(FailOnFlush(Cursor::new(Vec::new())), spec).unwrap();
        writer.write_sample(0_i16).unwrap();
        let error = finalize_wav(writer, Some("capture failed".to_string())).unwrap_err();
        assert!(error.contains("capture failed"));
        assert!(error.contains("flush failed"));
    }

    #[test]
    fn cleanup_preserves_active_recording_and_unrelated_files() {
        let directory = TestDirectory::new();
        let active = directory.0.join("active.wav");
        let old = directory.0.join("old.wav");
        let unrelated = directory.0.join("notes.txt");
        for path in [&active, &old, &unrelated] {
            std::fs::write(path, b"preserve or clean up").unwrap();
            let file = OpenOptions::new().write(true).open(path).unwrap();
            file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1))
                .unwrap();
        }

        assert_eq!(
            cleanup_recordings(&directory.0, Some(&active), u64::MAX).unwrap(),
            0
        );
        assert_eq!(
            cleanup_recordings(&directory.0, Some(&active), 7).unwrap(),
            1
        );
        assert!(active.exists());
        assert!(!old.exists());
        assert!(unrelated.exists());
    }
}
