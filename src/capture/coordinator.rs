use crate::privacy;
use crate::preview::{PreviewManager, PreviewId};
use eframe::egui;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use parking_lot::Mutex;

/// Frame data sent from capture threads
struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Manages all window capture sessions
pub struct CaptureCoordinator {
    /// Active capture sessions by preview ID
    sessions: HashMap<PreviewId, CaptureSession>,

    /// At most one pending frame per preview. Capture threads replace stale
    /// frames instead of building an unbounded queue when the UI is busy.
    latest_frames: Arc<Mutex<HashMap<PreviewId, CapturedFrame>>>,
}

/// A single capture session
struct CaptureSession {
    /// Target FPS, shared with the capture thread so changes apply live
    /// without restarting the capture session.
    target_fps: Arc<AtomicU32>,

    /// Is capture active?
    active: Arc<AtomicBool>,

    /// Is capture paused? (shared with capture thread)
    paused: Arc<AtomicBool>,

}

impl CaptureCoordinator {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            latest_frames: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Start capturing a window for a preview
    pub fn start_capture(&mut self, preview_id: PreviewId, hwnd: isize, window_title: String, target_fps: u32) {
        // Stop existing capture for this preview if any
        self.stop_capture(preview_id);

        let active = Arc::new(AtomicBool::new(true));
        let paused = Arc::new(AtomicBool::new(false));
        let fps = Arc::new(AtomicU32::new(target_fps.max(1)));
        let active_clone = active.clone();
        let paused_clone = paused.clone();
        let fps_clone = fps.clone();
        let latest_frames = self.latest_frames.clone();

        // Start capture in a new thread
        std::thread::spawn(move || {
            capture_window_loop(
                preview_id,
                hwnd,
                window_title,
                fps_clone,
                active_clone,
                paused_clone,
                latest_frames,
            );
        });

        let session = CaptureSession {
            target_fps: fps,
            active,
            paused,
        };

        self.sessions.insert(preview_id, session);
    }

    /// Stop capturing for a preview
    pub fn stop_capture(&mut self, preview_id: PreviewId) {
        if let Some(session) = self.sessions.remove(&preview_id) {
            // Signal the capture thread to stop
            session.active.store(false, Ordering::Relaxed);
        }
        self.latest_frames.lock().remove(&preview_id);
    }

    /// Update target FPS for a capture session; applies live on the
    /// capture thread's next frame, no restart needed.
    pub fn set_target_fps(&mut self, preview_id: PreviewId, fps: u32) {
        if let Some(session) = self.sessions.get_mut(&preview_id) {
            session.target_fps.store(fps.max(1), Ordering::Relaxed);
        }
    }

    /// Process the newest pending frame for each preview.
    pub fn process_frames(&mut self, preview_manager: &mut PreviewManager, _ctx: &egui::Context) {
        let frames = {
            let mut latest = self.latest_frames.lock();
            std::mem::take(&mut *latest)
        };
        for (preview_id, frame) in frames {
            if let Some(preview) = preview_manager.get_mut(preview_id) {
                preview.update_frame(frame.width, frame.height, frame.data);
            }
        }
    }

    /// Stop all captures
    pub fn stop_all(&mut self) {
        let ids: Vec<_> = self.sessions.keys().copied().collect();
        for id in ids {
            self.stop_capture(id);
        }
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

    /// True if at least one capture session is active and not paused.
    /// Used to decide how aggressively the UI should repaint.
    pub fn has_live_capture(&self) -> bool {
        self.sessions.values().any(|session| {
            session.active.load(Ordering::Relaxed) && !session.paused.load(Ordering::Relaxed)
        })
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
    active: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    latest_frames: Arc<Mutex<HashMap<PreviewId, CapturedFrame>>>,
) {
    use windows_capture::{
        capture::{Context, GraphicsCaptureApiHandler},
        frame::Frame,
        graphics_capture_api::InternalCaptureControl,
        settings::{
            ColorFormat, CursorCaptureSettings, DrawBorderSettings,
            SecondaryWindowSettings, MinimumUpdateIntervalSettings,
            DirtyRegionSettings, Settings,
        },
    };

    // Capture flags passed to the handler
    struct CaptureFlags {
        preview_id: PreviewId,
        latest_frames: Arc<Mutex<HashMap<PreviewId, CapturedFrame>>>,
        active: Arc<AtomicBool>,
        paused: Arc<AtomicBool>,
        fps: Arc<AtomicU32>,
    }

    struct Capture {
        preview_id: PreviewId,
        latest_frames: Arc<Mutex<HashMap<PreviewId, CapturedFrame>>>,
        active: Arc<AtomicBool>,
        paused: Arc<AtomicBool>,
        fps: Arc<AtomicU32>,
        last_frame: std::time::Instant,
    }

    impl GraphicsCaptureApiHandler for Capture {
        type Flags = CaptureFlags;
        type Error = Box<dyn std::error::Error + Send + Sync>;

        fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
            Ok(Self {
                preview_id: ctx.flags.preview_id,
                latest_frames: ctx.flags.latest_frames,
                active: ctx.flags.active,
                paused: ctx.flags.paused,
                fps: ctx.flags.fps,
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

            // Copy frame data without row padding
            let data = buffer.as_nopadding_buffer()?.to_vec();

            // Send frame to main thread
            let captured_frame = CapturedFrame {
                width,
                height,
                data,
            };
            self.latest_frames
                .lock()
                .insert(self.preview_id, captured_frame);

            Ok(())
        }

        fn on_closed(&mut self) -> Result<(), Self::Error> {
            log::info!("Capture closed for preview {:?}", self.preview_id);
            Ok(())
        }
    }

    let window = capture_target_from_hwnd(hwnd);
    log::info!("Capturing HWND for {}", privacy::redact_title(&window_title));

    // Use default minimum update interval (windows-capture handles FPS internally)
    // We do our own throttling in on_frame_arrived
    let min_interval = MinimumUpdateIntervalSettings::Default;

    // Configure capture settings
    let flags = CaptureFlags {
        preview_id,
        latest_frames,
        active,
        paused,
        fps: target_fps,
    };

    let settings = Settings::new(
        window,
        CursorCaptureSettings::WithoutCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        min_interval,
        DirtyRegionSettings::Default,
        ColorFormat::Rgba8,
        flags,
    );

    // Start capture - this blocks until capture is stopped
    if let Err(e) = Capture::start(settings) {
        log::error!("Failed to start capture: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::{capture_target_from_hwnd, CapturedFrame, CaptureCoordinator};
    use crate::preview::PreviewId;

    #[test]
    fn capture_target_preserves_supplied_hwnd() {
        let hwnd = 0x1234isize;
        let target = capture_target_from_hwnd(hwnd);
        assert_eq!(target.as_raw_hwnd() as isize, hwnd);
    }

    #[test]
    fn pending_frames_replace_stale_frames_per_preview() {
        let coordinator = CaptureCoordinator::new();
        let id = PreviewId(7);
        let mut pending = coordinator.latest_frames.lock();
        pending.insert(
            id,
            CapturedFrame {
                width: 1,
                height: 1,
                data: vec![1; 4],
            },
        );
        pending.insert(
            id,
            CapturedFrame {
                width: 2,
                height: 2,
                data: vec![2; 16],
            },
        );

        assert_eq!(pending.len(), 1);
        assert_eq!(pending[&id].width, 2);
        assert_eq!(pending[&id].data, vec![2; 16]);
    }
}
