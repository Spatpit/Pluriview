use eframe::egui::{self, Pos2, Vec2, Rect, Color32, Stroke, Sense, CursorIcon};
use crate::preview::{PreviewManager, PreviewId, FpsPreset};
use crate::capture::CaptureCoordinator;
use super::animation::{AnimationState, DragTracker};

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

        // Calculate delta time for animations
        let current_time = ui.input(|i| i.time);
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

        // Draw previews and handle their interactions (AFTER bg allocation)
        self.draw_and_interact_previews(ui, canvas_rect, preview_manager, ctx, capture_coordinator);

        // Draw selection rectangles and interactive resize handles
        // Handles are allocated AFTER previews so they have higher interaction priority
        self.draw_and_interact_selection(ui, canvas_rect, preview_manager);

        // Minimal Void: Floating status indicator (bottom-right corner)
        self.draw_floating_status(&painter, canvas_rect, preview_manager.count());

        // Handle canvas-level input using the pre-allocated bg_response
        self.handle_canvas_input_with_response(ui, canvas_rect, preview_manager, capture_coordinator, bg_response);

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
                    println!("Viewport culling: Resumed capture for '{}'", preview.title);
                } else if !is_visible && !preview.capture_paused {
                    // Pause capture - preview is now off-screen
                    capture_coordinator.pause_capture(id);
                    preview.capture_paused = true;
                    #[cfg(debug_assertions)]
                    println!("Viewport culling: Paused capture for '{}'", preview.title);
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

                // Restart capture with new FPS if it changed
                if preview.target_fps != old_fps {
                    if let Some(handle) = &preview.window_handle {
                        let hwnd = handle.hwnd;
                        let title = preview.title.clone();
                        let new_fps = preview.target_fps;
                        capture_coordinator.start_capture(
                            change.preview_id,
                            hwnd,
                            title,
                            new_fps,
                        );
                    }
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
    ) {
        // Use the pre-allocated background response

        let input = ui.input(|i| i.clone());

        // Update cursor based on drag state or handle hover
        if let Some(mouse_pos) = input.pointer.hover_pos() {
            if canvas_rect.contains(mouse_pos) {
                if let Some((_, handle)) = self.get_handle_at(mouse_pos, canvas_rect, preview_manager) {
                    ui.ctx().set_cursor_icon(handle.cursor());
                }
            }
        }

        // Zoom with scroll wheel - works anywhere on canvas, even over previews
        // We check canvas_rect.contains() instead of bg_response.hovered() because
        // bg_response.hovered() returns false when the mouse is over a preview widget
        if let Some(mouse_pos) = input.pointer.hover_pos() {
            if canvas_rect.contains(mouse_pos) {
                let scroll_delta = input.raw_scroll_delta.y;
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
        let alt_held = input.modifiers.alt;
        let middle_pressed = input.pointer.middle_down();
        let left_pressed = input.pointer.primary_down();
        let is_panning = (middle_pressed || (alt_held && left_pressed))
            && canvas_rect.contains(input.pointer.hover_pos().unwrap_or_default());

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
            if let Some(mouse_pos) = input.pointer.hover_pos() {
                self.pan_drag_tracker.record(mouse_pos, input.time);
            }

            // Use pointer delta directly instead of bg_response.drag_delta()
            // because bg_response.dragged() returns false when over a preview
            let delta = input.pointer.delta();
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
        if bg_response.clicked() && !input.modifiers.ctrl {
            if let Some(mouse_pos) = input.pointer.interact_pos() {
                let canvas_pos = self.screen_to_canvas(mouse_pos, canvas_rect);
                if preview_manager.get_preview_at(canvas_pos).is_none() {
                    self.selection.clear();
                }
            }
        }

        // Canvas context menu (right-click on empty space)
        bg_response.context_menu(|ui| {
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
                        preview_manager.remove(id);
                    }
                    self.selection.clear();
                    ui.close_menu();
                }
            }
        });

        // Keyboard shortcuts
        if bg_response.has_focus() || bg_response.hovered() {
            if input.key_pressed(egui::Key::Delete) {
                for id in self.selection.clone() {
                    capture_coordinator.stop_capture(id);
                    preview_manager.remove(id);
                }
                self.selection.clear();
            }

            if input.modifiers.ctrl && input.key_pressed(egui::Key::A) {
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
    ) {
        let viewport = self.get_viewport(canvas_rect);

        // Collect preview info first
        let preview_info: Vec<_> = {
            let previews = preview_manager.get_visible_previews(&viewport);
            previews.iter().map(|p| {
                (p.id, p.rect(), p.title.clone(), p.target_fps, p.fps_preset, p.crop_uv.is_some())
            }).collect()
        };

        let input = ui.input(|i| i.clone());

        for (id, rect, title, target_fps, current_preset, has_crop) in preview_info {
            let screen_rect = self.canvas_rect_to_screen(rect, canvas_rect);

            if !canvas_rect.intersects(screen_rect) {
                continue;
            }

            // Create interactive area for this preview
            let preview_response = ui.interact(
                screen_rect,
                ui.id().with(("preview", id.0)),
                Sense::click_and_drag(),
            );

            // Get the painter
            let painter = ui.painter_at(canvas_rect);

            // Minimal Void: No background fill - content fills entire area
            // Draw preview content (full rect, no title bar offset)
            let has_texture = if let Some(preview) = preview_manager.get_mut(id) {
                // Get UV rect first (immutable borrow ends before get_texture)
                let uv_rect = preview.get_uv_rect();
                if let Some(texture) = preview.get_texture(ctx) {
                    // Minimal Void: content fills entire rect
                    painter.image(
                        texture.id(),
                        screen_rect,
                        uv_rect,
                        Color32::WHITE,
                    );
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if !has_texture {
                // Minimal Void: subtle placeholder
                painter.rect_filled(screen_rect, 8.0, Color32::from_rgb(25, 25, 25));
                painter.text(
                    screen_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Loading...",
                    egui::FontId::proportional(12.0),
                    Color32::from_rgb(80, 80, 80),
                );
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
                painter.rect_filled(close_btn_rect, 4.0, Color32::from_rgba_unmultiplied(255, 80, 80, 200));
                painter.text(
                    close_btn_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "×",
                    egui::FontId::proportional(16.0),
                    Color32::WHITE,
                );

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
                painter.text(
                    screen_rect.left_top() + Vec2::new(12.0, 20.0),
                    egui::Align2::LEFT_CENTER,
                    &title_text,
                    egui::FontId::proportional(11.0),
                    Color32::from_rgb(200, 200, 200),
                );

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

            // Minimal Void: Only show border when selected (thin blue accent)
            if self.selection.contains(&id) {
                painter.rect_stroke(screen_rect, 8.0, Stroke::new(2.0, Color32::from_rgb(74, 158, 255)));
            }

            // Handle click to select
            if preview_response.clicked() {
                if input.modifiers.ctrl {
                    if self.selection.contains(&id) {
                        self.selection.retain(|&x| x != id);
                    } else {
                        self.selection.push(id);
                    }
                } else {
                    self.selection = vec![id];
                }
            }

            // Handle double-click to focus the source window
            if preview_response.double_clicked() {
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

            // Handle drag start - initialize spring and tracker
            if preview_response.drag_started() && !input.modifiers.alt && !input.pointer.middle_down() {
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
            if preview_response.dragged() && !input.modifiers.alt && !input.pointer.middle_down() {
                // Only move if we're not in a resize operation
                if self.drag_state.is_none() {
                    let delta = preview_response.drag_delta() / self.zoom;

                    // Track velocity for momentum
                    if let Some(mouse_pos) = input.pointer.hover_pos() {
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

                // Crop section
                ui.label("Crop:");
                if has_crop {
                    if ui.button("  Clear Crop (Show Full)").clicked() {
                        if let Some(preview) = preview_manager.get_mut(id) {
                            preview.clear_crop();
                        }
                        ui.close_menu();
                    }
                } else {
                    ui.label(egui::RichText::new("  Alt+drag handles to crop").weak().small());
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
                    preview_manager.remove(id);
                    self.selection.retain(|&x| x != id);
                    ui.close_menu();
                }
            });
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

    /// Draw selection indicators and interactive resize handles
    fn draw_and_interact_selection(
        &mut self,
        ui: &mut egui::Ui,
        canvas_rect: Rect,
        preview_manager: &mut PreviewManager,
    ) {
        let painter = ui.painter_at(canvas_rect);
        let input = ui.input(|i| i.clone());
        let alt_held = input.modifiers.alt;

        // Collect selection info to avoid borrow issues
        let selection_info: Vec<_> = self.selection.iter()
            .filter_map(|id| preview_manager.get(*id).map(|p| {
                (*id, p.rect(), p.source_aspect_ratio, p.crop_uv, p.frame_size)
            }))
            .collect();

        for (id, preview_rect, aspect_ratio, crop_uv, frame_size) in selection_info {
            let screen_rect = self.canvas_rect_to_screen(preview_rect, canvas_rect);

            // Minimal Void: Selection border with accent color
            let border_color = if alt_held {
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
                let handle_fill = if alt_held {
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
                if handle_response.drag_started() {
                    if alt_held && frame_size.is_some() {
                        // Start crop mode
                        let current_crop = crop_uv.unwrap_or((0.0, 0.0, 1.0, 1.0));
                        self.drag_state = Some(DragState::Cropping {
                            id,
                            handle: handle_type,
                            start_mouse: input.pointer.interact_pos().unwrap_or(handle_pos),
                            start_crop_uv: current_crop,
                        });
                    } else {
                        // Start resize mode with aspect ratio lock
                        self.drag_state = Some(DragState::Resizing {
                            id,
                            handle: handle_type,
                            start_rect: preview_rect,
                            start_mouse: input.pointer.interact_pos().unwrap_or(handle_pos),
                            aspect_ratio,
                        });
                    }
                }

                // Handle dragging
                if handle_response.dragged() {
                    // Handle resize mode
                    if let Some(DragState::Resizing { id: resize_id, handle, start_rect, start_mouse, aspect_ratio: ar }) = &self.drag_state {
                        if *resize_id == id && *handle == handle_type {
                            if let Some(current_pos) = input.pointer.interact_pos() {
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
                            if let Some(current_pos) = input.pointer.interact_pos() {
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
