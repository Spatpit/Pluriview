use eframe::egui::{self, Pos2, Vec2, Rect, TextureHandle};
use serde::{Serialize, Deserialize};
use std::sync::Arc;
use parking_lot::RwLock;

/// Unique identifier for a preview
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PreviewId(pub u64);

/// FPS presets for capture
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FpsPreset {
    Low = 15,
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

impl Default for FpsPreset {
    fn default() -> Self {
        FpsPreset::Medium
    }
}

/// Window handle information
#[derive(Clone, Debug)]
pub struct WindowHandle {
    pub hwnd: isize,
    #[allow(dead_code)]
    pub process_id: u32,
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

    /// Display title (cached from window)
    pub title: String,

    /// Is capture currently active?
    pub capture_active: bool,

    /// Is capture paused (e.g., for viewport culling)?
    pub capture_paused: bool,

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

    /// Original frame dimensions (updated when receiving frames)
    pub frame_size: Option<(u32, u32)>,

    /// Current frame texture
    texture: Option<TextureHandle>,

    /// Frame data buffer (BGRA)
    frame_buffer: Arc<RwLock<Option<FrameData>>>,
}

/// Raw frame data from capture
#[derive(Clone)]
pub struct FrameData {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
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
            title,
            capture_active: false,
            capture_paused: false,
            lock_aspect_ratio: true,
            source_aspect_ratio: aspect_ratio,
            z_order: 0,
            target_fps: FpsPreset::default().as_u32(),
            fps_preset: FpsPreset::default(),
            crop_uv: None,
            frame_size: None,
            texture: None,
            frame_buffer: Arc::new(RwLock::new(None)),
        }
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

    /// Get frame buffer for capture thread to write to
    #[allow(dead_code)]
    pub fn get_frame_buffer(&self) -> Arc<RwLock<Option<FrameData>>> {
        self.frame_buffer.clone()
    }

    /// Update frame data from capture
    pub fn update_frame(&mut self, width: u32, height: u32, data: Vec<u8>) {
        // Update source aspect ratio from actual frame dimensions
        if width > 0 && height > 0 {
            self.frame_size = Some((width, height));
            // Only update aspect ratio if we don't have a crop region
            if self.crop_uv.is_none() {
                self.source_aspect_ratio = width as f32 / height as f32;
            }
        }

        let mut buffer = self.frame_buffer.write();
        *buffer = Some(FrameData { width, height, data });
    }

    /// Get the effective aspect ratio (considering crop region)
    #[allow(dead_code)]
    pub fn effective_aspect_ratio(&self) -> f32 {
        if let (Some(crop), Some((w, h))) = (self.crop_uv, self.frame_size) {
            let crop_width = (crop.2 - crop.0) * w as f32;
            let crop_height = (crop.3 - crop.1) * h as f32;
            if crop_height > 0.0 {
                crop_width / crop_height
            } else {
                self.source_aspect_ratio
            }
        } else {
            self.source_aspect_ratio
        }
    }

    /// Get UV coordinates for rendering (either crop region or full frame)
    pub fn get_uv_rect(&self) -> Rect {
        if let Some(crop) = self.crop_uv {
            Rect::from_min_max(
                Pos2::new(crop.0, crop.1),
                Pos2::new(crop.2, crop.3),
            )
        } else {
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))
        }
    }

    /// Set crop region from pixel coordinates
    #[allow(dead_code)]
    pub fn set_crop_pixels(&mut self, min_x: u32, min_y: u32, max_x: u32, max_y: u32) {
        if let Some((w, h)) = self.frame_size {
            if w > 0 && h > 0 {
                self.crop_uv = Some((
                    min_x as f32 / w as f32,
                    min_y as f32 / h as f32,
                    max_x as f32 / w as f32,
                    max_y as f32 / h as f32,
                ));
                // Update aspect ratio for the crop region
                let crop_width = (max_x - min_x) as f32;
                let crop_height = (max_y - min_y) as f32;
                if crop_height > 0.0 {
                    self.source_aspect_ratio = crop_width / crop_height;
                }
            }
        }
    }

    /// Clear crop region (show full frame)
    pub fn clear_crop(&mut self) {
        self.crop_uv = None;
        // Restore aspect ratio from frame size
        if let Some((w, h)) = self.frame_size {
            if h > 0 {
                self.source_aspect_ratio = w as f32 / h as f32;
            }
        }
    }

    /// Check if there's a new frame to upload
    #[allow(dead_code)]
    pub fn has_pending_frame(&self) -> bool {
        self.frame_buffer.read().is_some()
    }

    /// Get or create texture from frame buffer
    pub fn get_texture(&mut self, ctx: &egui::Context) -> Option<&TextureHandle> {
        // Check if we have a new frame to upload
        let frame_data = {
            let mut buffer = self.frame_buffer.write();
            buffer.take()
        };

        if let Some(frame) = frame_data {
            // Create or update texture
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &rgba_from_bgra(&frame.data),
            );

            let texture = ctx.load_texture(
                format!("preview_{}", self.id.0),
                image,
                egui::TextureOptions::LINEAR,
            );

            self.texture = Some(texture);
        }

        self.texture.as_ref()
    }

    /// Check if this preview contains the given canvas point
    pub fn contains(&self, point: Pos2) -> bool {
        self.rect().contains(point)
    }
}

/// Convert BGRA to RGBA - optimized version
/// Processes 4 pixels at a time and uses extend_from_slice for efficiency
fn rgba_from_bgra(bgra: &[u8]) -> Vec<u8> {
    let mut rgba = vec![0u8; bgra.len()];
    // Process in chunks of 4 bytes (one pixel)
    for (src, dst) in bgra.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
        dst[0] = src[2]; // R <- B
        dst[1] = src[1]; // G <- G
        dst[2] = src[0]; // B <- R
        dst[3] = src[3]; // A <- A
    }
    rgba
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
}

impl From<&Preview> for PreviewLayout {
    fn from(preview: &Preview) -> Self {
        Self {
            position: (preview.position.x, preview.position.y),
            size: (preview.size.x, preview.size.y),
            window_title: preview.title.clone(),
            window_exe: None, // TODO: Get exe name from window handle
            lock_aspect_ratio: preview.lock_aspect_ratio,
            z_order: preview.z_order,
            fps_preset: preview.fps_preset,
            crop_uv: preview.crop_uv,
        }
    }
}
