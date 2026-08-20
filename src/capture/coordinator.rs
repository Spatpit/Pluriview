use crate::preview::{PreviewId, PreviewManager};
use crate::privacy;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use windows_capture::capture::GraphicsCaptureApiError;
use windows_capture::graphics_capture_api::GraphicsCaptureApi;
use windows_capture::settings::{CursorCaptureSettings, DrawBorderSettings};

use super::downsample::{downsample_rgba, fitted_capture_size};

/// Frame data sent from capture threads
pub(crate) struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Manages all window capture sessions
pub struct CaptureCoordinator {
    /// Active capture sessions by preview ID
    sessions: HashMap<PreviewId, CaptureSession>,

    /// Stopped/replaced workers are retained until they actually finish, then
    /// joined during regular UI upkeep instead of being silently detached.
    retired_workers: Vec<JoinHandle<()>>,
}

/// A single capture session
struct CaptureSession {
    /// Target FPS, shared with the capture thread so changes apply live
    /// without restarting the capture session.
    target_fps: Arc<AtomicU32>,

    /// Max output size in pixels. The worker fits the source inside this box
    /// without upscaling. Zero means native source size.
    target_width: Arc<AtomicU32>,
    target_height: Arc<AtomicU32>,

    /// Is capture active?
    active: Arc<AtomicBool>,

    /// Is capture paused? (shared with capture thread)
    paused: Arc<AtomicBool>,

    /// Per-session latest-frame slot. Replacing a capture gives the new worker
    /// a different slot, so an old worker can never publish a stale frame into it.
    latest_frame: Arc<Mutex<Option<CapturedFrame>>>,

    /// Set when the worker cannot start Windows Graphics Capture.
    failure: Arc<Mutex<Option<String>>>,

    /// Stops Windows Graphics Capture directly, even when a static window is
    /// no longer producing frame callbacks.
    stop_sender: Sender<()>,

    worker: Option<JoinHandle<()>>,
}

impl CaptureSession {
    fn is_live(&self) -> bool {
        self.active.load(Ordering::Relaxed)
            && self
                .worker
                .as_ref()
                .is_some_and(|worker| !worker.is_finished())
    }
}

impl CaptureCoordinator {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            retired_workers: Vec::new(),
        }
    }

    /// Start capturing a window for a preview
    pub fn start_capture(
        &mut self,
        preview_id: PreviewId,
        hwnd: isize,
        window_title: String,
        target_fps: u32,
    ) {
        // Windows Graphics Capture rejects a second session on the same HWND
        // while the previous one is still shutting down. Replacing a live
        // tile must wait for that release, or the new session fails and the
        // preview stays on its last frame until the host is recreated.
        self.retire_session(preview_id, true);

        let active = Arc::new(AtomicBool::new(true));
        let paused = Arc::new(AtomicBool::new(false));
        let fps = Arc::new(AtomicU32::new(target_fps.max(1)));
        let target_width = Arc::new(AtomicU32::new(0));
        let target_height = Arc::new(AtomicU32::new(0));
        let latest_frame = Arc::new(Mutex::new(None));
        let failure = Arc::new(Mutex::new(None));
        let active_clone = active.clone();
        let paused_clone = paused.clone();
        let fps_clone = fps.clone();
        let target_width_clone = target_width.clone();
        let target_height_clone = target_height.clone();
        let worker_frame = latest_frame.clone();
        let worker_failure = failure.clone();
        let (stop_sender, stop_receiver) = mpsc::channel();

        // Start capture in a new thread
        let worker = std::thread::spawn(move || {
            capture_window_loop(
                preview_id,
                hwnd,
                window_title,
                fps_clone,
                target_width_clone,
                target_height_clone,
                active_clone,
                paused_clone,
                worker_frame,
                worker_failure,
                stop_receiver,
            );
        });

        let session = CaptureSession {
            target_fps: fps,
            target_width,
            target_height,
            active,
            paused,
            latest_frame,
            failure,
            stop_sender,
            worker: Some(worker),
        };

        self.sessions.insert(preview_id, session);
    }

    /// Receive a Spout2 sender into a preview tile.
    pub fn start_spout_capture(
        &mut self,
        preview_id: PreviewId,
        sender_name: String,
        target_fps: u32,
    ) {
        self.retire_session(preview_id, true);

        let active = Arc::new(AtomicBool::new(true));
        let paused = Arc::new(AtomicBool::new(false));
        let fps = Arc::new(AtomicU32::new(target_fps.max(1)));
        let target_width = Arc::new(AtomicU32::new(0));
        let target_height = Arc::new(AtomicU32::new(0));
        let latest_frame = Arc::new(Mutex::new(None));
        let failure = Arc::new(Mutex::new(None));
        let active_clone = active.clone();
        let paused_clone = paused.clone();
        let fps_clone = fps.clone();
        let target_width_clone = target_width.clone();
        let target_height_clone = target_height.clone();
        let worker_frame = latest_frame.clone();
        let worker_failure = failure.clone();
        let (stop_sender, stop_receiver) = mpsc::channel();

        let worker = std::thread::spawn(move || {
            super::spout::capture_spout_loop(
                sender_name,
                fps_clone,
                target_width_clone,
                target_height_clone,
                active_clone,
                paused_clone,
                worker_frame,
                worker_failure,
                stop_receiver,
            );
        });

        self.sessions.insert(
            preview_id,
            CaptureSession {
                target_fps: fps,
                target_width,
                target_height,
                active,
                paused,
                latest_frame,
                failure,
                stop_sender,
                worker: Some(worker),
            },
        );
    }

    /// Stop capturing for a preview
    pub fn stop_capture(&mut self, preview_id: PreviewId) {
        self.retire_session(preview_id, false);
    }

    /// True while a capture worker is still attached and producing a session.
    #[cfg(test)]
    pub fn is_live(&self, preview_id: PreviewId) -> bool {
        self.sessions
            .get(&preview_id)
            .is_some_and(CaptureSession::is_live)
    }

    fn retire_session(&mut self, preview_id: PreviewId, join: bool) {
        if let Some(mut session) = self.sessions.remove(&preview_id) {
            session.active.store(false, Ordering::Relaxed);
            let _ = session.stop_sender.send(());
            session.latest_frame.lock().take();
            if let Some(worker) = session.worker.take() {
                if join {
                    if worker.join().is_err() {
                        log::error!("Capture worker panicked");
                    }
                } else {
                    self.retired_workers.push(worker);
                }
            }
        }
        self.reap_finished_workers();
    }

    /// Update target FPS for a capture session; applies live on the
    /// capture thread's next frame, no restart needed.
    pub fn set_target_fps(&mut self, preview_id: PreviewId, fps: u32) {
        if let Some(session) = self.sessions.get_mut(&preview_id) {
            session.target_fps.store(fps.max(1), Ordering::Relaxed);
        }
    }

    /// Live output cap for this session. The worker downscales into this box
    /// on the next frame; it never upscales a smaller source.
    pub fn set_target_size(&mut self, preview_id: PreviewId, width: u32, height: u32) {
        if let Some(session) = self.sessions.get_mut(&preview_id) {
            session.target_width.store(width, Ordering::Relaxed);
            session.target_height.store(height, Ordering::Relaxed);
        }
    }

    /// Process the newest pending frame for each preview.
    pub fn process_frames(&mut self, preview_manager: &mut PreviewManager) {
        for (preview_id, session) in &self.sessions {
            let frame = session.latest_frame.lock().take();
            let failure = session.failure.lock().take();
            if let Some(preview) = preview_manager.get_mut(*preview_id) {
                if let Some(frame) = frame {
                    preview.update_frame(frame.width, frame.height, frame.data);
                } else if let Some(error) = failure {
                    preview.set_capture_error(error);
                } else if session.is_live() {
                    preview.clear_capture_error();
                }
            }
        }
        self.reap_finished_workers();
    }

    /// Stop all captures
    pub fn stop_all(&mut self) {
        for (_, mut session) in self.sessions.drain() {
            session.active.store(false, Ordering::Relaxed);
            let _ = session.stop_sender.send(());
            session.latest_frame.lock().take();
            if let Some(worker) = session.worker.take() {
                self.retired_workers.push(worker);
            }
        }
        self.reap_finished_workers();
    }

    /// Pause capturing for a preview (viewport culling)
    pub fn pause_capture(&mut self, preview_id: PreviewId) {
        if let Some(session) = self.sessions.get(&preview_id) {
            session.paused.store(true, Ordering::Relaxed);
        }
    }

    /// Resume capturing for a preview
    pub fn resume_capture(&mut self, preview_id: PreviewId) {
        if let Some(session) = self.sessions.get(&preview_id) {
            session.paused.store(false, Ordering::Relaxed);
        }
    }

    /// Highest requested FPS among capture sessions that are currently live.
    /// The UI uses this to avoid repainting faster than its previews update.
    pub fn max_live_fps(&self) -> Option<u32> {
        self.sessions
            .values()
            .filter(|session| {
                session.active.load(Ordering::Relaxed) && !session.paused.load(Ordering::Relaxed)
            })
            .map(|session| session.target_fps.load(Ordering::Relaxed).max(1))
            .max()
    }

    fn reap_finished_workers(&mut self) {
        for session in self.sessions.values_mut() {
            let finished = session.worker.as_ref().is_some_and(JoinHandle::is_finished);
            if finished {
                // A panic bypasses the capture loop's normal completion flag.
                // Mark the session inactive before joining so repaint scheduling
                // cannot keep treating a dead worker as live.
                session.active.store(false, Ordering::Relaxed);
                if let Some(worker) = session.worker.take() {
                    if worker.join().is_err() {
                        log::error!("Capture worker panicked");
                    }
                }
            }
        }

        let mut index = 0;
        while index < self.retired_workers.len() {
            if self.retired_workers[index].is_finished() {
                let worker = self.retired_workers.swap_remove(index);
                if worker.join().is_err() {
                    log::error!("Retired capture worker panicked");
                }
            } else {
                index += 1;
            }
        }
    }
}

impl Default for CaptureCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CaptureCoordinator {
    fn drop(&mut self) {
        self.stop_all();
    }
}

fn capture_target_from_hwnd(hwnd: isize) -> windows_capture::window::Window {
    windows_capture::window::Window::from_raw_hwnd(hwnd as *mut std::ffi::c_void)
}

/// Capture loop running in a separate thread
fn capture_window_loop(
    preview_id: PreviewId,
    hwnd: isize,
    window_title: String,
    target_fps: Arc<AtomicU32>,
    target_width: Arc<AtomicU32>,
    target_height: Arc<AtomicU32>,
    active: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    latest_frame: Arc<Mutex<Option<CapturedFrame>>>,
    failure: Arc<Mutex<Option<String>>>,
    stop_receiver: Receiver<()>,
) {
    use windows_capture::{
        capture::{Context, GraphicsCaptureApiHandler},
        frame::Frame,
        graphics_capture_api::InternalCaptureControl,
        settings::{
            ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
            MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
        },
    };

    // Capture flags passed to the handler
    #[derive(Clone)]
    struct CaptureFlags {
        preview_id: PreviewId,
        latest_frame: Arc<Mutex<Option<CapturedFrame>>>,
        active: Arc<AtomicBool>,
        paused: Arc<AtomicBool>,
        fps: Arc<AtomicU32>,
        target_width: Arc<AtomicU32>,
        target_height: Arc<AtomicU32>,
    }

    struct Capture {
        preview_id: PreviewId,
        latest_frame: Arc<Mutex<Option<CapturedFrame>>>,
        active: Arc<AtomicBool>,
        paused: Arc<AtomicBool>,
        fps: Arc<AtomicU32>,
        target_width: Arc<AtomicU32>,
        target_height: Arc<AtomicU32>,
        last_frame: std::time::Instant,
    }

    impl GraphicsCaptureApiHandler for Capture {
        type Flags = CaptureFlags;
        type Error = Box<dyn std::error::Error + Send + Sync>;

        fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
            Ok(Self {
                preview_id: ctx.flags.preview_id,
                latest_frame: ctx.flags.latest_frame,
                active: ctx.flags.active,
                paused: ctx.flags.paused,
                fps: ctx.flags.fps,
                target_width: ctx.flags.target_width,
                target_height: ctx.flags.target_height,
                last_frame: std::time::Instant::now(),
            })
        }

        fn on_frame_arrived(
            &mut self,
            frame: &mut Frame,
            capture_control: InternalCaptureControl,
        ) -> Result<(), Self::Error> {
            // Check if we should stop
            if !self.active.load(Ordering::Relaxed) {
                capture_control.stop();
                return Ok(());
            }

            // Check if we're paused (viewport culling)
            if self.paused.load(Ordering::Relaxed) {
                return Ok(());
            }

            // Throttle frame rate (read live so preset changes apply instantly)
            let fps = self.fps.load(Ordering::Relaxed).max(1);
            let frame_interval = std::time::Duration::from_secs_f64(1.0 / fps as f64);
            let elapsed = self.last_frame.elapsed();
            if elapsed < frame_interval {
                return Ok(());
            }
            self.last_frame = std::time::Instant::now();

            // Get frame buffer
            let mut buffer = frame.buffer()?;
            let width = buffer.width();
            let height = buffer.height();
            let (out_width, out_height) = fitted_capture_size(
                width,
                height,
                self.target_width.load(Ordering::Relaxed),
                self.target_height.load(Ordering::Relaxed),
            );

            let captured_frame = if out_width == width && out_height == height {
                CapturedFrame {
                    width,
                    height,
                    data: buffer.as_nopadding_buffer()?.to_vec(),
                }
            } else {
                let stride = buffer.row_pitch();
                let raw = buffer.as_raw_buffer();
                let Some(data) = downsample_rgba(raw, width, height, stride, out_width, out_height)
                else {
                    return Err("Could not downscale the captured frame".into());
                };
                CapturedFrame {
                    width: out_width,
                    height: out_height,
                    data,
                }
            };
            *self.latest_frame.lock() = Some(captured_frame);

            Ok(())
        }

        fn on_closed(&mut self) -> Result<(), Self::Error> {
            self.active.store(false, Ordering::Relaxed);
            log::info!("Capture closed for preview {:?}", self.preview_id);
            Ok(())
        }
    }

    log::info!(
        "Capturing HWND for {}",
        privacy::redact_title(&window_title)
    );

    // Use default minimum update interval (windows-capture handles FPS internally)
    // We do our own throttling in on_frame_arrived
    let min_interval = MinimumUpdateIntervalSettings::Default;

    // Configure capture settings
    let completion_active = active.clone();
    let flags = CaptureFlags {
        preview_id,
        latest_frame,
        active: active.clone(),
        paused,
        fps: target_fps,
        target_width,
        target_height,
    };

    // Request WithoutCursor / WithoutBorder only when those Graphics Capture
    // properties exist. Windows 10 1903 has capture but not IsBorderRequired
    // (Win11) or, on 1903, IsCursorCaptureEnabled (2004+). The crate refuses to
    // start if those settings are not Default and the API is missing.
    let cursor = probed_cursor_capture_setting();
    let border = probed_draw_border_setting();
    let try_start = |cursor, border, flags| {
        let settings = Settings::new(
            capture_target_from_hwnd(hwnd),
            cursor,
            border,
            SecondaryWindowSettings::Default,
            min_interval,
            DirtyRegionSettings::Default,
            ColorFormat::Rgba8,
            flags,
        );
        Capture::start_free_threaded(settings)
    };

    let start_result = match try_start(cursor, border, flags.clone()) {
        Err(error)
            if capture_settings_unsupported(&error)
                && (cursor != CursorCaptureSettings::Default
                    || border != DrawBorderSettings::Default) =>
        {
            log::warn!("Retrying capture with default cursor and border settings: {error}");
            try_start(
                CursorCaptureSettings::Default,
                DrawBorderSettings::Default,
                flags,
            )
        }
        other => other,
    };

    // Keep an explicit CaptureControl so static/paused windows can be stopped
    // without waiting for another frame callback to arrive.
    match start_result {
        Ok(control) => {
            while active.load(Ordering::Relaxed) {
                match stop_receiver.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                }
            }
            if let Err(error) = control.stop() {
                log::error!("Failed to stop capture: {error}");
            }
        }
        Err(error) => {
            log::error!("Failed to start capture: {error}");
            *failure.lock() = Some(error.to_string());
        }
    }
    completion_active.store(false, Ordering::Relaxed);
}

fn cursor_capture_setting(supported: bool) -> CursorCaptureSettings {
    if supported {
        CursorCaptureSettings::WithoutCursor
    } else {
        CursorCaptureSettings::Default
    }
}

fn draw_border_setting(supported: bool) -> DrawBorderSettings {
    if supported {
        DrawBorderSettings::WithoutBorder
    } else {
        DrawBorderSettings::Default
    }
}

fn probed_cursor_capture_setting() -> CursorCaptureSettings {
    match GraphicsCaptureApi::is_cursor_settings_supported() {
        Ok(supported) => cursor_capture_setting(supported),
        Err(error) => {
            log::debug!("Could not probe cursor capture support: {error}");
            CursorCaptureSettings::WithoutCursor
        }
    }
}

fn probed_draw_border_setting() -> DrawBorderSettings {
    match GraphicsCaptureApi::is_border_settings_supported() {
        Ok(supported) => draw_border_setting(supported),
        Err(error) => {
            log::debug!("Could not probe border capture support: {error}");
            DrawBorderSettings::WithoutBorder
        }
    }
}

fn capture_settings_unsupported<E>(error: &GraphicsCaptureApiError<E>) -> bool {
    matches!(
        error,
        GraphicsCaptureApiError::GraphicsCaptureApiError(
            windows_capture::graphics_capture_api::Error::BorderConfigUnsupported
                | windows_capture::graphics_capture_api::Error::CursorConfigUnsupported
        )
    )
}

#[cfg(test)]
mod tests {
    use super::{capture_target_from_hwnd, CaptureCoordinator, CaptureSession, CapturedFrame};
    use crate::preview::{PreviewId, PreviewManager};
    use eframe::egui::{Pos2, Vec2};
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU32};
    use std::sync::mpsc;
    use std::sync::Arc;

    fn session(fps: u32, active: bool, paused: bool) -> CaptureSession {
        let (stop_sender, _stop_receiver) = mpsc::channel();
        CaptureSession {
            target_fps: Arc::new(AtomicU32::new(fps)),
            target_width: Arc::new(AtomicU32::new(0)),
            target_height: Arc::new(AtomicU32::new(0)),
            active: Arc::new(AtomicBool::new(active)),
            paused: Arc::new(AtomicBool::new(paused)),
            latest_frame: Arc::new(Mutex::new(None)),
            failure: Arc::new(Mutex::new(None)),
            stop_sender,
            worker: None,
        }
    }

    #[test]
    fn capture_target_preserves_supplied_hwnd() {
        let hwnd = 0x1234isize;
        let target = capture_target_from_hwnd(hwnd);
        assert_eq!(target.as_raw_hwnd() as isize, hwnd);
    }

    #[test]
    fn pending_frames_replace_stale_frames_per_preview() {
        let pending = Mutex::new(None);
        *pending.lock() = Some(CapturedFrame {
            width: 1,
            height: 1,
            data: vec![1; 4],
        });
        *pending.lock() = Some(CapturedFrame {
            width: 2,
            height: 2,
            data: vec![2; 16],
        });

        let latest = pending.lock().take().unwrap();
        assert_eq!(latest.width, 2);
        assert_eq!(latest.data, vec![2; 16]);
    }

    #[test]
    fn repaint_rate_uses_only_live_unpaused_sessions() {
        let mut coordinator = CaptureCoordinator::new();
        coordinator
            .sessions
            .insert(PreviewId(1), session(15, true, false));
        coordinator
            .sessions
            .insert(PreviewId(2), session(60, true, true));
        coordinator
            .sessions
            .insert(PreviewId(3), session(30, true, false));
        coordinator
            .sessions
            .insert(PreviewId(4), session(120, false, false));

        assert_eq!(coordinator.max_live_fps(), Some(30));
        assert!(!coordinator.is_live(PreviewId(1)));
        assert!(!coordinator.is_live(PreviewId(4)));
    }

    #[test]
    fn replaced_session_cannot_publish_into_the_new_session_slot() {
        let mut previews = PreviewManager::new();
        let preview_id = previews.add("test".to_owned(), Pos2::ZERO, Vec2::splat(10.0));
        let stale_slot = Arc::new(Mutex::new(None));
        let current = session(30, true, false);

        *stale_slot.lock() = Some(CapturedFrame {
            width: 1,
            height: 1,
            data: vec![1; 4],
        });
        *current.latest_frame.lock() = Some(CapturedFrame {
            width: 2,
            height: 2,
            data: vec![2; 16],
        });
        let mut coordinator = CaptureCoordinator::new();
        coordinator.sessions.insert(preview_id, current);
        coordinator.process_frames(&mut previews);

        assert_eq!(previews.get(preview_id).unwrap().frame_size, Some((2, 2)));
        assert_eq!(stale_slot.lock().as_ref().unwrap().width, 1);
    }

    #[test]
    fn replacing_a_session_joins_the_previous_worker() {
        let preview_id = PreviewId(91);
        let (stop_sender, stop_receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = stop_receiver.recv();
            std::thread::sleep(std::time::Duration::from_millis(40));
        });
        let mut coordinator = CaptureCoordinator::new();
        coordinator.sessions.insert(
            preview_id,
            CaptureSession {
                target_fps: Arc::new(AtomicU32::new(30)),
                target_width: Arc::new(AtomicU32::new(0)),
                target_height: Arc::new(AtomicU32::new(0)),
                active: Arc::new(AtomicBool::new(true)),
                paused: Arc::new(AtomicBool::new(false)),
                latest_frame: Arc::new(Mutex::new(None)),
                failure: Arc::new(Mutex::new(None)),
                stop_sender,
                worker: Some(worker),
            },
        );

        let started = std::time::Instant::now();
        coordinator.retire_session(preview_id, true);

        assert!(started.elapsed() >= std::time::Duration::from_millis(40));
        assert!(!coordinator.sessions.contains_key(&preview_id));
    }

    #[test]
    fn stopping_a_static_capture_signals_the_worker_directly() {
        let preview_id = PreviewId(88);
        let (stop_sender, stop_receiver) = mpsc::channel();
        let mut coordinator = CaptureCoordinator::new();
        coordinator.sessions.insert(
            preview_id,
            CaptureSession {
                target_fps: Arc::new(AtomicU32::new(30)),
                target_width: Arc::new(AtomicU32::new(0)),
                target_height: Arc::new(AtomicU32::new(0)),
                active: Arc::new(AtomicBool::new(true)),
                paused: Arc::new(AtomicBool::new(false)),
                latest_frame: Arc::new(Mutex::new(None)),
                failure: Arc::new(Mutex::new(None)),
                stop_sender,
                worker: None,
            },
        );

        coordinator.stop_capture(preview_id);

        assert!(stop_receiver.try_recv().is_ok());
        assert!(!coordinator.sessions.contains_key(&preview_id));
    }

    #[test]
    fn capture_settings_use_defaults_when_unsupported() {
        use super::{cursor_capture_setting, draw_border_setting};
        use windows_capture::settings::{CursorCaptureSettings, DrawBorderSettings};

        assert_eq!(
            cursor_capture_setting(true),
            CursorCaptureSettings::WithoutCursor
        );
        assert_eq!(
            cursor_capture_setting(false),
            CursorCaptureSettings::Default
        );
        assert_eq!(draw_border_setting(true), DrawBorderSettings::WithoutBorder);
        assert_eq!(draw_border_setting(false), DrawBorderSettings::Default);
    }

    #[test]
    fn process_frames_applies_start_failure_to_the_preview() {
        let mut previews = PreviewManager::new();
        let preview_id =
            previews.add_for_window(1, 42, "game".to_owned(), Pos2::ZERO, Vec2::splat(10.0));
        let failed = session(30, false, false);
        *failed.failure.lock() = Some("BorderConfigUnsupported".to_owned());
        let mut coordinator = CaptureCoordinator::new();
        coordinator.sessions.insert(preview_id, failed);

        coordinator.process_frames(&mut previews);

        assert_eq!(
            previews.get(preview_id).unwrap().capture_error.as_deref(),
            Some("BorderConfigUnsupported")
        );
    }

    #[test]
    fn live_session_clears_a_stale_capture_error() {
        let mut previews = PreviewManager::new();
        let preview_id =
            previews.add_for_window(1, 42, "game".to_owned(), Pos2::ZERO, Vec2::splat(10.0));
        previews
            .get_mut(preview_id)
            .unwrap()
            .set_capture_error("stale".to_owned());

        let (stop_sender, stop_receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = stop_receiver.recv();
        });
        let mut coordinator = CaptureCoordinator::new();
        coordinator.sessions.insert(
            preview_id,
            CaptureSession {
                target_fps: Arc::new(AtomicU32::new(30)),
                target_width: Arc::new(AtomicU32::new(0)),
                target_height: Arc::new(AtomicU32::new(0)),
                active: Arc::new(AtomicBool::new(true)),
                paused: Arc::new(AtomicBool::new(false)),
                latest_frame: Arc::new(Mutex::new(None)),
                failure: Arc::new(Mutex::new(None)),
                stop_sender,
                worker: Some(worker),
            },
        );

        coordinator.process_frames(&mut previews);
        assert!(previews.get(preview_id).unwrap().capture_error.is_none());

        coordinator.stop_capture(preview_id);
    }
}
