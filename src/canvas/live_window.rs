//! Canvas side of using a window tile in place. The app moves the real
//! window over the tile (see `crate::window_interaction`); the canvas frames
//! the tile at the window's real size, and draws a grab bar above the window
//! so the tile can be moved without clicking into the app.
use super::*;
use crate::ui_theme;
use crate::window_interaction::{grab_bar_rect, interaction_pan, window_rect_for_tile};

/// Space kept between the framed window and the canvas edges, in points.
const FRAME_MARGIN: f32 = 16.0;
const DONE_BUTTON_WIDTH: f32 = 58.0;

/// Runtime-only state of the window tile in use. Nothing here is saved.
#[derive(Clone, Default)]
pub struct LiveWindow {
    /// Tile whose real window is in use over it. Set by the app each frame.
    pub id: Option<PreviewId>,
    /// Where the real window covers the canvas, in points. Set by the app.
    pub window_rect: Option<Rect>,
    /// The grab bar or Done is pressed: Pluriview has the focus on purpose.
    pub held: bool,
    /// The grab bar was let go: the window comes back over its tile.
    pub released: bool,
    /// Done was clicked: put the window back.
    pub done: bool,
    bar: Option<Rect>,
    drag: Option<GrabDrag>,
    camera: Option<SavedCamera>,
}

impl LiveWindow {
    /// Click-through passes the pointer on; a drag in progress ends there.
    pub(super) fn drag_cancelled(&mut self) {
        self.drag = None;
        self.held = false;
    }

    /// The pointer is on the grab bar, where canvas gestures must not start.
    pub fn covers(&self, pointer: Pos2) -> bool {
        self.id.is_some() && self.bar.is_some_and(|bar| bar.contains(pointer))
    }
}

#[derive(Clone, Copy)]
struct GrabDrag {
    start: Pos2,
    origin: GrabOrigin,
}

#[derive(Clone, Copy)]
enum GrabOrigin {
    Canvas(Pos2),
    Pinned(ViewportPin),
}

/// The view before the tile was framed, restored afterwards unless the user
/// navigated meanwhile.
#[derive(Clone)]
struct SavedCamera {
    pan: Vec2,
    zoom: f32,
    focus: Option<FocusState>,
    framed_pan: Vec2,
    framed_zoom: f32,
}

impl CanvasState {
    /// Glide to show tile `id` at `zoom`, the window's real size, keeping it
    /// in view with room for the grab bar, and the window on `screen` (the
    /// monitor's usable area, in canvas points). Returns false, leaving the
    /// camera alone, for pinned tiles and zooms outside the navigation range.
    pub fn frame_live_window(
        &mut self,
        id: PreviewId,
        zoom: f32,
        screen: Option<Rect>,
        preview_manager: &PreviewManager,
        now: f64,
    ) -> bool {
        let Some(canvas_rect) = self.last_screen_rect else {
            return false;
        };
        let Some(preview) = preview_manager
            .get(id)
            .filter(|preview| preview.viewport_pin.is_none())
        else {
            return false;
        };
        if !zoom.is_finite() || !(self.zoom_min..=self.zoom_max).contains(&zoom) {
            return false;
        }
        let subject = window_rect_for_tile(preview.rect(), preview.crop_uv);
        let pan = interaction_pan(
            canvas_rect,
            self.pan,
            self.zoom,
            subject,
            zoom,
            FRAME_MARGIN,
            screen,
        );
        let center = (canvas_rect.center() - canvas_rect.min) / zoom - pan;
        // Tile focus owns the zoom; set it aside and bring it back after.
        let focus = self.focus.take();
        self.live_window.camera = Some(SavedCamera {
            pan: self.pan,
            zoom: self.zoom,
            focus,
            framed_pan: pan,
            framed_zoom: zoom,
        });
        self.start_camera_transition(center.to_pos2(), zoom, canvas_rect, now);
        true
    }

    /// Land exactly on the framed camera once the glide ends, so the tile is
    /// pixel for pixel even after interpolation rounding.
    pub fn settle_live_window_camera(&mut self) {
        if let Some(saved) = &self.live_window.camera {
            if !self.is_camera_transitioning() {
                self.pan = saved.framed_pan;
                self.zoom = saved.framed_zoom;
            }
        }
    }

    /// The window changed size after moving to another monitor's scale:
    /// match the new size right away, keeping the tile in view.
    pub fn refit_live_window(
        &mut self,
        id: PreviewId,
        zoom: f32,
        screen: Option<Rect>,
        preview_manager: &PreviewManager,
    ) -> bool {
        let (Some(canvas_rect), Some(preview)) = (self.last_screen_rect, preview_manager.get(id))
        else {
            return false;
        };
        if preview.viewport_pin.is_some()
            || !zoom.is_finite()
            || !(self.zoom_min..=self.zoom_max).contains(&zoom)
        {
            return false;
        }
        let subject = window_rect_for_tile(preview.rect(), preview.crop_uv);
        self.cancel_camera_transition();
        self.pan = interaction_pan(
            canvas_rect,
            self.pan,
            self.zoom,
            subject,
            zoom,
            FRAME_MARGIN,
            screen,
        );
        self.zoom = zoom;
        if let Some(saved) = &mut self.live_window.camera {
            saved.framed_pan = self.pan;
            saved.framed_zoom = zoom;
        }
        true
    }

    /// Go back to the view from before the tile was framed, unless the user
    /// moved the camera themselves in the meantime. `framing` ends while the
    /// glide in is still Pluriview's, even if Escape stopped it midway.
    pub fn restore_live_window_camera(&mut self, now: f64, framing: bool) {
        let Some(saved) = self.live_window.camera.take() else {
            return;
        };
        let untouched = framing
            || self.is_camera_transitioning()
            || ((self.zoom - saved.framed_zoom).abs() <= saved.framed_zoom * 1e-3
                && (self.pan - saved.framed_pan).length() * self.zoom <= 1.0);
        if !untouched {
            return;
        }
        if saved.focus.is_some() {
            // Focus re-fits its tile every frame; return to it directly.
            self.cancel_camera_transition();
            self.pan = saved.pan;
            self.zoom = saved.zoom;
            self.focus = saved.focus;
            return;
        }
        let Some(canvas_rect) = self.last_screen_rect else {
            return;
        };
        let center = (canvas_rect.center() - canvas_rect.min) / saved.zoom - saved.pan;
        self.start_camera_transition(center.to_pos2(), saved.zoom, canvas_rect, now);
    }

    /// Camera to save with the workspace: the view from before the tile was
    /// framed, so using a window never changes the saved layout.
    pub fn layout_camera(&self) -> (Vec2, f32) {
        self.live_window
            .camera
            .as_ref()
            .map_or((self.pan, self.zoom), |saved| (saved.pan, saved.zoom))
    }

    /// The user dragged the window itself by `delta` points: move its tile
    /// along so the two stay together.
    pub fn follow_live_window(
        &mut self,
        id: PreviewId,
        delta: Vec2,
        preview_manager: &mut PreviewManager,
    ) {
        let Some(canvas_rect) = self.last_screen_rect else {
            return;
        };
        let Some(pin) = preview_manager.get(id).map(|preview| preview.viewport_pin) else {
            return;
        };
        match pin {
            Some(pin) => {
                let moved = self.moved_pin(
                    id,
                    pin.rect(canvas_rect).translate(delta),
                    canvas_rect,
                    preview_manager,
                );
                if let Some(preview) = preview_manager.get_mut(id) {
                    preview.viewport_pin = Some(moved);
                }
            }
            None => {
                if let Some(preview) = preview_manager.get_mut(id) {
                    preview.translate(delta / self.zoom);
                }
            }
        }
    }

    pub fn is_editing_crop(&self, id: PreviewId) -> bool {
        self.crop_mode == Some(id)
    }

    /// The bar above the window in use: drag it to move the tile, click it
    /// to return to the app, or press Done to put the window back. Drawn
    /// after the tiles so nothing covers it.
    pub(super) fn live_window_bar(
        &mut self,
        ui: &mut egui::Ui,
        canvas_rect: Rect,
        preview_manager: &mut PreviewManager,
        input: &FrameInput,
    ) {
        self.live_window.held = false;
        self.live_window.released = false;
        self.live_window.done = false;
        let Some((id, tile, crop, title, pin)) = self.live_window.id.and_then(|id| {
            preview_manager
                .get(id)
                .filter(|preview| preview.removing.is_none())
                .map(|preview| {
                    (
                        id,
                        self.preview_screen_rect(preview, canvas_rect),
                        preview.crop_uv,
                        compact_title(&preview.title, 60),
                        preview.viewport_pin,
                    )
                })
        }) else {
            self.live_window.bar = None;
            self.live_window.drag = None;
            return;
        };
        let window = self
            .live_window
            .window_rect
            .unwrap_or_else(|| window_rect_for_tile(tile, crop));
        let bar = grab_bar_rect(window, canvas_rect);
        let painter = ui.painter_at(canvas_rect);
        painter.rect_stroke(
            window.expand(1.0),
            4.0,
            Stroke::new(2.0, ui_theme::GOLD),
            egui::StrokeKind::Middle,
        );

        let response = ui
            .interact(
                bar,
                ui.id().with(("live_window_bar", id.0)),
                Sense::click_and_drag(),
            )
            .on_hover_text(
                "Drag to move this tile. Click outside the app or press Done to put its window back.",
            );
        let done_rect = Rect::from_min_size(
            Pos2::new(bar.right() - DONE_BUTTON_WIDTH - 4.0, bar.top() + 4.0),
            Vec2::new(DONE_BUTTON_WIDTH, bar.height() - 8.0),
        );
        let done = ui
            .interact(
                done_rect,
                ui.id().with(("live_window_done", id.0)),
                Sense::click(),
            )
            .on_hover_text("Put the window back where it was");

        let below = bar.top() >= window.bottom() - 0.5;
        painter.rect_filled(
            bar,
            if below {
                egui::CornerRadius {
                    nw: 0,
                    ne: 0,
                    sw: 8,
                    se: 8,
                }
            } else {
                egui::CornerRadius {
                    nw: 8,
                    ne: 8,
                    sw: 0,
                    se: 0,
                }
            },
            if response.hovered() || self.live_window.drag.is_some() {
                ui_theme::HOVER
            } else {
                ui_theme::PANEL
            },
        );
        painter.text(
            Pos2::new(bar.left() + 14.0, bar.center().y),
            egui::Align2::CENTER_CENTER,
            egui_phosphor::regular::DOTS_SIX_VERTICAL,
            egui::FontId::proportional(14.0),
            ui_theme::GOLD,
        );
        let title_rect = Rect::from_min_max(
            Pos2::new(bar.left() + 26.0, bar.top() + 6.0),
            Pos2::new(done_rect.left() - 8.0, bar.bottom() - 4.0),
        );
        ui_theme::clipped_text(ui, title_rect, &title, 12.0, ui_theme::SECONDARY);
        painter.rect_filled(
            done_rect,
            5.0,
            if done.hovered() {
                ui_theme::GOLD_DARK
            } else {
                ui_theme::SURFACE
            },
        );
        painter.text(
            done_rect.center(),
            egui::Align2::CENTER_CENTER,
            "Done",
            egui::FontId::proportional(12.0),
            ui_theme::GOLD,
        );
        if response.hovered() && !done.hovered() {
            ui.ctx()
                .set_cursor_icon(if self.live_window.drag.is_some() {
                    CursorIcon::Grabbing
                } else {
                    CursorIcon::Grab
                });
        }

        if response.drag_started() {
            let start = ui
                .input(|i| i.pointer.press_origin())
                .or(input.interact_pos)
                .unwrap_or(bar.center());
            let origin = match pin {
                Some(pin) => GrabOrigin::Pinned(pin),
                None => GrabOrigin::Canvas(
                    preview_manager
                        .get(id)
                        .map_or(Pos2::ZERO, |preview| preview.position),
                ),
            };
            self.live_window.drag = Some(GrabDrag { start, origin });
        }
        if let (Some(drag), Some(pointer)) = (
            self.live_window.drag,
            input.hover_pos.or(input.interact_pos),
        ) {
            let delta = pointer - drag.start;
            match drag.origin {
                GrabOrigin::Canvas(position) => {
                    if let Some(preview) = preview_manager.get_mut(id) {
                        preview.position = position + delta / self.zoom;
                    }
                }
                GrabOrigin::Pinned(pin) => {
                    let moved = self.moved_pin(
                        id,
                        pin.rect(canvas_rect).translate(delta),
                        canvas_rect,
                        preview_manager,
                    );
                    if let Some(preview) = preview_manager.get_mut(id) {
                        preview.viewport_pin = Some(moved);
                    }
                }
            }
            ui.ctx().request_repaint();
        }
        if self.live_window.drag.is_some() && !input.primary_down {
            self.live_window.drag = None;
            self.live_window.released = true;
        }
        if response.clicked() {
            self.live_window.released = true;
        }
        self.live_window.done = done.clicked();
        self.live_window.held = self.live_window.drag.is_some()
            || response.is_pointer_button_down_on()
            || done.is_pointer_button_down_on();
        self.live_window.bar = Some(bar);
    }
}
