use eframe::egui::{Pos2, Vec2};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use super::{
    BrowserTileStatus, Preview, PreviewId, FpsPreset, VideoSource, VideoTileStatus, WindowHandle,
};
use crate::media::MediaFrame;

/// Snapshot of a preview captured right before it's actually dropped from
/// the manager, so the canvas can offer an "Undo" toast that restores it.
#[derive(Clone)]
pub struct RemovedPreviewInfo {
    pub title: String,
    pub window_handle: Option<WindowHandle>,
    pub position: Pos2,
    pub size: Vec2,
    pub fps_preset: FpsPreset,
    pub crop_uv: Option<(f32, f32, f32, f32)>,
    /// Set for browser tiles; undo recreates the WebView from this URL
    /// because the original host window is destroyed on removal.
    pub browser_url: Option<String>,
    /// Reapplied when undo recreates the browser tile.
    pub browser_muted: bool,
    /// Set for managed image and GIF tiles.
    pub media_path: Option<String>,
    /// Set for mpv-backed local video and Streamlink tiles.
    pub video_source: Option<VideoSource>,
}

/// Manages all preview windows
pub struct PreviewManager {
    /// All previews by ID
    previews: HashMap<PreviewId, Preview>,

    /// Next available ID
    next_id: u64,

    /// Highest z-order
    max_z_order: u32,
}

impl PreviewManager {
    pub fn new() -> Self {
        Self {
            previews: HashMap::new(),
            next_id: 1,
            max_z_order: 0,
        }
    }

    /// Generate a new unique ID
    fn generate_id(&mut self) -> PreviewId {
        let id = PreviewId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Add a new preview
    #[cfg(test)]
    pub fn add(&mut self, title: String, position: Pos2, size: Vec2) -> PreviewId {
        let id = self.generate_id();
        self.max_z_order += 1;

        let mut preview = Preview::new(id, title, position, size);
        preview.z_order = self.max_z_order;

        self.previews.insert(id, preview);
        id
    }

    /// Add a preview for a specific window
    pub fn add_for_window(
        &mut self,
        hwnd: isize,
        process_id: u32,
        title: String,
        position: Pos2,
        size: Vec2,
    ) -> PreviewId {
        let id = self.generate_id();
        self.max_z_order += 1;

        let mut preview = Preview::for_window(id, hwnd, process_id, title, position, size);
        preview.z_order = self.max_z_order;

        self.previews.insert(id, preview);
        id
    }

    /// Reserve a browser tile before its WebView/capture host is ready.
    pub fn add_browser_placeholder(
        &mut self,
        url: String,
        position: Pos2,
        size: Vec2,
        fps: FpsPreset,
    ) -> PreviewId {
        let id = self.generate_id();
        self.max_z_order += 1;

        let mut preview = Preview::new(id, url.clone(), position, size);
        preview.z_order = self.max_z_order;
        preview.browser_url = Some(url);
        preview.browser_status = BrowserTileStatus::PreparingAdblock { progress: 0.0 };
        preview.set_fps_preset(fps);
        self.previews.insert(id, preview);
        id
    }

    /// Add a decoded image or GIF tile.
    pub fn add_media(
        &mut self,
        managed_path: String,
        title: String,
        frames: Vec<MediaFrame>,
        position: Pos2,
        size: Vec2,
    ) -> PreviewId {
        let id = self.generate_id();
        self.max_z_order += 1;

        let mut preview = Preview::new(id, title, position, size);
        preview.z_order = self.max_z_order;
        preview.set_media(managed_path, frames);
        self.previews.insert(id, preview);
        id
    }

    /// Reserve a video tile before its mpv process and capture host are ready.
    pub fn add_video_placeholder(
        &mut self,
        source: VideoSource,
        title: String,
        position: Pos2,
        size: Vec2,
        fps: FpsPreset,
        paused_on_restore: bool,
    ) -> PreviewId {
        let id = self.generate_id();
        self.max_z_order += 1;

        let mut preview = Preview::new(id, title, position, size);
        preview.z_order = self.max_z_order;
        preview.video_source = Some(source);
        preview.video_status = if paused_on_restore {
            VideoTileStatus::PausedOnRestore
        } else {
            VideoTileStatus::Starting
        };
        preview.set_fps_preset(fps);
        self.previews.insert(id, preview);
        id
    }

    /// Begin the fade/shrink-out animation for a preview. The preview stays
    /// in the manager (still rendered, but non-interactive) until its
    /// removal animation finishes and `finalize_removals` reaps it.
    pub fn start_removal(&mut self, id: PreviewId) {
        if let Some(preview) = self.previews.get_mut(&id) {
            preview.start_removal();
        }
    }

    /// Drop any previews whose removal animation has finished, returning a
    /// snapshot of each one so the caller can offer an "Undo".
    pub fn finalize_removals(&mut self) -> Vec<RemovedPreviewInfo> {
        let done: Vec<PreviewId> = self.previews.values()
            .filter(|p| p.is_removal_complete())
            .map(|p| p.id)
            .collect();

        let mut removed = Vec::with_capacity(done.len());
        for id in done {
            if let Some(preview) = self.previews.remove(&id) {
                removed.push(RemovedPreviewInfo {
                    title: preview.title,
                    window_handle: preview.window_handle,
                    position: preview.position,
                    size: preview.size,
                    fps_preset: preview.fps_preset,
                    crop_uv: preview.crop_uv,
                    browser_url: preview.browser_url,
                    browser_muted: preview.browser_muted,
                    media_path: preview.media_path,
                    video_source: preview.video_source,
                });
            }
        }
        removed
    }

    /// Clear all previews
    pub fn clear(&mut self) {
        self.previews.clear();
        self.next_id = 1;
        self.max_z_order = 0;
    }

    /// Add a preview with window handle and specific settings (for restoring from layout)
    pub fn add_with_window(
        &mut self,
        title: String,
        position: Pos2,
        size: Vec2,
        hwnd: isize,
        fps_preset: FpsPreset,
        z_order: u32,
    ) -> PreviewId {
        let id = self.generate_id();
        if z_order > self.max_z_order {
            self.max_z_order = z_order;
        }

        let mut preview = Preview::for_window(id, hwnd, 0, title, position, size);
        preview.z_order = z_order;
        preview.set_fps_preset(fps_preset);
        // Restored layouts should appear instantly, not all spawn-animate at once.
        preview.created_at = Instant::now() - Duration::from_secs(1);

        self.previews.insert(id, preview);
        id
    }

    /// Get a preview by ID
    pub fn get(&self, id: PreviewId) -> Option<&Preview> {
        self.previews.get(&id)
    }

    /// Get a mutable preview by ID
    pub fn get_mut(&mut self, id: PreviewId) -> Option<&mut Preview> {
        self.previews.get_mut(&id)
    }

    /// Get all preview IDs
    pub fn all_ids(&self) -> Vec<PreviewId> {
        self.previews.keys().copied().collect()
    }

    /// Get the number of previews
    pub fn count(&self) -> usize {
        self.previews.len()
    }

    /// Get preview at a canvas position (topmost first)
    pub fn get_preview_at(&self, pos: Pos2) -> Option<PreviewId> {
        self.previews
            .values()
            .filter(|p| p.contains(pos))
            .max_by_key(|preview| preview.z_order)
            .map(|preview| preview.id)
    }

    /// Get all previews (immutable)
    pub fn all(&self) -> impl Iterator<Item = &Preview> {
        self.previews.values()
    }

    /// Get all previews mutably without allocating an intermediate ID list.
    pub fn all_mut(&mut self) -> impl Iterator<Item = &mut Preview> {
        self.previews.values_mut()
    }

    /// Translate a preview
    pub fn translate(&mut self, id: PreviewId, delta: Vec2) {
        if let Some(preview) = self.previews.get_mut(&id) {
            preview.translate(delta);
        }
    }

    /// Set a preview's z-order directly (used by layout restore), keeping
    /// the max-z counter in sync so bring-to-front keeps working.
    pub fn set_z_order(&mut self, id: PreviewId, z_order: u32) {
        if z_order > self.max_z_order {
            self.max_z_order = z_order;
        }
        if let Some(preview) = self.previews.get_mut(&id) {
            preview.z_order = z_order;
        }
    }

    /// Bring a preview to front
    pub fn bring_to_front(&mut self, id: PreviewId) {
        if self.previews.contains_key(&id) {
            self.max_z_order += 1;
            if let Some(preview) = self.previews.get_mut(&id) {
                preview.z_order = self.max_z_order;
            }
        }
    }

    /// Send a preview to back
    pub fn send_to_back(&mut self, id: PreviewId) {
        if let Some(preview) = self.previews.get_mut(&id) {
            preview.z_order = 0;
        }

        // Renumber all z-orders
        let mut sorted: Vec<_> = self.previews.values().map(|p| p.id).collect();
        sorted.sort_by(|a, b| {
            let za = self.previews.get(a).map(|p| p.z_order).unwrap_or(0);
            let zb = self.previews.get(b).map(|p| p.z_order).unwrap_or(0);
            za.cmp(&zb)
        });

        for (i, preview_id) in sorted.iter().enumerate() {
            if let Some(p) = self.previews.get_mut(preview_id) {
                p.z_order = i as u32;
            }
        }

        self.max_z_order = self.previews.len() as u32;
    }

}

impl Default for PreviewManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{FpsPreset, PreviewManager, VideoSource, VideoTileStatus};
    use eframe::egui::{Pos2, Vec2};
    use std::path::PathBuf;

    #[test]
    fn hit_testing_returns_the_topmost_preview_without_sorting() {
        let mut previews = PreviewManager::new();
        let lower = previews.add("lower".to_owned(), Pos2::ZERO, Vec2::splat(100.0));
        let upper = previews.add("upper".to_owned(), Pos2::ZERO, Vec2::splat(100.0));

        assert_eq!(
            previews.get_preview_at(Pos2::new(50.0, 50.0)),
            Some(upper)
        );

        previews.bring_to_front(lower);
        assert_eq!(
            previews.get_preview_at(Pos2::new(50.0, 50.0)),
            Some(lower)
        );
        assert_eq!(previews.get_preview_at(Pos2::new(150.0, 150.0)), None);
    }

    #[test]
    fn restored_video_placeholder_keeps_source_and_paused_state() {
        let mut previews = PreviewManager::new();
        let source = VideoSource::LocalFile {
            path: PathBuf::from(r"C:\media\saved.mp4"),
        };
        let id = previews.add_video_placeholder(
            source.clone(),
            "saved.mp4".to_owned(),
            Pos2::new(10.0, 20.0),
            Vec2::new(640.0, 360.0),
            FpsPreset::High,
            true,
        );
        let preview = previews.get(id).unwrap();

        assert_eq!(preview.video_source.as_ref(), Some(&source));
        assert_eq!(preview.video_status, VideoTileStatus::PausedOnRestore);
        assert_eq!(preview.fps_preset, FpsPreset::High);
    }
}
