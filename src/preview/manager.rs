use super::{
    BrowserTileStatus, FpsPreset, Preview, PreviewId, VideoPlaybackState, VideoSource,
    VideoTileStatus, ViewportPin, WindowHandle,
};
use crate::media::MediaFrame;
use crate::playlist::FolderPlaylist;
use eframe::egui::{Pos2, Vec2};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Snapshot of a preview captured right before it's actually dropped from
/// the manager, so the canvas can offer an "Undo" toast that restores it.
#[derive(Clone)]
pub struct RemovedPreviewInfo {
    pub title: String,
    pub window_handle: Option<WindowHandle>,
    pub window_exe: Option<String>,
    pub window_waiting_for_match: bool,
    pub position: Pos2,
    pub size: Vec2,
    pub z_order: u32,
    pub lock_aspect_ratio: bool,
    pub manually_frozen: bool,
    pub video_playback: VideoPlaybackState,
    pub fps_preset: FpsPreset,
    pub crop_uv: Option<(f32, f32, f32, f32)>,
    /// Set for browser tiles; undo recreates the WebView from this URL
    /// because the original host window is destroyed on removal.
    pub browser_url: Option<String>,
    /// Reapplied when undo recreates the browser tile.
    pub browser_muted: bool,
    /// Reapplied when undo recreates a window-capture tile.
    pub stream_audio: bool,
    /// Set when this tile receives a Spout2 sender.
    pub spout_sender: Option<String>,
    /// Restored when undo recreates a pinned tile.
    pub viewport_pin: Option<ViewportPin>,
    /// Restored when undo recreates a tile that passes primary clicks through.
    pub left_click_disabled: bool,
    /// Original local path for image and GIF tiles.
    pub media_path: Option<PathBuf>,
    /// Set for mpv-backed local video and Streamlink tiles.
    pub video_source: Option<VideoSource>,
    pub folder_playlist: Option<FolderPlaylist>,
    pub playlist_group: Option<u64>,
}

/// Manages all preview windows
pub struct PreviewManager {
    /// All previews by ID
    previews: HashMap<PreviewId, Preview>,

    /// Next available ID
    next_id: u64,

    /// Highest z-order
    max_z_order: u32,
    removal_batches: Vec<Vec<PreviewId>>,
}

impl PreviewManager {
    pub fn new() -> Self {
        Self {
            previews: HashMap::new(),
            next_id: 1,
            max_z_order: 0,
            removal_batches: Vec::new(),
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

    /// Restore a normal window tile whose application is not open yet.
    pub fn add_inactive_window(
        &mut self,
        title: String,
        window_exe: Option<String>,
        position: Pos2,
        size: Vec2,
        fps_preset: FpsPreset,
        z_order: u32,
    ) -> PreviewId {
        let id = self.generate_id();
        if z_order > self.max_z_order {
            self.max_z_order = z_order;
        }

        let mut preview = Preview::new(id, title, position, size);
        preview.z_order = z_order;
        preview.window_exe = window_exe;
        preview.window_waiting_for_match = true;
        preview.set_fps_preset(fps_preset);
        preview.created_at = Instant::now() - Duration::from_secs(1);
        self.previews.insert(id, preview);
        id
    }

    /// Add a preview that receives a named Spout2 sender.
    pub fn add_for_spout(
        &mut self,
        sender_name: String,
        position: Pos2,
        size: Vec2,
        fps: FpsPreset,
    ) -> PreviewId {
        let id = self.generate_id();
        self.max_z_order += 1;

        let mut preview = Preview::for_spout(id, sender_name, position, size);
        preview.z_order = self.max_z_order;
        preview.set_fps_preset(fps);
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
        preview.browser_waiting_for_content = true;
        preview.browser_status = BrowserTileStatus::PreparingAdblock { progress: 0.0 };
        preview.set_fps_preset(fps);
        self.previews.insert(id, preview);
        id
    }

    /// Add a decoded image or GIF tile.
    pub fn add_media(
        &mut self,
        path: PathBuf,
        title: String,
        frames: Vec<MediaFrame>,
        position: Pos2,
        size: Vec2,
    ) -> PreviewId {
        let id = self.generate_id();
        self.max_z_order += 1;

        let mut preview = Preview::new(id, title, position, size);
        preview.z_order = self.max_z_order;
        preview.set_media(path, frames);
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

    /// Add a folder playlist as a native canvas tile linked to one video tile.
    pub fn add_folder_playlist(
        &mut self,
        playlist: FolderPlaylist,
        title: String,
        position: Pos2,
        size: Vec2,
        group: u64,
        linked_video: Option<PreviewId>,
    ) -> PreviewId {
        let id = self.generate_id();
        self.max_z_order += 1;

        let mut preview = Preview::new(id, title, position, size);
        preview.z_order = self.max_z_order;
        preview.lock_aspect_ratio = false;
        preview.folder_playlist = Some(playlist);
        preview.playlist_group = Some(group);
        preview.playlist_linked_video = linked_video;
        self.previews.insert(id, preview);
        id
    }

    /// Begin the fade/shrink-out animation for a preview. The preview stays
    /// in the manager (still rendered, but non-interactive) until its
    /// removal animation finishes and `finalize_removals` reaps it.
    pub fn start_removal(&mut self, id: PreviewId) {
        self.start_removal_batch(&[id]);
    }

    pub fn start_removal_batch(&mut self, ids: &[PreviewId]) {
        let mut batch = Vec::new();
        for id in ids {
            if let Some(preview) = self.previews.get_mut(id) {
                if preview.removing.is_none() {
                    preview.start_removal();
                    batch.push(*id);
                }
            }
        }
        if !batch.is_empty() {
            self.removal_batches.push(batch);
        }
    }

    /// Drop any previews whose removal animation has finished, returning a
    /// snapshot of each one so the caller can offer an "Undo".
    pub fn finalize_removals(&mut self) -> Vec<Vec<RemovedPreviewInfo>> {
        let mut completed = Vec::new();
        let mut index = 0;
        while index < self.removal_batches.len() {
            if !self.removal_batches[index].iter().all(|id| {
                self.previews
                    .get(id)
                    .is_none_or(Preview::is_removal_complete)
            }) {
                index += 1;
                continue;
            }
            let done = self.removal_batches.remove(index);
            let mut removed = Vec::with_capacity(done.len());
            for id in done {
                if let Some(preview) = self.previews.remove(&id) {
                    removed.push(RemovedPreviewInfo {
                        title: preview.title,
                        window_handle: preview.window_handle,
                        window_exe: preview.window_exe,
                        window_waiting_for_match: preview.window_waiting_for_match,
                        position: preview.position,
                        size: preview.size,
                        z_order: preview.z_order,
                        lock_aspect_ratio: preview.lock_aspect_ratio,
                        manually_frozen: preview.manually_frozen,
                        video_playback: preview.video_playback,
                        fps_preset: preview.fps_preset,
                        crop_uv: preview.crop_uv,
                        browser_url: preview.browser_url,
                        browser_muted: preview.browser_muted,
                        stream_audio: preview.stream_audio,
                        spout_sender: preview.spout_sender,
                        viewport_pin: preview.viewport_pin,
                        left_click_disabled: preview.left_click_disabled,
                        media_path: preview.media_path,
                        video_source: preview.video_source,
                        folder_playlist: preview.folder_playlist,
                        playlist_group: preview.playlist_group,
                    });
                }
            }
            if !removed.is_empty() {
                completed.push(removed);
            }
        }
        completed
    }

    /// Clear all previews
    pub fn clear(&mut self) {
        self.previews.clear();
        self.removal_batches.clear();
        self.next_id = 1;
        self.max_z_order = 0;
    }

    /// Add a preview with window handle and specific settings (for restoring from layout)
    pub fn add_with_window(
        &mut self,
        title: String,
        position: Pos2,
        size: Vec2,
        window_handle: WindowHandle,
        fps_preset: FpsPreset,
        z_order: u32,
    ) -> PreviewId {
        let id = self.generate_id();
        if z_order > self.max_z_order {
            self.max_z_order = z_order;
        }

        let mut preview = Preview::for_window(
            id,
            window_handle.hwnd,
            window_handle.process_id,
            title,
            position,
            size,
        );
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
        if !self.previews.contains_key(&id) {
            return;
        }

        // Renumber all z-orders
        let mut sorted: Vec<_> = self.previews.values().map(|p| p.id).collect();
        sorted.sort_by_key(|preview_id| {
            (
                *preview_id != id,
                self.previews[preview_id].z_order,
                preview_id.0,
            )
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

        assert_eq!(previews.get_preview_at(Pos2::new(50.0, 50.0)), Some(upper));

        previews.bring_to_front(lower);
        assert_eq!(previews.get_preview_at(Pos2::new(50.0, 50.0)), Some(lower));
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

    #[test]
    fn deletion_batches_wait_for_all_tiles_and_preserve_settings() {
        let mut previews = PreviewManager::new();
        let first = previews.add("first".to_owned(), Pos2::ZERO, Vec2::splat(100.0));
        let second = previews.add("second".to_owned(), Pos2::ZERO, Vec2::splat(100.0));
        let original = previews.get_mut(first).unwrap();
        original.manually_frozen = true;
        original.lock_aspect_ratio = false;
        original.playlist_group = Some(7);
        original.video_playback.volume = 37.0;
        previews.start_removal_batch(&[first, second]);
        let finished = std::time::Instant::now() - std::time::Duration::from_secs(1);
        previews.get_mut(first).unwrap().removing = Some(finished);
        assert!(previews.finalize_removals().is_empty());
        assert!(previews.get(first).is_some());
        previews.get_mut(second).unwrap().removing = Some(finished);
        let batches = previews.finalize_removals();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 2);
        let first = &batches[0][0];
        assert!(first.manually_frozen);
        assert!(!first.lock_aspect_ratio);
        assert_eq!(first.z_order, 1);
        assert_eq!(first.playlist_group, Some(7));
        assert_eq!(first.video_playback.volume, 37.0);
        assert_eq!(previews.all().count(), 0);
    }

    #[test]
    fn repeated_send_to_back_preserves_a_strict_stack_order() {
        let mut previews = PreviewManager::new();
        let ids = (0..3)
            .map(|n| previews.add(n.to_string(), Pos2::ZERO, Vec2::splat(100.0)))
            .collect::<Vec<_>>();
        let mut expected = ids.clone();
        for id in [ids[2], ids[1], ids[0], ids[2], ids[2], ids[1]] {
            previews.send_to_back(id);
            expected.retain(|candidate| *candidate != id);
            expected.insert(0, id);
            let mut actual = previews.all().collect::<Vec<_>>();
            actual.sort_by_key(|preview| preview.z_order);
            assert_eq!(
                actual.iter().map(|preview| preview.id).collect::<Vec<_>>(),
                expected
            );
            assert_eq!(
                actual
                    .iter()
                    .map(|preview| preview.z_order)
                    .collect::<Vec<_>>(),
                vec![0, 1, 2]
            );
        }
    }
}
