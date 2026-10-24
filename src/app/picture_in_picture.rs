use super::*;

/// Contains "Pluriview" so the window picker never lists it as a source.
const TITLE: &str = "Pluriview Picture in Picture";
const BACKGROUND: egui::Color32 = egui::Color32::from_rgb(13, 13, 13);
/// The longer side of a newly opened window, in points.
const DEFAULT_LONG_SIDE: f32 = 480.0;
const MIN_SIDE: f32 = 96.0;
/// Pointer distance from the window edge that resizes instead of moving.
const RESIZE_BORDER: f32 = 8.0;
const SCREEN_MARGIN: f32 = 24.0;

/// Runtime-only window that keeps one tile, or its whole group, above other
/// apps. Nothing here is saved with the workspace.
#[derive(Default)]
pub(super) struct PictureInPicture {
    open: Option<OpenWindow>,
    /// Where the window was last, so reopening it keeps the user's spot.
    last_rect: Option<egui::Rect>,
    icon: Option<std::sync::Arc<egui::IconData>>,
    #[cfg(windows)]
    native: NativeWindow,
}

struct OpenWindow {
    /// A grouped tile follows its group's current members.
    group: Option<u64>,
    /// Members, painted in this order (bottom to top).
    tiles: Vec<PreviewId>,
    /// Each tile's place as fractions of the members' combined bounds.
    places: Vec<(PreviewId, egui::Rect)>,
    aspect: f32,
    /// The builder's geometry stays fixed while open: egui would otherwise
    /// move the window back whenever the requested geometry changed.
    initial: egui::Rect,
}

/// Each tile's place as fractions of the tiles' combined bounds, and the
/// bounds' width-to-height ratio.
fn arrange(rects: &[(PreviewId, egui::Rect)]) -> Option<(Vec<(PreviewId, egui::Rect)>, f32)> {
    let bounds = rects
        .iter()
        .map(|(_, rect)| *rect)
        .reduce(|a, b| a.union(b))?;
    if bounds.width() < 1.0 || bounds.height() < 1.0 {
        return None;
    }
    let places = rects
        .iter()
        .map(|(id, rect)| {
            let min = (rect.min - bounds.min) / bounds.size();
            let max = (rect.max - bounds.min) / bounds.size();
            (*id, egui::Rect::from_min_max(min.to_pos2(), max.to_pos2()))
        })
        .collect();
    Some((places, bounds.width() / bounds.height()))
}

/// The largest rect of `aspect` centered in `area`.
fn fit(aspect: f32, area: egui::Rect) -> egui::Rect {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        16.0 / 9.0
    };
    let width = area.width().min(area.height() * aspect);
    egui::Rect::from_center_size(area.center(), Vec2::new(width, width / aspect))
}

fn place_in(place: egui::Rect, content: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        content.min + place.min.to_vec2() * content.size(),
        content.min + place.max.to_vec2() * content.size(),
    )
}

/// Window size for content of `aspect`, keeping the previous window's longer
/// side when there was one.
fn window_size(aspect: f32, long_side: f32) -> Vec2 {
    let long_side = long_side.max(MIN_SIDE);
    let size = if aspect >= 1.0 {
        Vec2::new(long_side, long_side / aspect)
    } else {
        Vec2::new(long_side * aspect, long_side)
    };
    size.max(Vec2::splat(MIN_SIDE))
}

fn resize_direction(pointer: Pos2, window: egui::Rect) -> Option<egui::viewport::ResizeDirection> {
    use egui::viewport::ResizeDirection as RD;
    let left = pointer.x <= window.min.x + RESIZE_BORDER;
    let right = pointer.x >= window.max.x - RESIZE_BORDER;
    let top = pointer.y <= window.min.y + RESIZE_BORDER;
    let bottom = pointer.y >= window.max.y - RESIZE_BORDER;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(RD::NorthWest),
        (_, true, true, _) => Some(RD::NorthEast),
        (true, _, _, true) => Some(RD::SouthWest),
        (_, true, _, true) => Some(RD::SouthEast),
        (true, ..) => Some(RD::West),
        (_, true, ..) => Some(RD::East),
        (_, _, true, _) => Some(RD::North),
        (.., true) => Some(RD::South),
        _ => None,
    }
}

fn paint_tile(
    ctx: &egui::Context,
    painter: &egui::Painter,
    preview: &mut Preview,
    rect: egui::Rect,
) {
    #[cfg(windows)]
    if !matches!(preview.video_status, VideoTileStatus::Failed(_)) {
        if let Some(renderer) = preview.video_renderer.clone() {
            let crop_uv = preview.crop_uv;
            painter.add(egui::PaintCallback {
                rect,
                callback: std::sync::Arc::new(eframe::egui_glow::CallbackFn::new(
                    move |info, painter| renderer.paint_cropped(info, painter.gl(), crop_uv),
                )),
            });
            return;
        }
    }
    let uv_rect = preview.get_uv_rect();
    if let Some(texture) = preview.get_texture(ctx) {
        painter.image(texture.id(), rect, uv_rect, egui::Color32::WHITE);
        return;
    }
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(22, 22, 24));
    if rect.width() >= 80.0 && rect.height() >= 30.0 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            preview.title.chars().take(40).collect::<String>(),
            egui::FontId::proportional(12.0),
            egui::Color32::from_rgb(150, 150, 150),
        );
    }
}

impl PluriviewApp {
    /// Open the window for `id`, or close it if `id` is already shown there.
    pub(super) fn toggle_picture_in_picture(&mut self, id: PreviewId, ctx: &egui::Context) {
        let showing = self
            .picture_in_picture
            .open
            .as_ref()
            .is_some_and(|open| open.tiles.contains(&id));
        if showing {
            self.close_picture_in_picture();
        } else {
            self.open_picture_in_picture(id, ctx);
        }
    }

    /// Shift+P: the focused or selected tile, else the current one. With
    /// nothing to show, the shortcut closes an open window.
    pub(super) fn picture_in_picture_shortcut(&mut self, ctx: &egui::Context) {
        let target = self
            .canvas
            .capture_targets()
            .first()
            .copied()
            .or_else(|| self.canvas.current_tile(&self.preview_manager));
        match target {
            Some(id) => self.toggle_picture_in_picture(id, ctx),
            None => self.close_picture_in_picture(),
        }
    }

    fn open_picture_in_picture(&mut self, id: PreviewId, ctx: &egui::Context) {
        let Some(preview) = self.preview_manager.get(id) else {
            return;
        };
        let group = preview.tile_group.as_ref().map(|group| group.id);
        // A group goes as a whole; otherwise a selection goes together, as
        // it does for screenshots and recordings.
        let tiles = if group.is_some() {
            self.canvas.group_members(id, &self.preview_manager)
        } else if self.canvas.selection.len() > 1 && self.canvas.selection.contains(&id) {
            self.canvas.selection.clone()
        } else {
            vec![id]
        };
        self.close_picture_in_picture();
        let mut open = OpenWindow {
            group,
            tiles,
            places: Vec::new(),
            aspect: 16.0 / 9.0,
            initial: egui::Rect::NOTHING,
        };
        self.refresh_picture_in_picture(&mut open);
        if open.tiles.is_empty() {
            return;
        }

        let previous = self.picture_in_picture.last_rect;
        let size = window_size(
            open.aspect,
            previous.map_or(DEFAULT_LONG_SIDE, |rect| rect.width().max(rect.height())),
        );
        // Reopen over the previous window's bottom-right corner, else in the
        // bottom-right corner of the main window.
        let corner = previous.map(|rect| rect.max).or_else(|| {
            ctx.input(|input| input.viewport().inner_rect)
                .map(|rect| rect.max - Vec2::splat(SCREEN_MARGIN))
        });
        open.initial = match corner {
            Some(corner) => egui::Rect::from_min_size(corner - size, size),
            None => egui::Rect::from_min_size(Pos2::ZERO, size),
        };
        self.picture_in_picture.open = Some(open);
    }

    pub(super) fn close_picture_in_picture(&mut self) {
        if self.picture_in_picture.open.take().is_none() {
            return;
        }
        for preview in self.preview_manager.all_mut() {
            preview.picture_in_picture = None;
        }
        #[cfg(windows)]
        {
            self.picture_in_picture.native = NativeWindow::default();
        }
    }

    /// Follow group membership, deletions and canvas moves.
    fn refresh_picture_in_picture(&self, open: &mut OpenWindow) {
        if let Some(group) = open.group {
            let anchor = self.preview_manager.all().find(|preview| {
                preview.removing.is_none()
                    && preview.tile_group.as_ref().is_some_and(|g| g.id == group)
            });
            match anchor {
                Some(anchor) => {
                    open.tiles = self.canvas.group_members(anchor.id, &self.preview_manager);
                }
                // Ungrouped: keep showing the tiles that were in it.
                None => open.group = None,
            }
        }
        open.tiles.retain(|id| {
            self.preview_manager
                .get(*id)
                .is_some_and(|preview| preview.removing.is_none() && !preview.is_playlist())
        });
        // Bottom to top, as the canvas paints them.
        open.tiles.sort_by_key(|id| {
            self.preview_manager
                .get(*id)
                .map(|preview| (preview.viewport_pin.is_some(), preview.z_order))
        });
        let rects: Vec<_> = open
            .tiles
            .iter()
            .filter_map(|&id| {
                self.canvas
                    .tile_screen_rect(id, &self.preview_manager)
                    .map(|rect| (id, rect))
            })
            .collect();
        // Without a usable canvas (before its first frame) keep the last
        // arrangement; a lone tile only needs its own shape.
        if let Some((places, aspect)) = arrange(&rects) {
            open.places = places;
            open.aspect = aspect;
        } else if let [id] = open.tiles[..] {
            if let Some(preview) = self.preview_manager.get(id) {
                open.places = vec![(
                    id,
                    egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                )];
                open.aspect = preview.size.x / preview.size.y.max(1.0);
            }
        }
        open.places.retain(|(id, _)| open.tiles.contains(id));
    }

    /// Show the window as an immediate egui viewport. It shares the main
    /// window's GL context, so tiles reuse their textures and video renderers
    /// instead of capturing a second time.
    pub(super) fn picture_in_picture_ui(&mut self, ctx: &egui::Context) {
        let Some(mut open) = self.picture_in_picture.open.take() else {
            return;
        };
        self.refresh_picture_in_picture(&mut open);
        if open.tiles.is_empty() {
            self.picture_in_picture.open = Some(open);
            self.close_picture_in_picture();
            return;
        }

        let icon = self
            .picture_in_picture
            .icon
            .get_or_insert_with(|| {
                let size = 64;
                std::sync::Arc::new(egui::IconData {
                    rgba: crate::tray::create_app_icon_rgba(size),
                    width: size,
                    height: size,
                })
            })
            .clone();
        let builder = egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_icon(icon)
            .with_decorations(false)
            .with_resizable(true)
            .with_taskbar(false)
            .with_window_level(egui::WindowLevel::AlwaysOnTop)
            .with_min_inner_size([MIN_SIDE, MIN_SIDE * 0.5])
            .with_inner_size(open.initial.size())
            .with_position(open.initial.min);

        let preview_manager = &mut self.preview_manager;
        let mut close = false;
        let mut sizes: Vec<(PreviewId, Vec2)> = Vec::with_capacity(open.places.len());
        let mut window_rect = None;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of(TITLE),
            builder,
            |ctx, _class| {
                close |= ctx.input(|input| input.viewport().close_requested());
                window_rect = ctx.input(|input| input.viewport().inner_rect);
                egui::CentralPanel::default()
                    .frame(egui::Frame::none().fill(BACKGROUND))
                    .show(ctx, |ui| {
                        let area = ui.max_rect();
                        let content = fit(open.aspect, area);
                        let painter = ui.painter_at(area);
                        for (id, place) in &open.places {
                            let Some(preview) = preview_manager.get_mut(*id) else {
                                continue;
                            };
                            let rect = place_in(*place, content);
                            sizes.push((*id, rect.size()));
                            paint_tile(ctx, &painter, preview, rect);
                        }
                        close |= window_controls(ui, area);
                    });
            },
        );

        if let Some(rect) = window_rect {
            self.picture_in_picture.last_rect = Some(rect);
        }
        if close {
            self.picture_in_picture.open = Some(open);
            self.close_picture_in_picture();
            return;
        }
        // Capture sizes follow the window; every other tile returns to the
        // canvas's own sizing.
        for preview in preview_manager.all_mut() {
            if let Some((_, size)) = sizes.iter().find(|(id, _)| *id == preview.id) {
                preview.picture_in_picture = Some(*size);
            } else if !open.tiles.contains(&preview.id) {
                preview.picture_in_picture = None;
            }
        }
        self.picture_in_picture.open = Some(open);
        #[cfg(windows)]
        self.picture_in_picture.native.upkeep();
    }
}

/// Move by dragging anywhere, resize from the edges, and return the tiles to
/// the canvas with the close button or a double-click.
fn window_controls(ui: &mut egui::Ui, area: egui::Rect) -> bool {
    let ctx = ui.ctx().clone();
    let pointer = ctx.input(|input| input.pointer.hover_pos());
    let edge = pointer.and_then(|pointer| resize_direction(pointer, area));
    if let Some(direction) = edge {
        use egui::viewport::ResizeDirection as RD;
        ctx.set_cursor_icon(match direction {
            RD::NorthWest | RD::SouthEast => egui::CursorIcon::ResizeNwSe,
            RD::NorthEast | RD::SouthWest => egui::CursorIcon::ResizeNeSw,
            RD::North | RD::South => egui::CursorIcon::ResizeVertical,
            RD::East | RD::West => egui::CursorIcon::ResizeHorizontal,
        });
        if ctx.input(|input| input.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
        }
    }

    let response = ui.interact(
        area,
        egui::Id::new("picture_in_picture_move"),
        egui::Sense::click_and_drag(),
    );
    let mut close = response.double_clicked();
    let pressed_on_edge = ctx
        .input(|input| input.pointer.press_origin())
        .is_some_and(|origin| resize_direction(origin, area).is_some());
    if response.drag_started() && !pressed_on_edge {
        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }

    if pointer.is_some_and(|pointer| area.contains(pointer)) && area.width() >= 64.0 {
        let button =
            egui::Rect::from_min_size(area.right_top() + Vec2::new(-32.0, 8.0), Vec2::splat(24.0));
        let response = ui
            .interact(
                button,
                egui::Id::new("picture_in_picture_close"),
                egui::Sense::click(),
            )
            .on_hover_text("Back to the canvas");
        let fill = if response.hovered() {
            egui::Color32::from_rgba_unmultiplied(255, 100, 100, 230)
        } else {
            egui::Color32::from_black_alpha(170)
        };
        ui.painter().rect_filled(button, 4.0, fill);
        ui.painter().text(
            button.center(),
            egui::Align2::CENTER_CENTER,
            egui_phosphor::regular::X,
            egui::FontId::proportional(13.0),
            egui::Color32::WHITE,
        );
        close |= response.clicked();
    }
    close
}

/// The native window behind the viewport, which egui does not expose.
#[cfg(windows)]
#[derive(Default)]
struct NativeWindow {
    hwnd: Option<HWND>,
    last_foreground: Option<HWND>,
}

#[cfg(windows)]
impl NativeWindow {
    fn upkeep(&mut self) {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, IsWindow, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE,
            HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_EX_NOACTIVATE,
        };

        let hwnd = match self
            .hwnd
            .filter(|hwnd| unsafe { IsWindow(*hwnd) }.as_bool())
        {
            Some(hwnd) => hwnd,
            None => {
                let Some(hwnd) = find_own_window() else {
                    return;
                };
                // Clicking or dragging the window must not take focus from a
                // game, which would minimize or pause many of them.
                unsafe {
                    let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_NOACTIVATE.0 as isize);
                }
                self.hwnd = Some(hwnd);
                self.last_foreground = None;
                hwnd
            }
        };
        // A game's own window may also be topmost, and the active topmost
        // window rises above the others. Raise ours again whenever another
        // window takes the foreground.
        let foreground = unsafe { GetForegroundWindow() };
        if self.last_foreground != Some(foreground) {
            self.last_foreground = Some(foreground);
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }
}

#[cfg(windows)]
fn find_own_window() -> Option<HWND> {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;

    let title = HSTRING::from(TITLE);
    let process = unsafe { GetCurrentProcessId() };
    let mut after = HWND::default();
    loop {
        let hwnd = unsafe { FindWindowExW(HWND::default(), after, None, &title) }.ok()?;
        let mut owner = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
        if owner == process {
            return Some(hwnd);
        }
        after = hwnd;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
        egui::Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
    }

    fn assert_near(actual: egui::Rect, expected: egui::Rect) {
        let error = (actual.min - expected.min).length() + (actual.max - expected.max).length();
        assert!(error < 1e-5, "{actual:?} != {expected:?}");
    }

    #[test]
    fn group_keeps_its_canvas_arrangement() {
        let (places, aspect) = arrange(&[
            (PreviewId(1), rect(100.0, 50.0, 200.0, 100.0)),
            (PreviewId(2), rect(300.0, 50.0, 100.0, 50.0)),
        ])
        .unwrap();
        assert!((aspect - 3.0).abs() < 1e-5);
        assert_near(places[0].1, rect(0.0, 0.0, 2.0 / 3.0, 1.0));
        assert_near(places[1].1, rect(2.0 / 3.0, 0.0, 1.0 / 3.0, 0.5));
    }

    #[test]
    fn empty_or_flat_arrangements_are_rejected() {
        assert!(arrange(&[]).is_none());
        assert!(arrange(&[(PreviewId(1), rect(0.0, 0.0, 100.0, 0.0))]).is_none());
    }

    #[test]
    fn content_is_letterboxed_without_stretching() {
        let wide = fit(2.0, rect(0.0, 0.0, 400.0, 400.0));
        assert_eq!(wide, rect(0.0, 100.0, 400.0, 200.0));
        let tall = fit(0.5, rect(0.0, 0.0, 400.0, 400.0));
        assert_eq!(tall, rect(100.0, 0.0, 200.0, 400.0));
        assert_eq!(
            place_in(rect(0.5, 0.0, 0.5, 1.0), wide),
            rect(200.0, 100.0, 200.0, 200.0)
        );
    }

    #[test]
    fn window_keeps_the_previous_long_side() {
        assert_eq!(window_size(16.0 / 9.0, 480.0), Vec2::new(480.0, 270.0));
        assert_eq!(window_size(0.5, 480.0), Vec2::new(240.0, 480.0));
        assert_eq!(window_size(20.0, 480.0), Vec2::new(480.0, MIN_SIDE));
    }

    #[test]
    fn edges_resize_and_the_middle_moves() {
        use egui::viewport::ResizeDirection as RD;
        let window = rect(0.0, 0.0, 300.0, 200.0);
        assert!(resize_direction(Pos2::new(150.0, 100.0), window).is_none());
        assert!(matches!(
            resize_direction(Pos2::new(299.0, 199.0), window),
            Some(RD::SouthEast)
        ));
        assert!(matches!(
            resize_direction(Pos2::new(1.0, 100.0), window),
            Some(RD::West)
        ));
        assert!(matches!(
            resize_direction(Pos2::new(150.0, 2.0), window),
            Some(RD::North)
        ));
    }
}
