use super::*;
use crate::hotkeys::HotkeyPresses;
use crate::preview::{PreviewLayout, TileGroup, ViewportAnchor};

#[derive(Clone, Default)]
pub(super) struct GroupState {
    spotlight: Option<Spotlight>,
    pub(super) resize: Option<GroupResize>,
    editing: Option<PreviewId>,
    active_tile: Option<PreviewId>,
    pub(super) spotlight_drag: Option<SpotlightDrag>,
    motions: Vec<TileMotion>,
    animation_time: Option<f64>,
    error: Option<String>,
}

#[derive(Clone)]
struct Spotlight {
    originals: Vec<TileGeometry>,
    active: usize,
    selection: Vec<PreviewId>,
    order: Vec<PreviewId>,
}

#[derive(Clone)]
pub(super) struct SpotlightDrag {
    pub(super) id: PreviewId,
    origin: Rect,
    start: Pos2,
    pointer: Pos2,
    target: Option<PreviewId>,
}

#[derive(Clone)]
struct TileMotion {
    id: PreviewId,
    from: Rect,
    to: Rect,
    pinned: bool,
    started: f64,
}

const LAYOUT_DURATION: f64 = 0.18;

impl TileMotion {
    fn sample(&self, now: f64) -> Rect {
        let t = ((now - self.started) / LAYOUT_DURATION).clamp(0.0, 1.0) as f32;
        let t = t * t * (3.0 - 2.0 * t);
        Rect::from_min_size(
            self.from.min + (self.to.min - self.from.min) * t,
            self.from.size() + (self.to.size() - self.from.size()) * t,
        )
    }
}

const LAYOUTS: [(usize, HotkeySlot, &str); 4] = [
    (1, HotkeySlot::LayoutGrid, "One large tile"),
    (2, HotkeySlot::LayoutRow, "Two large tiles"),
    (3, HotkeySlot::LayoutColumn, "Three large tiles"),
    (usize::MAX, HotkeySlot::LayoutSideStack, "Equal tiles"),
];

#[derive(Clone, Copy)]
struct TileGeometry {
    id: PreviewId,
    position: Pos2,
    size: Vec2,
    pin: Option<ViewportPin>,
    z_order: u32,
}

impl TileGeometry {
    fn capture(p: &Preview) -> Self {
        Self {
            id: p.id,
            position: p.position,
            size: p.size,
            pin: p.viewport_pin,
            z_order: p.z_order,
        }
    }

    fn restore(self, p: &mut Preview) {
        p.position = self.position;
        p.size = self.size;
        p.viewport_pin = self.pin;
        p.z_order = self.z_order;
    }
}

#[derive(Clone)]
pub(super) struct GroupResize {
    bounds: Rect,
    pointer: Pos2,
    members: Vec<(PreviewId, Rect)>,
}

#[derive(Clone, Copy)]
enum Arrangement {
    #[cfg(test)]
    Grid,
    #[cfg(test)]
    Row,
    #[cfg(test)]
    Column,
    SideStack,
}

const SIZES: [(f32, HotkeySlot, &str); 2] = [
    (960.0, HotkeySlot::SizeMedium, "Medium · 960 wide"),
    (1440.0, HotkeySlot::SizeLarge, "Large · 1440 wide"),
];

#[cfg(test)]
impl Arrangement {
    const ALL: [(Self, HotkeySlot, &'static str); 4] = [
        (Self::Grid, HotkeySlot::LayoutGrid, "Grid"),
        (Self::Row, HotkeySlot::LayoutRow, "Row"),
        (Self::Column, HotkeySlot::LayoutColumn, "Column"),
        (
            Self::SideStack,
            HotkeySlot::LayoutSideStack,
            "Main + Side Stack",
        ),
    ];
}

/// Pack visible edges rather than centering different aspect ratios in equal
/// cells. Any unused space belongs outside the arrangement, not between tiles.
fn arranged_rects(aspects: &[f32], bounds: Rect, layout: Arrangement, main: usize) -> Vec<Rect> {
    let count = aspects.len();
    if count == 0 {
        return Vec::new();
    }
    let aspects: Vec<_> = aspects
        .iter()
        .map(|&aspect| {
            if aspect.is_finite() && aspect > 0.0 {
                aspect
            } else {
                1.0
            }
        })
        .collect();
    let gap = 8.0_f32
        .min(bounds.width() / (count as f32 * 4.0))
        .min(bounds.height() / (count as f32 * 4.0));
    let mut rects = Vec::with_capacity(count);
    match layout {
        Arrangement::SideStack if count > 1 => {
            let main = main.min(count - 1);
            // Size companions against the group's height and the tallest
            // possible stack, not the active tile's width. A portrait main
            // then uses less horizontal space without miniaturizing its peers.
            let inverse_sum: f32 = aspects.iter().map(|aspect| aspect.recip()).sum();
            let shortest = aspects
                .iter()
                .map(|aspect| aspect.recip())
                .fold(f32::INFINITY, f32::min);
            let stack_inverse_sum = inverse_sum - shortest;
            let companion_stack = |width: f32| {
                let height = width / aspects[main];
                let gap = gap.min(height / (count as f32 * 4.0));
                let side_width = (height * (16.0 / 9.0) * (0.24 / 0.76))
                    .min((height - gap * (count - 2) as f32) / stack_inverse_sum);
                (gap, side_width)
            };
            // Fit the actual main + stack envelope. Reserving a fixed fraction
            // for a narrow stack wastes room and shrinks repeated Spotlight entry.
            let mut width = bounds.width().min(bounds.height() * aspects[main]);
            let (initial_gap, initial_side) = companion_stack(width);
            if width + initial_gap + initial_side > bounds.width() {
                let mut low = 0.0;
                let mut high = width;
                for _ in 0..24 {
                    let candidate = (low + high) * 0.5;
                    let (gap, side) = companion_stack(candidate);
                    if candidate + gap + side <= bounds.width() {
                        low = candidate;
                    } else {
                        high = candidate;
                    }
                }
                width = low;
            }
            let main_size = Vec2::new(width, width / aspects[main]);
            let (gap, side_width) = companion_stack(width);
            let mut y = 0.0;
            for (index, aspect) in aspects.iter().enumerate() {
                if index == main {
                    rects.push(Rect::from_min_size(Pos2::ZERO, main_size));
                } else {
                    let size = Vec2::new(side_width, side_width / aspect);
                    rects.push(Rect::from_min_size(Pos2::new(main_size.x + gap, y), size));
                    y += size.y + gap;
                }
            }
        }
        #[cfg(test)]
        Arrangement::Column => {
            let inverse_sum: f32 = aspects.iter().map(|a| a.recip()).sum();
            let width = bounds
                .width()
                .min((bounds.height() - gap * (count - 1) as f32) / inverse_sum);
            let mut y = 0.0;
            for aspect in &aspects {
                let size = Vec2::new(width, width / aspect);
                rects.push(Rect::from_min_size(Pos2::new(0.0, y), size));
                y += size.y + gap;
            }
        }
        _ => {
            #[cfg(not(test))]
            let cols = count;
            #[cfg(test)]
            let cols = if matches!(layout, Arrangement::Grid) {
                (count as f32).sqrt().ceil() as usize
            } else {
                count
            };
            let mut y = 0.0;
            let mut previous_height = bounds.height();
            for row in aspects.chunks(cols) {
                let mut height =
                    (bounds.width() - gap * (row.len() - 1) as f32) / row.iter().sum::<f32>();
                // An incomplete last row should not balloon to fill the width.
                if row.len() < cols {
                    height = height.min(previous_height);
                }
                let mut x = 0.0;
                for aspect in row {
                    let size = Vec2::new(height * aspect, height);
                    rects.push(Rect::from_min_size(Pos2::new(x, y), size));
                    x += size.x + gap;
                }
                y += height + gap;
                previous_height = height;
            }
        }
    }
    let packed = bounds_of(rects.iter().copied()).unwrap();
    let scale = (bounds.width() / packed.width())
        .min(bounds.height() / packed.height())
        .min(1.0);
    let origin = if matches!(layout, Arrangement::SideStack) {
        bounds.min
    } else {
        bounds.center() - packed.size() * scale * 0.5
    };
    rects
        .into_iter()
        .map(|rect| {
            Rect::from_min_size(
                origin + (rect.min - packed.min) * scale,
                rect.size() * scale,
            )
        })
        .collect()
}

fn bounds_of(rects: impl Iterator<Item = Rect>) -> Option<Rect> {
    rects.reduce(|a, b| a.union(b))
}

/// The order is the slot assignment: large tiles first, then companions.
fn spotlight_rects(aspects: &[f32], bounds: Rect, large_count: usize) -> Vec<Rect> {
    let count = aspects.len();
    if count == 0 {
        return Vec::new();
    }
    let large_count = large_count.clamp(1, count);
    if large_count == 1 && count > 1 {
        return arranged_rects(aspects, bounds, Arrangement::SideStack, 0);
    }
    let gap = 8.0_f32.min(bounds.width().min(bounds.height()) / (count as f32 * 4.0));
    if large_count == count {
        // Equal visible area gives portrait and landscape sources equal weight
        // without stretching either into a common rectangle.
        let cols = (count as f32).sqrt().ceil() as usize;
        let mut rects = Vec::with_capacity(count);
        let mut y = 0.0;
        for row in aspects.chunks(cols) {
            let mut x = 0.0;
            let mut height = 0.0_f32;
            for aspect in row {
                let size = Vec2::new(aspect.sqrt() * 180.0, 180.0 / aspect.sqrt());
                rects.push(Rect::from_min_size(Pos2::new(x, y), size));
                x += size.x + gap;
                height = height.max(size.y);
            }
            y += height + gap;
        }
        let packed = bounds_of(rects.iter().copied()).unwrap();
        let scale = (bounds.width() / packed.width()).min(bounds.height() / packed.height());
        return rects
            .into_iter()
            .map(|r| Rect::from_min_size(bounds.min + r.min.to_vec2() * scale, r.size() * scale))
            .collect();
    }
    let primary_aspect: f32 = aspects[..large_count].iter().sum();
    let mut spans = Vec::with_capacity(count - large_count + 1);
    spans.push(primary_aspect);
    spans.extend_from_slice(&aspects[large_count..]);
    let packed = arranged_rects(&spans, bounds, Arrangement::SideStack, 0);
    let primary = packed[0];
    let gap = gap.min(primary.width() / (large_count as f32 * 4.0));
    let height = (primary.width() - gap * (large_count - 1) as f32) / primary_aspect;
    let mut x = primary.left();
    let mut rects = Vec::with_capacity(count);
    for aspect in &aspects[..large_count] {
        let size = Vec2::new(height * aspect, height);
        rects.push(Rect::from_min_size(Pos2::new(x, primary.top()), size));
        x += size.x + gap;
    }
    rects.extend_from_slice(&packed[1..]);
    rects
}

/// A layout change conserves visible content area. Fitting a column into the
/// previous row's bounds (then vice versa) would otherwise collapse the group.
#[cfg(test)]
fn arranged_group_rects(current: &[Rect], layout: Arrangement, main: usize) -> Vec<Rect> {
    let Some(bounds) = bounds_of(current.iter().copied()) else {
        return Vec::new();
    };
    let count = current.len();
    let cols = (count as f32).sqrt().ceil() as usize;
    let size = match layout {
        Arrangement::Grid => Vec2::new(cols as f32 * 320.0, count.div_ceil(cols) as f32 * 180.0),
        Arrangement::Row => Vec2::new(count as f32 * 320.0, 180.0),
        Arrangement::Column => Vec2::new(320.0, count as f32 * 180.0),
        Arrangement::SideStack => Vec2::new(1280.0, 720.0),
    };
    let aspects: Vec<_> = current.iter().map(Rect::aspect_ratio).collect();
    let slots = arranged_rects(
        &aspects,
        Rect::from_min_size(Pos2::ZERO, size),
        layout,
        main,
    );
    let origin = bounds_of(slots.iter().copied()).unwrap().min;
    let area = current.iter().map(Rect::area).sum::<f32>();
    let slot_area = slots.iter().map(Rect::area).sum::<f32>().max(0.001);
    let scale = (area / slot_area).sqrt();
    slots
        .into_iter()
        .map(|r| Rect::from_min_size(bounds.min + (r.min - origin) * scale, r.size() * scale))
        .collect()
}

/// Every group member uses the same corner, so viewport resizing cannot pull
/// a pinned group apart when members straddle the viewport's midpoint.
fn group_pin(rect: Rect, viewport: Rect) -> ViewportPin {
    ViewportPin {
        anchor: ViewportAnchor::TopLeft,
        offset: (rect.left() - viewport.left(), rect.top() - viewport.top()),
        size: (rect.width(), rect.height()),
    }
}

pub(super) fn pin_for_tile(preview: &Preview, rect: Rect, viewport: Rect) -> ViewportPin {
    if preview.tile_group.is_some() {
        group_pin(rect, viewport)
    } else {
        ViewportPin::from_rect(rect, viewport)
    }
}

impl CanvasState {
    pub(super) fn update_layout_animation(&mut self, now: f64) {
        self.groups.animation_time = Some(now);
        self.groups
            .motions
            .retain(|m| now - m.started < LAYOUT_DURATION);
        if self.interactive_browser.is_some() || self.is_focusing_tile() {
            self.groups.motions.clear();
            self.groups.spotlight_drag = None;
        }
    }

    pub(super) fn layout_is_animating(&self) -> bool {
        !self.groups.motions.is_empty()
    }

    pub(super) fn layout_screen_rect(&self, id: PreviewId, target: Rect, viewport: Rect) -> Rect {
        if self.is_focusing_tile() {
            return target;
        }
        if let Some(drag) = &self.groups.spotlight_drag {
            if drag.id == id {
                return drag.origin.translate(drag.pointer - drag.start);
            }
        }
        if let Some(motion) = self.groups.motions.iter().find(|m| m.id == id) {
            let rect = motion.sample(self.groups.animation_time.unwrap_or(motion.started));
            return if motion.pinned {
                rect.translate(viewport.min.to_vec2())
            } else {
                self.canvas_rect_to_screen(rect, viewport)
            };
        }
        target
    }

    fn animate_layout_from(
        &mut self,
        starts: Vec<(PreviewId, Rect)>,
        viewport: Rect,
        previews: &PreviewManager,
    ) {
        let Some(started) = self.groups.animation_time else {
            return;
        };
        for (id, from_screen) in starts {
            self.groups.motions.retain(|m| m.id != id);
            let Some(p) = previews.get(id) else {
                continue;
            };
            let (from, to, pinned) = if let Some(pin) = p.viewport_pin {
                (
                    from_screen.translate(-viewport.min.to_vec2()),
                    pin.rect(viewport).translate(-viewport.min.to_vec2()),
                    true,
                )
            } else {
                (
                    Rect::from_min_size(
                        self.screen_to_canvas(from_screen.min, viewport),
                        from_screen.size() / self.zoom,
                    ),
                    p.rect(),
                    false,
                )
            };
            if from.min.distance(to.min) > 0.01 || (from.size() - to.size()).length() > 0.01 {
                self.groups.motions.push(TileMotion {
                    id,
                    from,
                    to,
                    pinned,
                    started,
                });
            }
        }
    }

    pub(super) fn spotlight_drag_response(
        &mut self,
        id: PreviewId,
        response: &egui::Response,
        viewport: Rect,
        previews: &mut PreviewManager,
        input: &FrameInput,
    ) {
        if !self.is_spotlighting() || self.is_focusing_tile() {
            return;
        }
        if input.alt || input.middle_down || input.pointer_blocked {
            self.groups.spotlight_drag = None;
            return;
        }
        if response.drag_started() {
            if let (Some(pointer), Some(p)) = (response.interact_pointer_pos(), previews.get(id)) {
                let origin = self.preview_screen_rect(p, viewport);
                self.groups.spotlight_drag = Some(SpotlightDrag {
                    id,
                    origin,
                    start: pointer - response.drag_delta(),
                    pointer,
                    target: None,
                });
                self.selection = vec![id];
                self.remember_current_tile(id);
                if let Some(s) = &mut self.groups.spotlight {
                    if let Some(index) = s.originals.iter().position(|p| p.id == id) {
                        s.active = index;
                    }
                }
            }
        }
        if self
            .groups
            .spotlight_drag
            .as_ref()
            .is_some_and(|d| d.id == id)
        {
            if let Some(pointer) = response.interact_pointer_pos().or(input.hover_pos) {
                let target = self.groups.spotlight.as_ref().and_then(|s| {
                    s.order.iter().copied().find(|other| {
                        *other != id
                            && previews.get(*other).is_some_and(|p| {
                                !p.left_click_disabled
                                    && self.preview_screen_rect(p, viewport).contains(pointer)
                            })
                    })
                });
                let drag = self.groups.spotlight_drag.as_mut().unwrap();
                drag.pointer = pointer;
                drag.target = target;
            }
            if response.drag_stopped() {
                self.finish_spotlight_drag(
                    response.interact_pointer_pos().or(input.hover_pos),
                    viewport,
                    previews,
                );
            }
        }
    }

    pub(super) fn draw_spotlight_drop_target(
        &self,
        painter: &egui::Painter,
        viewport: Rect,
        previews: &PreviewManager,
    ) {
        if let Some(preview) = self
            .groups
            .spotlight_drag
            .as_ref()
            .and_then(|d| d.target)
            .and_then(|id| previews.get(id))
        {
            painter.rect_stroke(
                self.preview_screen_rect(preview, viewport).expand(2.0),
                4.0,
                Stroke::new(3.0, Color32::from_rgb(207, 161, 57)),
            );
        }
    }

    pub(super) fn update_spotlight_drag_pointer(&mut self, pointer: Option<Pos2>, viewport: Rect) {
        if let Some(drag) = &mut self.groups.spotlight_drag {
            if let Some(pointer) = pointer {
                drag.pointer = pointer;
                if !viewport.contains(pointer) {
                    drag.target = None;
                }
            }
        }
    }

    pub(super) fn finish_spotlight_drag(
        &mut self,
        pointer: Option<Pos2>,
        viewport: Rect,
        previews: &mut PreviewManager,
    ) {
        let Some(source) = self.groups.spotlight_drag.as_ref().map(|d| d.id) else {
            return;
        };
        let target = pointer
            .filter(|p| viewport.contains(*p))
            .and_then(|pointer| {
                self.groups.spotlight.as_ref().and_then(|s| {
                    s.order.iter().copied().find(|id| {
                        *id != source
                            && previews.get(*id).is_some_and(|p| {
                                !p.left_click_disabled
                                    && self.preview_screen_rect(p, viewport).contains(pointer)
                            })
                    })
                })
            });
        if let Some(target) = target {
            self.swap_spotlight_tiles(source, target, viewport, previews);
        } else if let Some(p) = previews.get(source) {
            let start = self.preview_screen_rect(p, viewport);
            self.animate_layout_from(vec![(source, start)], viewport, previews);
        }
        self.groups.spotlight_drag = None;
    }

    fn swap_spotlight_tiles(
        &mut self,
        source: PreviewId,
        target: PreviewId,
        viewport: Rect,
        previews: &mut PreviewManager,
    ) {
        let (Some(source_tile), Some(target_tile)) = (previews.get(source), previews.get(target))
        else {
            return;
        };
        let source_geometry = TileGeometry::capture(source_tile);
        let target_geometry = TileGeometry::capture(target_tile);
        let starts = vec![
            (source, self.preview_screen_rect(source_tile, viewport)),
            (target, self.preview_screen_rect(target_tile, viewport)),
        ];
        let Some(s) = &mut self.groups.spotlight else {
            return;
        };
        let (Some(a), Some(b)) = (
            s.order.iter().position(|id| *id == source),
            s.order.iter().position(|id| *id == target),
        ) else {
            return;
        };
        s.order.swap(a, b);
        s.active = s.originals.iter().position(|p| p.id == source).unwrap();
        // Exchange the actual destinations, independent of membership order or
        // which preset originally produced them. Other tiles never reflow.
        for (id, destination) in [(source, target_geometry), (target, source_geometry)] {
            if let Some(p) = previews.get_mut(id) {
                p.position = destination.position;
                p.size = destination.size;
                p.viewport_pin = destination.pin;
            }
        }
        self.selection = vec![source];
        self.remember_current_tile(source);
        self.animate_layout_from(starts, viewport, previews);
    }

    pub(super) fn remember_current_tile(&mut self, id: PreviewId) {
        self.groups.active_tile = Some(id);
    }

    /// Group membership and the tile targeted by focus are independent.
    pub fn current_tile(&self, previews: &PreviewManager) -> Option<PreviewId> {
        self.groups
            .active_tile
            .into_iter()
            .chain(self.selection.iter().copied())
            .find(|id| {
                self.selection.contains(id)
                    && previews
                        .get(*id)
                        .is_some_and(|p| p.removing.is_none() && !p.left_click_disabled)
            })
    }

    pub fn toggle_current_tile_focus(&mut self, previews: &PreviewManager) {
        if self.exit_focus() {
            return;
        }
        if let (Some(id), Some(viewport)) = (self.current_tile(previews), self.last_screen_rect) {
            if let Some(preview) = previews.get(id) {
                self.focus_on_tile(id, preview.rect(), viewport);
            }
        }
    }

    pub(super) fn has_selected_group(&self, previews: &PreviewManager) -> bool {
        self.selected_group(previews).is_some()
    }

    pub(super) fn select_group_tile(&mut self, id: PreviewId, previews: &PreviewManager) {
        self.groups.active_tile = Some(id);
        if self.groups.editing != Some(id) {
            self.groups.editing = None;
        }
        self.selection = self.selection_for_tile(id, previews);
    }

    pub(super) fn toggle_group_selection(&mut self, id: PreviewId, previews: &PreviewManager) {
        let members = self.selection_for_tile(id, previews);
        if members.iter().all(|id| self.selection.contains(id)) {
            self.selection.retain(|id| !members.contains(id));
        } else {
            self.groups.active_tile = Some(id);
            for id in members {
                if !self.selection.contains(&id) {
                    self.selection.push(id);
                }
            }
        }
    }

    pub(super) fn spotlight_tile(
        &mut self,
        id: PreviewId,
        _viewport: Rect,
        _previews: &mut PreviewManager,
    ) {
        self.exit_focus();
        let Some(spotlight) = &mut self.groups.spotlight else {
            return;
        };
        if let Some(index) = spotlight.originals.iter().position(|p| p.id == id) {
            spotlight.active = index;
            self.selection = vec![id];
            self.remember_current_tile(id);
        }
    }

    pub(super) fn spotlight_controls(
        &mut self,
        ui: &mut egui::Ui,
        viewport: Rect,
        previews: &mut PreviewManager,
    ) {
        if !self.is_spotlighting() || self.is_focusing_tile() {
            return;
        }
        let Some(bounds) = self
            .spotlight_screen_bounds(viewport)
            .filter(|bounds| bounds.intersects(viewport))
        else {
            return;
        };
        egui::Area::new(ui.id().with("spotlight_controls"))
            .fixed_pos(egui::pos2(
                (bounds.center().x - 146.0).clamp(
                    viewport.left(),
                    (viewport.right() - 292.0).max(viewport.left()),
                ),
                (bounds.bottom() + 6.0).clamp(
                    viewport.top(),
                    (viewport.bottom() - 36.0).max(viewport.top()),
                ),
            ))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::none()
                    .fill(crate::ui_theme::PANEL)
                    .rounding(6.0)
                    .inner_margin(5.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Spotlight").on_hover_text("Drag a tile onto another to swap their places. Alt+1–4 changes the number of large tiles.");
                            if ui.button("Previous").clicked() {
                                self.cycle_spotlight(false, viewport, previews);
                            }
                            if ui.button("Next").clicked() {
                                self.cycle_spotlight(true, viewport, previews);
                            }
                            if ui.button("Exit").clicked() {
                                self.exit_spotlight(previews);
                            }
                        });
                    });
            });
    }

    fn group_members(&self, id: PreviewId, previews: &PreviewManager) -> Vec<PreviewId> {
        let group = previews.get(id).and_then(|p| p.tile_group.as_ref());
        let Some(group) = group else {
            return vec![id];
        };
        let mut members: Vec<_> = previews
            .all()
            .filter(|p| p.removing.is_none())
            .filter_map(|p| {
                p.tile_group
                    .as_ref()
                    .filter(|g| g.id == group.id)
                    .map(|g| (g.order, p.id))
            })
            .collect();
        members.sort_by_key(|(order, id)| (*order, id.0));
        members.into_iter().map(|(_, id)| id).collect()
    }

    pub(super) fn selection_for_tile(
        &self,
        id: PreviewId,
        previews: &PreviewManager,
    ) -> Vec<PreviewId> {
        if self.groups.editing == Some(id) || self.crop_mode == Some(id) {
            vec![id]
        } else {
            self.group_members(id, previews)
        }
    }

    pub(super) fn expand_group_selection(&mut self, previews: &PreviewManager) {
        for id in self.selection.clone() {
            for member in self.selection_for_tile(id, previews) {
                if !self.selection.contains(&member) {
                    self.selection.push(member);
                }
            }
        }
    }

    fn selected_group(&self, previews: &PreviewManager) -> Option<PreviewId> {
        let id = *self.selection.first()?;
        if self.groups.editing == Some(id)
            || self.crop_mode.is_some()
            || previews.get(id)?.tile_group.is_none()
        {
            return None;
        }
        let members = self.group_members(id, previews);
        (members.len() == self.selection.len()
            && members.iter().all(|id| self.selection.contains(id)))
        .then_some(id)
    }

    fn set_group_screen_rect(
        &self,
        id: PreviewId,
        rect: Rect,
        viewport: Rect,
        previews: &mut PreviewManager,
    ) {
        let min = self.screen_to_canvas(rect.min, viewport);
        let size = rect.size() / self.zoom;
        if let Some(p) = previews.get_mut(id) {
            if p.viewport_pin.is_some() {
                p.viewport_pin = Some(group_pin(rect, viewport));
            } else {
                p.position = min;
                p.size = size;
            }
        }
    }

    fn assign_group(
        &mut self,
        ids: &[PreviewId],
        group: TileGroup,
        viewport: Rect,
        previews: &mut PreviewManager,
    ) {
        self.exit_spotlight(previews);
        self.finish_crop();
        self.groups.editing = None;
        let pinned = ids
            .iter()
            .all(|id| previews.get(*id).is_some_and(|p| p.viewport_pin.is_some()));
        for (order, id) in ids.iter().enumerate() {
            let Some(rect) = previews
                .get(*id)
                .map(|p| self.preview_screen_rect(p, viewport))
            else {
                continue;
            };
            let min = self.screen_to_canvas(rect.min, viewport);
            if let Some(p) = previews.get_mut(*id) {
                p.tile_group = Some(TileGroup {
                    order: order as u32,
                    ..group.clone()
                });
                p.viewport_pin = pinned.then(|| group_pin(rect, viewport));
                if !pinned {
                    p.position = min;
                    p.size = rect.size() / self.zoom;
                }
            }
        }
        self.selection = ids.to_vec();
    }

    fn create_group(&mut self, viewport: Rect, previews: &mut PreviewManager) {
        self.exit_spotlight(previews);
        if self.selected_group(previews).is_some() {
            return;
        }
        let mut ids: Vec<_> = self
            .selection
            .iter()
            .copied()
            .filter(|id| previews.get(*id).is_some_and(|p| p.removing.is_none()))
            .collect();
        if ids.len() < 2 {
            return;
        }
        ids.sort_by(|a, b| {
            let a = self.preview_screen_rect(previews.get(*a).unwrap(), viewport);
            let b = self.preview_screen_rect(previews.get(*b).unwrap(), viewport);
            a.top()
                .total_cmp(&b.top())
                .then(a.left().total_cmp(&b.left()))
        });
        let mut bytes = [0; 8];
        if getrandom::getrandom(&mut bytes).is_err() {
            self.groups.error = Some("Could not create a group. Please try again.".into());
            return;
        }
        let id = u64::from_le_bytes(bytes);
        if previews
            .all()
            .any(|p| p.tile_group.as_ref().is_some_and(|g| g.id == id))
        {
            return;
        }
        self.assign_group(
            &ids,
            TileGroup {
                id,
                name: "Group".into(),
                order: 0,
            },
            viewport,
            previews,
        );
        self.groups.error = None;
    }

    fn ungroup_selection(&mut self, previews: &mut PreviewManager) {
        self.exit_spotlight(previews);
        self.finish_crop();
        self.groups.editing = None;
        self.expand_group_selection(previews);
        for id in &self.selection {
            if let Some(p) = previews.get_mut(*id) {
                p.tile_group = None;
            }
        }
        self.groups.editing = None;
    }

    #[cfg(test)]
    fn arrange_selection(
        &mut self,
        layout: Arrangement,
        main: Option<PreviewId>,
        viewport: Rect,
        previews: &mut PreviewManager,
    ) {
        self.exit_spotlight(previews);
        self.exit_focus();
        self.finish_crop();
        let ids = self
            .selected_group(previews)
            .map(|id| self.group_members(id, previews))
            .unwrap_or_else(|| self.selection.clone());
        let members: Vec<_> = ids
            .iter()
            .filter_map(|id| {
                previews
                    .get(*id)
                    .filter(|p| p.removing.is_none())
                    .map(|p| (*id, self.preview_screen_rect(p, viewport)))
            })
            .collect();
        if members.len() < 2 {
            return;
        }
        let main = members
            .iter()
            .position(|(id, _)| Some(*id) == main)
            .unwrap_or(0);
        let current: Vec<_> = members.iter().map(|(_, rect)| *rect).collect();
        for ((id, _), rect) in members
            .iter()
            .zip(arranged_group_rects(&current, layout, main))
        {
            self.set_group_screen_rect(*id, rect, viewport, previews);
        }
    }

    pub fn is_spotlighting(&self) -> bool {
        self.groups.spotlight.is_some()
    }

    fn resize_selection(&mut self, width: f32, viewport: Rect, previews: &mut PreviewManager) {
        self.exit_spotlight(previews);
        self.exit_focus();
        self.finish_crop();
        let members: Vec<_> = self
            .selection
            .iter()
            .filter_map(|id| {
                previews
                    .get(*id)
                    .map(|p| (*id, self.preview_screen_rect(p, viewport)))
            })
            .collect();
        let Some(bounds) = bounds_of(members.iter().map(|(_, r)| *r)) else {
            return;
        };
        let pinned = self
            .selection
            .iter()
            .all(|id| previews.get(*id).is_some_and(|p| p.viewport_pin.is_some()));
        let scale = width * if pinned { 1.0 } else { self.zoom } / bounds.width().max(1.0);
        for (id, rect) in members {
            self.set_group_screen_rect(
                id,
                Rect::from_min_size(
                    bounds.min + (rect.min - bounds.min) * scale,
                    rect.size() * scale,
                ),
                viewport,
                previews,
            );
        }
    }

    pub fn exit_spotlight(&mut self, previews: &mut PreviewManager) -> bool {
        self.finish_spotlight(previews, false)
    }

    fn finish_spotlight(&mut self, previews: &mut PreviewManager, restore_geometry: bool) -> bool {
        let Some(spotlight) = self.groups.spotlight.take() else {
            return false;
        };
        let active = spotlight.originals[spotlight.active].id;
        self.exit_focus();
        self.groups.spotlight_drag = None;
        if restore_geometry {
            self.groups.motions.clear();
        }
        if !restore_geometry {
            for (order, id) in spotlight.order.iter().enumerate() {
                if let Some(group) = previews.get_mut(*id).and_then(|p| p.tile_group.as_mut()) {
                    group.order = order as u32;
                }
            }
        }
        if restore_geometry {
            for original in spotlight.originals {
                if let Some(p) = previews.get_mut(original.id) {
                    original.restore(p);
                }
            }
        }
        self.selection = spotlight
            .selection
            .into_iter()
            .filter(|id| previews.get(*id).is_some())
            .collect();
        if previews.get(active).is_some_and(|p| p.removing.is_none()) {
            // A member selected while Spotlight was open may not have belonged
            // to the entry selection (for example, when editing one tile).
            if !self.selection.contains(&active) {
                self.select_group_tile(active, previews);
            }
            self.remember_current_tile(active);
        }
        true
    }

    pub(super) fn exit_spotlight_for_tile_action(&mut self, previews: &mut PreviewManager) {
        if self.is_spotlighting() {
            let selection = self.selection.clone();
            // Deletion snapshots retain the arrangement from before Spotlight.
            self.finish_spotlight(previews, true);
            self.selection = selection;
        }
    }

    pub fn saved_tile_layout(&self, preview: &Preview) -> PreviewLayout {
        let mut saved = PreviewLayout::from(preview);
        if let Some(original) = self
            .groups
            .spotlight
            .as_ref()
            .and_then(|s| s.originals.iter().find(|p| p.id == preview.id))
        {
            saved.position = (original.position.x, original.position.y);
            saved.size = (original.size.x, original.size.y);
            saved.viewport_pin = original.pin;
            saved.z_order = original.z_order;
        }
        saved
    }

    fn toggle_spotlight(&mut self, id: PreviewId, _viewport: Rect, previews: &mut PreviewManager) {
        if self.exit_spotlight(previews) {
            return;
        }
        let members = self.group_members(id, previews);
        if members.len() < 2 {
            return;
        }
        self.exit_focus();
        self.cancel_pointer_interaction();
        self.cancel_camera_transition();
        self.animation.momentum_active = false;
        let originals = members
            .iter()
            .filter_map(|id| previews.get(*id).map(TileGeometry::capture))
            .collect();
        self.groups.spotlight = Some(Spotlight {
            originals,
            active: members.iter().position(|member| *member == id).unwrap_or(0),
            selection: self.selection.clone(),
            order: members,
        });
        self.selection = vec![id];
        self.remember_current_tile(id);
    }

    fn spotlight_screen_bounds(&self, viewport: Rect) -> Option<Rect> {
        bounds_of(self.groups.spotlight.as_ref()?.originals.iter().map(|p| {
            p.pin.map(|pin| pin.rect(viewport)).unwrap_or_else(|| {
                self.canvas_rect_to_screen(Rect::from_min_size(p.position, p.size), viewport)
            })
        }))
    }

    fn cycle_spotlight(&mut self, next: bool, _viewport: Rect, _previews: &mut PreviewManager) {
        self.exit_focus();
        let Some(spotlight) = &mut self.groups.spotlight else {
            return;
        };
        let count = spotlight.originals.len();
        spotlight.active = (spotlight.active + if next { 1 } else { count - 1 }) % count;
        let id = spotlight.originals[spotlight.active].id;
        self.selection = vec![id];
        self.remember_current_tile(id);
    }

    fn apply_spotlight_layout(
        &mut self,
        large_count: usize,
        target: Option<PreviewId>,
        viewport: Rect,
        previews: &mut PreviewManager,
    ) {
        self.exit_focus();
        self.finish_crop();
        self.groups.spotlight_drag = None;
        let mut order = if let Some(s) = &self.groups.spotlight {
            s.order.clone()
        } else if let Some(id) = target.or_else(|| self.current_tile(previews)) {
            self.group_members(id, previews)
        } else {
            return;
        };
        if order.len() < 2 {
            return;
        }
        let large_count = large_count.clamp(1, order.len());
        if let Some(slot) = target.and_then(|id| order.iter().position(|member| *member == id)) {
            if slot >= large_count || large_count == 1 {
                order.swap(large_count - 1, slot);
            }
        }
        let starts = order
            .iter()
            .filter_map(|id| {
                previews
                    .get(*id)
                    .map(|p| (*id, self.preview_screen_rect(p, viewport)))
            })
            .collect();
        let current: Vec<_> = order
            .iter()
            .filter_map(|id| previews.get(*id))
            .map(|p| {
                p.viewport_pin
                    .map(|pin| pin.rect(viewport))
                    .unwrap_or_else(|| p.rect())
            })
            .collect();
        let aspects: Vec<_> = current.iter().map(Rect::aspect_ratio).collect();
        let available = bounds_of(current.into_iter()).unwrap();
        let rects = spotlight_rects(&aspects, available, large_count);
        for (index, (id, rect)) in order.iter().zip(rects).enumerate() {
            if let Some(p) = previews.get_mut(*id) {
                if p.viewport_pin.is_some() {
                    p.viewport_pin = Some(group_pin(rect, viewport));
                } else {
                    p.position = rect.min;
                    p.size = rect.size();
                }
                if !self.is_spotlighting() {
                    if let Some(group) = &mut p.tile_group {
                        group.order = index as u32;
                    }
                }
            }
        }
        if let Some(s) = &mut self.groups.spotlight {
            s.order = order;
            if let Some(index) = s.originals.iter().position(|p| Some(p.id) == target) {
                s.active = index;
                self.selection = vec![s.originals[index].id];
            }
        }
        if let Some(id) = target {
            if !self.is_spotlighting() && !self.selection.contains(&id) {
                self.select_group_tile(id, previews);
            }
            self.remember_current_tile(id);
        }
        self.animate_layout_from(starts, viewport, previews);
    }

    pub(super) fn update_spotlight(&mut self, _viewport: Rect, previews: &mut PreviewManager) {
        if let Some(spotlight) = &self.groups.spotlight {
            if spotlight
                .originals
                .iter()
                .any(|p| previews.get(p.id).is_none_or(|p| p.removing.is_some()))
            {
                self.finish_spotlight(previews, true);
            }
        }
    }

    pub fn handle_group_shortcuts(
        &mut self,
        presses: &HotkeyPresses,
        pointer: Option<Pos2>,
        previews: &mut PreviewManager,
    ) {
        // Most frames have no group action; avoid another canvas hit-test then.
        if ![
            HotkeySlot::GroupTiles,
            HotkeySlot::UngroupTiles,
            HotkeySlot::LayoutGrid,
            HotkeySlot::LayoutRow,
            HotkeySlot::LayoutColumn,
            HotkeySlot::LayoutSideStack,
            HotkeySlot::SizeMedium,
            HotkeySlot::SizeLarge,
            HotkeySlot::ToggleSpotlight,
            HotkeySlot::PreviousSpotlight,
            HotkeySlot::NextSpotlight,
            HotkeySlot::FocusCurrentTile,
            HotkeySlot::DeleteSelected,
        ]
        .into_iter()
        .any(|slot| presses.pressed(slot))
        {
            return;
        }
        let Some(viewport) = self.last_screen_rect else {
            return;
        };
        let target = self.current_tile(previews).or_else(|| {
            pointer
                .filter(|p| viewport.contains(*p))
                .and_then(|p| self.left_click_preview_at_screen(p, viewport, previews))
        });
        if presses.pressed(HotkeySlot::GroupTiles) {
            self.create_group(viewport, previews);
        }
        if presses.pressed(HotkeySlot::UngroupTiles) {
            self.ungroup_selection(previews);
        }
        for (large_count, slot, _) in LAYOUTS {
            if presses.pressed(slot) {
                self.apply_spotlight_layout(large_count, target, viewport, previews);
            }
        }
        for (width, slot, _) in SIZES {
            if presses.pressed(slot) {
                self.resize_selection(width, viewport, previews);
            }
        }
        if presses.pressed(HotkeySlot::ToggleSpotlight) {
            if self.is_spotlighting() {
                self.exit_spotlight(previews);
            } else if let Some(id) = target {
                self.toggle_spotlight(id, viewport, previews);
            }
        }
        if presses.pressed(HotkeySlot::NextSpotlight) {
            self.cycle_spotlight(true, viewport, previews);
        }
        if presses.pressed(HotkeySlot::PreviousSpotlight) {
            self.cycle_spotlight(false, viewport, previews);
        }
        if presses.pressed(HotkeySlot::DeleteSelected) {
            self.exit_spotlight_for_tile_action(previews);
        }
    }

    pub(super) fn group_menu(
        &mut self,
        ui: &mut egui::Ui,
        id: PreviewId,
        viewport: Rect,
        previews: &mut PreviewManager,
        shortcuts: &HotkeyBindings,
    ) {
        let group = previews.get(id).and_then(|p| p.tile_group.clone());
        ui.menu_button("Group", |ui| {
            crate::ui_theme::menu(ui);
            if ui
                .add_enabled(
                    self.selection.len() > 1,
                    crate::ui_theme::shortcut_button(
                        "Group Selected",
                        shortcuts.get(HotkeySlot::GroupTiles),
                    ),
                )
                .clicked()
            {
                self.create_group(viewport, previews);
                ui.close_menu();
            }
            if let Some(group) = &group {
                let mut name = group.name.clone();
                ui.label("Group name");
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut name)
                            .char_limit(60)
                            .desired_width(180.0),
                    )
                    .changed()
                {
                    for member in self.group_members(id, previews) {
                        if let Some(g) =
                            previews.get_mut(member).and_then(|p| p.tile_group.as_mut())
                        {
                            g.name = name.clone();
                        }
                    }
                }
                if ui.button("Select This Tile Only").clicked() {
                    self.remember_current_tile(id);
                    self.groups.editing = Some(id);
                    self.selection = vec![id];
                    ui.close_menu();
                }
                if ui.button("Select Whole Group").clicked() {
                    self.remember_current_tile(id);
                    self.groups.editing = None;
                    self.selection = self.group_members(id, previews);
                    ui.close_menu();
                }
                if ui.button("Remove This Tile from Group").clicked() {
                    if let Some(p) = previews.get_mut(id) {
                        p.tile_group = None;
                    }
                    self.groups.editing = None;
                    self.selection = vec![id];
                    ui.close_menu();
                }
                if ui
                    .add(crate::ui_theme::shortcut_button(
                        "Ungroup",
                        shortcuts.get(HotkeySlot::UngroupTiles),
                    ))
                    .clicked()
                {
                    self.groups.editing = None;
                    self.selection = self.group_members(id, previews);
                    self.ungroup_selection(previews);
                    ui.close_menu();
                }
            }
            ui.menu_button("Add Selected to Group", |ui| {
                crate::ui_theme::menu(ui);
                let mut groups: Vec<_> = previews
                    .all()
                    .filter_map(|p| p.tile_group.clone().map(|g| (p.id, g)))
                    .collect();
                groups.sort_by_key(|(_, g)| g.id);
                groups.dedup_by_key(|(_, g)| g.id);
                for (member, group) in groups {
                    if ui
                        .button(if group.name.trim().is_empty() {
                            "Group"
                        } else {
                            &group.name
                        })
                        .clicked()
                    {
                        let mut ids = self.group_members(member, previews);
                        for selected in &self.selection {
                            if !ids.contains(selected) {
                                ids.push(*selected);
                            }
                        }
                        self.assign_group(&ids, group, viewport, previews);
                        ui.close_menu();
                    }
                }
            });
            if let Some(error) = &self.groups.error {
                ui.label(error);
            }
        });
        ui.menu_button("Arrange", |ui| {
            crate::ui_theme::menu(ui);
            for (large_count, slot, label) in LAYOUTS {
                if ui
                    .add_enabled(
                        self.group_members(id, previews).len() > 1,
                        crate::ui_theme::shortcut_button(label, shortcuts.get(slot)),
                    )
                    .clicked()
                {
                    self.apply_spotlight_layout(large_count, Some(id), viewport, previews);
                    ui.close_menu();
                }
            }
            ui.separator();
            ui.menu_button("Size", |ui| {
                crate::ui_theme::menu(ui);
                for (width, slot, label) in SIZES {
                    if ui
                        .add(crate::ui_theme::shortcut_button(label, shortcuts.get(slot)))
                        .clicked()
                    {
                        self.resize_selection(width, viewport, previews);
                        ui.close_menu();
                    }
                }
            });
        });
        if ui
            .add_enabled(
                self.group_members(id, previews).len() > 1,
                crate::ui_theme::shortcut_button(
                    "Spotlight This Tile",
                    shortcuts.get(HotkeySlot::ToggleSpotlight),
                ),
            )
            .clicked()
        {
            self.toggle_spotlight(id, viewport, previews);
            ui.close_menu();
        }
    }

    pub(super) fn spotlight_menu(
        &mut self,
        ui: &mut egui::Ui,
        viewport: Rect,
        previews: &mut PreviewManager,
    ) {
        for (count, _, label) in LAYOUTS {
            if ui.button(label).clicked() {
                self.apply_spotlight_layout(count, self.current_tile(previews), viewport, previews);
                ui.close_menu();
            }
        }
        ui.separator();
        if ui.button("Previous Spotlight Tile").clicked() {
            self.cycle_spotlight(false, viewport, previews);
            ui.close_menu();
        }
        if ui.button("Next Spotlight Tile").clicked() {
            self.cycle_spotlight(true, viewport, previews);
            ui.close_menu();
        }
        if ui.button("Exit Spotlight").clicked() {
            self.exit_spotlight(previews);
            ui.close_menu();
        }
    }

    pub(super) fn toggle_viewport_pin(
        &mut self,
        id: PreviewId,
        viewport: Rect,
        previews: &mut PreviewManager,
    ) {
        self.exit_spotlight(previews);
        self.exit_focus();
        let pinned = previews.get(id).is_some_and(|p| p.viewport_pin.is_some());
        let members = self.group_members(id, previews);
        if previews.get(id).is_some_and(|p| p.tile_group.is_none()) {
            self.toggle_single_viewport_pin(id, viewport, previews);
            return;
        }
        self.cancel_pointer_interaction();
        for member in members {
            let Some(rect) = previews
                .get(member)
                .map(|p| self.preview_screen_rect(p, viewport))
            else {
                continue;
            };
            let min = self.screen_to_canvas(rect.min, viewport);
            if let Some(p) = previews.get_mut(member) {
                p.viewport_pin = (!pinned).then(|| group_pin(rect, viewport));
                if pinned {
                    p.position = min;
                    p.size = rect.size() / self.zoom;
                }
            }
        }
    }

    pub(super) fn moved_pin(
        &self,
        id: PreviewId,
        rect: Rect,
        viewport: Rect,
        previews: &PreviewManager,
    ) -> ViewportPin {
        if previews.get(id).is_some_and(|p| p.tile_group.is_some()) {
            group_pin(rect, viewport)
        } else {
            ViewportPin::from_rect(rect, viewport)
        }
    }

    pub(super) fn group_handle_at(
        &self,
        pointer: Pos2,
        viewport: Rect,
        previews: &PreviewManager,
    ) -> Option<(PreviewId, ResizeHandle)> {
        if self.is_spotlighting() {
            return None;
        }
        let id = self.selected_group(previews)?;
        let bounds = bounds_of(
            self.selection
                .iter()
                .filter_map(|id| previews.get(*id))
                .map(|p| self.preview_screen_rect(p, viewport)),
        )?;
        Rect::from_center_size(bounds.right_bottom(), Vec2::splat(RESIZE_HANDLE_HIT_SIZE))
            .contains(pointer)
            .then_some((id, ResizeHandle::BottomRight))
    }

    pub(super) fn draw_group_selection(
        &mut self,
        ui: &mut egui::Ui,
        viewport: Rect,
        previews: &mut PreviewManager,
        input: &FrameInput,
    ) -> bool {
        let Some(id) = self.selected_group(previews) else {
            if !input.pointer_blocked && self.crop_mode.is_none() && !self.is_spotlighting() {
                if let Some(id) = input.hover_pos.and_then(|pointer| {
                    self.left_click_preview_at_screen(pointer, viewport, previews)
                }) {
                    if let Some(group) = previews.get(id).and_then(|p| p.tile_group.as_ref()) {
                        let bounds = bounds_of(
                            previews
                                .all()
                                .filter(|p| {
                                    p.removing.is_none()
                                        && p.tile_group.as_ref().is_some_and(|g| g.id == group.id)
                                })
                                .map(|p| self.preview_screen_rect(p, viewport)),
                        );
                        if let Some(bounds) = bounds {
                            ui.painter().rect_stroke(
                                bounds.expand(3.0),
                                5.0,
                                Stroke::new(1.0, crate::ui_theme::GOLD.gamma_multiply(0.5)),
                            );
                        }
                    }
                }
            }
            return false;
        };
        if input.alt {
            return false;
        }
        let members: Vec<_> = self
            .selection
            .iter()
            .filter_map(|id| {
                previews
                    .get(*id)
                    .map(|p| (*id, self.preview_screen_rect(p, viewport)))
            })
            .collect();
        let Some(bounds) = bounds_of(members.iter().map(|(_, r)| *r)) else {
            return false;
        };
        let color = crate::ui_theme::GOLD;
        ui.painter()
            .rect_stroke(bounds.expand(3.0), 5.0, Stroke::new(1.5, color));
        if let Some(active) = self.current_tile(previews).and_then(|id| previews.get(id)) {
            ui.painter().rect_stroke(
                self.preview_screen_rect(active, viewport),
                3.0,
                Stroke::new(1.5, color),
            );
        }
        let name = previews
            .get(id)
            .and_then(|p| p.tile_group.as_ref())
            .map(|g| g.name.as_str())
            .unwrap_or("Group");
        ui.painter().text(
            bounds.left_top() - Vec2::new(0.0, 5.0),
            egui::Align2::LEFT_BOTTOM,
            if name.trim().is_empty() {
                "Group"
            } else {
                name
            },
            egui::FontId::proportional(12.0),
            color,
        );
        let handle =
            Rect::from_center_size(bounds.right_bottom(), Vec2::splat(RESIZE_HANDLE_HIT_SIZE));
        ui.painter().rect_filled(handle.shrink(3.0), 2.0, color);
        let response = ui.interact(handle, ui.id().with(("group_resize", id.0)), Sense::drag());
        if response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeNwSe);
        }
        if response.drag_started() {
            self.groups.resize = Some(GroupResize {
                bounds,
                pointer: input.interact_pos.unwrap_or(bounds.max) - response.drag_delta(),
                members,
            });
        }
        if response.dragged() {
            if let (Some(resize), Some(pointer)) = (&self.groups.resize, input.interact_pos) {
                let extent = resize.bounds.size() + (pointer - resize.pointer);
                let scale = (extent.x / resize.bounds.width())
                    .max(extent.y / resize.bounds.height())
                    .clamp(0.05, 20.0);
                for (id, rect) in &resize.members {
                    let resized = Rect::from_min_size(
                        resize.bounds.min + (rect.min - resize.bounds.min) * scale,
                        rect.size() * scale,
                    );
                    self.set_group_screen_rect(*id, resized, viewport, previews);
                }
            }
        }
        if response.drag_stopped() {
            self.groups.resize = None;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (CanvasState, PreviewManager, Vec<PreviewId>, Rect) {
        let mut canvas = CanvasState {
            pan: Vec2::new(25.0, -10.0),
            zoom: 0.8,
            ..Default::default()
        };
        let viewport = Rect::from_min_size(Pos2::new(200.0, 40.0), Vec2::new(1200.0, 800.0));
        canvas.last_screen_rect = Some(viewport);
        let mut previews = PreviewManager::new();
        let ids: Vec<_> = (0..4)
            .map(|i| {
                previews.add(
                    format!("Tile {i}"),
                    Pos2::new((i % 2) as f32 * 320.0, (i / 2) as f32 * 200.0),
                    Vec2::new(300.0, 180.0),
                )
            })
            .collect();
        canvas.selection = ids.clone();
        canvas.create_group(viewport, &mut previews);
        (canvas, previews, ids, viewport)
    }

    #[test]
    fn spotlight_presets_have_the_requested_large_count_and_equal_area() {
        let bounds = Rect::from_min_size(Pos2::new(30.0, 50.0), Vec2::new(1400.0, 800.0));
        for count in [1, 2, 3, 6] {
            let rects = spotlight_rects(&[1.5; 6], bounds, count);
            for (index, rect) in rects.iter().enumerate() {
                assert!(bounds.expand(0.001).contains_rect(*rect));
                assert!((rect.aspect_ratio() - 1.5).abs() < 0.001);
                for other in &rects[..index] {
                    assert!(!rect.intersects(*other));
                }
                if index < count {
                    assert!((rect.area() / rects[0].area() - 1.0).abs() < 0.001);
                } else {
                    assert!(rect.area() < rects[0].area() * 0.5);
                }
            }
        }
        let aspects = [0.5, 1.0, 2.5, 16.0 / 9.0];
        let rects = spotlight_rects(&aspects, bounds, usize::MAX);
        for (index, rect) in rects.iter().enumerate() {
            assert!((rect.area() / rects[0].area() - 1.0).abs() < 0.001);
            assert!((rect.aspect_ratio() - aspects[index]).abs() < 0.001);
        }
    }

    #[test]
    fn selected_member_enters_a_large_slot_when_applying_a_preset() {
        let (mut canvas, mut previews, ids, viewport) = fixture();
        canvas.apply_spotlight_layout(2, Some(ids[0]), viewport, &mut previews);
        canvas.apply_spotlight_layout(2, Some(ids[3]), viewport, &mut previews);
        assert_eq!(
            &canvas.group_members(ids[0], &previews)[..2],
            &[ids[0], ids[3]]
        );
        canvas.apply_spotlight_layout(1, Some(ids[3]), viewport, &mut previews);
        assert!(!canvas.is_spotlighting());
        assert_eq!(canvas.group_members(ids[0], &previews)[0], ids[3]);
        assert_eq!(canvas.current_tile(&previews), Some(ids[3]));
    }

    #[test]
    fn spotlight_entry_preserves_manual_geometry_and_swaps_only_actual_slots() {
        for pinned in [false, true] {
            let (mut canvas, mut previews, ids, viewport) = fixture();
            // Deliberately unrelated to member order, including a portrait and
            // a large tile that is neither the first nor the selected member.
            for (i, (position, size)) in [
                (Pos2::new(830.0, 50.0), Vec2::new(90.0, 200.0)),
                (Pos2::new(95.0, 480.0), Vec2::new(190.0, 110.0)),
                (Pos2::new(70.0, 35.0), Vec2::new(700.0, 390.0)),
                (Pos2::new(350.0, 470.0), Vec2::new(230.0, 130.0)),
            ]
            .into_iter()
            .enumerate()
            {
                let p = previews.get_mut(ids[i]).unwrap();
                p.position = position;
                p.size = size;
                p.tile_group.as_mut().unwrap().order = (3 - i) as u32;
            }
            if pinned {
                canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
            }
            let before: Vec<_> = ids
                .iter()
                .map(|id| TileGeometry::capture(previews.get(*id).unwrap()))
                .collect();
            let order = canvas.group_members(ids[0], &previews);
            canvas.toggle_spotlight(ids[1], viewport, &mut previews);
            canvas.spotlight_tile(ids[0], viewport, &mut previews);
            canvas.cycle_spotlight(true, viewport, &mut previews);
            canvas.update_spotlight(
                Rect::from_min_size(viewport.min, Vec2::new(900.0, 600.0)),
                &mut previews,
            );
            assert_eq!(canvas.groups.spotlight.as_ref().unwrap().order, order);
            for (id, original) in ids.iter().zip(&before) {
                let p = previews.get(*id).unwrap();
                assert_eq!(
                    (p.position, p.size, p.viewport_pin, p.z_order),
                    (
                        original.position,
                        original.size,
                        original.pin,
                        original.z_order
                    )
                );
            }
            canvas.swap_spotlight_tiles(ids[2], ids[0], viewport, &mut previews);
            for (i, id) in ids.iter().enumerate() {
                let original = &before[match i {
                    0 => 2,
                    2 => 0,
                    _ => i,
                }];
                let p = previews.get(*id).unwrap();
                assert_eq!(
                    (p.position, p.size, p.viewport_pin),
                    (original.position, original.size, original.pin)
                );
            }
            assert_eq!(canvas.current_tile(&previews), Some(ids[2]));
        }
    }

    #[test]
    fn spotlight_swaps_large_small_and_peer_slots_and_saves_order() {
        for pinned in [false, true] {
            let (mut canvas, mut previews, ids, viewport) = fixture();
            if pinned {
                canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
            }
            canvas.apply_spotlight_layout(2, Some(ids[0]), viewport, &mut previews);
            canvas.toggle_spotlight(ids[2], viewport, &mut previews);
            let mut order = ids.clone();
            for (a, b) in [(0, 3), (0, 1), (2, 3)] {
                let before: Vec<_> = order
                    .iter()
                    .map(|id| {
                        let p = previews.get(*id).unwrap();
                        (p.position, p.size, p.viewport_pin, p.crop_uv, p.z_order)
                    })
                    .collect();
                canvas.swap_spotlight_tiles(order[a], order[b], viewport, &mut previews);
                for (index, id) in order.iter().enumerate() {
                    let p = previews.get(*id).unwrap();
                    let dest = if index == a {
                        b
                    } else if index == b {
                        a
                    } else {
                        index
                    };
                    assert_eq!(
                        (p.position, p.size, p.viewport_pin),
                        (before[dest].0, before[dest].1, before[dest].2)
                    );
                    assert_eq!((p.crop_uv, p.z_order), (before[index].3, before[index].4));
                }
                order.swap(a, b);
                assert_eq!(canvas.groups.spotlight.as_ref().unwrap().order, order);
                assert_eq!(canvas.current_tile(&previews), Some(order[b]));
            }
            canvas.apply_spotlight_layout(3, None, viewport, &mut previews);
            assert_eq!(canvas.groups.spotlight.as_ref().unwrap().order, order);
            canvas.exit_spotlight(&mut previews);
            assert_eq!(canvas.group_members(ids[0], &previews), order);
            for (index, id) in order.iter().enumerate() {
                assert_eq!(
                    canvas
                        .saved_tile_layout(previews.get(*id).unwrap())
                        .tile_group
                        .unwrap()
                        .order,
                    index as u32
                );
            }
        }
    }

    #[test]
    fn layout_animation_retargets_without_jumping_or_saving_intermediate_geometry() {
        for pinned in [false, true] {
            let (mut canvas, mut previews, ids, viewport) = fixture();
            if pinned {
                canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
            }
            let before = canvas.preview_screen_rect(previews.get(ids[0]).unwrap(), viewport);
            canvas.update_layout_animation(1.0);
            canvas.apply_spotlight_layout(2, Some(ids[0]), viewport, &mut previews);
            assert!(canvas.layout_is_animating());
            assert!(
                canvas
                    .preview_screen_rect(previews.get(ids[0]).unwrap(), viewport)
                    .min
                    .distance(before.min)
                    < 0.001
            );
            canvas.update_layout_animation(1.09);
            let midpoint = canvas.preview_screen_rect(previews.get(ids[0]).unwrap(), viewport);
            canvas.apply_spotlight_layout(3, None, viewport, &mut previews);
            let retarget = canvas.preview_screen_rect(previews.get(ids[0]).unwrap(), viewport);
            assert!(retarget.min.distance(midpoint.min) < 0.001);
            assert!((retarget.size() - midpoint.size()).length() < 0.001);
            canvas.exit_spotlight(&mut previews);
            let saved = canvas.saved_tile_layout(previews.get(ids[0]).unwrap());
            assert_eq!(
                saved.size,
                (
                    previews.get(ids[0]).unwrap().size.x,
                    previews.get(ids[0]).unwrap().size.y
                )
            );
            canvas.update_layout_animation(1.5);
            assert!(!canvas.layout_is_animating());
            let after = canvas.preview_screen_rect(previews.get(ids[0]).unwrap(), viewport);
            let target = previews
                .get(ids[0])
                .unwrap()
                .viewport_pin
                .map(|pin| pin.rect(viewport))
                .unwrap_or_else(|| {
                    canvas.canvas_rect_to_screen(previews.get(ids[0]).unwrap().rect(), viewport)
                });
            assert_eq!(after, target);
        }
    }

    #[test]
    fn pointer_drop_swaps_spotlight_slots_and_outside_drop_cancels() {
        for pinned in [false, true] {
            let (mut canvas, mut previews, ids, _) = fixture();
            let ctx = egui::Context::default();
            let mut captures = CaptureCoordinator::new();
            for id in &ids {
                previews.get_mut(*id).unwrap().created_at = Instant::now() - Duration::from_secs(1);
            }
            frame(&ctx, &mut canvas, &mut previews, &mut captures, vec![]);
            let viewport = canvas.last_screen_rect.unwrap();
            if pinned {
                canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
            }
            canvas.apply_spotlight_layout(2, Some(ids[0]), viewport, &mut previews);
            canvas.toggle_spotlight(ids[0], viewport, &mut previews);
            canvas.update_layout_animation(10.0);
            for outside in [false, true] {
                let start = canvas
                    .preview_screen_rect(previews.get(ids[0]).unwrap(), viewport)
                    .center();
                let end = if outside {
                    Pos2::new(-100.0, -100.0)
                } else {
                    canvas
                        .preview_screen_rect(previews.get(ids[3]).unwrap(), viewport)
                        .center()
                };
                let before = canvas.groups.spotlight.as_ref().unwrap().order.clone();
                frame(
                    &ctx,
                    &mut canvas,
                    &mut previews,
                    &mut captures,
                    vec![egui::Event::PointerMoved(start)],
                );
                frame(
                    &ctx,
                    &mut canvas,
                    &mut previews,
                    &mut captures,
                    vec![button(start, true)],
                );
                frame(
                    &ctx,
                    &mut canvas,
                    &mut previews,
                    &mut captures,
                    vec![egui::Event::PointerMoved(start + Vec2::new(20.0, 15.0))],
                );
                frame(
                    &ctx,
                    &mut canvas,
                    &mut previews,
                    &mut captures,
                    vec![egui::Event::PointerMoved(end)],
                );
                frame(
                    &ctx,
                    &mut canvas,
                    &mut previews,
                    &mut captures,
                    vec![button(end, false)],
                );
                assert!(canvas.groups.spotlight_drag.is_none());
                let mut expected = before;
                if !outside {
                    expected.swap(0, 3);
                }
                assert_eq!(canvas.groups.spotlight.as_ref().unwrap().order, expected);
                canvas.update_layout_animation(10.0);
            }
        }
    }

    #[test]
    fn layouts_fit_mixed_aspects_without_overlap_or_stretch() {
        let bounds = Rect::from_min_size(Pos2::new(30.0, -15.0), Vec2::new(1200.0, 700.0));
        let aspects = [16.0 / 9.0, 0.5, 1.0, 2.4, 4.0 / 3.0];
        for (layout, _, _) in Arrangement::ALL {
            let rects = arranged_rects(&aspects, bounds, layout, 2);
            for (i, rect) in rects.iter().enumerate() {
                assert!(bounds.expand(0.001).contains_rect(*rect));
                assert!((rect.aspect_ratio() - aspects[i]).abs() < 0.001);
                assert!(rect.is_positive());
                for other in &rects[..i] {
                    assert!(!rect.intersects(*other));
                }
            }
        }
    }

    #[test]
    fn mixed_aspect_layouts_pack_visible_edges_with_small_gaps() {
        let aspects = [0.25, 4.0, 1.0, 0.5, 2.4, 1.5, 0.75];
        for size in [Vec2::new(1200.0, 700.0), Vec2::new(350.0, 900.0)] {
            let bounds = Rect::from_min_size(Pos2::new(30.0, 50.0), size);
            for (layout, _, _) in Arrangement::ALL {
                for main in 0..aspects.len() {
                    let rects = arranged_rects(&aspects, bounds, layout, main);
                    for (index, rect) in rects.iter().enumerate() {
                        assert!(bounds.expand(0.001).contains_rect(*rect));
                        assert!((rect.aspect_ratio() - aspects[index]).abs() < 0.001);
                        for other in &rects[..index] {
                            assert!(!rect.intersects(*other));
                        }
                    }
                    let close = |gap: f32| assert!(gap > 0.0 && gap <= 8.001, "gap: {gap}");
                    match layout {
                        Arrangement::Row | Arrangement::Grid => {
                            let cols = if matches!(layout, Arrangement::Row) {
                                aspects.len()
                            } else {
                                3
                            };
                            for row in rects.chunks(cols) {
                                for pair in row.windows(2) {
                                    close(pair[1].left() - pair[0].right());
                                    assert!((pair[0].top() - pair[1].top()).abs() < 0.001);
                                    assert!((pair[0].height() - pair[1].height()).abs() < 0.001);
                                }
                            }
                            for index in (cols..rects.len()).step_by(cols) {
                                close(rects[index].top() - rects[index - cols].bottom());
                            }
                        }
                        Arrangement::Column => {
                            for pair in rects.windows(2) {
                                close(pair[1].top() - pair[0].bottom());
                                assert!((pair[0].left() - pair[1].left()).abs() < 0.001);
                                assert!((pair[0].width() - pair[1].width()).abs() < 0.001);
                            }
                        }
                        Arrangement::SideStack => {
                            let companions: Vec<_> = rects
                                .iter()
                                .enumerate()
                                .filter(|(index, _)| *index != main)
                                .map(|(_, rect)| rect)
                                .collect();
                            for rect in &companions {
                                close(rect.left() - rects[main].right());
                                assert!(rect.width() < rects[main].width() / 2.0);
                            }
                            for pair in companions.windows(2) {
                                close(pair[1].top() - pair[0].bottom());
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn portrait_spotlight_keeps_companion_scale_and_group_anchor() {
        // One portrait source and three landscape sources, matching the case
        // where selecting the portrait used to shrink and recenter everything.
        for portrait in [0.5, 0.92, 1.0] {
            let aspects = [portrait, 16.0 / 9.0, 16.0 / 9.0, 16.0 / 9.0];
            let bounds = Rect::from_min_size(Pos2::new(24.0, 127.0), Vec2::new(1900.0, 820.0));
            let landscape = arranged_rects(&aspects, bounds, Arrangement::SideStack, 1);
            let vertical = arranged_rects(&aspects, bounds, Arrangement::SideStack, 0);
            assert!(landscape[1].min.distance(bounds.min) < 0.001);
            assert!(vertical[0].min.distance(bounds.min) < 0.001);
            assert!((vertical[0].height() - bounds.height()).abs() < 0.001);
            assert!((vertical[0].width() - bounds.height() * portrait).abs() < 0.001);
            for index in [2, 3] {
                assert!((landscape[index].size() - vertical[index].size()).length() < 0.001);
            }
            assert!((vertical[1].width() - landscape[0].width()).abs() < 0.001);
            assert!((vertical[1].left() - vertical[0].right() - 8.0).abs() < 0.001);
            assert!((vertical[1].top() - bounds.top()).abs() < 0.001);
            assert!(vertical[1].width() > 250.0);
        }
    }

    #[test]
    fn compact_spotlight_does_not_shrink_when_reopened() {
        let aspects = [0.5, 16.0 / 9.0, 1.0, 2.4, 0.75];
        for main in 0..aspects.len() {
            let bounds = Rect::from_min_size(Pos2::new(30.0, 50.0), Vec2::new(1200.0, 700.0));
            let original = arranged_rects(&aspects, bounds, Arrangement::SideStack, main);
            let mut rects = original.clone();
            for _ in 0..20 {
                let bounds = bounds_of(rects.iter().copied()).unwrap();
                rects = arranged_rects(&aspects, bounds, Arrangement::SideStack, main);
            }
            for (before, after) in original.iter().zip(rects) {
                assert!(before.min.distance(after.min) < 0.01);
                assert!((before.size() - after.size()).length() < 0.01);
            }
        }
    }

    #[test]
    fn layouts_handle_empty_single_and_tiny_viewports() {
        assert!(arranged_rects(&[], Rect::EVERYTHING, Arrangement::Grid, 0).is_empty());
        let bounds = Rect::from_min_size(Pos2::ZERO, Vec2::splat(10.0));
        for count in [1, 2, 30] {
            for (layout, _, _) in Arrangement::ALL {
                for rect in arranged_rects(&vec![1.5; count], bounds, layout, 0) {
                    assert!(rect.is_positive());
                    assert!(bounds.expand(0.001).contains_rect(rect));
                }
            }
        }
    }

    #[test]
    fn cycling_layouts_preserves_content_area_and_reapplying_is_stable() {
        let mut rects = vec![
            Rect::from_min_size(Pos2::new(30.0, 50.0), Vec2::new(320.0, 180.0)),
            Rect::from_min_size(Pos2::new(390.0, 50.0), Vec2::new(100.0, 200.0)),
            Rect::from_min_size(Pos2::new(30.0, 290.0), Vec2::splat(180.0)),
        ];
        let area = rects.iter().map(Rect::area).sum::<f32>();
        let origin = bounds_of(rects.iter().copied()).unwrap().min;
        for _ in 0..20 {
            for (layout, _, _) in Arrangement::ALL {
                rects = arranged_group_rects(&rects, layout, 1);
                assert!((rects.iter().map(Rect::area).sum::<f32>() / area - 1.0).abs() < 0.001);
                assert!(
                    bounds_of(rects.iter().copied())
                        .unwrap()
                        .min
                        .distance(origin)
                        < 0.01
                );
                for (before, after) in rects.iter().zip(arranged_group_rects(&rects, layout, 1)) {
                    assert!(before.min.distance(after.min) < 0.02);
                    assert!((before.size() - after.size()).length() < 0.02);
                }
            }
        }
    }

    #[test]
    fn group_selection_member_edit_and_ungroup_preserve_geometry() {
        let (mut canvas, mut previews, ids, _) = fixture();
        let before: Vec<_> = ids
            .iter()
            .map(|id| previews.get(*id).unwrap().rect())
            .collect();
        canvas.select_group_tile(ids[1], &previews);
        assert_eq!(canvas.selection, ids);
        canvas.toggle_group_selection(ids[2], &previews);
        assert!(canvas.selection.is_empty());
        canvas.toggle_group_selection(ids[2], &previews);
        assert_eq!(canvas.selection, ids);
        canvas.groups.editing = Some(ids[1]);
        canvas.select_group_tile(ids[1], &previews);
        assert_eq!(canvas.selection, vec![ids[1]]);
        canvas.select_group_tile(ids[0], &previews);
        assert_eq!(canvas.selection, ids);
        canvas.ungroup_selection(&mut previews);
        for (id, before) in ids.iter().zip(before) {
            let p = previews.get(*id).unwrap();
            assert_eq!(p.rect(), before);
            assert!(p.tile_group.is_none());
        }
    }

    #[test]
    fn group_pin_survives_pan_zoom_and_viewport_resize_as_one_unit() {
        let (mut canvas, mut previews, ids, viewport) = fixture();
        let before: Vec<_> = ids
            .iter()
            .map(|id| canvas.preview_screen_rect(previews.get(*id).unwrap(), viewport))
            .collect();
        canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
        canvas.pan = Vec2::new(-900.0, 400.0);
        canvas.zoom = 3.0;
        let resized = Rect::from_min_size(viewport.min, Vec2::new(600.0, 420.0));
        for (id, before) in ids.iter().zip(&before) {
            assert_eq!(
                canvas.preview_screen_rect(previews.get(*id).unwrap(), resized),
                *before
            );
        }
        canvas.toggle_viewport_pin(ids[3], resized, &mut previews);
        for (id, before) in ids.iter().zip(before) {
            let p = previews.get(*id).unwrap();
            assert!(p.viewport_pin.is_none());
            let after = canvas.preview_screen_rect(p, resized);
            assert!(after.min.distance(before.min) < 0.001);
            assert!((after.size() - before.size()).length() < 0.001);
        }
    }

    #[test]
    fn grouping_mixed_pins_keeps_current_screen_positions() {
        let (mut canvas, mut previews, ids, viewport) = fixture();
        canvas.ungroup_selection(&mut previews);
        canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
        let before: Vec<_> = ids
            .iter()
            .map(|id| canvas.preview_screen_rect(previews.get(*id).unwrap(), viewport))
            .collect();
        canvas.create_group(viewport, &mut previews);
        for (id, before) in ids.iter().zip(before) {
            let p = previews.get(*id).unwrap();
            assert!(p.viewport_pin.is_none());
            let after = canvas.preview_screen_rect(p, viewport);
            assert!(after.min.distance(before.min) < 0.001);
            assert!((after.size() - before.size()).length() < 0.001);
        }
    }

    #[test]
    fn spotlight_cycles_resizes_and_saves_then_can_restore_exact_originals() {
        let (mut canvas, mut previews, ids, viewport) = fixture();
        canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
        previews.get_mut(ids[1]).unwrap().crop_uv = Some((0.1, 0.2, 0.9, 0.8));
        let before: Vec<_> = ids
            .iter()
            .map(|id| {
                serde_json::to_value(PreviewLayout::from(previews.get(*id).unwrap())).unwrap()
            })
            .collect();
        let camera = (canvas.pan, canvas.zoom);
        canvas.toggle_spotlight(ids[2], viewport, &mut previews);
        assert_eq!(canvas.selection, vec![ids[2]]);
        canvas.cycle_spotlight(true, viewport, &mut previews);
        assert_eq!(canvas.selection, vec![ids[3]]);
        canvas.cycle_spotlight(true, viewport, &mut previews);
        assert_eq!(canvas.selection, vec![ids[0]]);
        canvas.cycle_spotlight(false, viewport, &mut previews);
        assert_eq!(canvas.selection, vec![ids[3]]);
        let resized = Rect::from_min_size(viewport.min, Vec2::new(700.0, 420.0));
        canvas.update_spotlight(resized, &mut previews);
        // Pinning can place the original group partly outside the viewport;
        // Spotlight must honor that placement rather than recentering it.
        let group_bounds = canvas.spotlight_screen_bounds(resized).unwrap();
        for (id, original) in ids.iter().zip(&before) {
            let p = previews.get(*id).unwrap();
            assert_eq!(
                &serde_json::to_value(PreviewLayout::from(p)).unwrap(),
                original
            );
            assert!(group_bounds
                .expand(0.001)
                .contains_rect(canvas.preview_screen_rect(p, resized)));
            assert_eq!(
                &serde_json::to_value(canvas.saved_tile_layout(p)).unwrap(),
                original
            );
        }
        assert!(canvas.finish_spotlight(&mut previews, true));
        assert_eq!((canvas.pan, canvas.zoom), camera);
        assert_eq!(canvas.selection, ids);
        for (id, original) in ids.iter().zip(before) {
            assert_eq!(
                serde_json::to_value(PreviewLayout::from(previews.get(*id).unwrap())).unwrap(),
                original
            );
        }
    }

    #[test]
    fn leaving_spotlight_keeps_and_saves_the_last_main_tile() {
        for pinned in [false, true] {
            for member_only in [false, true] {
                let (mut canvas, mut previews, ids, viewport) = fixture();
                if pinned {
                    canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
                }
                if member_only {
                    canvas.groups.editing = Some(ids[0]);
                    canvas.selection = vec![ids[0]];
                }
                previews.get_mut(ids[2]).unwrap().crop_uv = Some((0.1, 0.2, 0.9, 0.8));
                canvas.apply_spotlight_layout(1, Some(ids[0]), viewport, &mut previews);
                canvas.toggle_spotlight(ids[0], viewport, &mut previews);
                canvas.swap_spotlight_tiles(ids[2], ids[0], viewport, &mut previews);
                let saved_order = canvas.groups.spotlight.as_ref().unwrap().order.clone();
                let expected: Vec<_> = ids
                    .iter()
                    .map(|id| {
                        serde_json::to_value(PreviewLayout::from(previews.get(*id).unwrap()))
                            .unwrap()
                    })
                    .collect();
                let camera = (canvas.pan, canvas.zoom);
                if !pinned {
                    canvas.toggle_current_tile_focus(&previews);
                }
                // The same toggle used to enter Spotlight must keep its final arrangement.
                canvas.toggle_spotlight(ids[2], viewport, &mut previews);
                assert!(!canvas.is_spotlighting());
                assert!(!canvas.is_focusing_tile());
                assert_eq!((canvas.pan, canvas.zoom), camera);
                assert_eq!(canvas.current_tile(&previews), Some(ids[2]));
                assert!(canvas.has_selected_group(&previews));
                for (id, mut expected) in ids.iter().zip(expected) {
                    expected["tile_group"]["order"] = serde_json::json!(saved_order
                        .iter()
                        .position(|member| member == id)
                        .unwrap());
                    let p = previews.get(*id).unwrap();
                    assert_eq!(
                        serde_json::to_value(PreviewLayout::from(p)).unwrap(),
                        expected
                    );
                    let saved = canvas.saved_tile_layout(p);
                    assert_eq!(serde_json::to_value(&saved).unwrap(), expected);
                    let restored: PreviewLayout = serde_json::from_value(expected).unwrap();
                    assert_eq!(restored.position, saved.position);
                    assert_eq!(restored.size, saved.size);
                    assert_eq!(restored.viewport_pin, saved.viewport_pin);
                }
                let main = canvas.preview_screen_rect(previews.get(ids[2]).unwrap(), viewport);
                let companion = canvas.preview_screen_rect(previews.get(ids[0]).unwrap(), viewport);
                assert!(main.width() > companion.width() * 2.0);
                canvas.toggle_spotlight(
                    canvas.current_tile(&previews).unwrap(),
                    viewport,
                    &mut previews,
                );
                assert_eq!(canvas.selection, vec![ids[2]]);
            }
        }
    }

    #[test]
    fn removing_spotlight_member_restores_geometry_before_undo_snapshot() {
        let (mut canvas, mut previews, ids, viewport) = fixture();
        let original = previews.get(ids[0]).unwrap().rect();
        canvas.toggle_spotlight(ids[0], viewport, &mut previews);
        previews.start_removal(ids[0]);
        canvas.update_spotlight(viewport, &mut previews);
        assert!(!canvas.is_spotlighting());
        assert_eq!(previews.get(ids[0]).unwrap().rect(), original);
        assert!(previews.get(ids[0]).unwrap().viewport_pin.is_none());
    }

    #[test]
    fn spotlight_stays_inside_group_bounds_in_canvas_and_pinned_space() {
        for pinned in [false, true] {
            let (mut canvas, mut previews, ids, viewport) = fixture();
            if pinned {
                canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
            }
            let bounds = bounds_of(
                ids.iter()
                    .map(|id| canvas.preview_screen_rect(previews.get(*id).unwrap(), viewport)),
            )
            .unwrap();
            let camera = (canvas.pan, canvas.zoom);
            let outsider =
                previews.add("Outside".into(), Pos2::new(900.0, 10.0), Vec2::splat(100.0));
            let outside_before = previews.get(outsider).unwrap().rect();
            canvas.toggle_spotlight(ids[2], viewport, &mut previews);
            for id in &ids {
                let p = previews.get(*id).unwrap();
                assert_eq!(p.viewport_pin.is_some(), pinned);
                assert!(bounds
                    .expand(0.001)
                    .contains_rect(canvas.preview_screen_rect(p, viewport)));
            }
            assert_eq!((canvas.pan, canvas.zoom), camera);
            assert_eq!(previews.get(outsider).unwrap().rect(), outside_before);
            assert_eq!(canvas.current_tile(&previews), Some(ids[2]));
        }
    }

    #[test]
    fn zoom_and_pan_preserve_spotlight_member_geometry_and_active_tile() {
        let (mut canvas, mut previews, ids, _) = fixture();
        let ctx = egui::Context::default();
        let mut captures = CaptureCoordinator::new();
        frame(&ctx, &mut canvas, &mut previews, &mut captures, vec![]);
        let viewport = canvas.last_screen_rect.unwrap();
        canvas.select_group_tile(ids[2], &previews);
        canvas.toggle_spotlight(ids[2], viewport, &mut previews);
        let before: Vec<_> = ids
            .iter()
            .map(|id| previews.get(*id).unwrap().rect())
            .collect();
        let original_zoom = canvas.zoom;
        frame(
            &ctx,
            &mut canvas,
            &mut previews,
            &mut captures,
            vec![
                egui::Event::PointerMoved(Pos2::new(1100.0, 700.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: Vec2::new(0.0, 50.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert!(canvas.zoom > original_zoom);
        assert!(canvas.is_spotlighting());
        assert_eq!(canvas.current_tile(&previews), Some(ids[2]));
        let start = Pos2::new(1100.0, 700.0);
        frame(
            &ctx,
            &mut canvas,
            &mut previews,
            &mut captures,
            vec![egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Middle,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        frame(
            &ctx,
            &mut canvas,
            &mut previews,
            &mut captures,
            vec![egui::Event::PointerMoved(start + Vec2::new(40.0, 30.0))],
        );
        frame(
            &ctx,
            &mut canvas,
            &mut previews,
            &mut captures,
            vec![egui::Event::PointerButton {
                pos: start + Vec2::new(40.0, 30.0),
                button: egui::PointerButton::Middle,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(canvas.is_spotlighting());
        for (id, before) in ids.iter().zip(before) {
            assert_eq!(previews.get(*id).unwrap().rect(), before);
        }
        let navigated = (canvas.pan, canvas.zoom);
        canvas.exit_spotlight(&mut previews);
        assert_eq!((canvas.pan, canvas.zoom), navigated);
        assert_eq!(canvas.current_tile(&previews), Some(ids[2]));
    }

    #[test]
    fn focus_uses_clicked_group_member_and_returns_to_spotlight_before_exiting_it() {
        let (mut canvas, mut previews, ids, viewport) = fixture();
        canvas.select_group_tile(ids[2], &previews);
        assert_eq!(canvas.selection, ids);
        assert_eq!(canvas.current_tile(&previews), Some(ids[2]));
        canvas.toggle_current_tile_focus(&previews);
        assert_eq!(canvas.focus.as_ref().unwrap().id, ids[2]);
        canvas.toggle_current_tile_focus(&previews);
        assert_eq!(canvas.current_tile(&previews), Some(ids[2]));
        canvas.toggle_spotlight(ids[2], viewport, &mut previews);
        let before: Vec<_> = ids
            .iter()
            .map(|id| previews.get(*id).unwrap().rect())
            .collect();
        let camera = (canvas.pan, canvas.zoom);
        canvas.toggle_current_tile_focus(&previews);
        let focused = canvas.preview_screen_rect(previews.get(ids[2]).unwrap(), viewport);
        assert!(focused.center().distance(viewport.center()) < 0.001);
        assert!(
            (focused.width() - viewport.width()).abs() < 0.001
                || (focused.height() - viewport.height()).abs() < 0.001
        );
        assert!(canvas.is_spotlighting());
        canvas.set_keyboard_input(CanvasKeyboardInput {
            exit_tile_focus: true,
            ..Default::default()
        });
        frame(
            &egui::Context::default(),
            &mut canvas,
            &mut previews,
            &mut CaptureCoordinator::new(),
            vec![],
        );
        assert!(!canvas.is_focusing_tile());
        assert!(canvas.is_spotlighting());
        assert_eq!((canvas.pan, canvas.zoom), camera);
        assert_eq!(canvas.current_tile(&previews), Some(ids[2]));
        for (id, before) in ids.iter().zip(before) {
            assert_eq!(previews.get(*id).unwrap().rect(), before);
        }
        canvas.set_keyboard_input(CanvasKeyboardInput {
            exit_tile_focus: true,
            ..Default::default()
        });
        frame(
            &egui::Context::default(),
            &mut canvas,
            &mut previews,
            &mut CaptureCoordinator::new(),
            vec![],
        );
        assert!(!canvas.is_spotlighting());
    }

    #[test]
    fn small_group_member_focus_fills_view_beyond_navigation_zoom_limit() {
        let (mut canvas, mut previews, ids, viewport) = fixture();
        previews.get_mut(ids[3]).unwrap().size = Vec2::new(40.0, 25.0);
        canvas.select_group_tile(ids[3], &previews);
        let camera = (canvas.pan, canvas.zoom);
        canvas.toggle_current_tile_focus(&previews);
        assert!(canvas.zoom > canvas.zoom_max);
        let focused = canvas.preview_screen_rect(previews.get(ids[3]).unwrap(), viewport);
        assert!((focused.width() - viewport.width()).abs() < 0.001);
        canvas.toggle_current_tile_focus(&previews);
        assert_eq!((canvas.pan, canvas.zoom), camera);
    }

    #[test]
    fn spotlight_delete_keeps_the_current_target_instead_of_deleting_the_group() {
        let (mut canvas, mut previews, ids, viewport) = fixture();
        let original = previews.get(ids[2]).unwrap().rect();
        canvas.toggle_spotlight(ids[2], viewport, &mut previews);
        canvas.set_keyboard_input(CanvasKeyboardInput {
            delete_selected: true,
            ..Default::default()
        });
        frame(
            &egui::Context::default(),
            &mut canvas,
            &mut previews,
            &mut CaptureCoordinator::new(),
            vec![],
        );
        assert!(!canvas.is_spotlighting());
        assert_eq!(previews.get(ids[2]).unwrap().rect(), original);
        for (index, id) in ids.iter().enumerate() {
            assert_eq!(previews.get(*id).unwrap().removing.is_some(), index == 2);
        }
    }

    #[test]
    fn regrouping_preserves_name_and_adding_members_preserves_one_identity() {
        let (mut canvas, mut previews, ids, viewport) = fixture();
        let mut group = previews.get(ids[0]).unwrap().tile_group.clone().unwrap();
        group.name = "Streams".into();
        canvas.assign_group(&ids, group.clone(), viewport, &mut previews);
        canvas.create_group(viewport, &mut previews);
        assert_eq!(
            previews
                .get(ids[0])
                .unwrap()
                .tile_group
                .as_ref()
                .unwrap()
                .name,
            "Streams"
        );
        let added = previews.add("Extra".into(), Pos2::new(20.0, 500.0), Vec2::splat(100.0));
        let mut members = ids.clone();
        members.push(added);
        canvas.assign_group(&members, group.clone(), viewport, &mut previews);
        assert_eq!(canvas.group_members(added, &previews), members);
        assert_eq!(
            previews.get(added).unwrap().tile_group.as_ref().unwrap().id,
            group.id
        );
        canvas.groups.editing = Some(added);
        canvas.selection = vec![added];
        canvas.ungroup_selection(&mut previews);
        assert!(members
            .iter()
            .all(|id| previews.get(*id).unwrap().tile_group.is_none()));
    }

    #[test]
    fn arrangements_leave_crop_and_unselected_tiles_unchanged() {
        let (mut canvas, mut previews, ids, viewport) = fixture();
        previews.get_mut(ids[0]).unwrap().crop_uv = Some((0.25, 0.0, 0.75, 1.0));
        let outsider = previews.add("Outside".into(), Pos2::new(-100.0, 20.0), Vec2::splat(30.0));
        let before = previews.get(outsider).unwrap().rect();
        for (layout, _, _) in Arrangement::ALL {
            canvas.arrange_selection(layout, Some(ids[0]), viewport, &mut previews);
            assert_eq!(
                previews.get(ids[0]).unwrap().crop_uv,
                Some((0.25, 0.0, 0.75, 1.0))
            );
            assert_eq!(previews.get(outsider).unwrap().rect(), before);
        }
    }

    #[test]
    fn legacy_tiles_default_to_ungrouped_and_membership_round_trips() {
        let (_, previews, ids, _) = fixture();
        let saved = PreviewLayout::from(previews.get(ids[0]).unwrap());
        let mut value = serde_json::to_value(&saved).unwrap();
        assert_eq!(
            serde_json::from_value::<PreviewLayout>(value.clone())
                .unwrap()
                .tile_group,
            saved.tile_group
        );
        value.as_object_mut().unwrap().remove("tile_group");
        assert!(serde_json::from_value::<PreviewLayout>(value)
            .unwrap()
            .tile_group
            .is_none());
    }

    fn frame(
        context: &egui::Context,
        canvas: &mut CanvasState,
        previews: &mut PreviewManager,
        captures: &mut CaptureCoordinator,
        events: Vec<egui::Event>,
    ) {
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::none())
                    .show(ctx, |ui| {
                        canvas.ui(ui, previews, captures, ctx, true);
                    });
            },
        );
    }

    fn button(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn pointer_drag_moves_all_group_members_with_and_without_pins() {
        for pinned in [false, true] {
            let (mut canvas, mut previews, ids, _) = fixture();
            let context = egui::Context::default();
            let mut captures = CaptureCoordinator::new();
            for id in &ids {
                previews.get_mut(*id).unwrap().created_at = Instant::now() - Duration::from_secs(1);
            }
            frame(&context, &mut canvas, &mut previews, &mut captures, vec![]);
            let viewport = canvas.last_screen_rect.unwrap();
            if pinned {
                canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
            }
            canvas.selection.clear();
            canvas.apply_spotlight_layout(1, Some(ids[0]), viewport, &mut previews);
            canvas.update_layout_animation(10.0);
            assert!(!canvas.is_spotlighting());
            let before: Vec<_> = ids
                .iter()
                .map(|id| canvas.preview_screen_rect(previews.get(*id).unwrap(), viewport))
                .collect();
            let start = before[0].center();
            frame(
                &context,
                &mut canvas,
                &mut previews,
                &mut captures,
                vec![egui::Event::PointerMoved(start)],
            );
            frame(
                &context,
                &mut canvas,
                &mut previews,
                &mut captures,
                vec![button(start, true)],
            );
            for delta in [Vec2::new(30.0, 20.0), Vec2::new(60.0, 40.0)] {
                frame(
                    &context,
                    &mut canvas,
                    &mut previews,
                    &mut captures,
                    vec![egui::Event::PointerMoved(start + delta)],
                );
            }
            frame(
                &context,
                &mut canvas,
                &mut previews,
                &mut captures,
                vec![button(start + Vec2::new(60.0, 40.0), false)],
            );
            for (id, before) in ids.iter().zip(before) {
                let after = canvas.preview_screen_rect(previews.get(*id).unwrap(), viewport);
                assert!(
                    ((after.min - before.min) - Vec2::new(60.0, 40.0)).length() < 0.01,
                    "pinned={pinned}: before={before:?} after={after:?}"
                );
            }
            assert!(!canvas.is_spotlighting());
        }
    }

    #[test]
    fn pointer_group_resize_preserves_relative_geometry() {
        for pinned in [false, true] {
            let (mut canvas, mut previews, ids, _) = fixture();
            let context = egui::Context::default();
            let mut captures = CaptureCoordinator::new();
            frame(&context, &mut canvas, &mut previews, &mut captures, vec![]);
            let viewport = canvas.last_screen_rect.unwrap();
            if pinned {
                canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
            }
            let before: Vec<_> = ids
                .iter()
                .map(|id| canvas.preview_screen_rect(previews.get(*id).unwrap(), viewport))
                .collect();
            let bounds = bounds_of(before.iter().copied()).unwrap();
            frame(
                &context,
                &mut canvas,
                &mut previews,
                &mut captures,
                vec![egui::Event::PointerMoved(bounds.max)],
            );
            frame(
                &context,
                &mut canvas,
                &mut previews,
                &mut captures,
                vec![button(bounds.max, true)],
            );
            let end = bounds.max + bounds.size() * 0.3;
            frame(
                &context,
                &mut canvas,
                &mut previews,
                &mut captures,
                vec![egui::Event::PointerMoved(end)],
            );
            frame(
                &context,
                &mut canvas,
                &mut previews,
                &mut captures,
                vec![button(end, false)],
            );
            for (id, before) in ids.iter().zip(before) {
                let after = canvas.preview_screen_rect(previews.get(*id).unwrap(), viewport);
                assert!(
                    (after.size() - before.size() * 1.3).length() < 0.01,
                    "pinned={pinned}: {before:?} -> {after:?}"
                );
                assert!(
                    ((after.min - bounds.min) - (before.min - bounds.min) * 1.3).length() < 0.01
                );
            }
        }
    }

    #[test]
    fn size_presets_use_canvas_units_or_pinned_screen_units() {
        for pinned in [false, true] {
            let (mut canvas, mut previews, ids, viewport) = fixture();
            if pinned {
                canvas.toggle_viewport_pin(ids[0], viewport, &mut previews);
            }
            for (width, _, _) in SIZES {
                canvas.resize_selection(width, viewport, &mut previews);
                let bounds =
                    bounds_of(ids.iter().map(|id| {
                        canvas.preview_screen_rect(previews.get(*id).unwrap(), viewport)
                    }))
                    .unwrap();
                assert!(
                    (bounds.width() - width * if pinned { 1.0 } else { canvas.zoom }).abs() < 0.01
                );
            }
        }
    }
}
