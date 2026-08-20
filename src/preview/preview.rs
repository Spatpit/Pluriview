use eframe::egui::{self, Pos2, Rect, TextureHandle, Vec2};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::media::MediaFrame;
use crate::playlist::{FolderPlaylist, FolderPlaylistLayout};

/// How long the spawn-in / fade-out animations take.
const SPAWN_DURATION_SECS: f32 = 0.22;
const REMOVE_DURATION_SECS: f32 = 0.2;

/// Cubic ease-out: starts fast, settles smoothly.
fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Unique identifier for a preview
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PreviewId(pub u64);

/// Browser-specific startup state shown inside a tile before capture begins.
#[derive(Clone, Debug, PartialEq)]
pub enum BrowserTileStatus {
    Ready,
    PreparingAdblock { progress: f32 },
    Starting,
    Failed(String),
}

/// Persistent source for an optional mpv-backed video tile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VideoSource {
    LocalFile { path: PathBuf },
    Stream { url: String, quality: String },
}

/// Startup/runtime state rendered while an mpv tile has no captured frame.
#[derive(Clone, Debug, PartialEq)]
pub enum VideoTileStatus {
    Starting,
    Ready,
    PausedOnRestore,
    Buffering,
    Failed(String),
}

/// Track metadata mirrored from mpv's `track-list` property.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoTrack {
    pub id: i64,
    pub kind: String,
    pub title: Option<String>,
    pub language: Option<String>,
    pub selected: bool,
}

/// Playback properties mirrored from mpv JSON IPC for canvas controls.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoPlaybackState {
    pub connected: bool,
    pub paused: bool,
    pub time_pos: Option<f64>,
    pub duration: Option<f64>,
    pub volume: f64,
    pub muted: bool,
    pub speed: f64,
    pub looping: bool,
    pub tracks: Vec<VideoTrack>,
    pub audio_track: Option<i64>,
    pub subtitle_track: Option<i64>,
    pub seekable: bool,
}

impl Default for VideoPlaybackState {
    fn default() -> Self {
        Self {
            connected: false,
            paused: true,
            time_pos: None,
            duration: None,
            volume: 100.0,
            muted: false,
            speed: 1.0,
            looping: false,
            tracks: Vec::new(),
            audio_track: None,
            subtitle_track: None,
            seekable: false,
        }
    }
}

/// FPS presets for capture
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FpsPreset {
    Low = 15,
    #[default]
    Medium = 30,
    High = 60,
}

impl FpsPreset {
    pub fn as_u32(self) -> u32 {
        self as u32
    }

    pub fn label(self) -> &'static str {
        match self {
            FpsPreset::Low => "15 FPS (Low)",
            FpsPreset::Medium => "30 FPS (Medium)",
            FpsPreset::High => "60 FPS (High)",
        }
    }
}

/// Window handle information
#[derive(Clone, Debug)]
pub struct WindowHandle {
    pub hwnd: isize,
    pub process_id: u32,
}

/// Viewport corner used to keep a pinned Spout tile stable when the app window
/// changes size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewportAnchor {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Screen-space placement for a Spout tile that ignores canvas pan and zoom.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewportPin {
    pub anchor: ViewportAnchor,
    pub offset: (f32, f32),
    pub size: (f32, f32),
}

impl ViewportPin {
    pub fn from_rect(rect: Rect, viewport: Rect) -> Self {
        let horizontal_left = rect.center().x <= viewport.center().x;
        let vertical_top = rect.center().y <= viewport.center().y;
        let anchor = match (horizontal_left, vertical_top) {
            (true, true) => ViewportAnchor::TopLeft,
            (false, true) => ViewportAnchor::TopRight,
            (true, false) => ViewportAnchor::BottomLeft,
            (false, false) => ViewportAnchor::BottomRight,
        };
        let offset = match anchor {
            ViewportAnchor::TopLeft => (rect.left() - viewport.left(), rect.top() - viewport.top()),
            ViewportAnchor::TopRight => {
                (viewport.right() - rect.right(), rect.top() - viewport.top())
            }
            ViewportAnchor::BottomLeft => (
                rect.left() - viewport.left(),
                viewport.bottom() - rect.bottom(),
            ),
            ViewportAnchor::BottomRight => (
                viewport.right() - rect.right(),
                viewport.bottom() - rect.bottom(),
            ),
        };
        Self {
            anchor,
            offset,
            size: (rect.width().max(1.0), rect.height().max(1.0)),
        }
    }

    pub fn rect(self, viewport: Rect) -> Rect {
        let size = self.size_vec2();
        let offset = Vec2::new(self.offset.0, self.offset.1);
        let min = match self.anchor {
            ViewportAnchor::TopLeft => viewport.left_top() + offset,
            ViewportAnchor::TopRight => Pos2::new(
                viewport.right() - offset.x - size.x,
                viewport.top() + offset.y,
            ),
            ViewportAnchor::BottomLeft => Pos2::new(
                viewport.left() + offset.x,
                viewport.bottom() - offset.y - size.y,
            ),
            ViewportAnchor::BottomRight => viewport.right_bottom() - offset - size,
        };
        Rect::from_min_size(min, size)
    }

    pub fn size_vec2(self) -> Vec2 {
        Vec2::new(self.size.0.max(1.0), self.size.1.max(1.0))
    }
}

/// A live preview on the canvas
pub struct Preview {
    /// Unique ID
    pub id: PreviewId,

    /// Position on canvas (top-left corner, in canvas coordinates)
    pub position: Pos2,

    /// Size on canvas (in canvas coordinates)
    pub size: Vec2,

    /// Window being captured
    pub window_handle: Option<WindowHandle>,

    /// Spout2 sender name when this tile receives a GPU texture share.
    pub spout_sender: Option<String>,

    /// Screen-space placement when a Spout tile is pinned above the canvas.
    pub viewport_pin: Option<ViewportPin>,

    /// Display title (cached from window)
    pub title: String,

    /// Is capture paused (e.g., for viewport culling)?
    pub capture_paused: bool,

    /// User-requested freeze. Frozen tiles keep their last painted frame but
    /// must not advance media or restart a culled capture until resumed.
    /// This is deliberately runtime-only: saved workspaces start live.
    pub manually_frozen: bool,

    /// Lock aspect ratio when resizing? (always true by default)
    pub lock_aspect_ratio: bool,

    /// Source aspect ratio from the captured window (width/height)
    pub source_aspect_ratio: f32,

    /// Z-order (higher = on top)
    pub z_order: u32,

    /// Target FPS for capture
    pub target_fps: u32,

    /// FPS preset
    pub fps_preset: FpsPreset,

    /// Crop region in UV coordinates (0.0-1.0), None = full frame
    /// (min_u, min_v, max_u, max_v) where (0,0) is top-left and (1,1) is bottom-right
    pub crop_uv: Option<(f32, f32, f32, f32)>,

    /// Dimensions of the texture currently uploaded for this preview.
    pub frame_size: Option<(u32, u32)>,

    /// Native dimensions produced by a live capture source before adaptive
    /// downsampling. Used for pixel-accurate sizing and crop coordinates.
    pub source_frame_size: Option<(u32, u32)>,

    /// Current frame texture
    texture: Option<TextureHandle>,

    /// Frame data buffer (BGRA)
    frame_buffer: Option<FrameData>,

    /// Current URL when this preview is a browser tile, None otherwise.
    /// Kept up to date as the page navigates so layouts save where the
    /// user actually is, not where the tile started.
    pub browser_url: Option<String>,

    /// Is this browser tile's audio muted? (Only meaningful for browsers.)
    pub browser_muted: bool,

    /// Replay this window-capture tile's process audio through the stream
    /// audio monitor (Discord/OBS). Ignored for browser, video, and image tiles.
    pub stream_audio: bool,

    /// Set when Windows Graphics Capture fails to start. Cleared when a frame
    /// arrives or a new capture session is connecting.
    pub capture_error: Option<String>,

    /// Loading/error state rendered while a browser host is being prepared.
    pub browser_status: BrowserTileStatus,

    /// Managed filename in `pluriview_data/media` for image and GIF tiles.
    pub media_path: Option<String>,

    /// Local file or Streamlink URL when this is an mpv-backed video tile.
    pub video_source: Option<VideoSource>,

    /// Folder-backed playlist displayed as its own canvas tile.
    pub folder_playlist: Option<FolderPlaylist>,

    /// Persistent relationship shared by a folder playlist and its video tile.
    pub playlist_group: Option<u64>,

    /// Runtime target for playlist selections. Rebuilt from `playlist_group`
    /// when a workspace is restored.
    pub playlist_linked_video: Option<PreviewId>,

    /// Loading/error state for an mpv-backed video tile.
    pub video_status: VideoTileStatus,

    /// Last playback state received from mpv.
    pub video_playback: VideoPlaybackState,

    /// In-process libmpv renderer. Unlike a captured external mpv window,
    /// this paints directly into Pluriview's OpenGL canvas.
    #[cfg(windows)]
    pub video_renderer: Option<std::sync::Arc<crate::libmpv::VideoRenderer>>,

    /// Separately decoded frame shown while hovering a seekable video's seek bar.
    seek_preview_time: Option<f64>,
    seek_preview_texture: Option<TextureHandle>,
    seek_preview_frame: Option<FrameData>,

    /// Decoded frames for a managed image. A single frame is a static image.
    media_frames: Vec<MediaFrame>,
    media_frame_index: usize,
    media_frame_dirty: bool,
    media_frame_started: Instant,

    /// When this preview was created (drives the spawn-in animation)
    pub created_at: Instant,

    /// Set when removal has been requested; drives the fade/shrink-out
    /// animation. The preview is only actually dropped from the manager
    /// once `removal_progress()` reaches 1.0.
    pub removing: Option<Instant>,
}

/// Raw frame data from capture
struct FrameData {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl Preview {
    /// Create a new preview
    pub fn new(id: PreviewId, title: String, position: Pos2, size: Vec2) -> Self {
        let aspect_ratio = size.x / size.y;
        Self {
            id,
            position,
            size,
            window_handle: None,
            spout_sender: None,
            viewport_pin: None,
            title,
            capture_paused: false,
            manually_frozen: false,
            lock_aspect_ratio: true,
            source_aspect_ratio: aspect_ratio,
            z_order: 0,
            target_fps: FpsPreset::default().as_u32(),
            fps_preset: FpsPreset::default(),
            crop_uv: None,
            frame_size: None,
            source_frame_size: None,
            texture: None,
            frame_buffer: None,
            browser_url: None,
            browser_muted: false,
            stream_audio: false,
            capture_error: None,
            browser_status: BrowserTileStatus::Ready,
            media_path: None,
            video_source: None,
            folder_playlist: None,
            playlist_group: None,
            playlist_linked_video: None,
            video_status: VideoTileStatus::Ready,
            video_playback: VideoPlaybackState::default(),
            #[cfg(windows)]
            video_renderer: None,
            seek_preview_time: None,
            seek_preview_texture: None,
            seek_preview_frame: None,
            media_frames: Vec::new(),
            media_frame_index: 0,
            media_frame_dirty: false,
            media_frame_started: Instant::now(),
            created_at: Instant::now(),
            removing: None,
        }
    }

    /// Is this preview an app-owned browser tile?
    pub fn is_browser(&self) -> bool {
        self.browser_url.is_some()
    }

    /// Is this preview backed by a managed image or GIF?
    pub fn is_media(&self) -> bool {
        self.media_path.is_some()
    }

    /// Is this preview backed by an optional mpv session?
    pub fn is_video(&self) -> bool {
        self.video_source.is_some()
    }

    pub fn is_playlist(&self) -> bool {
        self.folder_playlist.is_some()
    }

    /// Captured OS window/game — not a browser, video, image, playlist, or Spout tile.
    pub fn is_window_capture(&self) -> bool {
        self.window_handle.is_some()
            && self.spout_sender.is_none()
            && !self.is_browser()
            && !self.is_media()
            && !self.is_video()
            && !self.is_playlist()
    }

    pub fn is_spout_capture(&self) -> bool {
        self.spout_sender.is_some()
    }

    /// Window Graphics Capture or a Spout sender feeding this tile.
    pub fn is_live_capture(&self) -> bool {
        self.is_window_capture() || self.is_spout_capture()
    }

    /// Record that Windows Graphics Capture could not start for this tile.
    pub fn set_capture_error(&mut self, error: String) {
        self.capture_error = Some(error.clone());
        if self.is_browser() {
            self.browser_status = BrowserTileStatus::Failed(error);
        }
    }

    /// Clear a previous capture start failure, restoring Connecting/Ready.
    pub fn clear_capture_error(&mut self) {
        self.capture_error = None;
        if self.is_browser() && matches!(self.browser_status, BrowserTileStatus::Failed(_)) {
            self.browser_status = BrowserTileStatus::Ready;
        }
    }

    pub fn supports_seek_preview(&self) -> bool {
        let duration_ok = self
            .video_playback
            .duration
            .is_some_and(|duration| duration.is_finite() && duration > 0.0);
        match self.video_source.as_ref() {
            Some(VideoSource::LocalFile { .. }) => duration_ok,
            Some(VideoSource::Stream { .. }) => duration_ok && self.video_playback.seekable,
            None => false,
        }
    }

    pub fn update_seek_preview(&mut self, time: f64, width: u32, height: u32, data: Vec<u8>) {
        self.seek_preview_time = Some(time);
        self.seek_preview_frame = Some(FrameData {
            width,
            height,
            data,
        });
    }

    pub fn clear_seek_preview(&mut self) {
        self.seek_preview_time = None;
        self.seek_preview_texture = None;
        self.seek_preview_frame = None;
    }

    pub fn seek_preview_time(&self) -> Option<f64> {
        self.seek_preview_time
    }

    pub fn get_seek_preview_texture(&mut self, ctx: &egui::Context) -> Option<&TextureHandle> {
        if let Some(frame) = self.seek_preview_frame.take() {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &frame.data,
            );
            if let Some(texture) = self.seek_preview_texture.as_mut() {
                texture.set(image, egui::TextureOptions::LINEAR);
            } else {
                self.seek_preview_texture = Some(ctx.load_texture(
                    format!("seek_preview_{}", self.id.0),
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        self.seek_preview_texture.as_ref()
    }

    /// Attach decoded image data to this preview.
    pub fn set_media(&mut self, managed_path: String, frames: Vec<MediaFrame>) {
        self.media_path = Some(managed_path);
        self.media_frames = frames;
        self.media_frame_index = 0;
        self.media_frame_dirty = true;
        self.media_frame_started = Instant::now();
        if let Some(frame) = self.media_frames.first() {
            self.frame_size = Some((frame.width, frame.height));
            self.source_aspect_ratio = frame.width as f32 / frame.height as f32;
        }
    }

    /// Create a preview that receives a Spout2 sender.
    pub fn for_spout(id: PreviewId, sender_name: String, position: Pos2, size: Vec2) -> Self {
        let mut preview = Self::new(id, sender_name.clone(), position, size);
        preview.spout_sender = Some(sender_name);
        preview
    }

    /// Create a preview for a specific window
    pub fn for_window(
        id: PreviewId,
        hwnd: isize,
        process_id: u32,
        title: String,
        position: Pos2,
        size: Vec2,
    ) -> Self {
        let mut preview = Self::new(id, title, position, size);
        preview.window_handle = Some(WindowHandle { hwnd, process_id });
        preview
    }

    /// Get the bounding rectangle
    pub fn rect(&self) -> Rect {
        Rect::from_min_size(self.position, self.size)
    }

    /// Set the FPS preset
    pub fn set_fps_preset(&mut self, preset: FpsPreset) {
        self.fps_preset = preset;
        self.target_fps = preset.as_u32();
    }

    /// Update position
    pub fn translate(&mut self, delta: Vec2) {
        self.position += delta;
    }

    /// Update frame data from capture
    pub fn update_frame(&mut self, width: u32, height: u32, data: Vec<u8>) {
        self.update_capture_frame(width, height, width, height, data);
    }

    /// Update a live-capture texture while retaining the source's native size.
    pub fn update_capture_frame(
        &mut self,
        width: u32,
        height: u32,
        source_width: u32,
        source_height: u32,
        data: Vec<u8>,
    ) {
        self.clear_capture_error();
        if source_width > 0 && source_height > 0 {
            self.source_frame_size = Some((source_width, source_height));
            // Only update aspect ratio if we don't have a crop region
            if self.crop_uv.is_none() {
                self.source_aspect_ratio = source_width as f32 / source_height as f32;
            }
        }
        if width > 0 && height > 0 {
            self.frame_size = Some((width, height));
        }

        self.frame_buffer = Some(FrameData {
            width,
            height,
            data,
        });
    }

    /// Get UV coordinates for rendering (either crop region or full frame)
    pub fn get_uv_rect(&self) -> Rect {
        if let Some(crop) = self.crop_uv {
            Rect::from_min_max(Pos2::new(crop.0, crop.1), Pos2::new(crop.2, crop.3))
        } else {
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))
        }
    }

    /// Clear crop region (show full frame)
    pub fn clear_crop(&mut self) {
        self.crop_uv = None;
        // Restore aspect ratio from frame size
        if let Some((w, h)) = self.source_frame_size.or(self.frame_size) {
            if h > 0 {
                self.source_aspect_ratio = w as f32 / h as f32;
            }
        }
    }

    /// Get or create texture from frame buffer
    pub fn get_texture(&mut self, ctx: &egui::Context) -> Option<&TextureHandle> {
        self.advance_media_animation(ctx);

        // Media frames already own their RGBA bytes, so build the upload
        // image directly from that slice instead of cloning a second buffer.
        let media_image = if self.media_frame_dirty {
            self.media_frame_dirty = false;
            self.media_frames.get(self.media_frame_index).map(|frame| {
                egui::ColorImage::from_rgba_unmultiplied(
                    [frame.width as usize, frame.height as usize],
                    &frame.rgba,
                )
            })
        } else {
            None
        };
        let capture_image = self.frame_buffer.take().map(|frame| {
            egui::ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &frame.data,
            )
        });

        if let Some(image) = media_image.or(capture_image) {
            if let Some(texture) = self.texture.as_mut() {
                texture.set(image, egui::TextureOptions::LINEAR);
            } else {
                self.texture = Some(ctx.load_texture(
                    format!("preview_{}", self.id.0),
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }

        self.texture.as_ref()
    }

    /// Advance an animated image according to its authored per-frame delays.
    fn advance_media_animation(&mut self, ctx: &egui::Context) {
        if self.manually_frozen || self.media_frames.len() <= 1 {
            return;
        }

        let mut elapsed = self.media_frame_started.elapsed();
        let mut advanced = false;
        // Consume overdue frames so GIFs recover after the UI thread was busy,
        // while bounding work per paint.
        for _ in 0..self.media_frames.len() {
            let delay = self.media_frames[self.media_frame_index].duration;
            if elapsed < delay {
                break;
            }
            elapsed = elapsed.saturating_sub(delay);
            self.media_frame_index = (self.media_frame_index + 1) % self.media_frames.len();
            self.media_frame_started = Instant::now() - elapsed;
            advanced = true;
        }

        if advanced {
            self.media_frame_dirty = true;
        }

        let delay = self.media_frames[self.media_frame_index].duration;
        let remaining = delay.saturating_sub(self.media_frame_started.elapsed());
        ctx.request_repaint_after(remaining.max(Duration::from_millis(1)));
    }

    /// Check if this preview contains the given canvas point
    pub fn contains(&self, point: Pos2) -> bool {
        self.rect().contains(point)
    }

    /// 0.0 (just created) .. 1.0 (fully spawned in), eased.
    pub fn spawn_progress(&self) -> f32 {
        let t = self.created_at.elapsed().as_secs_f32() / SPAWN_DURATION_SECS;
        ease_out_cubic(t)
    }

    /// Mark this preview as pending removal (idempotent). The actual
    /// removal from the manager happens once the fade-out completes.
    pub fn start_removal(&mut self) {
        if self.removing.is_none() {
            self.removing = Some(Instant::now());
        }
    }

    /// 0.0 (not removing / just started) .. 1.0 (fade-out complete), eased.
    pub fn removal_progress(&self) -> f32 {
        match self.removing {
            Some(started) => ease_out_cubic(started.elapsed().as_secs_f32() / REMOVE_DURATION_SECS),
            None => 0.0,
        }
    }

    /// True once a removal has been requested and its animation has finished.
    pub fn is_removal_complete(&self) -> bool {
        self.removing
            .map(|started| started.elapsed().as_secs_f32() >= REMOVE_DURATION_SECS)
            .unwrap_or(false)
    }
}

/// Serializable layout for persistence
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreviewLayout {
    pub position: (f32, f32),
    pub size: (f32, f32),
    pub window_title: String,
    pub window_exe: Option<String>,
    pub lock_aspect_ratio: bool,
    pub z_order: u32,
    pub fps_preset: FpsPreset,
    /// Crop region in UV coordinates (optional)
    #[serde(default)]
    pub crop_uv: Option<(f32, f32, f32, f32)>,
    /// Browser tiles restore by recreating a WebView at this URL instead of
    /// matching an open window.
    #[serde(default)]
    pub browser_url: Option<String>,
    /// WebView2 mute is per-session, so remember it and reapply on restore.
    #[serde(default)]
    pub browser_muted: bool,
    /// Replay this captured window's audio through the stream monitor.
    #[serde(default)]
    pub stream_audio: bool,
    /// Managed filename for an image/GIF tile. Kept relative so portable
    /// installs can be moved as a unit.
    #[serde(default)]
    pub media_path: Option<String>,
    /// Optional mpv-backed local file or Streamlink source.
    #[serde(default)]
    pub video_source: Option<VideoSource>,
    /// Relationship shared by a folder playlist and its linked video.
    #[serde(default)]
    pub playlist_group: Option<u64>,
    /// Persisted folder playlist state; entries are rescanned on restore.
    #[serde(default)]
    pub folder_playlist: Option<FolderPlaylistLayout>,
    /// Spout2 sender name for tiles that receive a GPU texture share.
    #[serde(default)]
    pub spout_sender: Option<String>,
    /// Viewport placement for Spout tiles pinned independently of the canvas.
    #[serde(default)]
    pub viewport_pin: Option<ViewportPin>,
}

impl From<&Preview> for PreviewLayout {
    fn from(preview: &Preview) -> Self {
        Self {
            position: (preview.position.x, preview.position.y),
            size: (preview.size.x, preview.size.y),
            window_title: match preview.video_source.as_ref() {
                Some(source) => video_tile_title(source, Some(&preview.title)),
                None => preview.title.clone(),
            },
            window_exe: None, // TODO: Get exe name from window handle
            lock_aspect_ratio: preview.lock_aspect_ratio,
            z_order: preview.z_order,
            fps_preset: preview.fps_preset,
            crop_uv: preview.crop_uv,
            browser_url: preview.browser_url.clone(),
            browser_muted: preview.browser_muted,
            stream_audio: preview.stream_audio,
            media_path: preview.media_path.clone(),
            video_source: preview.video_source.as_ref().map(scrub_video_source),
            playlist_group: preview.playlist_group,
            folder_playlist: preview.folder_playlist.as_ref().map(FolderPlaylist::layout),
            spout_sender: preview.spout_sender.clone(),
            viewport_pin: preview.viewport_pin,
        }
    }
}

fn scrub_video_source(source: &VideoSource) -> VideoSource {
    let VideoSource::Stream { url, quality } = source else {
        return source.clone();
    };
    let Ok(mut parsed) = url::Url::parse(url) else {
        return VideoSource::Stream {
            url: String::new(),
            quality: quality.clone(),
        };
    };
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.set_fragment(None);
    let retained_query: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(name, _)| !is_sensitive_stream_parameter(name))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    parsed.set_query(None);
    if !retained_query.is_empty() {
        parsed.query_pairs_mut().extend_pairs(retained_query);
    }
    VideoSource::Stream {
        url: parsed.into(),
        quality: quality.clone(),
    }
}

fn is_sensitive_stream_parameter(name: &str) -> bool {
    let name = name.to_ascii_lowercase().replace(['-', '.'], "_");
    matches!(
        name.as_str(),
        "access_token"
            | "api_key"
            | "auth"
            | "authorization"
            | "credential"
            | "expire"
            | "expires"
            | "exp"
            | "jwt"
            | "key"
            | "password"
            | "policy"
            | "secret"
            | "sig"
            | "signature"
            | "token"
    ) || name.ends_with("_token")
        || name.ends_with("_key")
        || name.ends_with("_password")
        || name.ends_with("_signature")
        || name.contains("credential")
        || name.starts_with("x_amz_")
}

const MAX_USABLE_TITLE_CHARS: usize = 80;

/// True when mpv's `media-title` is a real name, not a CDN URL or query blob.
pub fn is_usable_media_title(title: &str) -> bool {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > MAX_USABLE_TITLE_CHARS {
        return false;
    }
    let lower = title.to_ascii_lowercase();
    if lower.contains("://") || lower.starts_with("http:") || lower.starts_with("https:") {
        return false;
    }
    if lower.contains("videoplayback")
        || lower.contains("googlevideo")
        || lower.contains("mime=")
        || lower.contains("clen=")
        || lower.contains("ratebypass=")
    {
        return false;
    }
    !(title.contains('&') && title.contains('='))
}

/// Compact label for menus and overlays so a long title cannot stretch the UI.
pub fn compact_title(title: &str, max_chars: usize) -> String {
    let title = title.trim();
    if title.chars().count() <= max_chars {
        return title.to_owned();
    }
    let truncated: String = title.chars().take(max_chars.saturating_sub(3)).collect();
    format!("{truncated}...")
}

/// Display name for a video tile: a usable title, else a short name from the source.
pub fn video_tile_title(source: &VideoSource, preferred: Option<&str>) -> String {
    if let Some(title) = preferred.filter(|title| is_usable_media_title(title)) {
        return title.trim().to_owned();
    }
    match source {
        VideoSource::LocalFile { path } => path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| "Video".to_owned()),
        VideoSource::Stream { url, .. } => stream_display_title(url),
    }
}

fn stream_display_title(url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else {
        return compact_title(url, 48);
    };
    let host = parsed
        .host_str()
        .unwrap_or("stream")
        .trim_start_matches("www.");
    if host.contains("googlevideo.com") {
        return "YouTube stream".to_owned();
    }
    if is_youtube_host(host) {
        return match youtube_video_id(&parsed) {
            Some(id) => format!("YouTube · {id}"),
            None => "YouTube".to_owned(),
        };
    }
    if host == "twitch.tv" || host.ends_with(".twitch.tv") {
        return match twitch_label(&parsed) {
            Some(label) => format!("Twitch · {label}"),
            None => "Twitch".to_owned(),
        };
    }
    let leaf = parsed
        .path()
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");
    if leaf.is_empty() || leaf.contains('&') || leaf.contains('=') {
        compact_title(host, 48)
    } else {
        compact_title(&format!("{host} · {leaf}"), 48)
    }
}

fn is_youtube_host(host: &str) -> bool {
    matches!(
        host,
        "youtube.com" | "m.youtube.com" | "music.youtube.com" | "youtube-nocookie.com" | "youtu.be"
    ) || host.ends_with(".youtube.com")
        || host.ends_with(".youtu.be")
}

fn youtube_video_id(parsed: &url::Url) -> Option<String> {
    if parsed
        .host_str()
        .is_some_and(|host| host.trim_start_matches("www.") == "youtu.be")
    {
        let id = parsed.path().trim_matches('/').split('/').next()?;
        if !id.is_empty() {
            return Some(id.to_owned());
        }
    }
    if let Some((_, id)) = parsed.query_pairs().find(|(name, _)| name == "v") {
        if !id.is_empty() {
            return Some(id.into_owned());
        }
    }
    let path = parsed.path().trim_matches('/');
    for prefix in ["shorts/", "embed/", "live/", "v/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            let id = rest.split('/').next().unwrap_or("");
            if !id.is_empty() {
                return Some(id.to_owned());
            }
        }
    }
    None
}

fn twitch_label(parsed: &url::Url) -> Option<String> {
    let mut segments = parsed.path().trim_matches('/').split('/');
    let first = segments.next().filter(|segment| !segment.is_empty())?;
    if first.eq_ignore_ascii_case("videos") {
        let id = segments.next().filter(|segment| !segment.is_empty())?;
        Some(format!("video {id}"))
    } else {
        Some(first.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Preview, PreviewId, PreviewLayout, VideoSource, ViewportAnchor, ViewportPin, WindowHandle,
    };
    use crate::media::MediaFrame;
    use eframe::egui::{Context, Pos2, Rect, Vec2};
    use std::time::{Duration, Instant};

    #[test]
    fn frame_updates_reuse_the_texture() {
        let context = Context::default();
        let mut preview = Preview::new(
            PreviewId(1),
            "test".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );

        preview.update_frame(1, 1, vec![255, 0, 0, 255]);
        let first = preview.get_texture(&context).unwrap().id();
        preview.update_frame(1, 1, vec![0, 255, 0, 255]);
        let second = preview.get_texture(&context).unwrap().id();

        assert_eq!(first, second);
    }

    #[test]
    fn media_upload_does_not_stage_a_second_rgba_buffer() {
        let context = Context::default();
        let mut preview = Preview::new(
            PreviewId(1),
            "test".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        preview.set_media(
            "test.gif".to_owned(),
            vec![MediaFrame {
                width: 1,
                height: 1,
                rgba: vec![255, 0, 0, 255],
                duration: Duration::from_millis(100),
            }],
        );

        assert!(preview.frame_buffer.is_none());
        assert!(preview.media_frame_dirty);
        assert!(preview.get_texture(&context).is_some());
        assert!(!preview.media_frame_dirty);
        assert!(preview.frame_buffer.is_none());
    }

    #[test]
    fn frozen_gif_does_not_advance_or_schedule_from_elapsed_time() {
        let context = Context::default();
        let mut preview = Preview::new(
            PreviewId(1),
            "test".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        preview.set_media(
            "test.gif".to_owned(),
            vec![
                MediaFrame {
                    width: 1,
                    height: 1,
                    rgba: vec![255, 0, 0, 255],
                    duration: Duration::from_millis(10),
                },
                MediaFrame {
                    width: 1,
                    height: 1,
                    rgba: vec![0, 255, 0, 255],
                    duration: Duration::from_millis(10),
                },
            ],
        );
        preview.manually_frozen = true;
        preview.media_frame_started = Instant::now() - Duration::from_secs(1);

        let _ = preview.get_texture(&context);

        assert_eq!(preview.media_frame_index, 0);
    }

    #[test]
    fn older_saved_tiles_default_to_no_media() {
        let preview = Preview::new(
            PreviewId(1),
            "test".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        let mut value = serde_json::to_value(PreviewLayout::from(&preview)).unwrap();
        value.as_object_mut().unwrap().remove("media_path");

        let restored: PreviewLayout = serde_json::from_value(value).unwrap();
        assert!(restored.media_path.is_none());
    }

    #[test]
    fn older_saved_tiles_default_to_no_spout_sender() {
        let preview = Preview::new(
            PreviewId(1),
            "test".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        let mut value = serde_json::to_value(PreviewLayout::from(&preview)).unwrap();
        value.as_object_mut().unwrap().remove("spout_sender");

        let restored: PreviewLayout = serde_json::from_value(value).unwrap();
        assert!(restored.spout_sender.is_none());
    }

    #[test]
    fn older_saved_tiles_default_to_not_pinned() {
        let preview = Preview::new(
            PreviewId(1),
            "test".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        let mut value = serde_json::to_value(PreviewLayout::from(&preview)).unwrap();
        value.as_object_mut().unwrap().remove("viewport_pin");

        let restored: PreviewLayout = serde_json::from_value(value).unwrap();
        assert!(restored.viewport_pin.is_none());
    }

    #[test]
    fn viewport_pin_preserves_its_nearest_corner_offset() {
        let initial_viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0));
        let pin = ViewportPin::from_rect(
            Rect::from_min_size(Pos2::new(700.0, 500.0), Vec2::new(200.0, 200.0)),
            initial_viewport,
        );
        assert_eq!(pin.anchor, ViewportAnchor::BottomRight);
        assert_eq!(pin.offset, (100.0, 100.0));

        let resized_viewport = Rect::from_min_size(Pos2::new(20.0, 30.0), Vec2::new(1200.0, 900.0));
        assert_eq!(
            pin.rect(resized_viewport),
            Rect::from_min_size(Pos2::new(920.0, 630.0), Vec2::new(200.0, 200.0))
        );
    }

    #[test]
    fn older_saved_tiles_default_stream_audio_off() {
        let preview = Preview::new(
            PreviewId(1),
            "test".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        let mut value = serde_json::to_value(PreviewLayout::from(&preview)).unwrap();
        value.as_object_mut().unwrap().remove("stream_audio");

        let restored: PreviewLayout = serde_json::from_value(value).unwrap();
        assert!(!restored.stream_audio);
    }

    #[test]
    fn stream_audio_round_trips_through_layout() {
        let mut preview = Preview::for_window(
            PreviewId(1),
            1,
            42,
            "game".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        preview.stream_audio = true;

        let restored = PreviewLayout::from(&preview);
        assert!(restored.stream_audio);
        assert!(preview.is_window_capture());
    }

    #[test]
    fn spout_tiles_round_trip_and_are_not_window_captures() {
        let mut preview = Preview::for_spout(
            PreviewId(1),
            "VTube Studio".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        preview.viewport_pin = Some(ViewportPin {
            anchor: ViewportAnchor::BottomRight,
            offset: (24.0, 24.0),
            size: (320.0, 480.0),
        });
        assert!(preview.is_spout_capture());
        assert!(!preview.is_window_capture());
        assert!(preview.is_live_capture());

        let restored = PreviewLayout::from(&preview);
        assert_eq!(restored.spout_sender.as_deref(), Some("VTube Studio"));
        assert_eq!(restored.viewport_pin, preview.viewport_pin);
    }

    #[test]
    fn browser_tiles_are_not_window_captures() {
        let mut preview = Preview::new(
            PreviewId(1),
            "browser".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        preview.window_handle = Some(WindowHandle {
            hwnd: 1,
            process_id: 2,
        });
        preview.browser_url = Some("https://example.com".to_owned());
        assert!(!preview.is_window_capture());
    }

    #[test]
    fn capture_error_marks_window_and_browser_tiles() {
        let mut window = Preview::for_window(
            PreviewId(1),
            1,
            42,
            "game".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        window.set_capture_error("start failed".to_owned());
        assert_eq!(window.capture_error.as_deref(), Some("start failed"));

        let mut browser = Preview::new(
            PreviewId(2),
            "browser".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        browser.browser_url = Some("https://example.com".to_owned());
        browser.set_capture_error("start failed".to_owned());
        assert!(matches!(
            browser.browser_status,
            super::BrowserTileStatus::Failed(_)
        ));
        browser.clear_capture_error();
        assert!(browser.capture_error.is_none());
        assert_eq!(browser.browser_status, super::BrowserTileStatus::Ready);
    }

    #[test]
    fn frame_update_clears_a_capture_error() {
        let mut preview = Preview::for_window(
            PreviewId(1),
            1,
            42,
            "game".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        preview.set_capture_error("start failed".to_owned());
        preview.update_frame(1, 1, vec![255, 0, 0, 255]);
        assert!(preview.capture_error.is_none());
    }

    #[test]
    fn older_saved_tiles_default_to_no_video() {
        let preview = Preview::new(
            PreviewId(1),
            "test".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        let mut value = serde_json::to_value(PreviewLayout::from(&preview)).unwrap();
        value.as_object_mut().unwrap().remove("video_source");

        let restored: PreviewLayout = serde_json::from_value(value).unwrap();
        assert!(restored.video_source.is_none());
    }

    #[test]
    fn older_saved_tiles_default_to_no_folder_playlist() {
        let preview = Preview::new(
            PreviewId(1),
            "test".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        let mut value = serde_json::to_value(PreviewLayout::from(&preview)).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("folder_playlist");
        object.remove("playlist_group");

        let restored: PreviewLayout = serde_json::from_value(value).unwrap();
        assert!(restored.folder_playlist.is_none());
        assert!(restored.playlist_group.is_none());
    }

    #[test]
    fn video_source_round_trips_through_layout() {
        let mut preview = Preview::new(
            PreviewId(1),
            "stream".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        preview.video_source = Some(VideoSource::Stream {
            url: "https://twitch.tv/example".to_owned(),
            quality: "best".to_owned(),
        });

        let value = serde_json::to_value(PreviewLayout::from(&preview)).unwrap();
        let restored: PreviewLayout = serde_json::from_value(value).unwrap();
        assert_eq!(restored.video_source, preview.video_source);
    }

    #[test]
    fn persisted_stream_urls_remove_credentials_and_tokens() {
        let mut preview = Preview::new(
            PreviewId(1),
            "stream".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        preview.video_source = Some(VideoSource::Stream {
            url: "https://user:password@youtube.com/watch?v=abc123&token=secret&sig=private&api_key=hidden&password=hidden&expire=1&X-Amz-Credential=hidden&X-Amz-Signature=hidden#chat".to_owned(),
            quality: "best".to_owned(),
        });

        let layout = PreviewLayout::from(&preview);
        assert_eq!(
            layout.video_source,
            Some(VideoSource::Stream {
                url: "https://youtube.com/watch?v=abc123".to_owned(),
                quality: "best".to_owned(),
            })
        );
    }

    #[test]
    fn unparseable_stream_urls_are_not_persisted_verbatim() {
        let mut preview = Preview::new(
            PreviewId(1),
            "stream".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        preview.video_source = Some(VideoSource::Stream {
            url: "not a url with token=secret".to_owned(),
            quality: "best".to_owned(),
        });

        let layout = PreviewLayout::from(&preview);
        assert_eq!(
            layout.video_source,
            Some(VideoSource::Stream {
                url: String::new(),
                quality: "best".to_owned(),
            })
        );
    }

    #[test]
    fn local_files_support_seek_preview_once_duration_is_known() {
        let mut preview = Preview::new(
            PreviewId(1),
            "local".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        preview.video_source = Some(VideoSource::LocalFile {
            path: std::path::PathBuf::from("clip.mp4"),
        });
        assert!(!preview.supports_seek_preview());
        preview.video_playback.duration = Some(90.0);
        assert!(preview.supports_seek_preview());
    }

    #[test]
    fn livestreams_do_not_support_seek_preview() {
        let mut preview = Preview::new(
            PreviewId(1),
            "live".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        preview.video_source = Some(VideoSource::Stream {
            url: "https://example.test/live".to_owned(),
            quality: "best".to_owned(),
        });
        preview.video_playback.duration = Some(30.0);
        preview.video_playback.seekable = false;
        assert!(!preview.supports_seek_preview());
        preview.video_playback.seekable = true;
        assert!(preview.supports_seek_preview());
    }

    #[test]
    fn stream_cdn_blobs_are_not_usable_titles() {
        assert!(super::is_usable_media_title("Interview clip"));
        assert!(!super::is_usable_media_title(
            "mp4&rqh=1&gir=yes&clen=90158012&ratebypass=yes&dur=2756.765"
        ));
        assert!(!super::is_usable_media_title(
            "https://www.youtube.com/watch?v=abc123"
        ));
        assert_eq!(
            super::video_tile_title(
                &VideoSource::Stream {
                    url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".to_owned(),
                    quality: "best".to_owned(),
                },
                Some("mp4&rqh=1&gir=yes&clen=90158012&ratebypass=yes")
            ),
            "YouTube · dQw4w9WgXcQ"
        );
        assert_eq!(
            super::video_tile_title(
                &VideoSource::Stream {
                    url: "https://www.twitch.tv/example".to_owned(),
                    quality: "best".to_owned(),
                },
                Some("https://www.twitch.tv/example")
            ),
            "Twitch · example"
        );
        assert_eq!(
            super::video_tile_title(
                &VideoSource::Stream {
                    url: "https://rr5---sn-abc.googlevideo.com/videoplayback?expire=1".to_owned(),
                    quality: "best".to_owned(),
                },
                None
            ),
            "YouTube stream"
        );
        assert_eq!(super::compact_title("short", 42), "short");
        assert_eq!(
            super::compact_title("abcdefghijklmnopqrstuvwxyz", 10),
            "abcdefg..."
        );
    }

    #[test]
    fn persisted_stream_titles_drop_cdn_query_blobs() {
        let mut preview = Preview::new(
            PreviewId(1),
            "mp4&rqh=1&gir=yes&clen=90158012&ratebypass=yes&dur=2756.765".to_owned(),
            Pos2::ZERO,
            Vec2::splat(1.0),
        );
        preview.video_source = Some(VideoSource::Stream {
            url: "https://www.youtube.com/watch?v=abc123".to_owned(),
            quality: "best".to_owned(),
        });
        let layout = PreviewLayout::from(&preview);
        assert_eq!(layout.window_title, "YouTube · abc123");
    }
}
