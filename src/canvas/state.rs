use eframe::egui::{self, Pos2, Vec2, Rect, Color32, Stroke, Sense, CursorIcon};
use std::time::Instant;
#[cfg(debug_assertions)]
use crate::privacy;
use crate::preview::{PreviewManager, PreviewId, FpsPreset, RemovedPreviewInfo};
use crate::capture::CaptureCoordinator;
use super::animation::{AnimationState, DragTracker};

/// How long the "Removed '...' · Undo" toast stays on screen.
const UNDO_TOAST_SECS: f32 = 4.0;

#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SetForegroundWindow, SW_RESTORE};
#[cfg(windows)]
use windows::Win32::Foundation::HWND;

/// Represents the current drag operation
#[derive(Clone, Debug)]
pub enum DragState {
    /// Resizing a preview (with aspect ratio lock)
    Resizing {
        id: PreviewId,
        handle: ResizeHandle,
        start_rect: Rect,
        start_mouse: Pos2,
        /// Aspect ratio to maintain during resize (width/height)
        aspect_ratio: f32,
    },
    /// Cropping a preview (Alt+drag to adjust UV coordinates)
    Cropping {
        id: PreviewId,
        handle: ResizeHandle,
        start_mouse: Pos2,
        /// Starting crop UV coordinates (min_u, min_v, max_u, max_v)
        start_crop_uv: (f32, f32, f32, f32),
    },
}

#[cfg(test)]
mod tests {
    use super::CanvasState;

    #[test]
    fn canvas_screen_rect_starts_empty() {
        assert!(CanvasState::default().last_screen_rect.is_none());
    }

    #[test]
    fn double_click_target_starts_empty() {
        assert!(CanvasState::default().last_double_clicked.is_none());
    }

    #[test]
    fn browser_add_request_starts_empty() {
        assert!(CanvasState::default().pending_browser_add.is_none());
    }
}

/// Resize handle positions
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeHandle {
    TopLeft, Top, TopRight,
    Left, Right,
    BottomLeft, Bottom, BottomRight,
}

impl ResizeHandle {
    /// Get cursor icon for this handle
    fn cursor(&self) -> CursorIcon {
        match self {
            ResizeHandle::TopLeft | ResizeHandle::BottomRight => CursorIcon::ResizeNwSe,
            ResizeHandle::TopRight | ResizeHandle::BottomLeft => CursorIcon::ResizeNeSw,
            ResizeHandle::Top | ResizeHandle::Bottom => CursorIcon::ResizeVertical,
            ResizeHandle::Left | ResizeHandle::Right => CursorIcon::ResizeHorizontal,
        }
    }
}

/// Pending FPS change (to be applied after UI pass)
#[derive(Clone)]
pub struct PendingFpsChange {
    pub preview_id: PreviewId,
    pub new_fps: FpsPreset,
}

/// Actions requested from a browser tile's hover controls or context menu.
/// The canvas only queues these; the app (which owns the browser hosts)
/// consumes them after the UI pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserAction {
    Back,
    Forward,
    Reload,
    ToggleMute,
    OpenExternal,
    CopyUrl,
    EditUrl,
}

/// Snapshot of the input state the canvas actually needs, gathered once per
/// frame instead of cloning the entire egui `InputState` several times.
struct FrameInput {
    hover_pos: Option<Pos2>,
    interact_pos: Option<Pos2>,
    pointer_delta: Vec2,
    scroll_y: f32,
    alt: bool,
    ctrl: bool,
    middle_down: bool,
    primary_down: bool,
    time: f64,
    delete_pressed: bool,
    select_all: bool,
}

/// Per-tile data collected up front so the manager isn't borrowed during
/// the interaction pass.
struct TileInfo {
    id: PreviewId,
    rect: Rect,
    title: String,
    target_fps: u32,
    fps_preset: FpsPreset,
    has_crop: bool,
    is_removing: bool,
    spawn_t: f32,
    remove_t: f32,
    is_browser: bool,
    muted: bool,
}

/// Canvas state managing pan, zoom, and interactions
#[derive(Clone)]
pub struct CanvasState {
    /// Current pan offset (in canvas coordinates)
    pub pan: Vec2,

    /// Current zoom level (1.0 = 100%)
    pub zoom: f32,

    /// Zoom constraints
    pub zoom_min: f32,
    pub zoom_max: f32,

    /// Currently selected preview IDs
    pub selection: Vec<PreviewId>,

    /// Current drag operation
    pub drag_state: Option<DragState>,

    /// Grid visibility
    pub show_grid: bool,

    /// Grid size in canvas units
    pub grid_size: f32,

    /// Pending FPS changes to apply
    pending_fps_changes: Vec<PendingFpsChange>,

    /// Animation state for smooth movements
    pub animation: AnimationState,

    /// Is a preview currently being dragged?
    preview_dragging: bool,

    /// Is the canvas currently being panned?
    canvas_panning: bool,

    /// Drag tracker for canvas pan momentum
    pan_drag_tracker: DragTracker,

    /// Preview ID pending region selection (set from context menu, consumed by app)
    pub pending_region_select: Option<PreviewId>,

    /// Most recently removed preview, kept briefly to power the "Undo" toast.
    last_removed: Option<(Instant, RemovedPreviewInfo)>,

    /// Screen position of the last right-click on the canvas background,
    /// used to anchor the "Add Window..." quick-add popup.
    last_secondary_click: Option<Pos2>,

    /// Set by the "Add Window..." context menu item: (canvas position to
    /// place the new preview, screen position to anchor the popup). The app
    /// consumes this to open the quick-add popup.
    pub pending_quick_add: Option<(Pos2, Pos2)>,

    /// Canvas position requested by the "Add Browser..." context action.
    pub pending_browser_add: Option<Pos2>,

    /// Browser tile actions queued by hover controls / context menus,
    /// consumed by the app.
    pub pending_browser_actions: Vec<(PreviewId, BrowserAction)>,

    /// A removed browser tile whose "Undo" was clicked; the app recreates
    /// the WebView from its saved URL (the original host is already gone).
    pub pending_browser_restore: Option<RemovedPreviewInfo>,

    /// The browser tile currently in interaction mode, set by the app each
    /// frame so the canvas can outline it in the accent color.
    pub interactive_browser: Option<PreviewId>,

    /// Last canvas rectangle in egui screen coordinates.
    pub last_screen_rect: Option<Rect>,

    /// Preview most recently double-clicked, consumed by the app.
    pub last_double_clicked: Option<PreviewId>,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
            zoom_min: 0.1,
            zoom_max: 5.0,
            selection: Vec::new(),
            drag_state: None,
            show_grid: true,
            grid_size: 50.0,
            pending_fps_changes: Vec::new(),
            animation: AnimationState::new(),
            preview_dragging: false,
            canvas_panning: false,
            pan_drag_tracker: DragTracker::new(),
            pending_region_select: None,
            last_removed: None,
            last_secondary_click: None,
            pending_quick_add: None,
            pending_browser_add: None,
            pending_browser_actions: Vec::new(),
            pending_browser_restore: None,
            interactive_browser: None,
            last_screen_rect: None,
            last_double_clicked: None,
        }
    }
}

impl CanvasState {
    /// Reset canvas to default view
    pub fn reset(&mut self) {
        self.pan = Vec2::ZERO;
        self.zoom = 1.0;
        self.selection.clear();
        self.drag_state = None;
    }

    /// Convert screen position to canvas position
    pub fn screen_to_canvas(&self, screen_pos: Pos2, canvas_rect: Rect) -> Pos2 {
        let relative = screen_pos - canvas_rect.min;
        Pos2::new(
            (relative.x / self.zoom) - self.pan.x,
            (relative.y / self.zoom) - self.pan.y,
        )
    }

    /// Convert canvas position to screen position
    pub fn canvas_to_screen(&self, canvas_pos: Pos2, canvas_rect: Rect) -> Pos2 {
        Pos2::new(
            (canvas_pos.x + self.pan.x) * self.zoom + canvas_rect.min.x,
            (canvas_pos.y + self.pan.y) * self.zoom + canvas_rect.min.y,
        )
    }

    /// Convert canvas rect to screen rect
    pub fn canvas_rect_to_screen(&self, canvas_rect: Rect, screen_canvas_rect: Rect) -> Rect {
        let min = self.canvas_to_screen(canvas_rect.min, screen_canvas_rect);
        let max = self.canvas_to_screen(canvas_rect.max, screen_canvas_rect);
        Rect::from_min_max(min, max)
    }

    /// Get the visible canvas area
    pub fn get_viewport(&self, screen_rect: Rect) -> Rect {
        let min = self.screen_to_canvas(screen_rect.min, screen_rect);
        let max = self.screen_to_canvas(screen_rect.max, screen_rect);
        Rect::from_min_max(min, max)
    }

    /// Check if mouse is over a resize handle, returns (preview_id, handle)
    fn get_handle_at(&self, screen_pos: Pos2, canvas_rect: Rect, preview_manager: &PreviewManager) -> Option<(PreviewId, ResizeHandle)> {
        let handle_size = 12.0; // Slightly larger hit area

        for id in &self.selection {
            if let Some(preview) = preview_manager.get(*id) {
                let screen_rect = self.canvas_rect_to_screen(preview.rect(), canvas_rect);

                let handles = [
                    (screen_rect.left_top(), ResizeHandle::TopLeft),
                    (screen_rect.center_top(), ResizeHandle::Top),
                    (screen_rect.right_top(), ResizeHandle::TopRight),
                    (screen_rect.left_center(), ResizeHandle::Left),
                    (screen_rect.right_center(), ResizeHandle::Right),
                    (screen_rect.left_bottom(), ResizeHandle::BottomLeft),
                    (screen_rect.center_bottom(), ResizeHandle::Bottom),
                    (screen_rect.right_bottom(), ResizeHandle::BottomRight),
                ];

                for (handle_pos, handle_type) in handles {
                    let handle_rect = Rect::from_center_size(handle_pos, Vec2::splat(handle_size));
                    if handle_rect.contains(screen_pos) {
                        return Some((*id, handle_type));
                    }
                }
            }
        }
        None
    }

    /// Main UI rendering for the canvas
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        preview_manager: &mut PreviewManager,
        capture_coordinator: &mut CaptureCoordinator,
        ctx: &egui::Context,
    ) {
        let canvas_rect = ui.available_rect_before_wrap();
        self.last_screen_rect = Some(canvas_rect);

        // Snapshot the input fields we need once, instead of cloning the
        // whole InputState in every interaction pass.
        let input = ui.input(|i| FrameInput {
            hover_pos: i.pointer.hover_pos(),
            interact_pos: i.pointer.interact_pos(),
            pointer_delta: i.pointer.delta(),
            scroll_y: i.raw_scroll_delta.y,
            alt: i.modifiers.alt,
            ctrl: i.modifiers.ctrl,
            middle_down: i.pointer.middle_down(),
            primary_down: i.pointer.primary_down(),
            time: i.time,
            delete_pressed: i.key_pressed(egui::Key::Delete),
            select_all: i.modifiers.ctrl && i.key_pressed(egui::Key::A),
        });

        // Calculate delta time for animations
        let current_time = input.time;
        let dt = (current_time - self.animation.last_frame_time) as f32;
        self.animation.last_frame_time = current_time;

        // Update all animations
        self.animation.update(dt);

        // Apply momentum to pan (smooth inertia scrolling)
        if self.animation.momentum_active {
            let momentum_delta = self.animation.get_momentum_delta();
            self.pan += momentum_delta / self.zoom;
        }

        // Update preview positions from their spring animations
        self.update_preview_animations(preview_manager);

        // Reap any previews whose fade/shrink-out animation has finished,
        // keeping the most recent one around briefly for the undo toast.
        let finished_removals = preview_manager.finalize_removals();
        if let Some(info) = finished_removals.into_iter().last() {
            self.last_removed = Some((Instant::now(), info));
        }

        // CRITICAL: Allocate background interaction FIRST
        // In egui, later interactions take priority over earlier ones.
        // By allocating the canvas background first, preview interactions
        // (allocated later) will take priority and receive clicks/drags.
        let bg_response = ui.interact(
            canvas_rect,
            ui.id().with("canvas_bg"),
            Sense::click_and_drag(),
        );

        // Get the painter for drawing
        let painter = ui.painter_at(canvas_rect);

        // Draw background - Minimal Void theme (#0d0d0d)
        painter.rect_filled(canvas_rect, 0.0, Color32::from_rgb(13, 13, 13));

        // Draw grid
        if self.show_grid {
            self.draw_grid(&painter, canvas_rect);
        }

        // Empty-canvas hint (only relevant before anything has been added)
        if preview_manager.count() == 0 {
            self.draw_empty_state(&painter, canvas_rect);
        }

        // Draw previews and handle their interactions (AFTER bg allocation)
        self.draw_and_interact_previews(ui, canvas_rect, preview_manager, ctx, capture_coordinator, &input);

        // Draw selection rectangles and interactive resize handles
        // Handles are allocated AFTER previews so they have higher interaction priority
        self.draw_and_interact_selection(ui, canvas_rect, preview_manager, &input);

        // Minimal Void: Floating status indicator (bottom-right corner)
        self.draw_floating_status(&painter, canvas_rect, preview_manager.count());

        // Undo toast for the most recently removed preview
        self.draw_and_interact_undo_toast(ui, canvas_rect, preview_manager, capture_coordinator);

        // Handle canvas-level input using the pre-allocated bg_response
        self.handle_canvas_input_with_response(ui, canvas_rect, preview_manager, capture_coordinator, bg_response, &input);

        // Apply pending FPS changes
        self.apply_pending_fps_changes(preview_manager, capture_coordinator);

        // Viewport culling: pause/resume captures based on visibility
        self.update_viewport_culling(canvas_rect, preview_manager, capture_coordinator);

        // Request repaint if animations are active
        if self.animation.is_animating() {
            ctx.request_repaint();
        }
    }

    /// Update preview positions from their spring animations
    fn update_preview_animations(&mut self, preview_manager: &mut PreviewManager) {
        for (id, spring) in &self.animation.preview_springs {
            if spring.is_animating() {
                if let Some(preview) = preview_manager.get_mut(*id) {
                    preview.position = spring.current_pos();
                }
            }
        }
    }

    /// Update viewport culling - pause captures for off-screen previews
    fn update_viewport_culling(
        &self,
        canvas_rect: Rect,
        preview_manager: &mut PreviewManager,
        capture_coordinator: &mut CaptureCoordinator,
    ) {
        let viewport = self.get_viewport(canvas_rect);

        // Check each preview for visibility
        for id in preview_manager.all_ids() {
            if let Some(preview) = preview_manager.get_mut(id) {
                let preview_rect = preview.rect();
                let is_visible = viewport.intersects(preview_rect);

                // Update pause state based on visibility
                if is_visible && preview.capture_paused {
                    // Resume capture - preview is now visible
                    capture_coordinator.resume_capture(id);
                    preview.capture_paused = false;
                    #[cfg(debug_assertions)]
                    println!("Viewport culling: Resumed capture for '{}'", privacy::redact_title(&preview.title));
                } else if !is_visible && !preview.capture_paused {
                    // Pause capture - preview is now off-screen
                    capture_coordinator.pause_capture(id);
                    preview.capture_paused = true;
                    #[cfg(debug_assertions)]
                    println!("Viewport culling: Paused capture for '{}'", privacy::redact_title(&preview.title));
                }
            }
        }
    }

    /// Apply any pending FPS changes
    fn apply_pending_fps_changes(
        &mut self,
        preview_manager: &mut PreviewManager,
        capture_coordinator: &mut CaptureCoordinator,
    ) {
        for change in self.pending_fps_changes.drain(..) {
            if let Some(preview) = preview_manager.get_mut(change.preview_id) {
                let old_fps = preview.target_fps;
                preview.set_fps_preset(change.new_fps);

                // The capture thread reads the target live; no restart (and
                // no black flash) needed.
                if preview.target_fps != old_fps {
                    capture_coordinator.set_target_fps(change.preview_id, preview.target_fps);
                }
            }
        }
    }

    /// Handle canvas-level input (background clicks, pan, zoom)
    fn handle_canvas_input_with_response(
        &mut self,
        ui: &mut egui::Ui,
        canvas_rect: Rect,
        preview_manager: &mut PreviewManager,
        capture_coordinator: &mut CaptureCoordinator,
        bg_response: egui::Response,
        input: &FrameInput,
    ) {
        // Use the pre-allocated background response

        // Update cursor based on drag state or handle hover
        if let Some(mouse_pos) = input.hover_pos {
            if canvas_rect.contains(mouse_pos) {
                if let Some((_, handle)) = self.get_handle_at(mouse_pos, canvas_rect, preview_manager) {
                    ui.ctx().set_cursor_icon(handle.cursor());
                }
            }
        }

        // Zoom with scroll wheel - works anywhere on canvas, even over previews
        // We check canvas_rect.contains() instead of bg_response.hovered() because
        // bg_response.hovered() returns false when the mouse is over a preview widget
        if let Some(mouse_pos) = input.hover_pos {
            if canvas_rect.contains(mouse_pos) {
                let scroll_delta = input.scroll_y;
                if scroll_delta != 0.0 {
                    let zoom_factor = if scroll_delta > 0.0 { 1.1 } else { 0.9 };
                    let new_zoom = (self.zoom * zoom_factor).clamp(self.zoom_min, self.zoom_max);

                    let canvas_pos = self.screen_to_canvas(mouse_pos, canvas_rect);
                    self.zoom = new_zoom;
                    let new_canvas_pos = self.screen_to_canvas(mouse_pos, canvas_rect);
                    self.pan += new_canvas_pos.to_vec2() - canvas_pos.to_vec2();
                }
            }
        }

        // Pan with middle mouse button or Alt+Left drag
        // Works anywhere on canvas, even over previews (similar to zoom)
        let is_panning = (input.middle_down || (input.alt && input.primary_down))
            && canvas_rect.contains(input.hover_pos.unwrap_or_default());

        if is_panning {
            // Start panning
            if !self.canvas_panning {
                self.canvas_panning = true;
                self.pan_drag_tracker.clear();
                // Stop any existing momentum
                self.animation.momentum_active = false;
                self.animation.momentum_velocity = Vec2::ZERO;
            }

            // Track velocity for momentum
            if let Some(mouse_pos) = input.hover_pos {
                self.pan_drag_tracker.record(mouse_pos, input.time);
            }

            // Use pointer delta directly instead of bg_response.drag_delta()
            // because bg_response.dragged() returns false when over a preview
            let delta = input.pointer_delta;
            if delta != Vec2::ZERO {
                self.pan += delta / self.zoom;
            }
        } else if self.canvas_panning {
            // Just released pan - apply momentum
            self.canvas_panning = false;
            let velocity = self.pan_drag_tracker.get_velocity();
            self.animation.start_momentum(velocity);
        }

        // Click on empty space to deselect
        if bg_response.clicked() && !input.ctrl {
            if let Some(mouse_pos) = input.interact_pos {
                let canvas_pos = self.screen_to_canvas(mouse_pos, canvas_rect);
                if preview_manager.get_preview_at(canvas_pos).is_none() {
                    self.selection.clear();
                }
            }
        }

        // Canvas context menu (right-click on empty space)
        if bg_response.secondary_clicked() {
            self.last_secondary_click = input.interact_pos;
        }

        bg_response.context_menu(|ui| {
            if ui.button("Add Window...").clicked() {
                if let Some(screen_pos) = self.last_secondary_click {
                    let canvas_pos = self.screen_to_canvas(screen_pos, canvas_rect);
                    self.pending_quick_add = Some((canvas_pos, screen_pos));
                }
                ui.close_menu();
            }
            if ui.button("Add Browser...").clicked() {
                if let Some(screen_pos) = self.last_secondary_click {
                    self.pending_browser_add = Some(self.screen_to_canvas(screen_pos, canvas_rect));
                }
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Reset View").clicked() {
                self.reset();
                ui.close_menu();
            }
            ui.separator();
            ui.checkbox(&mut self.show_grid, "Show Grid");
            ui.separator();
            if !self.selection.is_empty() {
                if ui.button("Remove Selected").clicked() {
                    for id in self.selection.clone() {
                        capture_coordinator.stop_capture(id);
                        preview_manager.start_removal(id);
                    }
                    self.selection.clear();
                    ui.close_menu();
                }
            }
        });

        // Keyboard shortcuts
        if bg_response.has_focus() || bg_response.hovered() {
            if input.delete_pressed {
                for id in self.selection.clone() {
                    capture_coordinator.stop_capture(id);
                    preview_manager.start_removal(id);
                }
                self.selection.clear();
            }

            if input.select_all {
                self.selection = preview_manager.all_ids();
            }
        }
    }

    /// Draw previews and handle their individual interactions
    fn draw_and_interact_previews(
        &mut self,
        ui: &mut egui::Ui,
        canvas_rect: Rect,
        preview_manager: &mut PreviewManager,
        ctx: &egui::Context,
        capture_coordinator: &mut CaptureCoordinator,
        input: &FrameInput,
    ) {
        let viewport = self.get_viewport(canvas_rect);

        // Collect preview info first
        let preview_info: Vec<TileInfo> = {
            let previews = preview_manager.get_visible_previews(&viewport);
            previews.iter().map(|p| TileInfo {
                id: p.id,
                rect: p.rect(),
                title: p.title.clone(),
                target_fps: p.target_fps,
                fps_preset: p.fps_preset,
                has_crop: p.crop_uv.is_some(),
                is_removing: p.removing.is_some(),
                spawn_t: p.spawn_progress(),
                remove_t: p.removal_progress(),
                is_browser: p.is_browser(),
                muted: p.browser_muted,
            }).collect()
        };

        let mut any_spawn_or_remove_animating = false;

        for info in preview_info {
            let TileInfo {
                id, rect, title, target_fps, fps_preset: current_preset, has_crop,
                is_removing, spawn_t, remove_t, is_browser, muted,
            } = info;
            let screen_rect = self.canvas_rect_to_screen(rect, canvas_rect);

            if !canvas_rect.intersects(screen_rect) {
                continue;
            }

            // Spawn-in / fade-out: ease alpha and a subtle scale toward center.
            let (alpha, scale) = if is_removing {
                any_spawn_or_remove_animating |= remove_t < 1.0;
                (1.0 - remove_t, 1.0 - remove_t * 0.12)
            } else if spawn_t < 1.0 {
                any_spawn_or_remove_animating = true;
                (spawn_t, 0.85 + spawn_t * 0.15)
            } else {
                (1.0, 1.0)
            };
            let anim_rect = if (scale - 1.0).abs() > f32::EPSILON {
                Rect::from_center_size(screen_rect.center(), screen_rect.size() * scale)
            } else {
                screen_rect
            };
            let alpha_u8 = (alpha.clamp(0.0, 1.0) * 255.0) as u8;

            let painter = ui.painter_at(canvas_rect);

            if is_removing {
                // Fading out: paint the last frame only, no interaction.
                if let Some(preview) = preview_manager.get_mut(id) {
                    let uv_rect = preview.get_uv_rect();
                    if let Some(texture) = preview.get_texture(ctx) {
                        painter.image(texture.id(), anim_rect, uv_rect, Color32::from_white_alpha(alpha_u8));
                    }
                }
                continue;
            }

            // Create interactive area for this preview
            let preview_response = ui.interact(
                screen_rect,
                ui.id().with(("preview", id.0)),
                Sense::click_and_drag(),
            );

            let is_active = self.selection.contains(&id) || preview_response.dragged();

            // Soft drop shadow underneath the preview, stronger when selected/dragged.
            let shadow_alpha = ((if is_active { 90.0 } else { 40.0 }) * alpha) as u8;
            let shadow_offset = if is_active { Vec2::new(0.0, 6.0) } else { Vec2::new(0.0, 3.0) };
            painter.rect_filled(
                anim_rect.translate(shadow_offset),
                8.0,
                Color32::from_rgba_unmultiplied(0, 0, 0, shadow_alpha),
            );

            // Minimal Void: No background fill - content fills entire area
            // Draw preview content (full rect, no title bar offset)
            let has_texture = if let Some(preview) = preview_manager.get_mut(id) {
                // Get UV rect first (immutable borrow ends before get_texture)
                let uv_rect = preview.get_uv_rect();
                if let Some(texture) = preview.get_texture(ctx) {
                    // Minimal Void: content fills entire rect
                    painter.image(
                        texture.id(),
                        anim_rect,
                        uv_rect,
                        Color32::from_white_alpha(alpha_u8),
                    );
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if !has_texture {
                // Shimmering placeholder while the capture connects
                let t = input.time as f32;
                let pulse = (t * 1.8).sin() * 0.5 + 0.5;
                let v = (18.0 + pulse * 14.0) as u8;
                painter.rect_filled(anim_rect, 8.0, Color32::from_rgb(v, v, v + 2));
                painter.text(
                    anim_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Connecting...",
                    egui::FontId::proportional(12.0),
                    Color32::from_rgb(95, 95, 95),
                );
                any_spawn_or_remove_animating = true;
            }

            // Minimal Void: Hover-reveal controls (no permanent title bar)
            if preview_response.hovered() {
                // Semi-transparent overlay gradient at top for controls
                let overlay_rect = Rect::from_min_size(
                    screen_rect.min,
                    Vec2::new(screen_rect.width(), 40.0),
                );
                painter.rect_filled(
                    overlay_rect,
                    egui::Rounding { nw: 8.0, ne: 8.0, sw: 0.0, se: 0.0 },
                    Color32::from_rgba_unmultiplied(0, 0, 0, 120),
                );

                // Close button (top-right)
                let close_btn_rect = Rect::from_min_size(
                    screen_rect.right_top() + Vec2::new(-32.0, 8.0),
                    Vec2::new(24.0, 24.0),
                );
                let close_response = ui.interact(
                    close_btn_rect,
                    ui.id().with(("preview_close", id.0)),
                    Sense::click(),
                );
                let close_bg = if close_response.hovered() {
                    Color32::from_rgba_unmultiplied(255, 100, 100, 230)
                } else {
                    Color32::from_rgba_unmultiplied(255, 80, 80, 200)
                };
                painter.rect_filled(close_btn_rect, 4.0, close_bg);
                painter.text(
                    close_btn_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    egui_phosphor::regular::X,
                    egui::FontId::proportional(13.0),
                    Color32::WHITE,
                );
                if close_response.clicked() {
                    capture_coordinator.stop_capture(id);
                    preview_manager.start_removal(id);
                    self.selection.retain(|&x| x != id);
                }

                // FPS badge (left of close button)
                let fps_text = format!("{}", target_fps);
                let fps_rect = Rect::from_min_size(
                    screen_rect.right_top() + Vec2::new(-72.0, 10.0),
                    Vec2::new(36.0, 20.0),
                );
                painter.rect_filled(fps_rect, 10.0, Color32::from_rgba_unmultiplied(0, 0, 0, 180));
                painter.text(
                    fps_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &fps_text,
                    egui::FontId::proportional(10.0),
                    Color32::from_rgb(150, 150, 150),
                );

                // Title (truncated, on the left) - handle UTF-8 properly
                let title_text = if title.chars().count() > 25 {
                    let truncated: String = title.chars().take(22).collect();
                    format!("{}...", truncated)
                } else {
                    title.clone()
                };
                let title_pos = if is_browser {
                    // Globe badge marks browser tiles; shift the title right.
                    painter.text(
                        screen_rect.left_top() + Vec2::new(12.0, 20.0),
                        egui::Align2::LEFT_CENTER,
                        egui_phosphor::regular::GLOBE,
                        egui::FontId::proportional(12.0),
                        Color32::from_rgb(107, 170, 75),
                    );
                    screen_rect.left_top() + Vec2::new(28.0, 20.0)
                } else {
                    screen_rect.left_top() + Vec2::new(12.0, 20.0)
                };
                painter.text(
                    title_pos,
                    egui::Align2::LEFT_CENTER,
                    &title_text,
                    egui::FontId::proportional(11.0),
                    Color32::from_rgb(200, 200, 200),
                );

                // Browser tiles: navigation + audio controls along the bottom
                if is_browser {
                    let bottom_overlay = Rect::from_min_size(
                        screen_rect.left_bottom() + Vec2::new(0.0, -42.0),
                        Vec2::new(screen_rect.width(), 42.0),
                    );
                    painter.rect_filled(
                        bottom_overlay,
                        egui::Rounding { nw: 0.0, ne: 0.0, sw: 8.0, se: 8.0 },
                        Color32::from_rgba_unmultiplied(0, 0, 0, 120),
                    );

                    let buttons: [(&str, BrowserAction, &str); 5] = [
                        (egui_phosphor::regular::CARET_LEFT, BrowserAction::Back, "Back"),
                        (egui_phosphor::regular::CARET_RIGHT, BrowserAction::Forward, "Forward"),
                        (egui_phosphor::regular::ARROW_CLOCKWISE, BrowserAction::Reload, "Reload"),
                        (
                            if muted { egui_phosphor::regular::SPEAKER_SLASH } else { egui_phosphor::regular::SPEAKER_HIGH },
                            BrowserAction::ToggleMute,
                            if muted { "Unmute" } else { "Mute" },
                        ),
                        (egui_phosphor::regular::ARROW_SQUARE_OUT, BrowserAction::OpenExternal, "Open in browser"),
                    ];
                    for (idx, (icon, action, tip)) in buttons.iter().enumerate() {
                        let btn_rect = Rect::from_min_size(
                            screen_rect.left_bottom() + Vec2::new(10.0 + idx as f32 * 30.0, -34.0),
                            Vec2::splat(26.0),
                        );
                        let resp = ui
                            .interact(
                                btn_rect,
                                ui.id().with(("browser_btn", id.0, idx)),
                                Sense::click(),
                            )
                            .on_hover_text(*tip);
                        if resp.hovered() {
                            painter.rect_filled(btn_rect, 6.0, Color32::from_rgba_unmultiplied(255, 255, 255, 35));
                        }
                        let icon_color = if *action == BrowserAction::ToggleMute && muted {
                            Color32::from_rgb(255, 150, 100)
                        } else {
                            Color32::from_rgb(215, 215, 220)
                        };
                        painter.text(
                            btn_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            *icon,
                            egui::FontId::proportional(14.0),
                            icon_color,
                        );
                        if resp.clicked() {
                            self.pending_browser_actions.push((id, *action));
                        }
                    }
                }

                // Crop indicator (if has crop)
                if has_crop {
                    let crop_rect = Rect::from_min_size(
                        screen_rect.left_bottom() + Vec2::new(8.0, -28.0),
                        Vec2::new(50.0, 20.0),
                    );
                    painter.rect_filled(crop_rect, 10.0, Color32::from_rgba_unmultiplied(255, 150, 100, 180));
                    painter.text(
                        crop_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "CROP",
                        egui::FontId::proportional(9.0),
                        Color32::WHITE,
                    );
                }
            }

            // Muted badge stays visible even without hover so silent tiles
            // are recognizable at a glance.
            if is_browser && muted && !preview_response.hovered() {
                let badge_rect = Rect::from_min_size(
                    screen_rect.right_top() + Vec2::new(-30.0, 8.0),
                    Vec2::splat(22.0),
                );
                painter.rect_filled(badge_rect, 6.0, Color32::from_rgba_unmultiplied(0, 0, 0, 160));
                painter.text(
                    badge_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    egui_phosphor::regular::SPEAKER_SLASH,
                    egui::FontId::proportional(12.0),
                    Color32::from_rgb(255, 150, 100),
                );
            }

            // Minimal Void: Only show border when selected (thin blue accent);
            // green accent marks the browser tile currently in interaction mode.
            if self.interactive_browser == Some(id) {
                painter.rect_stroke(screen_rect, 8.0, Stroke::new(2.0, Color32::from_rgb(107, 170, 75)));
            } else if self.selection.contains(&id) {
                painter.rect_stroke(screen_rect, 8.0, Stroke::new(2.0, Color32::from_rgb(74, 158, 255)));
            }

            // Handle click to select
            if preview_response.clicked() {
                if input.ctrl {
                    if self.selection.contains(&id) {
                        self.selection.retain(|&x| x != id);
                    } else {
                        self.selection.push(id);
                    }
                } else {
                    self.selection = vec![id];
                }
            }

            // Handle double-click: browsers enter interaction mode (the app
            // consumes last_double_clicked); other previews focus their
            // source window.
            if preview_response.double_clicked() {
                self.last_double_clicked = Some(id);
                if !is_browser {
                    if let Some(preview) = preview_manager.get(id) {
                        if let Some(ref handle) = preview.window_handle {
                            #[cfg(windows)]
                            unsafe {
                                let hwnd = HWND(handle.hwnd as *mut _);
                                let _ = ShowWindow(hwnd, SW_RESTORE);
                                let _ = SetForegroundWindow(hwnd);
                            }
                        }
                    }
                }
            }

            // Handle drag start - initialize spring and tracker
            if preview_response.drag_started() && !input.alt && !input.middle_down {
                self.preview_dragging = true;
                self.animation.drag_tracker.clear();

                // Initialize springs for dragged previews at their current position
                let ids_to_init: Vec<PreviewId> = if self.selection.contains(&id) {
                    self.selection.clone()
                } else {
                    vec![id]
                };

                for sel_id in ids_to_init {
                    if let Some(preview) = preview_manager.get(sel_id) {
                        let spring = self.animation.get_or_create_spring(sel_id, preview.position);
                        spring.set_immediate_pos(preview.position);
                    }
                }
            }

            // Handle drag to move (only when not panning with Alt or middle mouse)
            // Resize is handled separately in draw_and_interact_selection()
            if preview_response.dragged() && !input.alt && !input.middle_down {
                // Only move if we're not in a resize operation
                if self.drag_state.is_none() {
                    let delta = preview_response.drag_delta() / self.zoom;

                    // Track velocity for momentum
                    if let Some(mouse_pos) = input.hover_pos {
                        self.animation.drag_tracker.record(mouse_pos, input.time);
                    }

                    // Move previews directly during drag (immediate feedback)
                    if self.selection.contains(&id) {
                        for sel_id in &self.selection {
                            preview_manager.translate(*sel_id, delta);
                            // Keep spring in sync during drag
                            if let Some(preview) = preview_manager.get(*sel_id) {
                                if let Some(spring) = self.animation.preview_springs.get_mut(sel_id) {
                                    spring.set_immediate_pos(preview.position);
                                }
                            }
                        }
                    } else {
                        preview_manager.translate(id, delta);
                        if let Some(preview) = preview_manager.get(id) {
                            if let Some(spring) = self.animation.preview_springs.get_mut(&id) {
                                spring.set_immediate_pos(preview.position);
                            }
                        }
                    }
                }
            }

            // Handle drag end - apply momentum and snap-to-grid
            if preview_response.drag_stopped() && self.preview_dragging {
                self.preview_dragging = false;

                // Get velocity from tracker
                let velocity = self.animation.drag_tracker.get_velocity() / self.zoom;

                // Apply to all dragged previews
                let ids_to_animate: Vec<PreviewId> = if self.selection.contains(&id) {
                    self.selection.clone()
                } else {
                    vec![id]
                };

                for sel_id in ids_to_animate {
                    if let Some(preview) = preview_manager.get(sel_id) {
                        // Calculate target with subtle momentum
                        let momentum_offset = velocity * 0.05; // Very subtle momentum
                        let target_pos = preview.position + momentum_offset;

                        // Optionally snap to grid
                        let final_target = if self.animation.snap_config.enabled {
                            self.animation.snap_config.snap_position(target_pos)
                        } else {
                            target_pos
                        };

                        // Set spring target for smooth animation to final position
                        let spring = self.animation.get_or_create_spring(sel_id, preview.position);
                        spring.set_target_pos(final_target);

                        // Add minimal velocity for subtle ease-out
                        spring.add_velocity(velocity * 0.1);
                    }
                }
            }

            // Context menu for preview
            preview_response.context_menu(|ui| {
                ui.label(egui::RichText::new(&title).strong());
                ui.separator();

                ui.label("Frame Rate:");
                for preset in [FpsPreset::Low, FpsPreset::Medium, FpsPreset::High] {
                    let is_current = current_preset == preset;
                    let label = if is_current {
                        format!("  {} ✓", preset.label())
                    } else {
                        format!("  {}", preset.label())
                    };

                    if ui.selectable_label(is_current, label).clicked() {
                        self.pending_fps_changes.push(PendingFpsChange {
                            preview_id: id,
                            new_fps: preset,
                        });
                        ui.close_menu();
                    }
                }

                ui.separator();

                if is_browser {
                    // Browser tiles: navigation and audio instead of crop
                    // (a cropped page has ambiguous interactive coordinates).
                    if ui.button("Interact").clicked() {
                        self.last_double_clicked = Some(id);
                        ui.close_menu();
                    }
                    if ui.button(if muted { "Unmute" } else { "Mute" }).clicked() {
                        self.pending_browser_actions.push((id, BrowserAction::ToggleMute));
                        ui.close_menu();
                    }
                    if ui.button("Reload").clicked() {
                        self.pending_browser_actions.push((id, BrowserAction::Reload));
                        ui.close_menu();
                    }
                    if ui.button("Change URL...").clicked() {
                        self.pending_browser_actions.push((id, BrowserAction::EditUrl));
                        ui.close_menu();
                    }
                    if ui.button("Copy URL").clicked() {
                        self.pending_browser_actions.push((id, BrowserAction::CopyUrl));
                        ui.close_menu();
                    }
                    if ui.button("Open in Default Browser").clicked() {
                        self.pending_browser_actions.push((id, BrowserAction::OpenExternal));
                        ui.close_menu();
                    }
                } else {
                    // Crop section
                    ui.menu_button("Crop", |ui| {
                        // Select Region button (ShareX-style)
                        if ui.button("Select Region...").clicked() {
                            self.pending_region_select = Some(id);
                            ui.close_menu();
                        }

                        if has_crop {
                            if ui.button("Clear Crop").clicked() {
                                if let Some(preview) = preview_manager.get_mut(id) {
                                    preview.clear_crop();
                                }
                                ui.close_menu();
                            }
                        }

                        ui.separator();
                        ui.label(egui::RichText::new("Tip: Alt+drag corners to fine-tune").weak().small());
                    });
                }

                ui.separator();

                if ui.button("Bring to Front").clicked() {
                    preview_manager.bring_to_front(id);
                    ui.close_menu();
                }

                if ui.button("Send to Back").clicked() {
                    preview_manager.send_to_back(id);
                    ui.close_menu();
                }

                ui.separator();

                if ui.button("Remove").clicked() {
                    capture_coordinator.stop_capture(id);
                    preview_manager.start_removal(id);
                    self.selection.retain(|&x| x != id);
                    ui.close_menu();
                }
            });
        }

        // Keep repainting while any preview is spawning in, fading out, or
        // still waiting on its first frame so the animations stay smooth.
        if any_spawn_or_remove_animating {
            ctx.request_repaint();
        }
    }

    /// Draw the background grid - Minimal Void: very subtle
    fn draw_grid(&self, painter: &egui::Painter, canvas_rect: Rect) {
        let viewport = self.get_viewport(canvas_rect);
        // Minimal Void: very subtle grid (opacity 5 instead of 15)
        let grid_color = Color32::from_rgba_unmultiplied(255, 255, 255, 5);

        let screen_grid_size = self.grid_size * self.zoom;

        if screen_grid_size < 10.0 {
            return;
        }

        let start_x = (viewport.min.x / self.grid_size).floor() * self.grid_size;
        let start_y = (viewport.min.y / self.grid_size).floor() * self.grid_size;

        let mut x = start_x;
        while x < viewport.max.x {
            let screen_x = self.canvas_to_screen(Pos2::new(x, 0.0), canvas_rect).x;
            if screen_x >= canvas_rect.min.x && screen_x <= canvas_rect.max.x {
                painter.line_segment(
                    [Pos2::new(screen_x, canvas_rect.min.y), Pos2::new(screen_x, canvas_rect.max.y)],
                    Stroke::new(1.0, grid_color),
                );
            }
            x += self.grid_size;
        }

        let mut y = start_y;
        while y < viewport.max.y {
            let screen_y = self.canvas_to_screen(Pos2::new(0.0, y), canvas_rect).y;
            if screen_y >= canvas_rect.min.y && screen_y <= canvas_rect.max.y {
                painter.line_segment(
                    [Pos2::new(canvas_rect.min.x, screen_y), Pos2::new(canvas_rect.max.x, screen_y)],
                    Stroke::new(1.0, grid_color),
                );
            }
            y += self.grid_size;
        }

        // Origin crosshair - Minimal Void: very subtle white instead of red
        let origin_screen = self.canvas_to_screen(Pos2::ZERO, canvas_rect);
        if canvas_rect.contains(origin_screen) {
            let origin_color = Color32::from_rgba_unmultiplied(255, 255, 255, 12);
            painter.line_segment(
                [Pos2::new(origin_screen.x, canvas_rect.min.y), Pos2::new(origin_screen.x, canvas_rect.max.y)],
                Stroke::new(1.0, origin_color),
            );
            painter.line_segment(
                [Pos2::new(canvas_rect.min.x, origin_screen.y), Pos2::new(canvas_rect.max.x, origin_screen.y)],
                Stroke::new(1.0, origin_color),
            );
        }
    }

    /// Minimal Void: Draw floating status indicator in bottom-right corner
    fn draw_floating_status(&self, painter: &egui::Painter, canvas_rect: Rect, preview_count: usize) {
        let status_text = format!("{}%  {}⬚", (self.zoom * 100.0) as i32, preview_count);

        // Position in bottom-right with padding
        let padding = 16.0;
        let badge_width = 80.0;
        let badge_height = 24.0;

        let status_rect = Rect::from_min_size(
            Pos2::new(
                canvas_rect.max.x - badge_width - padding,
                canvas_rect.max.y - badge_height - padding,
            ),
            Vec2::new(badge_width, badge_height),
        );

        // Semi-transparent dark background with rounded corners
        painter.rect_filled(
            status_rect,
            12.0,
            Color32::from_rgba_unmultiplied(0, 0, 0, 150),
        );

        // Status text
        painter.text(
            status_rect.center(),
            egui::Align2::CENTER_CENTER,
            &status_text,
            egui::FontId::proportional(11.0),
            Color32::from_rgb(120, 120, 120),
        );
    }

    /// Empty-canvas hint shown before any preview has been added.
    fn draw_empty_state(&self, painter: &egui::Painter, canvas_rect: Rect) {
        let center = canvas_rect.center();

        painter.text(
            center + Vec2::new(0.0, -18.0),
            egui::Align2::CENTER_CENTER,
            egui_phosphor::regular::APP_WINDOW,
            egui::FontId::proportional(40.0),
            Color32::from_rgb(55, 55, 60),
        );
        painter.text(
            center + Vec2::new(0.0, 22.0),
            egui::Align2::CENTER_CENTER,
            "No windows yet",
            egui::FontId::proportional(15.0),
            Color32::from_rgb(110, 110, 118),
        );
        painter.text(
            center + Vec2::new(0.0, 44.0),
            egui::Align2::CENTER_CENTER,
            "Add a window from the left panel, or right-click for a window or browser tile",
            egui::FontId::proportional(12.0),
            Color32::from_rgb(75, 75, 82),
        );
    }

    /// Floating "Removed '...' · Undo" toast for the most recently removed preview.
    fn draw_and_interact_undo_toast(
        &mut self,
        ui: &mut egui::Ui,
        canvas_rect: Rect,
        preview_manager: &mut PreviewManager,
        capture_coordinator: &mut CaptureCoordinator,
    ) {
        let Some((removed_at, info)) = self.last_removed.clone() else { return; };

        let age = removed_at.elapsed().as_secs_f32();
        if age >= UNDO_TOAST_SECS {
            self.last_removed = None;
            return;
        }

        // Fade in quickly, fade out over the last half-second.
        let fade_in = (age / 0.15).clamp(0.0, 1.0);
        let fade_out = ((UNDO_TOAST_SECS - age) / 0.5).clamp(0.0, 1.0);
        let fade = fade_in.min(fade_out);
        let bg_alpha = (fade * 220.0) as u8;
        let text_alpha = (fade * 255.0) as u8;

        let title = if info.title.chars().count() > 28 {
            let truncated: String = info.title.chars().take(25).collect();
            format!("{}...", truncated)
        } else {
            info.title.clone()
        };
        let label = format!("Removed \"{}\"", title);

        let padding = 16.0;
        let toast_height = 32.0;
        let toast_width = 230.0;
        let toast_rect = Rect::from_min_size(
            Pos2::new(canvas_rect.min.x + padding, canvas_rect.max.y - toast_height - padding),
            Vec2::new(toast_width, toast_height),
        );

        let painter = ui.painter_at(canvas_rect);
        painter.rect_filled(
            toast_rect,
            10.0,
            Color32::from_rgba_unmultiplied(24, 24, 28, bg_alpha),
        );
        painter.text(
            Pos2::new(toast_rect.min.x + 12.0, toast_rect.center().y),
            egui::Align2::LEFT_CENTER,
            &label,
            egui::FontId::proportional(11.5),
            Color32::from_rgba_unmultiplied(210, 210, 215, text_alpha),
        );

        let undo_rect = Rect::from_min_size(
            Pos2::new(toast_rect.max.x - 56.0, toast_rect.min.y + 6.0),
            Vec2::new(48.0, toast_height - 12.0),
        );
        let undo_response = ui.interact(undo_rect, ui.id().with("undo_toast_btn"), Sense::click());
        let undo_color = if undo_response.hovered() {
            Color32::from_rgba_unmultiplied(140, 200, 255, text_alpha)
        } else {
            Color32::from_rgba_unmultiplied(74, 158, 255, text_alpha)
        };
        painter.text(
            undo_rect.center(),
            egui::Align2::CENTER_CENTER,
            "Undo",
            egui::FontId::proportional(12.0),
            undo_color,
        );

        if undo_response.clicked() {
            if info.browser_url.is_some() {
                // The browser's host window was destroyed with the tile, so
                // the app must recreate the WebView from the saved URL.
                self.pending_browser_restore = Some(info.clone());
            } else if let Some(handle) = info.window_handle {
                let id = preview_manager.add_for_window(
                    handle.hwnd,
                    handle.process_id,
                    info.title.clone(),
                    info.position,
                    info.size,
                );
                if let Some(preview) = preview_manager.get_mut(id) {
                    preview.capture_active = true;
                    preview.set_fps_preset(info.fps_preset);
                    preview.crop_uv = info.crop_uv;
                }
                capture_coordinator.start_capture(id, handle.hwnd, info.title.clone(), info.fps_preset.as_u32());
            }
            self.last_removed = None;
        }

        // Keep repainting while the toast is visible so it can fade out.
        ui.ctx().request_repaint();
    }

    /// Draw selection indicators and interactive resize handles
    fn draw_and_interact_selection(
        &mut self,
        ui: &mut egui::Ui,
        canvas_rect: Rect,
        preview_manager: &mut PreviewManager,
        input: &FrameInput,
    ) {
        let painter = ui.painter_at(canvas_rect);
        let alt_held = input.alt;

        // Collect selection info to avoid borrow issues
        let selection_info: Vec<_> = self.selection.iter()
            .filter_map(|id| preview_manager.get(*id).map(|p| {
                (*id, p.rect(), p.source_aspect_ratio, p.crop_uv, p.frame_size, p.is_browser())
            }))
            .collect();

        for (id, preview_rect, aspect_ratio, crop_uv, frame_size, is_browser) in selection_info {
            let screen_rect = self.canvas_rect_to_screen(preview_rect, canvas_rect);

            // Minimal Void: Selection border with accent color
            // (browsers can't be cropped, so no orange crop hint for them)
            let border_color = if self.interactive_browser == Some(id) {
                Color32::from_rgb(107, 170, 75) // Green: live interaction mode
            } else if alt_held && !is_browser {
                Color32::from_rgb(255, 150, 100) // Orange for crop mode
            } else {
                Color32::from_rgb(74, 158, 255) // #4a9eff blue accent
            };
            painter.rect_stroke(
                screen_rect,
                8.0, // Rounded corners
                Stroke::new(2.0, border_color),
            );

            // Minimal Void: Subtle crop indicator (bottom-left badge, only when selected)
            if crop_uv.is_some() {
                let crop_badge_rect = Rect::from_min_size(
                    screen_rect.left_bottom() + Vec2::new(8.0, -28.0),
                    Vec2::new(50.0, 20.0),
                );
                painter.rect_filled(crop_badge_rect, 10.0, Color32::from_rgba_unmultiplied(255, 150, 100, 200));
                painter.text(
                    crop_badge_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "CROP",
                    egui::FontId::proportional(9.0),
                    Color32::WHITE,
                );
            }

            // Minimal Void: Smaller, more subtle resize handles
            let handle_size = 6.0; // Reduced from 8.0
            let handle_hit_size = 14.0; // Keep large hit area for usability
            let handles = [
                (screen_rect.left_top(), ResizeHandle::TopLeft),
                (screen_rect.center_top(), ResizeHandle::Top),
                (screen_rect.right_top(), ResizeHandle::TopRight),
                (screen_rect.left_center(), ResizeHandle::Left),
                (screen_rect.right_center(), ResizeHandle::Right),
                (screen_rect.left_bottom(), ResizeHandle::BottomLeft),
                (screen_rect.center_bottom(), ResizeHandle::Bottom),
                (screen_rect.right_bottom(), ResizeHandle::BottomRight),
            ];

            for (handle_pos, handle_type) in handles {
                let handle_rect = Rect::from_center_size(handle_pos, Vec2::splat(handle_size));
                let hit_rect = Rect::from_center_size(handle_pos, Vec2::splat(handle_hit_size));

                // Minimal Void: Clean handles matching selection color
                let handle_fill = if alt_held && !is_browser {
                    Color32::from_rgb(255, 150, 100) // Orange for crop mode
                } else {
                    Color32::from_rgb(74, 158, 255) // Match accent color
                };
                painter.rect_filled(handle_rect, 3.0, handle_fill);
                // No stroke - cleaner look

                // Create interactive area for this handle
                let handle_response = ui.interact(
                    hit_rect,
                    ui.id().with(("resize_handle", id.0, handle_type as u8)),
                    Sense::drag(),
                );

                // Show cursor on hover
                if handle_response.hovered() {
                    ui.ctx().set_cursor_icon(handle_type.cursor());
                }

                // Handle drag start - check if Alt is held for crop mode
                // (browser tiles never crop: interactive coordinates would
                // no longer match the page)
                if handle_response.drag_started() {
                    if alt_held && frame_size.is_some() && !is_browser {
                        // Start crop mode
                        let current_crop = crop_uv.unwrap_or((0.0, 0.0, 1.0, 1.0));
                        self.drag_state = Some(DragState::Cropping {
                            id,
                            handle: handle_type,
                            start_mouse: input.interact_pos.unwrap_or(handle_pos),
                            start_crop_uv: current_crop,
                        });
                    } else {
                        // Start resize mode with aspect ratio lock
                        self.drag_state = Some(DragState::Resizing {
                            id,
                            handle: handle_type,
                            start_rect: preview_rect,
                            start_mouse: input.interact_pos.unwrap_or(handle_pos),
                            aspect_ratio,
                        });
                    }
                }

                // Handle dragging
                if handle_response.dragged() {
                    // Handle resize mode
                    if let Some(DragState::Resizing { id: resize_id, handle, start_rect, start_mouse, aspect_ratio: ar }) = &self.drag_state {
                        if *resize_id == id && *handle == handle_type {
                            if let Some(current_pos) = input.interact_pos {
                                let delta = (current_pos - *start_mouse) / self.zoom;
                                let new_rect = apply_resize(*handle, *start_rect, delta, Some(*ar));

                                // Apply minimum size
                                let min_size = 100.0;
                                if new_rect.width() >= min_size && new_rect.height() >= min_size {
                                    if let Some(preview) = preview_manager.get_mut(id) {
                                        preview.position = new_rect.min;
                                        preview.size = new_rect.size();
                                    }
                                }
                            }
                        }
                    }

                    // Handle crop mode
                    if let Some(DragState::Cropping { id: crop_id, handle, start_mouse, start_crop_uv }) = &self.drag_state {
                        if *crop_id == id && *handle == handle_type {
                            if let Some(current_pos) = input.interact_pos {
                                // Calculate delta in screen space, then convert to UV delta
                                let delta_screen = current_pos - *start_mouse;

                                // Convert screen delta to UV delta
                                // UV delta = screen delta / (preview screen size)
                                let preview_screen_size = screen_rect.size();
                                let uv_delta_x = delta_screen.x / preview_screen_size.x;
                                let uv_delta_y = delta_screen.y / preview_screen_size.y;

                                // Apply crop adjustment based on handle
                                let (min_u, min_v, max_u, max_v) = *start_crop_uv;
                                let mut new_crop = (min_u, min_v, max_u, max_v);

                                match handle {
                                    ResizeHandle::TopLeft => {
                                        new_crop.0 = (min_u + uv_delta_x).clamp(0.0, new_crop.2 - 0.1);
                                        new_crop.1 = (min_v + uv_delta_y).clamp(0.0, new_crop.3 - 0.1);
                                    }
                                    ResizeHandle::Top => {
                                        new_crop.1 = (min_v + uv_delta_y).clamp(0.0, new_crop.3 - 0.1);
                                    }
                                    ResizeHandle::TopRight => {
                                        new_crop.2 = (max_u + uv_delta_x).clamp(new_crop.0 + 0.1, 1.0);
                                        new_crop.1 = (min_v + uv_delta_y).clamp(0.0, new_crop.3 - 0.1);
                                    }
                                    ResizeHandle::Left => {
                                        new_crop.0 = (min_u + uv_delta_x).clamp(0.0, new_crop.2 - 0.1);
                                    }
                                    ResizeHandle::Right => {
                                        new_crop.2 = (max_u + uv_delta_x).clamp(new_crop.0 + 0.1, 1.0);
                                    }
                                    ResizeHandle::BottomLeft => {
                                        new_crop.0 = (min_u + uv_delta_x).clamp(0.0, new_crop.2 - 0.1);
                                        new_crop.3 = (max_v + uv_delta_y).clamp(new_crop.1 + 0.1, 1.0);
                                    }
                                    ResizeHandle::Bottom => {
                                        new_crop.3 = (max_v + uv_delta_y).clamp(new_crop.1 + 0.1, 1.0);
                                    }
                                    ResizeHandle::BottomRight => {
                                        new_crop.2 = (max_u + uv_delta_x).clamp(new_crop.0 + 0.1, 1.0);
                                        new_crop.3 = (max_v + uv_delta_y).clamp(new_crop.1 + 0.1, 1.0);
                                    }
                                }

                                // Apply the new crop
                                if let Some(preview) = preview_manager.get_mut(id) {
                                    preview.crop_uv = Some(new_crop);
                                    // Update aspect ratio based on new crop region
                                    if let Some((w, h)) = preview.frame_size {
                                        let crop_width = (new_crop.2 - new_crop.0) * w as f32;
                                        let crop_height = (new_crop.3 - new_crop.1) * h as f32;
                                        if crop_height > 0.0 {
                                            preview.source_aspect_ratio = crop_width / crop_height;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Clear drag state on release
                if handle_response.drag_stopped() {
                    if let Some(DragState::Resizing { id: resize_id, handle, .. }) = &self.drag_state {
                        if *resize_id == id && *handle == handle_type {
                            self.drag_state = None;
                        }
                    }
                    if let Some(DragState::Cropping { id: crop_id, handle, .. }) = &self.drag_state {
                        if *crop_id == id && *handle == handle_type {
                            self.drag_state = None;
                        }
                    }
                }
            }
        }
    }
}

/// Apply resize delta based on handle position, optionally maintaining aspect ratio
fn apply_resize(handle: ResizeHandle, start_rect: Rect, delta: Vec2, aspect_ratio: Option<f32>) -> Rect {
    let mut min = start_rect.min;
    let mut max = start_rect.max;

    match handle {
        ResizeHandle::TopLeft => {
            min.x += delta.x;
            min.y += delta.y;
        }
        ResizeHandle::Top => {
            min.y += delta.y;
        }
        ResizeHandle::TopRight => {
            max.x += delta.x;
            min.y += delta.y;
        }
        ResizeHandle::Left => {
            min.x += delta.x;
        }
        ResizeHandle::Right => {
            max.x += delta.x;
        }
        ResizeHandle::BottomLeft => {
            min.x += delta.x;
            max.y += delta.y;
        }
        ResizeHandle::Bottom => {
            max.y += delta.y;
        }
        ResizeHandle::BottomRight => {
            max.x += delta.x;
            max.y += delta.y;
        }
    }

    // Ensure min < max
    if min.x > max.x {
        std::mem::swap(&mut min.x, &mut max.x);
    }
    if min.y > max.y {
        std::mem::swap(&mut min.y, &mut max.y);
    }

    let mut result = Rect::from_min_max(min, max);

    // Apply aspect ratio constraint if provided
    if let Some(ar) = aspect_ratio {
        let width = result.width();
        let height = result.height();
        let current_ar = width / height;

        // Determine which dimension to adjust based on handle and direction
        match handle {
            // Corner handles - use the dominant movement direction
            ResizeHandle::TopLeft | ResizeHandle::TopRight |
            ResizeHandle::BottomLeft | ResizeHandle::BottomRight => {
                if current_ar > ar {
                    // Too wide - adjust width to match height
                    let new_width = height * ar;
                    match handle {
                        ResizeHandle::TopLeft | ResizeHandle::BottomLeft => {
                            result = Rect::from_min_size(
                                Pos2::new(result.max.x - new_width, result.min.y),
                                Vec2::new(new_width, height),
                            );
                        }
                        _ => {
                            result = Rect::from_min_size(
                                result.min,
                                Vec2::new(new_width, height),
                            );
                        }
                    }
                } else {
                    // Too tall - adjust height to match width
                    let new_height = width / ar;
                    match handle {
                        ResizeHandle::TopLeft | ResizeHandle::TopRight => {
                            result = Rect::from_min_size(
                                Pos2::new(result.min.x, result.max.y - new_height),
                                Vec2::new(width, new_height),
                            );
                        }
                        _ => {
                            result = Rect::from_min_size(
                                result.min,
                                Vec2::new(width, new_height),
                            );
                        }
                    }
                }
            }
            // Edge handles - adjust the perpendicular dimension
            ResizeHandle::Top | ResizeHandle::Bottom => {
                let new_width = height * ar;
                let center_x = result.center().x;
                result = Rect::from_center_size(
                    Pos2::new(center_x, result.center().y),
                    Vec2::new(new_width, height),
                );
            }
            ResizeHandle::Left | ResizeHandle::Right => {
                let new_height = width / ar;
                let center_y = result.center().y;
                result = Rect::from_center_size(
                    Pos2::new(result.center().x, center_y),
                    Vec2::new(width, new_height),
                );
            }
        }
    }

    result
}
