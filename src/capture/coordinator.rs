use crate::preview::{PreviewManager, PreviewId};
use eframe::egui;
use std::collections::HashMap;
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

    /// Target FPS
    target_fps: u32,

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
        let active_clone = active.clone();
        let paused_clone = paused.clone();
        let sender = self.frame_sender.clone();
        let title_clone = window_title.clone();

        // Start capture in a new thread
        let handle = std::thread::spawn(move || {
            capture_window_loop(preview_id, hwnd, title_clone, target_fps, active_clone, paused_clone, sender);
        });

        let session = CaptureSession {
            preview_id,
            hwnd,
            window_title,
            target_fps,
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

    /// Update target FPS for a capture session
    #[allow(dead_code)]
    pub fn set_target_fps(&mut self, preview_id: PreviewId, fps: u32) {
        if let Some(session) = self.sessions.get_mut(&preview_id) {
            session.target_fps = fps;
            // Note: The actual FPS change will happen on next capture restart
            // For live update, we'd need to use a shared atomic or channel
        }
    }

    /// Process any pending captured frames
    pub fn process_frames(&mut self, preview_manager: &mut PreviewManager, _ctx: &egui::Context) {
        // Process up to 10 frames per update to avoid blocking
        for _ in 0..10 {
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

/// Capture loop running in a separate thread
fn capture_window_loop(
    preview_id: PreviewId,
    _hwnd: isize,
    window_title: String,
    target_fps: u32,
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
        window::Window,
    };

    // Capture flags passed to the handler
    struct CaptureFlags {
        preview_id: PreviewId,
        sender: Sender<CapturedFrame>,
        active: Arc<RwLock<bool>>,
        paused: Arc<RwLock<bool>>,
        fps: u32,
    }

    struct Capture {
        preview_id: PreviewId,
        sender: Sender<CapturedFrame>,
        active: Arc<RwLock<bool>>,
        paused: Arc<RwLock<bool>>,
        frame_interval: std::time::Duration,
        last_frame: std::time::Instant,
    }

    impl GraphicsCaptureApiHandler for Capture {
        type Flags = CaptureFlags;
        type Error = Box<dyn std::error::Error + Send + Sync>;

        fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
            let frame_interval = std::time::Duration::from_secs_f64(1.0 / ctx.flags.fps as f64);
            Ok(Self {
                preview_id: ctx.flags.preview_id,
                sender: ctx.flags.sender,
                active: ctx.flags.active,
                paused: ctx.flags.paused,
                frame_interval,
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

            // Throttle frame rate
            let elapsed = self.last_frame.elapsed();
            if elapsed < self.frame_interval {
                return Ok(());
            }
            self.last_frame = std::time::Instant::now();

            // Get frame buffer
            let mut buffer = frame.buffer()?;
            let width = buffer.width();
            let height = buffer.height();

            // Copy frame data (BGRA format) - get buffer without padding
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

    // Find the window by title
    let window = {
        // First try exact title match
        match Window::from_name(&window_title) {
            Ok(w) => {
                log::info!("Found window by exact title: {}", window_title);
                w
            }
            Err(_) => {
                // Try partial title match (contains)
                match Window::from_contains_name(&window_title) {
                    Ok(w) => {
                        log::info!("Found window by partial title: {}", window_title);
                        w
                    }
                    Err(_) => {
                        // Last resort: enumerate and find by title substring
                        let mut found_window = None;

                        if let Ok(windows) = Window::enumerate() {
                            for win in windows {
                                if win.is_valid() {
                                    if let Ok(title) = win.title() {
                                        // Check if titles match (case-insensitive partial match)
                                        if title.to_lowercase().contains(&window_title.to_lowercase())
                                            || window_title.to_lowercase().contains(&title.to_lowercase())
                                        {
                                            log::info!("Found window by enumeration: {} (looking for {})", title, window_title);
                                            found_window = Some(win);
                                            break;
                                        }
                                    }
                                }
                            }
                        }

                        match found_window {
                            Some(w) => w,
                            None => {
                                log::error!("Could not find window with title: {}", window_title);
                                return;
                            }
                        }
                    }
                }
            }
        }
    };

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
        ColorFormat::Bgra8,
        flags,
    );

    // Start capture - this blocks until capture is stopped
    if let Err(e) = Capture::start(settings) {
        log::error!("Failed to start capture: {}", e);
    }
}
