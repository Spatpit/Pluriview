use crate::privacy;
use crate::preview::{PreviewManager, PreviewId};
use eframe::egui;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use parking_lot::RwLock;
use std::sync::mpsc::{self, Sender, Receiver};

/// Frame data sent from capture threads
pub struct CapturedFrame {
    pub preview_id: PreviewId,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Manages all window capture sessions
pub struct CaptureCoordinator {
    /// Active capture sessions by preview ID
    sessions: HashMap<PreviewId, CaptureSession>,

    /// Channel receiver for captured frames
    frame_receiver: Receiver<CapturedFrame>,

    /// Channel sender (cloned to capture threads)
    frame_sender: Sender<CapturedFrame>,
}

/// A single capture session
struct CaptureSession {
    /// Preview ID this session belongs to
    #[allow(dead_code)]
    preview_id: PreviewId,

    /// Window handle being captured (kept for reference)
    #[allow(dead_code)]
    hwnd: isize,

    /// Window title for matching (kept for reference)
    #[allow(dead_code)]
    window_title: String,

    /// Target FPS, shared with the capture thread so changes apply live
    /// without restarting the capture session.
    target_fps: Arc<AtomicU32>,

    /// Is capture active?
    active: Arc<RwLock<bool>>,

    /// Is capture paused? (shared with capture thread)
    paused: Arc<RwLock<bool>>,

    /// Handle to the capture task
    #[allow(dead_code)]
    handle: Option<std::thread::JoinHandle<()>>,
}

impl CaptureCoordinator {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();

        Self {
            sessions: HashMap::new(),
            frame_receiver: receiver,
            frame_sender: sender,
        }
    }

    /// Start capturing a window for a preview
    pub fn start_capture(&mut self, preview_id: PreviewId, hwnd: isize, window_title: String, target_fps: u32) {
        // Stop existing capture for this preview if any
        self.stop_capture(preview_id);

        let active = Arc::new(RwLock::new(true));
        let paused = Arc::new(RwLock::new(false));
        let fps = Arc::new(AtomicU32::new(target_fps.max(1)));
        let active_clone = active.clone();
        let paused_clone = paused.clone();
        let fps_clone = fps.clone();
        let sender = self.frame_sender.clone();
        let title_clone = window_title.clone();

        // Start capture in a new thread
        let handle = std::thread::spawn(move || {
            capture_window_loop(preview_id, hwnd, title_clone, fps_clone, active_clone, paused_clone, sender);
        });

        let session = CaptureSession {
            preview_id,
            hwnd,
            window_title,
            target_fps: fps,
            active,
            paused,
            handle: Some(handle),
        };

        self.sessions.insert(preview_id, session);
    }

    /// Stop capturing for a preview
    pub fn stop_capture(&mut self, preview_id: PreviewId) {
        if let Some(session) = self.sessions.remove(&preview_id) {
            // Signal the capture thread to stop
            *session.active.write() = false;
        }
    }

    /// Update target FPS for a capture session; applies live on the
    /// capture thread's next frame, no restart needed.
    pub fn set_target_fps(&mut self, preview_id: PreviewId, fps: u32) {
        if let Some(session) = self.sessions.get_mut(&preview_id) {
            session.target_fps.store(fps.max(1), Ordering::Relaxed);
        }
    }

    /// Process any pending captured frames. Drains the channel completely:
    /// each preview keeps only its newest frame, so a stalled UI can never
    /// accumulate a backlog of multi-megabyte video frames.
    pub fn process_frames(&mut self, preview_manager: &mut PreviewManager, _ctx: &egui::Context) {
        loop {
            match self.frame_receiver.try_recv() {
                Ok(frame) => {
                    if let Some(preview) = preview_manager.get_mut(frame.preview_id) {
                        preview.update_frame(frame.width, frame.height, frame.data);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    log::error!("Frame channel disconnected");
                    break;
                }
            }
        }
    }

    /// Check if a preview has an active capture
    #[allow(dead_code)]
    pub fn is_capturing(&self, preview_id: PreviewId) -> bool {
        self.sessions.get(&preview_id)
            .map(|s| *s.active.read())
            .unwrap_or(false)
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
            *session.paused.write() = true;
        }
    }

    /// Resume capturing for a preview
    pub fn resume_capture(&mut self, preview_id: PreviewId) {
        if let Some(session) = self.sessions.get(&preview_id) {
            *session.paused.write() = false;
        }
    }

    /// Check if a preview's capture is paused
    #[allow(dead_code)]
    pub fn is_paused(&self, preview_id: PreviewId) -> bool {
        self.sessions.get(&preview_id)
            .map(|s| *s.paused.read())
            .unwrap_or(false)
    }

    /// True if at least one capture session is active and not paused.
    /// Used to decide how aggressively the UI should repaint.
    pub fn has_live_capture(&self) -> bool {
        self.sessions.values().any(|s| *s.active.read() && !*s.paused.read())
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
    active: Arc<RwLock<bool>>,
    paused: Arc<RwLock<bool>>,
    sender: Sender<CapturedFrame>,
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
        sender: Sender<CapturedFrame>,
        active: Arc<RwLock<bool>>,
        paused: Arc<RwLock<bool>>,
        fps: Arc<AtomicU32>,
    }

    struct Capture {
        preview_id: PreviewId,
        sender: Sender<CapturedFrame>,
        active: Arc<RwLock<bool>>,
        paused: Arc<RwLock<bool>>,
        fps: Arc<AtomicU32>,
        last_frame: std::time::Instant,
    }

    impl GraphicsCaptureApiHandler for Capture {
        type Flags = CaptureFlags;
        type Error = Box<dyn std::error::Error + Send + Sync>;

        fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
            Ok(Self {
                preview_id: ctx.flags.preview_id,
                sender: ctx.flags.sender,
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
            if !*self.active.read() {
                capture_control.stop();
                return Ok(());
            }

            // Check if we're paused (viewport culling)
            if *self.paused.read() {
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
                preview_id: self.preview_id,
                width,
                height,
                data,
            };

            if self.sender.send(captured_frame).is_err() {
                capture_control.stop();
            }

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
        sender,
        active: active.clone(),
        paused: paused.clone(),
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
    use super::capture_target_from_hwnd;

    #[test]
    fn capture_target_preserves_supplied_hwnd() {
        let hwnd = 0x1234isize;
        let target = capture_target_from_hwnd(hwnd);
        assert_eq!(target.as_raw_hwnd() as isize, hwnd);
    }
}
