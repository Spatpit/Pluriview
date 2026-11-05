use std::collections::hash_map::Entry;
use std::hash::{DefaultHasher, Hash, Hasher};

use super::*;
use crate::hotkeys::HotkeyPresses;
use crate::preview::{GroupArrangement, GroupGap, TileGroup, ViewportAnchor};

#[derive(Clone, Default)]
pub(super) struct GroupState {
    pub(super) resize: Option<GroupResize>,
    editing: Option<PreviewId>,
    active_tile: Option<PreviewId>,
    pub(super) reorder: Option<ReorderDrag>,
    motions: Vec<TileMotion>,
    animation_time: Option<f64>,
    error: Option<String>,
    /// Source shapes the layouts use, once a change has held for a moment.
    aspects: HashMap<PreviewId, SettledAspect>,
    /// A hash of every arranged group's members as last laid out, so an
    /// unchanged group costs one hash per member per frame.
    signatures: HashMap<u64, u64>,
    scratch: HashMap<u64, u64>,
    changed: Vec<u64>,
    /// Each arranged group's last box, which settles a tie between members
    /// that disagree about it.
    frames: HashMap<u64, (bool, Rect)>,
    /// When a source shape that is still changing should be looked at again.
    aspect_wake: Option<f64>,
    /// A group name being edited in the group bar.
    renaming: Option<(u64, String)>,
}

#[derive(Clone, Copy)]
struct SettledAspect {
    value: f32,
    pending: Option<(f32, f64)>,
}

#[derive(Clone)]
pub(super) struct ReorderDrag {
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

const PRESETS: [(GroupArrangement, HotkeySlot, &str); 4] = [
    (
        GroupArrangement::Main(1),
        HotkeySlot::LayoutOneLarge,
        "One Large Tile",
    ),
    (
        GroupArrangement::Main(2),
        HotkeySlot::LayoutTwoLarge,
        "Two Large Tiles",
    ),
    (
        GroupArrangement::Main(3),
        HotkeySlot::LayoutThreeLarge,
        "Three Large Tiles",
    ),
    (
        GroupArrangement::Equal,
        HotkeySlot::LayoutEqual,
        "Equal Tiles",
    ),
];

const SIZES: [(f32, HotkeySlot, &str); 2] = [
    (960.0, HotkeySlot::SizeMedium, "Medium · 960 wide"),
    (1440.0, HotkeySlot::SizeLarge, "Large · 1440 wide"),
];

/// A source has to keep a new shape this long before its group re-flows, so
/// a window being dragged larger or briefly reporting odd sizes does not
/// shuffle the group on every frame.
const ASPECT_SETTLE: f64 = 0.6;
/// Shape changes smaller than this fraction are ignored.
const ASPECT_TOLERANCE: f32 = 0.005;
/// The tiles beside or below the large ones never get more than this share of
/// a large tile's area...
const COMPANION_MAX_AREA: f32 = 0.1;
/// ...and wrap into more columns or rows rather than shrink below this
/// fraction of a large tile's size.
const COMPANION_MIN_SCALE: f32 = 0.2;

fn gap_fraction(gap: GroupGap) -> f32 {
    match gap {
        GroupGap::None => 0.0,
        GroupGap::Small => 0.008,
        GroupGap::Large => 0.022,
    }
}

#[derive(Clone)]
pub(super) struct GroupResize {
    group: u64,
    handle: ResizeHandle,
    /// On screen at the start: the box of an arranged group, or the outline
    /// of a free group's tiles.
    bounds: Rect,
    pointer: Pos2,
    /// Free groups scale their tiles together...
    members: Vec<(PreviewId, Rect)>,
    min_scale: f32,
    /// ...while arranged groups lay out again inside the dragged box, never
    /// leaving a tile smaller than this (in tile units).
    arranged: bool,
    smallest: f32,
}

fn bounds_of(rects: impl Iterator<Item = Rect>) -> Option<Rect> {
    rects.reduce(|a, b| a.union(b))
}

/// Lay tiles of the given shapes out inside `frame`, in member order. Tiles
/// keep their shapes and only `gap` separates them; any room the shapes
/// cannot use is shared evenly around the arrangement.
fn layout_rects(
    aspects: &[f32],
    frame: Rect,
    arrangement: GroupArrangement,
    gap: GroupGap,
) -> Vec<Rect> {
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
    let size = frame.size().max(Vec2::splat(1.0));
    // Proportional to the box, so scaling a group scales its gaps, and a
    // member joining or leaving does not change them.
    let gap =
        (gap_fraction(gap) * (size.x * size.y).sqrt()).min(size.min_elem() / (count as f32 * 4.0));
    let rects = match arrangement {
        GroupArrangement::Main(large) if (large.max(1) as usize) < count => {
            main_rects(&aspects, size, large.max(1) as usize, gap)
        }
        _ => equal_rects(&aspects, size, gap),
    };
    let packed = bounds_of(rects.iter().copied()).unwrap_or(Rect::ZERO);
    let offset = frame.center() - packed.center();
    rects
        .into_iter()
        .map(|rect| rect.translate(offset))
        .collect()
}

/// Equal visible area gives portrait and landscape sources the same weight
/// without stretching either. The column count that lets the tiles grow the
/// most wins, so a wide box becomes one row instead of a square grid. Rows
/// are centered, and tiles are centered within their row.
fn equal_rects(aspects: &[f32], size: Vec2, gap: f32) -> Vec<Rect> {
    let row_width = |row: &[f32], scale: f32| {
        row.iter().map(|a| a.sqrt() * scale).sum::<f32>() + gap * (row.len() - 1) as f32
    };
    let row_height =
        |row: &[f32], scale: f32| row.iter().map(|a| scale / a.sqrt()).fold(0.0_f32, f32::max);
    let fit = |cols: usize| {
        let mut scale = f32::INFINITY;
        let mut height = 0.0;
        let mut rows = 0;
        for row in aspects.chunks(cols) {
            let tiles = row.iter().map(|a| a.sqrt()).sum::<f32>();
            scale = scale.min((size.x - gap * (row.len() - 1) as f32) / tiles);
            height += row_height(row, 1.0);
            rows += 1;
        }
        scale.min((size.y - gap * (rows - 1) as f32) / height)
    };
    let (cols, scale) = (1..=aspects.len())
        .map(|cols| (cols, fit(cols)))
        .reduce(|best, next| if next.1 > best.1 * 1.0001 { next } else { best })
        .unwrap();
    let scale = scale.max(0.0);
    let width = aspects
        .chunks(cols)
        .map(|row| row_width(row, scale))
        .fold(0.0_f32, f32::max);
    let mut rects = Vec::with_capacity(aspects.len());
    let mut y = 0.0;
    for row in aspects.chunks(cols) {
        let height = row_height(row, scale);
        let mut x = (width - row_width(row, scale)) / 2.0;
        for aspect in row {
            let tile = Vec2::new(aspect.sqrt() * scale, scale / aspect.sqrt());
            rects.push(Rect::from_min_size(
                Pos2::new(x, y + (height - tile.y) / 2.0),
                tile,
            ));
            x += tile.x + gap;
        }
        y += height + gap;
    }
    rects
}

/// Where the tiles that are not large go.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Strip {
    /// Up to this many columns beside the large tiles.
    Side(usize),
    /// Up to this many rows below them.
    Below(usize),
}

/// The large tiles share one row. The others wrap into one to three columns
/// beside it or rows below it, whichever leaves the large tiles biggest while
/// keeping the others at least `COMPANION_MIN_SCALE` of their size.
fn main_rects(aspects: &[f32], size: Vec2, large: usize, gap: f32) -> Vec<Rect> {
    let companions = aspects.len() - large;
    let mut best: Option<(Strip, f32, bool, f32)> = None;
    for lines in 1..=companions.min(3) {
        for strip in [Strip::Side(lines), Strip::Below(lines)] {
            let height = main_fit_height(aspects, size, large, gap, strip);
            let scale = companion_scale(aspects, large, gap, strip, height);
            let readable = scale >= COMPANION_MIN_SCALE - 1e-4;
            let better = best.is_none_or(|(_, best_height, best_readable, best_scale)| {
                match (readable, best_readable) {
                    (true, false) => true,
                    (false, true) => false,
                    (true, true) => height > best_height * 1.0005,
                    (false, false) => scale > best_scale * 1.0005,
                }
            });
            if better {
                best = Some((strip, height, readable, scale));
            }
        }
    }
    let (strip, height, _, _) = best.expect("a main layout has companions");
    main_layout(aspects, large, gap, strip, height)
}

/// The width (beside) or height (below) every companion shares when the
/// large tiles are `height` tall.
fn companion_extent(aspects: &[f32], large: usize, gap: f32, strip: Strip, height: f32) -> f32 {
    let (big, small) = aspects.split_at(large);
    let big_width = height * big.iter().sum::<f32>() + gap * (large - 1) as f32;
    let big_area = height * height * big.iter().copied().fold(f32::INFINITY, f32::min);
    match strip {
        Strip::Side(cols) => {
            let per_column = small.len().div_ceil(cols);
            let fit = small
                .chunks(per_column)
                .map(|column| {
                    (height - gap * (column.len() - 1) as f32)
                        / column.iter().map(|a| a.recip()).sum::<f32>()
                })
                .fold(f32::INFINITY, f32::min);
            let narrowest = small.iter().copied().fold(f32::INFINITY, f32::min);
            fit.min((COMPANION_MAX_AREA * big_area * narrowest).sqrt())
                .max(0.0)
        }
        Strip::Below(rows) => {
            let per_row = small.len().div_ceil(rows);
            let fit = small
                .chunks(per_row)
                .map(|row| (big_width - gap * (row.len() - 1) as f32) / row.iter().sum::<f32>())
                .fold(f32::INFINITY, f32::min);
            let widest = small.iter().copied().fold(0.0_f32, f32::max);
            fit.min((COMPANION_MAX_AREA * big_area / widest).sqrt())
                .max(0.0)
        }
    }
}

fn main_extent(aspects: &[f32], large: usize, gap: f32, strip: Strip, height: f32) -> Vec2 {
    let small = aspects.len() - large;
    let big_width = height * aspects[..large].iter().sum::<f32>() + gap * (large - 1) as f32;
    let shared = companion_extent(aspects, large, gap, strip, height);
    match strip {
        Strip::Side(cols) => {
            let used = small.div_ceil(small.div_ceil(cols));
            Vec2::new(big_width + used as f32 * (shared + gap), height)
        }
        Strip::Below(rows) => {
            let used = small.div_ceil(small.div_ceil(rows));
            Vec2::new(big_width, height + used as f32 * (shared + gap))
        }
    }
}

/// The tallest large row whose whole arrangement fits `size`.
fn main_fit_height(aspects: &[f32], size: Vec2, large: usize, gap: f32, strip: Strip) -> f32 {
    let fits = |height: f32| {
        let extent = main_extent(aspects, large, gap, strip, height);
        extent.x <= size.x * 1.00001 && extent.y <= size.y * 1.00001
    };
    let mut high = size
        .y
        .min((size.x - gap * (large - 1) as f32) / aspects[..large].iter().sum::<f32>())
        .max(0.0);
    if fits(high) {
        return high;
    }
    let mut low = 0.0;
    for _ in 0..32 {
        let middle = (low + high) * 0.5;
        if fits(middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}

/// How big the smallest companion is next to the smallest large tile.
fn companion_scale(aspects: &[f32], large: usize, gap: f32, strip: Strip, height: f32) -> f32 {
    let (big, small) = aspects.split_at(large);
    let shared = companion_extent(aspects, large, gap, strip, height);
    let smallest = small
        .iter()
        .map(|a| match strip {
            Strip::Side(_) => shared * shared / a,
            Strip::Below(_) => shared * shared * a,
        })
        .fold(f32::INFINITY, f32::min);
    let big_area = height * height * big.iter().copied().fold(f32::INFINITY, f32::min);
    if big_area > 0.0 {
        (smallest / big_area).sqrt()
    } else {
        0.0
    }
}

fn main_layout(aspects: &[f32], large: usize, gap: f32, strip: Strip, height: f32) -> Vec<Rect> {
    let (big, small) = aspects.split_at(large);
    let big_width = height * big.iter().sum::<f32>() + gap * (large - 1) as f32;
    let shared = companion_extent(aspects, large, gap, strip, height);
    let mut rects = Vec::with_capacity(aspects.len());
    let mut x = 0.0;
    for aspect in big {
        rects.push(Rect::from_min_size(
            Pos2::new(x, 0.0),
            Vec2::new(height * aspect, height),
        ));
        x += height * aspect + gap;
    }
    match strip {
        Strip::Side(cols) => {
            let left = big_width + gap;
            for (index, column) in small.chunks(small.len().div_ceil(cols)).enumerate() {
                let column_height = column.iter().map(|a| shared / a).sum::<f32>()
                    + gap * (column.len() - 1) as f32;
                let mut y = (height - column_height) / 2.0;
                for aspect in column {
                    rects.push(Rect::from_min_size(
                        Pos2::new(left + index as f32 * (shared + gap), y),
                        Vec2::new(shared, shared / aspect),
                    ));
                    y += shared / aspect + gap;
                }
            }
        }
        Strip::Below(rows) => {
            let mut y = height + gap;
            for row in small.chunks(small.len().div_ceil(rows)) {
                let row_width =
                    row.iter().map(|a| shared * a).sum::<f32>() + gap * (row.len() - 1) as f32;
                let mut x = (big_width - row_width) / 2.0;
                for aspect in row {
                    rects.push(Rect::from_min_size(
                        Pos2::new(x, y),
                        Vec2::new(shared * aspect, shared),
                    ));
                    x += shared * aspect + gap;
                }
                y += shared + gap;
            }
        }
    }
    rects
}

/// The box a group gets when it starts being arranged. The arrangement keeps
/// the tiles' total visible area and the group's top-left corner; fitting it
/// into the old outline instead would shrink tiles that were spread apart.
fn initial_frame(
    current: &[Rect],
    aspects: &[f32],
    arrangement: GroupArrangement,
    gap: GroupGap,
) -> Option<Rect> {
    let outline = bounds_of(current.iter().copied())?;
    let trial = layout_rects(aspects, outline, arrangement, gap);
    let content = bounds_of(trial.iter().copied())?;
    let area = current.iter().map(Rect::area).sum::<f32>();
    let arranged = trial.iter().map(Rect::area).sum::<f32>();
    let scale = if area > 0.0 && arranged > 0.0 {
        (area / arranged).sqrt()
    } else {
        1.0
    };
    Some(Rect::from_min_size(outline.min, content.size() * scale))
}

/// Smallest side, in tile units, that dragging a group's resize handle can
/// leave on any member. Matches the single-tile resize limit.
const MIN_TILE_SIDE: f32 = 100.0;

/// A group that is already smaller than the limit can grow but not shrink.
fn group_resize_min_scale(members: &[(PreviewId, Rect)], screen_per_unit: f32) -> f32 {
    let smallest = members
        .iter()
        .map(|(_, r)| r.width().min(r.height()))
        .fold(f32::INFINITY, f32::min)
        / screen_per_unit;
    if smallest.is_finite() && smallest > 0.0 {
        (MIN_TILE_SIDE / smallest).min(1.0)
    } else {
        1.0
    }
}

fn smallest_side(rects: &[Rect]) -> f32 {
    rects
        .iter()
        .map(|r| r.width().min(r.height()))
        .fold(f32::INFINITY, f32::min)
}

/// Slide a rearranged group back toward the window when its new shape sticks
/// out past the window further than the old shape did. A group that was
/// already partly off-screen keeps that placement instead of jumping.
fn keep_in_view(rects: Vec<Rect>, previous: Rect, viewport: Rect) -> Vec<Rect> {
    let Some(bounds) = bounds_of(rects.iter().copied()) else {
        return rects;
    };
    let allowed = viewport.union(previous);
    let back = |min: f32, max: f32, low: f32, high: f32| {
        if max > high {
            (high - max).max(low - min).min(0.0)
        } else {
            0.0
        }
    };
    let shift = Vec2::new(
        back(
            bounds.left(),
            bounds.right(),
            allowed.left(),
            allowed.right(),
        ),
        back(
            bounds.top(),
            bounds.bottom(),
            allowed.top(),
            allowed.bottom(),
        ),
    );
    rects.into_iter().map(|r| r.translate(shift)).collect()
}

/// The shape a tile's picture needs to show without distortion. Window,
/// screen, image and Spout frames are drawn across the whole tile, so a tile
/// resized to another shape (or whose source window changed shape) stretches
/// them. Browsers render at the tile's own size and videos are drawn by their
/// player, so their tile shape is already right and they report `None`.
fn content_aspect(p: &Preview) -> Option<f32> {
    if p.is_browser() || p.is_video() || p.is_playlist() {
        return None;
    }
    let (width, height) = p.source_frame_size.or(p.frame_size)?;
    let (u0, v0, u1, v1) = p.crop_uv.unwrap_or((0.0, 0.0, 1.0, 1.0));
    let aspect = ((u1 - u0) * width as f32) / ((v1 - v0) * height as f32);
    (aspect.is_finite() && aspect > 0.0).then_some(aspect)
}

/// The shape a member is laid out at. A source shape counts once it has held
/// for `ASPECT_SETTLE` seconds; `wake` collects when to look again.
fn settled_aspect(
    aspects: &mut HashMap<PreviewId, SettledAspect>,
    p: &Preview,
    rect: Rect,
    now: f64,
    wake: &mut Option<f64>,
) -> f32 {
    let Some(raw) = content_aspect(p) else {
        aspects.remove(&p.id);
        return rect.aspect_ratio();
    };
    let near = |a: f32, b: f32| (a / b - 1.0).abs() <= ASPECT_TOLERANCE;
    let mut later = |at: f64| *wake = Some(wake.map_or(at, |wake| wake.min(at)));
    match aspects.entry(p.id) {
        Entry::Vacant(entry) => {
            entry.insert(SettledAspect {
                value: raw,
                pending: None,
            });
            raw
        }
        Entry::Occupied(mut entry) => {
            let settled = entry.get_mut();
            if near(raw, settled.value) {
                settled.pending = None;
            } else {
                match settled.pending {
                    Some((shape, since)) if near(shape, raw) => {
                        if now - since >= ASPECT_SETTLE {
                            settled.value = raw;
                            settled.pending = None;
                        } else {
                            later(since + ASPECT_SETTLE);
                        }
                    }
                    _ => {
                        settled.pending = Some((raw, now));
                        later(now + ASPECT_SETTLE);
                    }
                }
            }
            settled.value
        }
    }
}

/// A member's rect in its group's layout space: canvas units, or points from
/// the viewport's top-left corner for a pinned group.
fn layout_rect(p: &Preview, viewport: Rect) -> Rect {
    match p.viewport_pin {
        Some(pin) => pin.rect(viewport).translate(-viewport.min.to_vec2()),
        None => p.rect(),
    }
}

fn set_layout_rect(p: &mut Preview, rect: Rect, viewport: Rect) {
    if p.viewport_pin.is_some() {
        p.viewport_pin = Some(group_pin(rect.translate(viewport.min.to_vec2()), viewport));
    } else {
        p.position = rect.min;
        p.size = rect.size();
    }
}

fn frame_from(member: Rect, relative: [f32; 4]) -> Rect {
    let size = member.size();
    Rect::from_min_size(
        member.min + Vec2::new(relative[0] * size.x, relative[1] * size.y),
        Vec2::new(relative[2] * size.x, relative[3] * size.y),
    )
}

fn relative_frame(frame: Rect, member: Rect) -> Option<[f32; 4]> {
    let size = member.size();
    (size.x > 0.0 && size.y > 0.0).then(|| {
        [
            (frame.min.x - member.min.x) / size.x,
            (frame.min.y - member.min.y) / size.y,
            frame.width() / size.x,
            frame.height() / size.y,
        ]
    })
}

fn rects_close(a: Rect, b: Rect, tolerance: f32) -> bool {
    (a.min - b.min).abs().max_elem() <= tolerance && (a.max - b.max).abs().max_elem() <= tolerance
}

fn frame_tolerance(frame: Rect) -> f32 {
    frame.size().max_elem() * 1e-4 + 1e-3
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

/// An arranged group's members as laid out now.
struct Arranged {
    settings: TileGroup,
    pinned: bool,
    members: Vec<(PreviewId, Rect, Option<[f32; 4]>)>,
    aspects: Vec<f32>,
}

impl Arranged {
    fn rects(&self) -> Vec<Rect> {
        self.members.iter().map(|(_, rect, _)| *rect).collect()
    }
}

impl CanvasState {
    pub(super) fn update_layout_animation(&mut self, now: f64) {
        self.groups.animation_time = Some(now);
        self.groups
            .motions
            .retain(|m| now - m.started < LAYOUT_DURATION);
        if self.interactive_browser.is_some()
            || self.live_window.id.is_some()
            || self.is_focusing_tile()
        {
            self.groups.motions.clear();
            self.groups.reorder = None;
        }
    }

    pub(super) fn layout_is_animating(&self) -> bool {
        !self.groups.motions.is_empty()
    }

    pub(super) fn layout_screen_rect(&self, id: PreviewId, target: Rect, viewport: Rect) -> Rect {
        if self.is_focusing_tile() {
            return target;
        }
        if let Some(drag) = &self.groups.reorder {
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

    fn group_settings(&self, id: PreviewId, previews: &PreviewManager) -> Option<TileGroup> {
        let first = *self.group_members(id, previews).first()?;
        previews.get(first)?.tile_group.clone()
    }

    pub(super) fn is_arranged(&self, id: PreviewId, previews: &PreviewManager) -> bool {
        previews
            .get(id)
            .and_then(|p| p.tile_group.as_ref())
            .is_some_and(|g| g.arrangement != GroupArrangement::Free)
    }

    /// Shift+drag inside an arranged group swaps two tiles' places.
    pub(super) fn reorder_drag_response(
        &mut self,
        id: PreviewId,
        response: &egui::Response,
        viewport: Rect,
        previews: &mut PreviewManager,
        input: &FrameInput,
    ) {
        if self.is_focusing_tile() || self.crop_mode.is_some() {
            return;
        }
        if input.alt || input.middle_down || input.pointer_blocked {
            self.groups.reorder = None;
            return;
        }
        if response.drag_started()
            && input.shift
            && self.groups.resize.is_none()
            && self.is_arranged(id, previews)
            && self.group_members(id, previews).len() > 1
        {
            if let (Some(pointer), Some(p)) = (response.interact_pointer_pos(), previews.get(id)) {
                let origin = self.preview_screen_rect(p, viewport);
                self.groups.reorder = Some(ReorderDrag {
                    id,
                    origin,
                    start: pointer - response.drag_delta(),
                    pointer,
                    target: None,
                });
                if !self.selection.contains(&id) {
                    self.select_group_tile(id, previews);
                }
                self.remember_current_tile(id);
            }
        }
        if self.groups.reorder.as_ref().is_some_and(|d| d.id == id) {
            if let Some(pointer) = response.interact_pointer_pos().or(input.hover_pos) {
                let target = self.reorder_target(id, pointer, viewport, previews);
                let drag = self.groups.reorder.as_mut().unwrap();
                drag.pointer = pointer;
                drag.target = target;
            }
            if response.drag_stopped() {
                self.finish_reorder_drag(
                    response.interact_pointer_pos().or(input.hover_pos),
                    viewport,
                    previews,
                );
            }
        }
    }

    fn reorder_target(
        &self,
        source: PreviewId,
        pointer: Pos2,
        viewport: Rect,
        previews: &PreviewManager,
    ) -> Option<PreviewId> {
        if !viewport.contains(pointer) {
            return None;
        }
        self.group_members(source, previews)
            .into_iter()
            .find(|other| {
                *other != source
                    && previews.get(*other).is_some_and(|p| {
                        !p.left_click_disabled
                            && self.preview_screen_rect(p, viewport).contains(pointer)
                    })
            })
    }

    pub(super) fn draw_reorder_drop_target(
        &self,
        painter: &egui::Painter,
        viewport: Rect,
        previews: &PreviewManager,
    ) {
        if let Some(preview) = self
            .groups
            .reorder
            .as_ref()
            .and_then(|d| d.target)
            .and_then(|id| previews.get(id))
        {
            painter.rect_stroke(
                self.preview_screen_rect(preview, viewport).expand(2.0),
                4.0,
                Stroke::new(3.0, Color32::from_rgb(207, 161, 57)),
                egui::StrokeKind::Middle,
            );
        }
    }

    pub(super) fn update_reorder_pointer(&mut self, pointer: Option<Pos2>, viewport: Rect) {
        if let Some(drag) = &mut self.groups.reorder {
            if let Some(pointer) = pointer {
                drag.pointer = pointer;
                if !viewport.contains(pointer) {
                    drag.target = None;
                }
            }
        }
    }

    pub(super) fn finish_reorder_drag(
        &mut self,
        pointer: Option<Pos2>,
        viewport: Rect,
        previews: &mut PreviewManager,
    ) {
        let Some(source) = self.groups.reorder.as_ref().map(|d| d.id) else {
            return;
        };
        let target =
            pointer.and_then(|pointer| self.reorder_target(source, pointer, viewport, previews));
        if let Some(target) = target {
            self.swap_member_order(source, target, viewport, previews);
        } else if let Some(p) = previews.get(source) {
            let start = self.preview_screen_rect(p, viewport);
            self.groups.reorder = None;
            self.animate_layout_from(vec![(source, start)], viewport, previews);
        }
        self.groups.reorder = None;
    }

    /// Trade two members' places in their group's order. Their tiles move
    /// to each other's slots at their own shapes; nobody else's slot does
    /// unless the shapes need it.
    fn swap_member_order(
        &mut self,
        source: PreviewId,
        target: PreviewId,
        viewport: Rect,
        previews: &mut PreviewManager,
    ) {
        let mut order = self.group_members(source, previews);
        let (Some(a), Some(b)) = (
            order.iter().position(|id| *id == source),
            order.iter().position(|id| *id == target),
        ) else {
            return;
        };
        order.swap(a, b);
        self.set_member_order(&order, previews);
        self.arrange_group_of(source, viewport, previews);
        self.remember_current_tile(source);
    }

    fn set_member_order(&self, order: &[PreviewId], previews: &mut PreviewManager) {
        for (index, id) in order.iter().enumerate() {
            if let Some(group) = previews.get_mut(*id).and_then(|p| p.tile_group.as_mut()) {
                group.order = index as u32;
            }
        }
    }

    pub(super) fn remember_current_tile(&mut self, id: PreviewId) {
        self.groups.active_tile = Some(id);
    }

    pub(super) fn groups_active_tile(&self) -> Option<PreviewId> {
        self.groups.active_tile
    }

    pub(super) fn forget_current_tile(&mut self) {
        self.groups.active_tile = None;
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

    pub(super) fn members_of(group: u64, previews: &PreviewManager) -> Vec<PreviewId> {
        let mut members: Vec<_> = previews
            .all()
            .filter(|p| p.removing.is_none())
            .filter_map(|p| {
                p.tile_group
                    .as_ref()
                    .filter(|g| g.id == group)
                    .map(|g| (g.order, p.id))
            })
            .collect();
        members.sort_by_key(|(order, id)| (*order, id.0));
        members.into_iter().map(|(_, id)| id).collect()
    }

    pub fn group_members(&self, id: PreviewId, previews: &PreviewManager) -> Vec<PreviewId> {
        match previews.get(id).and_then(|p| p.tile_group.as_ref()) {
            Some(group) => Self::members_of(group.id, previews),
            None => vec![id],
        }
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

    /// Where a tile is going, ignoring any layout animation still playing.
    fn target_screen_rect(&self, p: &Preview, viewport: Rect) -> Rect {
        p.viewport_pin
            .map(|pin| pin.rect(viewport))
            .unwrap_or_else(|| self.canvas_rect_to_screen(p.rect(), viewport))
    }

    fn to_layout_space(&self, screen: Rect, pinned: bool, viewport: Rect) -> Rect {
        if pinned {
            screen.translate(-viewport.min.to_vec2())
        } else {
            Rect::from_min_size(
                self.screen_to_canvas(screen.min, viewport),
                screen.size() / self.zoom,
            )
        }
    }

    fn to_screen_space(&self, rect: Rect, pinned: bool, viewport: Rect) -> Rect {
        if pinned {
            rect.translate(viewport.min.to_vec2())
        } else {
            self.canvas_rect_to_screen(rect, viewport)
        }
    }

    /// An arranged group's members with the shapes they are laid out at.
    /// `None` for a free group, or one whose members are not all pinned or
    /// all unpinned.
    fn arranged(
        &mut self,
        group: u64,
        viewport: Rect,
        previews: &PreviewManager,
        now: f64,
    ) -> Option<Arranged> {
        let ids = Self::members_of(group, previews);
        let first = previews.get(*ids.first()?)?;
        let settings = first.tile_group.clone()?;
        let pinned = first.viewport_pin.is_some();
        if settings.arrangement == GroupArrangement::Free {
            return None;
        }
        let mut wake = self.groups.aspect_wake;
        let mut members = Vec::with_capacity(ids.len());
        let mut aspects = Vec::with_capacity(ids.len());
        for p in ids.iter().filter_map(|id| previews.get(*id)) {
            if p.viewport_pin.is_some() != pinned {
                return None;
            }
            let rect = layout_rect(p, viewport);
            aspects.push(settled_aspect(
                &mut self.groups.aspects,
                p,
                rect,
                now,
                &mut wake,
            ));
            members.push((p.id, rect, p.tile_group.as_ref().and_then(|g| g.frame)));
        }
        self.groups.aspect_wake = wake;
        Some(Arranged {
            settings,
            pinned,
            members,
            aspects,
        })
    }

    /// The box the most members agree on. A member resized on its own (a
    /// crop, say) disagrees with the others, and a new member has no say.
    /// A tie goes to the box the group had last, then to the earlier member.
    fn resolve_frame(&self, arranged: &Arranged) -> Option<Rect> {
        let mut votes: Vec<(Rect, usize)> = Vec::new();
        for (_, rect, relative) in &arranged.members {
            let Some(frame) = relative
                .map(|relative| frame_from(*rect, relative))
                .filter(|f| f.is_finite() && f.width() > 0.0 && f.height() > 0.0)
            else {
                continue;
            };
            match votes
                .iter_mut()
                .find(|(other, _)| rects_close(*other, frame, frame_tolerance(frame)))
            {
                Some((_, count)) => *count += 1,
                None => votes.push((frame, 1)),
            }
        }
        let last = self
            .groups
            .frames
            .get(&arranged.settings.id)
            .filter(|(pinned, _)| *pinned == arranged.pinned)
            .map(|(_, frame)| *frame);
        votes
            .into_iter()
            .enumerate()
            .max_by(|(a_index, (a, a_votes)), (b_index, (b, b_votes))| {
                let was = |frame: &Rect| {
                    last.is_some_and(|last| rects_close(last, *frame, frame_tolerance(last)))
                };
                a_votes
                    .cmp(b_votes)
                    .then(was(a).cmp(&was(b)))
                    .then(b_index.cmp(a_index))
            })
            .map(|(_, (frame, _))| frame)
    }

    /// Move an arranged group's members into `frame`, gliding there unless
    /// `animate` is off (while a handle drags the box).
    fn place_group(
        &mut self,
        arranged: &Arranged,
        frame: Rect,
        viewport: Rect,
        previews: &mut PreviewManager,
        animate: bool,
    ) {
        let settings = &arranged.settings;
        if !animate {
            self.groups
                .motions
                .retain(|m| !arranged.members.iter().any(|(id, _, _)| *id == m.id));
        }
        let expected = layout_rects(&arranged.aspects, frame, settings.arrangement, settings.gap);
        let tolerance = frame_tolerance(frame);
        let mut starts = Vec::new();
        for ((id, rect, _), slot) in arranged.members.iter().zip(&expected) {
            let moved = !rects_close(*rect, *slot, tolerance);
            if moved && animate {
                if let Some(p) = previews.get(*id) {
                    starts.push((*id, self.preview_screen_rect(p, viewport)));
                }
            }
            let Some(p) = previews.get_mut(*id) else {
                continue;
            };
            if moved {
                set_layout_rect(p, *slot, viewport);
            }
            let fresh = relative_frame(frame, *slot);
            let Some(group) = p.tile_group.as_mut() else {
                continue;
            };
            let stale = match (group.frame, fresh) {
                (Some(old), Some(new)) => old
                    .iter()
                    .zip(new)
                    .any(|(old, new)| (old - new).abs() > 1e-4 * new.abs().max(1.0)),
                (old, new) => old.is_some() != new.is_some(),
            };
            if moved || stale {
                group.frame = fresh;
            }
            if group.arrangement != settings.arrangement || group.gap != settings.gap {
                group.arrangement = settings.arrangement;
                group.gap = settings.gap;
            }
            if group.name != settings.name {
                group.name.clone_from(&settings.name);
            }
        }
        self.groups
            .frames
            .insert(settings.id, (arranged.pinned, frame));
        if !starts.is_empty() {
            self.animate_layout_from(starts, viewport, previews);
        }
    }

    /// Lay an arranged group out inside its box again. A group without a
    /// box yet gets one that keeps its tiles' total area.
    fn arrange_group(
        &mut self,
        group: u64,
        viewport: Rect,
        previews: &mut PreviewManager,
        now: f64,
    ) {
        let Some(arranged) = self.arranged(group, viewport, previews, now) else {
            return;
        };
        let frame = self.resolve_frame(&arranged).or_else(|| {
            let current = arranged.rects();
            let outline = bounds_of(current.iter().copied())?;
            let frame = initial_frame(
                &current,
                &arranged.aspects,
                arranged.settings.arrangement,
                arranged.settings.gap,
            )?;
            let window = self.to_layout_space(viewport, arranged.pinned, viewport);
            keep_in_view(vec![frame], outline, window).pop()
        });
        if let Some(frame) = frame {
            self.place_group(&arranged, frame, viewport, previews, true);
        }
    }

    fn arrange_group_of(&mut self, id: PreviewId, viewport: Rect, previews: &mut PreviewManager) {
        if let Some(group) = previews.get(id).and_then(|p| p.tile_group.as_ref()) {
            let group = group.id;
            let now = self.groups.animation_time.unwrap_or(0.0);
            self.arrange_group(group, viewport, previews, now);
        }
    }

    fn group_layouts_paused(&self) -> bool {
        self.drag_state.is_some()
            || self.preview_dragging
            || self.groups.resize.is_some()
            || self.groups.reorder.is_some()
            || self.pinned_pointer_drag.is_some()
            || self.crop_mode.is_some()
            || self.is_focusing_tile()
            || self.interactive_browser.is_some()
            || self.live_window.id.is_some()
    }

    /// Keep arranged groups laid out as members join or leave, change order
    /// or their sources change shape. Moving or scaling a whole group keeps
    /// every member's view of the box, so it never triggers a re-flow.
    /// Returns when a source shape that is still changing should be checked.
    pub(super) fn update_group_layouts(
        &mut self,
        viewport: Rect,
        previews: &mut PreviewManager,
        now: f64,
    ) -> Option<f64> {
        if self.group_layouts_paused() {
            return None;
        }
        self.groups.aspect_wake = None;
        let mut wake = None;
        let mut scratch = std::mem::take(&mut self.groups.scratch);
        scratch.clear();
        for p in previews.all() {
            let Some(group) = p
                .tile_group
                .as_ref()
                .filter(|g| g.arrangement != GroupArrangement::Free && p.removing.is_none())
            else {
                continue;
            };
            let rect = layout_rect(p, viewport);
            let aspect = settled_aspect(&mut self.groups.aspects, p, rect, now, &mut wake);
            let mut hasher = DefaultHasher::new();
            (
                p.id.0,
                group.order,
                group.arrangement,
                group.gap,
                p.viewport_pin.is_some(),
            )
                .hash(&mut hasher);
            for value in [rect.min.x, rect.min.y, rect.max.x, rect.max.y, aspect] {
                value.to_bits().hash(&mut hasher);
            }
            group.frame.map(|f| f.map(f32::to_bits)).hash(&mut hasher);
            let sum = scratch.entry(group.id).or_insert(0);
            *sum = sum.wrapping_add(hasher.finish());
        }
        let mut changed = std::mem::take(&mut self.groups.changed);
        changed.clear();
        changed.extend(
            scratch
                .iter()
                .filter(|(group, signature)| self.groups.signatures.get(group) != Some(signature))
                .map(|(group, _)| *group),
        );
        std::mem::swap(&mut self.groups.signatures, &mut scratch);
        self.groups.scratch = scratch;
        for group in &changed {
            self.arrange_group(*group, viewport, previews, now);
        }
        self.groups.changed = changed;
        if self.groups.aspects.len() > self.groups.signatures.len() * 4 + 16 {
            self.groups.aspects.retain(|id, _| {
                previews.get(*id).is_some_and(|p| {
                    p.tile_group
                        .as_ref()
                        .is_some_and(|g| g.arrangement != GroupArrangement::Free)
                })
            });
        }
        let signatures = &self.groups.signatures;
        self.groups
            .frames
            .retain(|group, _| signatures.contains_key(group));
        [wake, self.groups.aspect_wake]
            .into_iter()
            .flatten()
            .reduce(f64::min)
    }

    /// Change how a group arranges its tiles. Going from free to arranged
    /// gives the group a box that keeps its total visible area; switching
    /// between arrangements keeps the box, so switching back restores the
    /// previous layout exactly. With a large-tile arrangement, `target`
    /// takes a large slot.
    pub(super) fn set_group_arrangement(
        &mut self,
        id: PreviewId,
        arrangement: GroupArrangement,
        target: Option<PreviewId>,
        viewport: Rect,
        previews: &mut PreviewManager,
    ) {
        let Some(settings) = self.group_settings(id, previews) else {
            return;
        };
        let mut order = self.group_members(id, previews);
        if order.len() < 2 {
            return;
        }
        self.exit_focus();
        self.finish_crop();
        self.groups.reorder = None;
        if let (GroupArrangement::Main(large), Some(slot)) = (
            arrangement,
            target.and_then(|target| order.iter().position(|member| *member == target)),
        ) {
            let large = (large.max(1) as usize).min(order.len());
            if slot >= large || large == 1 {
                order.swap(large - 1, slot);
            }
        }
        let starting = settings.arrangement == GroupArrangement::Free;
        for (index, member) in order.iter().enumerate() {
            if let Some(group) = previews
                .get_mut(*member)
                .and_then(|p| p.tile_group.as_mut())
            {
                group.order = index as u32;
                group.arrangement = arrangement;
                if starting {
                    group.frame = None;
                }
            }
            // The user asked for a layout now; use the sources' shapes as
            // they are rather than waiting for a change to settle.
            self.groups.aspects.remove(member);
        }
        if arrangement != GroupArrangement::Free {
            self.arrange_group(
                settings.id,
                viewport,
                previews,
                self.groups.animation_time.unwrap_or(0.0),
            );
        }
        if let Some(target) = target.filter(|target| order.contains(target)) {
            if !self.selection.contains(&target) {
                self.select_group_tile(target, previews);
            }
            self.remember_current_tile(target);
        }
    }

    fn set_group_gap(
        &mut self,
        id: PreviewId,
        gap: GroupGap,
        viewport: Rect,
        previews: &mut PreviewManager,
    ) {
        let members = self.group_members(id, previews);
        for member in &members {
            if let Some(group) = previews
                .get_mut(*member)
                .and_then(|p| p.tile_group.as_mut())
            {
                group.gap = gap;
            }
        }
        self.arrange_group_of(id, viewport, previews);
    }

    /// Moving or resizing some members of an arranged group by hand takes
    /// the group out of its arrangement, so the next layout pass keeps the
    /// change instead of undoing it.
    pub(super) fn release_partial_groups(
        &mut self,
        ids: &[PreviewId],
        previews: &mut PreviewManager,
    ) {
        let mut groups: Vec<u64> = ids
            .iter()
            .filter_map(|id| previews.get(*id)?.tile_group.as_ref())
            .filter(|g| g.arrangement != GroupArrangement::Free)
            .map(|g| g.id)
            .collect();
        groups.sort_unstable();
        groups.dedup();
        for group in groups {
            let members = Self::members_of(group, previews);
            if members.iter().all(|member| ids.contains(member)) {
                continue;
            }
            for member in members {
                if let Some(group) = previews.get_mut(member).and_then(|p| p.tile_group.as_mut()) {
                    group.arrangement = GroupArrangement::Free;
                }
            }
        }
    }

    /// Put `id` in its group's large slot. A group without large tiles
    /// switches to one large tile.
    pub(super) fn make_main_tile(
        &mut self,
        id: PreviewId,
        viewport: Rect,
        previews: &mut PreviewManager,
    ) {
        let arrangement = match self.group_settings(id, previews).map(|g| g.arrangement) {
            Some(GroupArrangement::Main(large)) => GroupArrangement::Main(large),
            Some(_) => GroupArrangement::Main(1),
            None => return,
        };
        self.set_group_arrangement(id, arrangement, Some(id), viewport, previews);
    }

    /// With large tiles, the next or previous member becomes the large one.
    /// Otherwise the current tile moves to the next or previous member.
    fn cycle_main_tile(
        &mut self,
        next: bool,
        target: Option<PreviewId>,
        viewport: Rect,
        previews: &mut PreviewManager,
    ) {
        let Some(id) = target else {
            return;
        };
        let mut order = self.group_members(id, previews);
        if order.len() < 2 {
            return;
        }
        self.exit_focus();
        let arrangement = self.group_settings(id, previews).map(|g| g.arrangement);
        let current = if let Some(GroupArrangement::Main(_)) = arrangement {
            if next {
                order.rotate_left(1);
            } else {
                order.rotate_right(1);
            }
            self.set_member_order(&order, previews);
            self.arrange_group_of(id, viewport, previews);
            order[0]
        } else {
            let index = order.iter().position(|member| *member == id).unwrap_or(0);
            let step = if next { 1 } else { order.len() - 1 };
            order[(index + step) % order.len()]
        };
        if !self.selection.contains(&current) {
            self.select_group_tile(current, previews);
        }
        self.remember_current_tile(current);
    }

    /// Scale the whole group to fill the window. An arranged group lays out
    /// again in the window's shape; a free one keeps its arrangement.
    fn fit_group_to_view(&mut self, id: PreviewId, viewport: Rect, previews: &mut PreviewManager) {
        self.exit_focus();
        self.finish_crop();
        let area = viewport.shrink(viewport.width().min(viewport.height()) * 0.04);
        if let Some(group) = previews
            .get(id)
            .and_then(|p| p.tile_group.as_ref())
            .filter(|g| g.arrangement != GroupArrangement::Free)
            .map(|g| g.id)
        {
            let now = self.groups.animation_time.unwrap_or(0.0);
            if let Some(arranged) = self.arranged(group, viewport, previews, now) {
                let frame = self.to_layout_space(area, arranged.pinned, viewport);
                self.place_group(&arranged, frame, viewport, previews, true);
                return;
            }
        }
        let members: Vec<_> = self
            .group_members(id, previews)
            .into_iter()
            .filter_map(|id| previews.get(id).map(|p| (id, p)))
            .map(|(id, p)| {
                (
                    id,
                    self.preview_screen_rect(p, viewport),
                    self.target_screen_rect(p, viewport),
                )
            })
            .collect();
        let Some(bounds) = bounds_of(members.iter().map(|(_, _, r)| *r)) else {
            return;
        };
        let scale =
            (area.width() / bounds.width().max(1.0)).min(area.height() / bounds.height().max(1.0));
        if !(scale.is_finite() && scale > 0.0) {
            return;
        }
        let origin = area.center() - bounds.size() * scale * 0.5;
        for (id, _, rect) in &members {
            self.set_group_screen_rect(
                *id,
                Rect::from_min_size(
                    origin + (rect.min - bounds.min) * scale,
                    rect.size() * scale,
                ),
                viewport,
                previews,
            );
        }
        let starts = members
            .into_iter()
            .map(|(id, from, _)| (id, from))
            .collect();
        self.animate_layout_from(starts, viewport, previews);
    }

    fn assign_group(
        &mut self,
        ids: &[PreviewId],
        group: TileGroup,
        viewport: Rect,
        previews: &mut PreviewManager,
    ) {
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
            self.groups.aspects.remove(id);
            if let Some(p) = previews.get_mut(*id) {
                // A member keeps its view of the box; a new one has none.
                let frame = p
                    .tile_group
                    .as_ref()
                    .filter(|g| g.id == group.id)
                    .and_then(|g| g.frame);
                p.tile_group = Some(TileGroup {
                    order: order as u32,
                    frame,
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

    /// "Group 1", "Group 2"... the first number no other group uses.
    fn next_group_name(previews: &PreviewManager) -> String {
        (1..)
            .map(|n| format!("Group {n}"))
            .find(|name| {
                !previews
                    .all()
                    .any(|p| p.tile_group.as_ref().is_some_and(|g| &g.name == name))
            })
            .unwrap()
    }

    fn create_group(&mut self, viewport: Rect, previews: &mut PreviewManager) {
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
        let name = Self::next_group_name(previews);
        self.assign_group(&ids, TileGroup::new(id, name, 0), viewport, previews);
        self.groups.error = None;
    }

    fn ungroup_selection(&mut self, previews: &mut PreviewManager) {
        self.finish_crop();
        self.groups.editing = None;
        self.expand_group_selection(previews);
        for id in &self.selection {
            if let Some(p) = previews.get_mut(*id) {
                p.tile_group = None;
            }
        }
    }

    fn resize_selection(&mut self, width: f32, viewport: Rect, previews: &mut PreviewManager) {
        self.exit_focus();
        self.finish_crop();
        let selection = self.selection.clone();
        self.release_partial_groups(&selection, previews);
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

    /// Bring a tile's whole group above (or below) everything else, keeping
    /// the members' order among themselves.
    fn restack_group(&mut self, id: PreviewId, front: bool, previews: &mut PreviewManager) {
        let mut members: Vec<_> = self
            .group_members(id, previews)
            .into_iter()
            .filter_map(|id| previews.get(id).map(|p| (p.z_order, id)))
            .collect();
        members.sort_by_key(|(z, _)| *z);
        if front {
            for (_, member) in members {
                previews.bring_to_front(member);
            }
        } else {
            for (_, member) in members.into_iter().rev() {
                previews.send_to_back(member);
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
            HotkeySlot::LayoutOneLarge,
            HotkeySlot::LayoutTwoLarge,
            HotkeySlot::LayoutThreeLarge,
            HotkeySlot::LayoutEqual,
            HotkeySlot::SizeMedium,
            HotkeySlot::SizeLarge,
            HotkeySlot::MakeMainTile,
            HotkeySlot::PreviousMainTile,
            HotkeySlot::NextMainTile,
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
        if let Some(id) = target {
            for (arrangement, slot, _) in PRESETS {
                if presses.pressed(slot) {
                    self.set_group_arrangement(id, arrangement, Some(id), viewport, previews);
                }
            }
            if presses.pressed(HotkeySlot::MakeMainTile) {
                self.make_main_tile(id, viewport, previews);
            }
        }
        for (width, slot, _) in SIZES {
            if presses.pressed(slot) {
                self.resize_selection(width, viewport, previews);
            }
        }
        if presses.pressed(HotkeySlot::NextMainTile) {
            self.cycle_main_tile(true, target, viewport, previews);
        }
        if presses.pressed(HotkeySlot::PreviousMainTile) {
            self.cycle_main_tile(false, target, viewport, previews);
        }
    }

    fn rename_group(&mut self, group: u64, name: &str, previews: &mut PreviewManager) {
        for member in Self::members_of(group, previews) {
            if let Some(g) = previews.get_mut(member).and_then(|p| p.tile_group.as_mut()) {
                g.name = name.to_owned();
            }
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
        crate::ui_theme::menu_button(ui, "Group", |ui| {
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
                ui.close_kind(egui::UiKind::Menu);
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
                    self.rename_group(group.id, &name, previews);
                }
                if ui.button("Select This Tile Only").clicked() {
                    self.remember_current_tile(id);
                    self.groups.editing = Some(id);
                    self.selection = vec![id];
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.button("Select Whole Group").clicked() {
                    self.remember_current_tile(id);
                    self.groups.editing = None;
                    self.selection = self.group_members(id, previews);
                    ui.close_kind(egui::UiKind::Menu);
                }
                if ui.button("Remove This Tile from Group").clicked() {
                    if let Some(p) = previews.get_mut(id) {
                        p.tile_group = None;
                    }
                    // The others close the gap; keep the tile visibly on top.
                    previews.bring_to_front(id);
                    self.groups.editing = None;
                    self.selection = vec![id];
                    ui.close_kind(egui::UiKind::Menu);
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
                    ui.close_kind(egui::UiKind::Menu);
                }
            }
            crate::ui_theme::menu_button(ui, "Add Selected to Group", |ui| {
                crate::ui_theme::menu(ui);
                let mut groups: Vec<_> = previews
                    .all()
                    .filter(|p| p.removing.is_none())
                    .filter_map(|p| p.tile_group.clone().map(|g| (p.id, g)))
                    .collect();
                groups.sort_by_key(|(_, g)| g.id);
                groups.dedup_by_key(|(_, g)| g.id);
                for (member, group) in groups {
                    let count = Self::members_of(group.id, previews).len();
                    let name = if group.name.trim().is_empty() {
                        "Group"
                    } else {
                        &group.name
                    };
                    if ui.button(format!("{name} · {count} tiles")).clicked() {
                        let mut ids = self.group_members(member, previews);
                        for selected in &self.selection {
                            if !ids.contains(selected) {
                                ids.push(*selected);
                            }
                        }
                        self.assign_group(&ids, group, viewport, previews);
                        ui.close_kind(egui::UiKind::Menu);
                    }
                }
            });
            if let Some(error) = &self.groups.error {
                ui.label(error);
            }
        });
    }

    /// Group arrangements (for a tile in a group) and stacking order, in the
    /// Tile Settings submenu.
    pub(super) fn arrange_items(
        &mut self,
        ui: &mut egui::Ui,
        id: PreviewId,
        viewport: Rect,
        previews: &mut PreviewManager,
        shortcuts: &HotkeyBindings,
    ) {
        let settings = self
            .group_settings(id, previews)
            .filter(|_| self.group_members(id, previews).len() > 1);
        if let Some(settings) = &settings {
            let mut free = settings.arrangement == GroupArrangement::Free;
            if crate::ui_theme::toggle(ui, &mut free, "Free Arrangement")
                .on_hover_text("Tiles stay wherever you put them")
                .clicked()
            {
                self.set_group_arrangement(id, GroupArrangement::Free, None, viewport, previews);
                ui.close_kind(egui::UiKind::Menu);
            }
            for (arrangement, slot, label) in PRESETS {
                let mut chosen = settings.arrangement == arrangement;
                if crate::ui_theme::shortcut_toggle(ui, &mut chosen, label, shortcuts.get(slot))
                    .clicked()
                {
                    self.set_group_arrangement(id, arrangement, Some(id), viewport, previews);
                    ui.close_kind(egui::UiKind::Menu);
                }
            }
            if ui
                .add(crate::ui_theme::shortcut_button(
                    "Make Main Tile",
                    shortcuts.get(HotkeySlot::MakeMainTile),
                ))
                .on_hover_text("Show this tile in the group's large slot")
                .clicked()
            {
                self.make_main_tile(id, viewport, previews);
                ui.close_kind(egui::UiKind::Menu);
            }
            if settings.arrangement != GroupArrangement::Free {
                crate::ui_theme::menu_button(ui, "Gap", |ui| {
                    crate::ui_theme::menu(ui);
                    for (gap, label) in [
                        (GroupGap::None, "None"),
                        (GroupGap::Small, "Small"),
                        (GroupGap::Large, "Large"),
                    ] {
                        let mut chosen = settings.gap == gap;
                        if crate::ui_theme::toggle(ui, &mut chosen, label).clicked() {
                            self.set_group_gap(id, gap, viewport, previews);
                            ui.close_kind(egui::UiKind::Menu);
                        }
                    }
                });
            }
            if ui.button("Fit Group to View").clicked() {
                self.fit_group_to_view(id, viewport, previews);
                ui.close_kind(egui::UiKind::Menu);
            }
            ui.separator();
        }
        if ui.button("Bring to Front").clicked() {
            self.restack_group(id, true, previews);
            ui.close_kind(egui::UiKind::Menu);
        }
        if ui.button("Send to Back").clicked() {
            self.restack_group(id, false, previews);
            ui.close_kind(egui::UiKind::Menu);
        }
    }

    /// Size presets for the selection, last in the Tile Settings submenu.
    pub(super) fn size_menu(
        &mut self,
        ui: &mut egui::Ui,
        viewport: Rect,
        previews: &mut PreviewManager,
        shortcuts: &HotkeyBindings,
    ) {
        crate::ui_theme::menu_button(ui, "Size", |ui| {
            crate::ui_theme::menu(ui);
            for (width, slot, label) in SIZES {
                if ui
                    .add(crate::ui_theme::shortcut_button(label, shortcuts.get(slot)))
                    .clicked()
                {
                    self.resize_selection(width, viewport, previews);
                    ui.close_kind(egui::UiKind::Menu);
                }
            }
        });
    }

    pub(super) fn toggle_viewport_pin(
        &mut self,
        id: PreviewId,
        viewport: Rect,
        previews: &mut PreviewManager,
    ) {
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

    /// The group's outline on screen: an arranged group's box, or the
    /// outline of a free group's tiles.
    fn group_outline(
        &self,
        id: PreviewId,
        viewport: Rect,
        previews: &PreviewManager,
    ) -> Option<(Rect, bool)> {
        let members = self.group_members(id, previews);
        let arranged = self.is_arranged(id, previews);
        if arranged {
            let frame = members.iter().find_map(|member| {
                let p = previews.get(*member)?;
                let relative = p.tile_group.as_ref()?.frame?;
                let pinned = p.viewport_pin.is_some();
                let frame = frame_from(layout_rect(p, viewport), relative);
                Some(self.to_screen_space(frame, pinned, viewport))
            });
            if let Some(frame) = frame.filter(|f| f.is_finite() && f.is_positive()) {
                return Some((frame, true));
            }
        }
        bounds_of(
            members
                .iter()
                .filter_map(|member| previews.get(*member))
                .map(|p| self.preview_screen_rect(p, viewport)),
        )
        .map(|bounds| (bounds, false))
    }

    /// Arranged groups resize their box from any edge or corner; free groups
    /// scale from a corner.
    fn group_handles(outline: Rect, arranged: bool) -> Vec<(Pos2, ResizeHandle)> {
        let mut handles = vec![
            (outline.left_top(), ResizeHandle::TopLeft),
            (outline.right_top(), ResizeHandle::TopRight),
            (outline.left_bottom(), ResizeHandle::BottomLeft),
            (outline.right_bottom(), ResizeHandle::BottomRight),
        ];
        if arranged {
            handles.extend([
                (outline.center_top(), ResizeHandle::Top),
                (outline.left_center(), ResizeHandle::Left),
                (outline.right_center(), ResizeHandle::Right),
                (outline.center_bottom(), ResizeHandle::Bottom),
            ]);
        }
        handles
    }

    pub(super) fn group_handle_at(
        &self,
        pointer: Pos2,
        viewport: Rect,
        previews: &PreviewManager,
    ) -> Option<(PreviewId, ResizeHandle)> {
        let id = self.selected_group(previews)?;
        let (outline, arranged) = self.group_outline(id, viewport, previews)?;
        Self::group_handles(outline.expand(3.0), arranged)
            .into_iter()
            .find(|(at, _)| {
                Rect::from_center_size(*at, Vec2::splat(RESIZE_HANDLE_HIT_SIZE)).contains(pointer)
            })
            .map(|(_, handle)| (id, handle))
    }

    fn start_group_resize(
        &mut self,
        id: PreviewId,
        handle: ResizeHandle,
        (outline, arranged): (Rect, bool),
        pointer: Pos2,
        viewport: Rect,
        previews: &PreviewManager,
    ) {
        let Some(group) = previews.get(id).and_then(|p| p.tile_group.as_ref()) else {
            return;
        };
        let members: Vec<_> = self
            .group_members(id, previews)
            .into_iter()
            .filter_map(|member| {
                previews
                    .get(member)
                    .map(|p| (member, self.target_screen_rect(p, viewport)))
            })
            .collect();
        let pinned = members
            .iter()
            .all(|(id, _)| previews.get(*id).is_some_and(|p| p.viewport_pin.is_some()));
        let unit = if pinned { 1.0 } else { self.zoom };
        self.groups.resize = Some(GroupResize {
            group: group.id,
            handle,
            bounds: if arranged {
                outline
            } else {
                bounds_of(members.iter().map(|(_, r)| *r)).unwrap_or(outline)
            },
            pointer,
            min_scale: group_resize_min_scale(&members, unit),
            smallest: members
                .iter()
                .map(|(_, r)| r.width().min(r.height()) / unit)
                .fold(f32::INFINITY, f32::min),
            members,
            arranged,
        });
    }

    fn drag_group_resize(&mut self, pointer: Pos2, viewport: Rect, previews: &mut PreviewManager) {
        let Some(resize) = self.groups.resize.clone() else {
            return;
        };
        let delta = pointer - resize.pointer;
        if resize.arranged {
            let now = self.groups.animation_time.unwrap_or(0.0);
            let Some(arranged) = self.arranged(resize.group, viewport, previews, now) else {
                return;
            };
            let screen = apply_resize(resize.handle, resize.bounds, delta, None);
            let frame = self.to_layout_space(screen, arranged.pinned, viewport);
            let slots = layout_rects(
                &arranged.aspects,
                frame,
                arranged.settings.arrangement,
                arranged.settings.gap,
            );
            // Stop before any tile gets smaller than the single-tile limit,
            // or smaller than it already was.
            if smallest_side(&slots) + 0.01 >= MIN_TILE_SIDE.min(resize.smallest) {
                self.place_group(&arranged, frame, viewport, previews, false);
            }
            return;
        }
        // The opposite corner stays put.
        let (anchor, sign) = match resize.handle {
            ResizeHandle::TopLeft => (resize.bounds.right_bottom(), Vec2::new(-1.0, -1.0)),
            ResizeHandle::TopRight => (resize.bounds.left_bottom(), Vec2::new(1.0, -1.0)),
            ResizeHandle::BottomLeft => (resize.bounds.right_top(), Vec2::new(-1.0, 1.0)),
            _ => (resize.bounds.left_top(), Vec2::new(1.0, 1.0)),
        };
        let extent = resize.bounds.size() + delta * sign;
        let scale = (extent.x / resize.bounds.width())
            .max(extent.y / resize.bounds.height())
            .clamp(resize.min_scale, 20.0);
        for (id, rect) in &resize.members {
            let resized = Rect::from_two_pos(
                anchor + (rect.min - anchor) * scale,
                anchor + (rect.max - anchor) * scale,
            );
            self.set_group_screen_rect(*id, resized, viewport, previews);
        }
    }

    pub(super) fn draw_group_selection(
        &mut self,
        ui: &mut egui::Ui,
        viewport: Rect,
        previews: &mut PreviewManager,
        input: &FrameInput,
    ) -> bool {
        let Some(id) = self.selected_group(previews) else {
            if !input.pointer_blocked && self.crop_mode.is_none() {
                if let Some((outline, _)) = input
                    .hover_pos
                    .and_then(|pointer| {
                        self.left_click_preview_at_screen(pointer, viewport, previews)
                    })
                    .filter(|id| previews.get(*id).is_some_and(|p| p.tile_group.is_some()))
                    .and_then(|id| self.group_outline(id, viewport, previews))
                {
                    ui.painter().rect_stroke(
                        outline.expand(3.0),
                        5.0,
                        Stroke::new(1.0, crate::ui_theme::GOLD.gamma_multiply(0.5)),
                        egui::StrokeKind::Middle,
                    );
                }
            }
            return false;
        };
        if input.alt {
            return false;
        }
        let Some((outline, arranged)) = self.group_outline(id, viewport, previews) else {
            return false;
        };
        let color = crate::ui_theme::GOLD;
        let outline = outline.expand(3.0);
        ui.painter().rect_stroke(
            outline,
            5.0,
            Stroke::new(1.5, color),
            egui::StrokeKind::Middle,
        );
        if let Some(active) = self.current_tile(previews).and_then(|id| previews.get(id)) {
            ui.painter().rect_stroke(
                self.preview_screen_rect(active, viewport),
                3.0,
                Stroke::new(1.5, color),
                egui::StrokeKind::Middle,
            );
        }
        for (at, handle) in Self::group_handles(outline, arranged) {
            let hit = Rect::from_center_size(at, Vec2::splat(RESIZE_HANDLE_HIT_SIZE));
            ui.painter().rect_filled(hit.shrink(3.0), 2.0, color);
            let response = ui.interact(
                hit,
                ui.id().with(("group_resize", id.0, handle as u8)),
                Sense::drag(),
            );
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(handle.cursor());
            }
            if response.drag_started() {
                let pointer = input.interact_pos.unwrap_or(at) - response.drag_delta();
                self.start_group_resize(
                    id,
                    handle,
                    (outline.shrink(3.0), arranged),
                    pointer,
                    viewport,
                    previews,
                );
            }
            if response.dragged() {
                if let Some(pointer) = input.interact_pos {
                    self.drag_group_resize(pointer, viewport, previews);
                }
            }
            if response.drag_stopped() {
                self.groups.resize = None;
            }
        }
        true
    }

    /// A small bar above a selected group: its name (double-click to
    /// rename) and its arrangement.
    pub(super) fn group_bar(
        &mut self,
        ui: &mut egui::Ui,
        viewport: Rect,
        previews: &mut PreviewManager,
        shortcuts: &HotkeyBindings,
    ) {
        let selected = self
            .selected_group(previews)
            .filter(|_| !self.is_focusing_tile() && self.marquee.is_none());
        let Some((id, settings, (outline, _))) = selected.and_then(|id| {
            Some((
                id,
                self.group_settings(id, previews)?,
                self.group_outline(id, viewport, previews)?,
            ))
        }) else {
            self.groups.renaming = None;
            return;
        };
        if !outline.intersects(viewport) {
            return;
        }
        if self
            .groups
            .renaming
            .as_ref()
            .is_some_and(|(group, _)| *group != settings.id)
        {
            self.groups.renaming = None;
        }
        // Above the group, or below it when the group reaches the window's
        // top, so the bar never covers the group's own handles.
        const BAR_HEIGHT: f32 = 32.0;
        let above = outline.top() - BAR_HEIGHT - 12.0;
        let below = outline.bottom() + 12.0;
        let top = if above >= viewport.top() || below + BAR_HEIGHT > viewport.bottom() {
            above
        } else {
            below
        };
        let position = egui::pos2(
            (outline.left() - 3.0).clamp(
                viewport.left(),
                (viewport.right() - 300.0).max(viewport.left()),
            ),
            top.clamp(
                viewport.top(),
                (viewport.bottom() - BAR_HEIGHT).max(viewport.top()),
            ),
        );
        egui::Area::new(ui.id().with("group_bar"))
            .fixed_pos(position)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(crate::ui_theme::PANEL)
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(6, 3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            let mut finished = None;
                            if let Some((group, name)) = &mut self.groups.renaming {
                                let edit = ui.add(
                                    egui::TextEdit::singleline(name)
                                        .char_limit(60)
                                        .desired_width(120.0),
                                );
                                if !edit.has_focus() && !edit.lost_focus() {
                                    edit.request_focus();
                                }
                                if edit.lost_focus() {
                                    let cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));
                                    let name = name.trim().to_owned();
                                    finished = Some((
                                        *group,
                                        (!cancel && !name.is_empty()).then_some(name),
                                    ));
                                }
                            } else {
                                let name = if settings.name.trim().is_empty() {
                                    "Group"
                                } else {
                                    settings.name.as_str()
                                };
                                if ui
                                    .add(
                                        egui::Label::new(
                                            egui::RichText::new(name)
                                                .color(crate::ui_theme::GOLD)
                                                .strong(),
                                        )
                                        .sense(Sense::click()),
                                    )
                                    .on_hover_text("Double-click to rename")
                                    .double_clicked()
                                {
                                    self.groups.renaming =
                                        Some((settings.id, settings.name.clone()));
                                }
                            }
                            if let Some((group, name)) = finished {
                                self.groups.renaming = None;
                                if let Some(name) = name {
                                    self.rename_group(group, &name, previews);
                                }
                            }
                            ui.separator();
                            if ui
                                .selectable_label(
                                    settings.arrangement == GroupArrangement::Free,
                                    "Free",
                                )
                                .on_hover_text("Tiles stay wherever you put them")
                                .clicked()
                            {
                                self.set_group_arrangement(
                                    id,
                                    GroupArrangement::Free,
                                    None,
                                    viewport,
                                    previews,
                                );
                            }
                            for (arrangement, slot, label) in PRESETS {
                                let short = match arrangement {
                                    GroupArrangement::Main(large) => large.to_string(),
                                    _ => "Equal".to_owned(),
                                };
                                let hint = format!(
                                    "{label} ({}). Shift+drag a tile onto another to swap them.",
                                    shortcuts.get(slot).display()
                                );
                                if ui
                                    .selectable_label(settings.arrangement == arrangement, short)
                                    .on_hover_text(hint)
                                    .clicked()
                                {
                                    let target = self.current_tile(previews);
                                    self.set_group_arrangement(
                                        id,
                                        arrangement,
                                        target,
                                        viewport,
                                        previews,
                                    );
                                }
                            }
                        });
                    });
            });
    }
}

#[cfg(test)]
#[path = "groups_tests.rs"]
mod tests;
