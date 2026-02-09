use eframe::egui::{Pos2, Vec2, Rect};
use std::collections::HashMap;
use super::{Preview, PreviewId, FpsPreset};

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
    #[allow(dead_code)]
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

    /// Remove a preview
    pub fn remove(&mut self, id: PreviewId) {
        self.previews.remove(&id);
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

    /// Check if there are any active captures
    #[allow(dead_code)]
    pub fn has_active_captures(&self) -> bool {
        self.previews.values().any(|p| p.capture_active)
    }

    /// Get preview at a canvas position (topmost first)
    pub fn get_preview_at(&self, pos: Pos2) -> Option<PreviewId> {
        let mut candidates: Vec<_> = self.previews
            .values()
            .filter(|p| p.contains(pos))
            .collect();

        // Sort by z-order descending (topmost first)
        candidates.sort_by(|a, b| b.z_order.cmp(&a.z_order));

        candidates.first().map(|p| p.id)
    }

    /// Get all visible previews within the viewport, sorted by z-order
    pub fn get_visible_previews(&self, viewport: &Rect) -> Vec<&Preview> {
        let mut visible: Vec<_> = self.previews
            .values()
            .filter(|p| p.rect().intersects(*viewport))
            .collect();

        // Sort by z-order ascending (draw bottom to top)
        visible.sort_by(|a, b| a.z_order.cmp(&b.z_order));

        visible
    }

    /// Get all previews as mutable (for updating textures)
    #[allow(dead_code)]
    pub fn all_mut(&mut self) -> impl Iterator<Item = &mut Preview> {
        self.previews.values_mut()
    }

    /// Get all previews (immutable)
    pub fn all(&self) -> impl Iterator<Item = &Preview> {
        self.previews.values()
    }

    /// Translate a preview
    pub fn translate(&mut self, id: PreviewId, delta: Vec2) {
        if let Some(preview) = self.previews.get_mut(&id) {
            preview.translate(delta);
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

    /// Set FPS preset for a preview
    #[allow(dead_code)]
    pub fn set_fps_preset(&mut self, id: PreviewId, preset: FpsPreset) {
        if let Some(preview) = self.previews.get_mut(&id) {
            preview.set_fps_preset(preset);
        }
    }

    /// Get all previews with their window handles (for capture coordination)
    #[allow(dead_code)]
    pub fn get_capture_targets(&self) -> Vec<(PreviewId, isize)> {
        self.previews
            .values()
            .filter_map(|p| {
                p.window_handle.as_ref().map(|h| (p.id, h.hwnd))
            })
            .collect()
    }
}

impl Default for PreviewManager {
    fn default() -> Self {
        Self::new()
    }
}
