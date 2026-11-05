use super::*;
use crate::ui_theme;
use egui::{Color32, Rect, Stroke};
use egui_phosphor::regular as icons;
use std::sync::Arc;

const TITLE: &str = "Pluriview Control Tile Center";
/// Wide enough for three cards in a row (three of the narrowest cards and
/// their gaps, the side margins and the scroll bar).
const DEFAULT_SIZE: Vec2 = Vec2::new(520.0, 620.0);
const MIN_SIZE: Vec2 = Vec2::new(260.0, 240.0);
const CARD_MIN_WIDTH: f32 = 150.0;
const CARD_GAP: f32 = 10.0;
const CARD_PADDING: f32 = 6.0;
const CARD_TITLE_HEIGHT: f32 = 22.0;
const THUMBNAIL_BACKGROUND: Color32 = Color32::from_rgb(13, 13, 13);
/// Canvas units of space kept around everything shown on the map.
const MAP_MARGIN: f32 = 0.06;

fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of(TITLE)
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum View {
    #[default]
    Grid,
    Map,
}

/// What dragging on the map does. A click on a tile focuses it either way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MapMode {
    Camera,
    #[default]
    Tiles,
}

/// The Control Tile Center: a separate window listing every tile, to focus,
/// hide or show them and control their sound. The window is runtime-only
/// (its size and place are kept in the settings); hidden state is saved with
/// the workspace.
#[derive(Default)]
pub(super) struct TileControl {
    open: bool,
    shared: Shared,
    /// Where the window was last, so reopening it keeps the user's spot.
    last_rect: Option<Rect>,
    /// The builder's geometry stays fixed while open: egui would otherwise
    /// move the window back whenever the requested geometry changed.
    initial: Option<Rect>,
    /// Reused each frame to list the tiles in a stable order.
    tile_ids: Vec<PreviewId>,
    /// When the window last moved or resized without the new geometry
    /// being saved to the settings yet.
    unsaved_rect_since: Option<Instant>,
    icon: Option<Arc<egui::IconData>>,
}

/// Everything the window reads and writes.
#[derive(Default)]
struct Shared {
    snapshot: Snapshot,
    ui: UiState,
    actions: Vec<Action>,
    close_requested: bool,
    window_rect: Option<Rect>,
}

struct UiState {
    view: View,
    map_mode: MapMode,
    search: String,
    /// List tiles by name rather than in the user's own order. Kept in the
    /// settings.
    sort_by_name: bool,
    on_top: bool,
    /// While the map is dragged its scale stays put, or panning the canvas
    /// would rescale the map under the pointer.
    map_drag_bounds: Option<Rect>,
    /// Where the current map drag was pressed, and the canvas point (tile
    /// position or view center) it started from. Following the total
    /// offset keeps what is dragged under the pointer.
    map_drag_origin: Option<(Pos2, Pos2)>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            view: View::default(),
            map_mode: MapMode::default(),
            search: String::new(),
            sort_by_name: false,
            on_top: true,
            map_drag_bounds: None,
            map_drag_origin: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Focus(PreviewId),
    SetHidden(Vec<PreviewId>, bool),
    Select(PreviewId),
    PictureInPicture(PreviewId),
    TogglePin(PreviewId),
    /// Move a tile, with its group, so its top-left corner lands here.
    /// Absolute, so a snapshot older than the last move cannot add twice.
    MoveTileTo(PreviewId, Pos2),
    /// Resize a pinned tile to this many canvas units of the current view,
    /// keeping its top-left corner. Absolute, like `MoveTileTo`.
    ResizePinnedTo(PreviewId, Vec2),
    BringToFront(PreviewId),
    SendToBack(PreviewId),
    SetFps(PreviewId, FpsPreset),
    ToggleMute(PreviewId),
    ToggleStreamAudio(PreviewId),
    /// Move a tile within the custom order, next to another tile.
    MoveInList(PreviewId, Place),
    SetVolume(PreviewId, f64),
    LookAt {
        point: Pos2,
        animate: bool,
    },
}

/// Where a dragged card lands in the custom order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    Before(PreviewId),
    After(PreviewId),
}

/// A card being dragged to a new place. Cards only move within their own
/// section (the loose tiles, or one group).
struct CardDrag {
    id: PreviewId,
    section: Option<u64>,
    title: String,
}

/// The tiles and canvas state the window shows, copied from the app.
#[derive(Clone, Default)]
struct Snapshot {
    tiles: Vec<TileView>,
    focused: Option<PreviewId>,
    selection: Vec<PreviewId>,
    /// The canvas area currently on screen, in canvas units.
    viewport: Option<Rect>,
    /// A Stream Audio Monitor device is chosen, so Stream Audio is heard.
    stream_monitor_ready: bool,
}

#[derive(Clone)]
struct TileView {
    id: PreviewId,
    title: String,
    icon: &'static str,
    /// Place in the custom order.
    list_order: u64,
    /// Canvas rect (top-left is the tile's position). A pinned tile's is the
    /// part of the canvas it covers in the current view.
    rect: Rect,
    /// Screen points when pinned, else canvas units.
    shown_size: Vec2,
    z_order: u32,
    /// Group id, name and the tile's order in it.
    group: Option<(u64, String, u32)>,
    hidden: bool,
    pinned: bool,
    frozen: bool,
    in_picture_in_picture: bool,
    inactive: bool,
    can_pop_out: bool,
    /// Set for tiles with a capture frame rate (not images or playlists).
    fps: Option<FpsPreset>,
    thumbnail: Thumbnail,
    /// The page's part of a browser tile with a toolbar, as fractions.
    page: Option<Rect>,
    sound: Option<Sound>,
}

#[derive(Clone)]
enum Thumbnail {
    None,
    Texture {
        id: egui::TextureId,
        uv: Rect,
    },
    #[cfg(windows)]
    Video {
        renderer: Arc<crate::libmpv::VideoRenderer>,
        crop_uv: Option<(f32, f32, f32, f32)>,
    },
}

/// A tile's own sound. Window tiles offer Stream Audio instead of mute:
/// Pluriview never changes another app's volume.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Sound {
    Video {
        muted: bool,
        playing: bool,
        volume: f32,
    },
    Browser {
        muted: bool,
        playing: bool,
    },
    Window {
        streaming: bool,
    },
}

impl Sound {
    fn audible(self) -> bool {
        match self {
            Self::Video { muted, playing, .. } | Self::Browser { muted, playing } => {
                playing && !muted
            }
            Self::Window { .. } => false,
        }
    }

    fn muted(self) -> Option<bool> {
        match self {
            Self::Video { muted, .. } | Self::Browser { muted, .. } => Some(muted),
            Self::Window { .. } => None,
        }
    }
}

/// Tiles listed under one heading: the ungrouped ones, or one group.
struct Section<'a> {
    name: Option<&'a str>,
    tiles: Vec<&'a TileView>,
}

/// Names compared without regard to case, without allocating.
fn name_order(a: &str, b: &str) -> std::cmp::Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
}

fn tile_order(a: &TileView, b: &TileView, by_name: bool) -> std::cmp::Ordering {
    if by_name {
        name_order(&a.title, &b.title).then(a.id.0.cmp(&b.id.0))
    } else {
        (a.list_order, a.id.0).cmp(&(b.list_order, b.id.0))
    }
}

/// Loose tiles first, then each group as its own section, all in the
/// user's order (or by name). The canvas layout does not affect it, so cards
/// stay put while tiles move.
fn sections<'a>(tiles: &'a [TileView], search: &str, by_name: bool) -> Vec<Section<'a>> {
    let needle = search.trim().to_lowercase();
    let matches = |tile: &TileView| {
        needle.is_empty()
            || tile.title.to_lowercase().contains(&needle)
            || tile
                .group
                .as_ref()
                .is_some_and(|(_, name, _)| name.to_lowercase().contains(&needle))
    };
    let mut loose = Vec::new();
    let mut groups: Vec<(u64, &str, Vec<&TileView>)> = Vec::new();
    for tile in tiles.iter().filter(|tile| matches(tile)) {
        match &tile.group {
            Some((id, name, _)) => match groups.iter_mut().find(|(group, ..)| group == id) {
                Some((.., members)) => members.push(tile),
                None => groups.push((*id, name, vec![tile])),
            },
            None => loose.push(tile),
        }
    }
    loose.sort_by(|a, b| tile_order(a, b, by_name));
    for (.., members) in &mut groups {
        members.sort_by(|a, b| tile_order(a, b, by_name));
    }
    // A group takes the place of its first member.
    groups.sort_by(|a, b| {
        if by_name {
            name_order(a.1, b.1).then(a.0.cmp(&b.0))
        } else {
            tile_order(a.2[0], b.2[0], false)
        }
    });

    let mut result = Vec::with_capacity(groups.len() + 1);
    if !loose.is_empty() {
        result.push(Section {
            name: None,
            tiles: loose,
        });
    }
    for (_, name, members) in groups {
        result.push(Section {
            name: Some(name),
            tiles: members,
        });
    }
    result
}

/// Move `id` next to another tile in `order`. False when nothing moves.
fn move_in_order(order: &mut Vec<PreviewId>, id: PreviewId, place: Place) -> bool {
    let target = match place {
        Place::Before(target) | Place::After(target) => target,
    };
    if target == id {
        return false;
    }
    let Some(from) = order.iter().position(|other| *other == id) else {
        return false;
    };
    let before = order.clone();
    order.remove(from);
    let Some(at) = order.iter().position(|other| *other == target) else {
        *order = before;
        return false;
    };
    let at = match place {
        Place::Before(_) => at,
        Place::After(_) => at + 1,
    };
    order.insert(at, id);
    *order != before
}

/// The drop place nearest the pointer among a section's cards, and the bar
/// that shows it: between two cards, or before the first or after the last
/// card of a row. None when the pointer is away from the section.
fn drop_place(cards: &[(PreviewId, Rect)], pointer: Pos2) -> Option<(Place, Rect)> {
    let bounds = cards
        .iter()
        .map(|(_, rect)| *rect)
        .reduce(|a, b| a.union(b))?;
    if !bounds.expand(CARD_GAP * 2.0).contains(pointer) {
        return None;
    }
    let bar = |x: f32, card: Rect| {
        Rect::from_min_max(
            Pos2::new(x - 1.5, card.min.y),
            Pos2::new(x + 1.5, card.max.y),
        )
    };
    cards
        .iter()
        .flat_map(|(id, rect)| {
            [
                (Place::Before(*id), bar(rect.min.x - CARD_GAP / 2.0, *rect)),
                (Place::After(*id), bar(rect.max.x + CARD_GAP / 2.0, *rect)),
            ]
        })
        .min_by(|a, b| {
            a.1.center()
                .distance_sq(pointer)
                .total_cmp(&b.1.center().distance_sq(pointer))
        })
}

/// Columns and card width for the grid at `width`.
fn grid_columns(width: f32) -> (usize, f32) {
    let columns = ((width + CARD_GAP) / (CARD_MIN_WIDTH + CARD_GAP))
        .floor()
        .max(1.0);
    let card_width = ((width - CARD_GAP * (columns - 1.0)) / columns).max(1.0);
    (columns as usize, card_width)
}

/// The largest rect of `aspect` centered in `area`.
fn fit(aspect: f32, area: Rect) -> Rect {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        16.0 / 9.0
    };
    let width = area.width().min(area.height() * aspect);
    Rect::from_center_size(area.center(), Vec2::new(width, width / aspect))
}

/// Maps canvas units onto the map drawn in the window.
#[derive(Clone, Copy)]
struct MapTransform {
    canvas: Rect,
    map: Rect,
}

impl MapTransform {
    fn new(bounds: Rect, area: Rect) -> Self {
        let bounds = bounds.expand(bounds.size().max_elem() * MAP_MARGIN);
        Self {
            canvas: bounds,
            map: fit(bounds.width() / bounds.height().max(1.0), area),
        }
    }

    fn scale(&self) -> f32 {
        self.map.width() / self.canvas.width().max(1.0)
    }

    fn map_rect(&self, rect: Rect) -> Rect {
        let scale = self.scale();
        Rect::from_min_max(
            self.map.min + (rect.min - self.canvas.min) * scale,
            self.map.min + (rect.max - self.canvas.min) * scale,
        )
    }

    fn canvas_point(&self, point: Pos2) -> Pos2 {
        self.canvas.min + (point - self.map.min) / self.scale()
    }
}

fn kind_icon(preview: &Preview) -> &'static str {
    if preview.is_playlist() {
        icons::PLAYLIST
    } else if preview.is_media() {
        icons::IMAGE
    } else if preview.is_browser() {
        icons::GLOBE
    } else if preview.is_video() {
        match preview.video_source {
            Some(VideoSource::Stream { .. }) => icons::BROADCAST,
            _ => icons::FILM_STRIP,
        }
    } else if preview.is_spout_capture() {
        icons::BROADCAST
    } else {
        icons::APP_WINDOW
    }
}

fn tile_sound(preview: &Preview) -> Option<Sound> {
    if preview.is_video() {
        let playback = &preview.video_playback;
        Some(Sound::Video {
            muted: playback.muted,
            playing: !playback.paused && !preview.manually_frozen,
            volume: playback.volume as f32,
        })
    } else if preview.is_browser() {
        Some(Sound::Browser {
            muted: preview.browser_muted,
            playing: preview.browser_playing_audio,
        })
    } else if preview.is_window_capture() {
        Some(Sound::Window {
            streaming: preview.stream_audio,
        })
    } else {
        None
    }
}

/// The tile's latest frame, as already uploaded for the canvas. Nothing
/// here captures or decodes; a pending captured frame is uploaded as the
/// canvas would.
fn tile_thumbnail(ctx: &egui::Context, preview: &mut Preview) -> Thumbnail {
    #[cfg(windows)]
    if !matches!(preview.video_status, VideoTileStatus::Failed(_)) {
        if let Some(renderer) = preview.video_renderer.clone() {
            return Thumbnail::Video {
                renderer,
                crop_uv: preview.crop_uv,
            };
        }
    }
    let uv = preview.get_uv_rect();
    match preview.get_texture(ctx) {
        Some(texture) => Thumbnail::Texture {
            id: texture.id(),
            uv,
        },
        None => Thumbnail::None,
    }
}

/// Update `view` to show `preview`, reusing its strings' buffers so a
/// per-frame refresh does not allocate.
fn fill_tile_view(view: &mut TileView, ctx: &egui::Context, preview: &mut Preview) {
    let shown_size = preview
        .viewport_pin
        .map(|pin| pin.size_vec2())
        .unwrap_or(preview.size);
    let unit = Rect::from_min_size(Pos2::ZERO, shown_size.max(Vec2::splat(1.0)));
    view.page = Some(crate::preview::browser_chrome::page_rect_for(preview, unit))
        .filter(|page| *page != unit)
        .map(|page| {
            Rect::from_min_max(
                (page.min.to_vec2() / unit.size()).to_pos2(),
                (page.max.to_vec2() / unit.size()).to_pos2(),
            )
        });
    view.id = preview.id;
    view.title.clone_from(&preview.title);
    view.icon = kind_icon(preview);
    view.list_order = preview.list_order;
    view.rect = preview.rect();
    view.shown_size = shown_size;
    view.z_order = preview.z_order;
    match (&mut view.group, &preview.tile_group) {
        (Some((id, name, order)), Some(group)) => {
            *id = group.id;
            name.clone_from(&group.name);
            *order = group.order;
        }
        (group, tile_group) => {
            *group = tile_group
                .as_ref()
                .map(|group| (group.id, group.name.clone(), group.order));
        }
    }
    view.hidden = preview.hidden;
    view.pinned = preview.viewport_pin.is_some();
    view.frozen = preview.manually_frozen;
    view.in_picture_in_picture = preview.picture_in_picture.is_some();
    view.inactive = preview.is_inactive_window();
    view.can_pop_out = !preview.is_playlist();
    view.fps = (!preview.is_media() && !preview.is_playlist()).then_some(preview.fps_preset);
    view.sound = tile_sound(preview);
    view.thumbnail = tile_thumbnail(ctx, preview);
}

fn empty_tile_view(id: PreviewId) -> TileView {
    TileView {
        id,
        title: String::new(),
        icon: icons::APP_WINDOW,
        list_order: 0,
        rect: Rect::NOTHING,
        shown_size: Vec2::ZERO,
        z_order: 0,
        group: None,
        hidden: false,
        pinned: false,
        frozen: false,
        in_picture_in_picture: false,
        inactive: false,
        can_pop_out: false,
        fps: None,
        thumbnail: Thumbnail::None,
        page: None,
        sound: None,
    }
}

impl PluriviewApp {
    pub(super) fn toggle_tile_control(&mut self, ctx: &egui::Context) {
        if self.tile_control.open {
            self.close_tile_control();
            return;
        }
        let saved = self
            .app_config
            .control_tile_center_rect
            .filter(|rect| rect.iter().all(|value| value.is_finite()))
            .map(|[x, y, width, height]| {
                Rect::from_min_size(Pos2::new(x, y), Vec2::new(width, height).max(MIN_SIZE))
            });
        let control = &mut self.tile_control;
        // Reopen where it was (this session, else as saved), else beside the
        // main window's right edge. A spot on a monitor that has since been
        // unplugged falls back to that default too.
        let initial = control
            .last_rect
            .or(saved)
            .filter(|rect| crate::is_on_screen(&(rect.min.x, rect.min.y)))
            .unwrap_or_else(|| {
                let anchor = ctx
                    .input(|input| input.viewport().inner_rect)
                    .map_or(Pos2::new(80.0, 80.0), |rect| {
                        Pos2::new(rect.max.x - DEFAULT_SIZE.x - 24.0, rect.min.y + 48.0)
                    });
                Rect::from_min_size(anchor, DEFAULT_SIZE)
            });
        control.initial = Some(initial);
        control.open = true;
        control.shared.ui.sort_by_name = self.app_config.control_tile_center_sort_by_name;
    }

    fn close_tile_control(&mut self) {
        self.save_tile_control_rect(true, None);
        let control = &mut self.tile_control;
        control.open = false;
        control.initial = None;
        let shared = &mut control.shared;
        shared.ui.map_drag_bounds = None;
        shared.ui.map_drag_origin = None;
        shared.actions.clear();
        shared.close_requested = false;
        // Release textures and video renderers held for thumbnails.
        shared.snapshot = Snapshot::default();
    }

    /// Bring the window's copy of the tiles up to date. Runs every frame
    /// while the window is open and only allocates when tiles are added, so
    /// the window stays in step with the canvas.
    fn refresh_tile_control(&mut self, ctx: &egui::Context) {
        let control = &mut self.tile_control;
        control.tile_ids.clear();
        control.tile_ids.extend(
            self.preview_manager
                .all()
                .filter(|preview| preview.removing.is_none())
                .map(|preview| preview.id),
        );
        control.tile_ids.sort_by_key(|id| id.0);

        let snapshot = &mut control.shared.snapshot;
        snapshot.tiles.truncate(control.tile_ids.len());
        for (index, id) in control.tile_ids.iter().enumerate() {
            let Some(preview) = self.preview_manager.get_mut(*id) else {
                continue;
            };
            if index == snapshot.tiles.len() {
                snapshot.tiles.push(empty_tile_view(*id));
            }
            let view = &mut snapshot.tiles[index];
            fill_tile_view(view, ctx, preview);
            view.rect = self.canvas.tile_canvas_rect(preview);
        }

        snapshot.focused = self.canvas.focused_tile();
        snapshot.selection.clone_from(&self.canvas.selection);
        snapshot.viewport = self
            .canvas
            .last_screen_rect
            .map(|rect| self.canvas.get_viewport(rect));
        snapshot.stream_monitor_ready = self.monitor_device.is_some();
    }

    /// Show the window as an immediate egui viewport, drawn in the same frame
    /// as the canvas and sharing its GL context, so thumbnails reuse each
    /// tile's texture. A deferred viewport repainting on its own schedule
    /// made the canvas stutter and map drags lag. Requests are applied right
    /// after, in the same frame.
    pub(super) fn tile_control_ui(&mut self, ctx: &egui::Context) {
        if !self.tile_control.open {
            return;
        }
        let Some(initial) = self.tile_control.initial else {
            return;
        };
        self.refresh_tile_control(ctx);

        let icon = self
            .tile_control
            .icon
            .get_or_insert_with(|| {
                let size = 64;
                Arc::new(egui::IconData {
                    rgba: crate::tray::create_app_icon_rgba(size),
                    width: size,
                    height: size,
                })
            })
            .clone();
        // Fixed while open (see `initial`); the pin button changes the
        // level with a viewport command instead.
        let builder = egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_icon(icon)
            .with_resizable(true)
            .with_min_inner_size(MIN_SIZE)
            .with_inner_size(initial.size())
            .with_position(initial.min)
            .with_window_level(egui::WindowLevel::AlwaysOnTop);
        let shared = &mut self.tile_control.shared;
        ctx.show_viewport_immediate(viewport_id(), builder, |ctx, _class| {
            if ctx.input(|input| input.viewport().close_requested()) {
                shared.close_requested = true;
            }
            // Reopen at the frame's position with the content's size: the
            // builder takes an outer position but an inner size.
            shared.window_rect = ctx
                .input(|input| {
                    let viewport = input.viewport();
                    viewport
                        .outer_rect
                        .zip(viewport.inner_rect)
                        .map(|(outer, inner)| Rect::from_min_size(outer.min, inner.size()))
                })
                .or(shared.window_rect);
            window_ui(ctx, &mut shared.ui, &shared.snapshot, &mut shared.actions);
        });

        if let Some(rect) = self.tile_control.shared.window_rect {
            if self.tile_control.last_rect != Some(rect) {
                self.tile_control.last_rect = Some(rect);
                self.tile_control.unsaved_rect_since = Some(Instant::now());
            }
        }
        self.save_tile_control_rect(false, Some(ctx));
        let sort_by_name = self.tile_control.shared.ui.sort_by_name;
        if self.app_config.control_tile_center_sort_by_name != sort_by_name {
            self.app_config.control_tile_center_sort_by_name = sort_by_name;
            self.save_app_config();
        }
        if std::mem::take(&mut self.tile_control.shared.close_requested) {
            self.close_tile_control();
            return;
        }
        let actions = std::mem::take(&mut self.tile_control.shared.actions);
        if !actions.is_empty() {
            ctx.request_repaint();
        }
        for action in actions {
            self.apply_tile_control_action(action, ctx);
        }
    }

    /// Keep the window's size and place in the settings, once it has been
    /// still for a moment (or now, when closing).
    fn save_tile_control_rect(&mut self, now: bool, ctx: Option<&egui::Context>) {
        const SETTLE: Duration = Duration::from_secs(1);
        let Some(since) = self.tile_control.unsaved_rect_since else {
            return;
        };
        let Some(rect) = self.tile_control.last_rect else {
            return;
        };
        if !now && since.elapsed() < SETTLE {
            if let Some(ctx) = ctx {
                ctx.request_repaint_after(SETTLE.saturating_sub(since.elapsed()));
            }
            return;
        }
        self.tile_control.unsaved_rect_since = None;
        let value = [rect.min.x, rect.min.y, rect.width(), rect.height()];
        if self.app_config.control_tile_center_rect != Some(value) {
            self.app_config.control_tile_center_rect = Some(value);
            self.save_app_config();
        }
    }

    fn apply_tile_control_action(&mut self, action: Action, ctx: &egui::Context) {
        let now = ctx.input(|input| input.time);
        match action {
            Action::Focus(id) => self.canvas.toggle_tile_focus(id, &mut self.preview_manager),
            Action::SetHidden(ids, hidden) => {
                self.canvas
                    .set_tiles_hidden(&ids, hidden, &mut self.preview_manager)
            }
            Action::Select(id) => self.canvas.select_tile(id, &mut self.preview_manager),
            Action::PictureInPicture(id) => self.toggle_picture_in_picture(id, ctx),
            Action::MoveTileTo(id, target) => {
                if let Some(rect) = self
                    .preview_manager
                    .get(id)
                    .map(|p| self.canvas.tile_canvas_rect(p))
                {
                    self.canvas
                        .move_tile_by(id, target - rect.min, &mut self.preview_manager);
                }
            }
            Action::ResizePinnedTo(id, size) => {
                self.canvas
                    .resize_pinned_tile(id, size, &mut self.preview_manager);
            }
            Action::TogglePin(id) => self.canvas.toggle_tile_pin(id, &mut self.preview_manager),
            Action::BringToFront(id) => self.preview_manager.bring_to_front(id),
            Action::SendToBack(id) => self.preview_manager.send_to_back(id),
            Action::SetFps(id, fps) => self.canvas.set_tile_fps(id, fps),
            Action::ToggleMute(id) => self.toggle_tile_mute(id, ctx),
            Action::ToggleStreamAudio(id) => self.canvas.toggle_stream_audio(id),
            Action::MoveInList(id, place) => {
                let mut order: Vec<(u64, PreviewId)> = self
                    .preview_manager
                    .all()
                    .filter(|preview| preview.removing.is_none())
                    .map(|preview| (preview.list_order, preview.id))
                    .collect();
                order.sort_by_key(|(order, id)| (*order, id.0));
                let mut ids: Vec<PreviewId> = order.into_iter().map(|(_, id)| id).collect();
                if move_in_order(&mut ids, id, place) {
                    self.preview_manager.set_list_order(&ids);
                }
            }
            Action::SetVolume(id, volume) => self
                .canvas
                .pending_video_actions
                .push((id, VideoAction::SetVolume(volume))),
            Action::LookAt { point, animate } => self.canvas.look_at(point, animate, now),
        }
    }

    /// The same mute as the tile's own controls: libmpv for videos, the
    /// WebView for browsers.
    fn toggle_tile_mute(&mut self, id: PreviewId, ctx: &egui::Context) {
        let Some(preview) = self.preview_manager.get(id) else {
            return;
        };
        if preview.is_video() {
            self.canvas
                .pending_video_actions
                .push((id, VideoAction::ToggleMute));
        } else if preview.is_browser() {
            // Directly, not through the canvas queue, which would also
            // refocus an interactive page and pull focus from this window.
            #[cfg(windows)]
            self.handle_browser_action(ctx, id, BrowserAction::ToggleMute);
        }
        #[cfg(not(windows))]
        let _ = ctx;
    }
}

fn window_ui(
    ctx: &egui::Context,
    ui_state: &mut UiState,
    snapshot: &Snapshot,
    actions: &mut Vec<Action>,
) {
    let on_top = ui_state.on_top;
    header(ctx, ui_state, snapshot, actions);
    if ui_state.on_top != on_top {
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(if ui_state.on_top {
            egui::WindowLevel::AlwaysOnTop
        } else {
            egui::WindowLevel::Normal
        }));
    }
    egui::CentralPanel::default()
        .frame(
            egui::Frame::none()
                .fill(ui_theme::PANEL)
                .inner_margin(egui::Margin::symmetric(12.0, 10.0)),
        )
        .show(ctx, |ui| {
            let searching = !ui_state.search.trim().is_empty();
            let sections = sections(&snapshot.tiles, &ui_state.search, ui_state.sort_by_name);
            if sections.is_empty() {
                empty_state(ui, searching);
                return;
            }
            // Rearranging only makes sense in the user's own, full order.
            let reorderable = !ui_state.sort_by_name && !searching;
            match ui_state.view {
                View::Grid => grid(ui, &sections, snapshot, reorderable, actions),
                View::Map => map(ui, ui_state, &sections, snapshot, actions),
            }
        });
}

fn header(
    ctx: &egui::Context,
    ui_state: &mut UiState,
    snapshot: &Snapshot,
    actions: &mut Vec<Action>,
) {
    let total = snapshot.tiles.len();
    let hidden = snapshot.tiles.iter().filter(|tile| tile.hidden).count();
    egui::TopBottomPanel::top("tile_control_header")
        .frame(
            egui::Frame::none()
                .fill(ui_theme::SURFACE)
                .inner_margin(egui::Margin::symmetric(12.0, 10.0)),
        )
        .show_separator_line(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Tiles").size(17.0).strong());
                let summary = match (total, hidden) {
                    (1, 0) => "1 tile".to_owned(),
                    (total, 0) => format!("{total} tiles"),
                    (total, hidden) => format!("{total} tiles · {hidden} hidden"),
                };
                ui.label(egui::RichText::new(summary).color(ui_theme::SECONDARY));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let pin = if ui_state.on_top {
                        egui::RichText::new(icons::PUSH_PIN).color(ui_theme::GOLD)
                    } else {
                        egui::RichText::new(icons::PUSH_PIN_SIMPLE)
                    };
                    if ui
                        .selectable_label(ui_state.on_top, pin)
                        .on_hover_text("Keep this window above other windows")
                        .clicked()
                    {
                        ui_state.on_top = !ui_state.on_top;
                    }
                    ui.add_space(6.0);
                    for (view, icon, hint) in [
                        (
                            View::Map,
                            icons::MAP_TRIFOLD,
                            "Map: where tiles sit on the canvas",
                        ),
                        (
                            View::Grid,
                            icons::SQUARES_FOUR,
                            "Grid: every tile as a card",
                        ),
                    ] {
                        if ui
                            .selectable_label(ui_state.view == view, icon)
                            .on_hover_text(hint)
                            .clicked()
                        {
                            ui_state.view = view;
                            ui_state.map_drag_bounds = None;
                            ui_state.map_drag_origin = None;
                        }
                    }
                });
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let show_all = hidden > 0;
                let reserved = if show_all { 92.0 } else { 0.0 } + 52.0;
                ui.add(
                    egui::TextEdit::singleline(&mut ui_state.search)
                        .hint_text(format!("{}  Search tiles", icons::MAGNIFYING_GLASS))
                        .desired_width((ui.available_width() - reserved).max(80.0)),
                );
                let hint = if ui_state.sort_by_name {
                    "Listed by name. Click to go back to your own order"
                } else {
                    "List by name. Your own order is kept for when you switch back"
                };
                if ui
                    .selectable_label(
                        ui_state.sort_by_name,
                        format!("{} A–Z", icons::SORT_ASCENDING),
                    )
                    .on_hover_text(hint)
                    .clicked()
                {
                    ui_state.sort_by_name = !ui_state.sort_by_name;
                }
                if show_all
                    && ui
                        .button(format!("{}  Show all", icons::EYE))
                        .on_hover_text("Put every hidden tile back on the canvas")
                        .clicked()
                {
                    let ids = snapshot
                        .tiles
                        .iter()
                        .filter(|tile| tile.hidden)
                        .map(|tile| tile.id)
                        .collect();
                    actions.push(Action::SetHidden(ids, false));
                }
            });
        });
}

fn empty_state(ui: &mut egui::Ui, searching: bool) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.3);
        ui.label(
            egui::RichText::new(icons::SQUARES_FOUR)
                .size(36.0)
                .color(ui_theme::BORDER),
        );
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(if searching {
                "No tiles match"
            } else {
                "No tiles on the canvas yet"
            })
            .color(ui_theme::SECONDARY),
        );
    });
}

fn section_header(ui: &mut egui::Ui, name: &str, tiles: &[&TileView], actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui_theme::section(ui, &name.to_uppercase());
        ui.label(
            egui::RichText::new(tiles.len().to_string())
                .size(11.5)
                .color(ui_theme::BORDER),
        );
        let all_hidden = tiles.iter().all(|tile| tile.hidden);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (icon, hint) = if all_hidden {
                (icons::EYE_SLASH, "Show this group")
            } else {
                (icons::EYE, "Hide this group")
            };
            if ui
                .add(egui::Button::new(icon).frame(false))
                .on_hover_text(hint)
                .clicked()
            {
                let ids = tiles.iter().map(|tile| tile.id).collect();
                actions.push(Action::SetHidden(ids, !all_hidden));
            }
        });
    });
    ui.add_space(4.0);
}

/// The cards, by section. In the user's own order (not by name or while
/// searching), dragging a card moves it within its section.
fn grid(
    ui: &mut egui::Ui,
    sections: &[Section],
    snapshot: &Snapshot,
    reorderable: bool,
    actions: &mut Vec<Action>,
) {
    let dragging = egui::DragAndDrop::payload::<CardDrag>(ui.ctx()).filter(|_| reorderable);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(CARD_GAP);
            let (columns, card_width) = grid_columns(ui.available_width());
            let thumbnail_height = (card_width - CARD_PADDING * 2.0) * 9.0 / 16.0;
            let card_size = Vec2::new(
                card_width,
                CARD_PADDING * 2.0 + thumbnail_height + CARD_TITLE_HEIGHT,
            );
            for (index, section) in sections.iter().enumerate() {
                if let Some(name) = section.name {
                    if index > 0 {
                        ui.add_space(6.0);
                    }
                    section_header(ui, name, &section.tiles, actions);
                } else if sections.len() > 1 {
                    ui_theme::section(ui, "TILES");
                    ui.add_space(4.0);
                }
                let key = section.tiles[0].group.as_ref().map(|(id, ..)| *id);
                let drag_here = dragging.as_ref().filter(|drag| drag.section == key);
                // Card rects, only needed while a card is dragged here.
                let mut placed = Vec::new();
                for row in section.tiles.chunks(columns) {
                    ui.horizontal(|ui| {
                        for tile in row {
                            let (rect, _) = ui.allocate_exact_size(card_size, egui::Sense::hover());
                            if drag_here.is_some() {
                                placed.push((tile.id, rect));
                            }
                            if ui.is_rect_visible(rect) {
                                let drag = reorderable.then_some(key);
                                card(ui, rect, tile, snapshot, drag, actions);
                            }
                        }
                    });
                }
                let Some(drag) = drag_here else {
                    continue;
                };
                let target = ui
                    .ctx()
                    .pointer_latest_pos()
                    .and_then(|pointer| drop_place(&placed, pointer));
                if let Some((place, bar)) = target {
                    ui.painter().rect_filled(bar, 1.5, ui_theme::GOLD);
                    if ui.input(|input| input.pointer.any_released()) {
                        actions.push(Action::MoveInList(drag.id, place));
                    }
                }
            }
        });
    if let (Some(drag), Some(pointer)) = (dragging, ui.ctx().pointer_latest_pos()) {
        drag_ghost(ui.ctx(), &drag.title, pointer);
    }
}

/// The dragged card's name, following the pointer.
fn drag_ghost(ctx: &egui::Context, title: &str, pointer: Pos2) {
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("tile_control_card_ghost"),
    ));
    let galley = painter.layout_no_wrap(
        title.chars().take(40).collect(),
        egui::FontId::proportional(13.0),
        ui_theme::TEXT,
    );
    let rect = Rect::from_min_size(
        pointer + Vec2::new(14.0, 10.0),
        galley.size() + Vec2::new(16.0, 10.0),
    );
    painter.rect(
        rect,
        6.0,
        ui_theme::SURFACE,
        Stroke::new(1.0, ui_theme::GOLD),
    );
    painter.galley(rect.min + Vec2::new(8.0, 5.0), galley, ui_theme::TEXT);
}

fn paint_icon(painter: &egui::Painter, center: Pos2, icon: &str, size: f32, color: Color32) {
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(size),
        color,
    );
}

/// A small icon button drawn over a thumbnail.
fn overlay_button(
    ui: &mut egui::Ui,
    rect: Rect,
    id: egui::Id,
    icon: &str,
    color: Color32,
) -> egui::Response {
    let response = ui.interact(rect, id, egui::Sense::click());
    let fill = if response.hovered() {
        ui_theme::GOLD_DARK
    } else {
        Color32::from_black_alpha(190)
    };
    ui.painter().rect_filled(rect, 5.0, fill);
    let color = if response.hovered() {
        ui_theme::GOLD
    } else {
        color
    };
    paint_icon(ui.painter(), rect.center(), icon, 14.0, color);
    response
}

/// Draw the tile's latest frame in `rect`. Videos reuse the frame libmpv
/// last rendered for the canvas.
fn paint_thumbnail(painter: &egui::Painter, tile: &TileView, rect: Rect) -> bool {
    let content = match tile.page {
        Some(page) => {
            painter.rect_filled(rect, 0.0, ui_theme::SURFACE);
            Rect::from_min_max(
                rect.min + page.min.to_vec2() * rect.size(),
                rect.min + page.max.to_vec2() * rect.size(),
            )
        }
        None => rect,
    };
    match &tile.thumbnail {
        Thumbnail::None => false,
        Thumbnail::Texture { id, uv } => {
            painter.image(*id, content, *uv, Color32::WHITE);
            true
        }
        #[cfg(windows)]
        Thumbnail::Video { renderer, crop_uv } => {
            let renderer = Arc::clone(renderer);
            let crop_uv = *crop_uv;
            painter.add(egui::PaintCallback {
                rect: content,
                callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |info, painter| {
                    renderer.paint_last_frame(info, painter.gl(), crop_uv)
                })),
            });
            true
        }
    }
}

fn sound_icon(sound: Sound) -> (&'static str, Color32) {
    match sound {
        Sound::Window { streaming: true } => (icons::WAVEFORM, ui_theme::GOLD),
        Sound::Window { streaming: false } => (icons::WAVEFORM_SLASH, ui_theme::SECONDARY),
        _ if sound.muted() == Some(true) => (icons::SPEAKER_SLASH, ui_theme::DANGER),
        _ if sound.audible() => (icons::SPEAKER_HIGH, ui_theme::GOLD),
        _ => (icons::SPEAKER_NONE, Color32::WHITE),
    }
}

fn stream_audio_hint(streaming: bool, monitor_ready: bool) -> &'static str {
    match (streaming, monitor_ready) {
        (true, true) => "Streaming this window's audio to Discord/OBS. Click to stop",
        (true, false) => "Stream Audio is on. Pick a device under Tools → Stream Audio Monitor",
        (false, true) => "Stream this window's audio to Discord/OBS",
        (false, false) => {
            "Stream this window's audio. Pick a device under Tools → Stream Audio Monitor first"
        }
    }
}

/// One tile's card. `drag_section` is set when the card can be dragged to a
/// new place within that section.
fn card(
    ui: &mut egui::Ui,
    rect: Rect,
    tile: &TileView,
    snapshot: &Snapshot,
    drag_section: Option<Option<u64>>,
    actions: &mut Vec<Action>,
) {
    let id = tile.id;
    let sense = if drag_section.is_some() {
        egui::Sense::click_and_drag()
    } else {
        egui::Sense::click()
    };
    let response = ui
        .interact(rect, egui::Id::new(("tile_control_card", id.0)), sense)
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if let (true, Some(section)) = (response.drag_started(), drag_section) {
        egui::DragAndDrop::set_payload(
            ui.ctx(),
            CardDrag {
                id,
                section,
                title: tile.title.clone(),
            },
        );
    }
    let being_dragged =
        egui::DragAndDrop::payload::<CardDrag>(ui.ctx()).is_some_and(|drag| drag.id == id);
    let focused = snapshot.focused == Some(id);
    let selected = snapshot.selection.contains(&id);
    // The buttons and badges sit on top of the card and take egui's hover;
    // the card still counts as hovered while the pointer is over them.
    let hovered = response.contains_pointer();

    let painter = ui.painter_at(rect);
    let (fill, stroke) = if focused {
        (ui_theme::GOLD_DARK, Stroke::new(2.0, ui_theme::GOLD))
    } else if hovered {
        (ui_theme::HOVER, Stroke::new(1.0, ui_theme::GOLD))
    } else if selected {
        (
            ui_theme::SURFACE,
            Stroke::new(1.0, ui_theme::GOLD.gamma_multiply(0.6)),
        )
    } else {
        (ui_theme::SURFACE, Stroke::new(1.0, ui_theme::BORDER))
    };
    painter.rect(rect.shrink(0.5), 8.0, fill, stroke);

    let thumbnail = Rect::from_min_size(
        rect.min + Vec2::splat(CARD_PADDING),
        Vec2::new(
            rect.width() - CARD_PADDING * 2.0,
            rect.height() - CARD_PADDING * 2.0 - CARD_TITLE_HEIGHT,
        ),
    );
    painter.rect_filled(thumbnail, 5.0, THUMBNAIL_BACKGROUND);
    let size = tile.shown_size;
    let content = fit(size.x / size.y.max(1.0), thumbnail.shrink(2.0));
    if !paint_thumbnail(&painter, tile, content) {
        paint_icon(
            &painter,
            thumbnail.center(),
            tile.icon,
            26.0,
            ui_theme::BORDER,
        );
    }
    if tile.hidden {
        painter.rect_filled(thumbnail, 5.0, Color32::from_black_alpha(170));
        paint_icon(
            &painter,
            thumbnail.center() - Vec2::new(0.0, 7.0),
            icons::EYE_SLASH,
            22.0,
            ui_theme::SECONDARY,
        );
        painter.text(
            thumbnail.center() + Vec2::new(0.0, 14.0),
            egui::Align2::CENTER_CENTER,
            "Hidden",
            egui::FontId::proportional(12.0),
            ui_theme::SECONDARY,
        );
    }

    if being_dragged {
        painter.rect_filled(rect, 8.0, Color32::from_black_alpha(150));
    }

    // Small state badges along the thumbnail's top-left corner.
    let badges = [
        (tile.pinned, icons::PUSH_PIN, "Pinned to the viewport"),
        (tile.frozen, icons::SNOWFLAKE, "Frozen"),
        (
            tile.in_picture_in_picture,
            icons::PICTURE_IN_PICTURE,
            "In Picture in Picture",
        ),
        (
            tile.inactive,
            icons::APP_WINDOW,
            "Not active: the app is closed",
        ),
    ];
    for (index, (_, badge, hint)) in badges.iter().filter(|(shown, ..)| *shown).enumerate() {
        let badge_rect = Rect::from_min_size(
            thumbnail.min + Vec2::new(5.0 + index as f32 * 23.0, 5.0),
            Vec2::splat(20.0),
        );
        painter.rect_filled(badge_rect, 4.0, Color32::from_black_alpha(180));
        paint_icon(&painter, badge_rect.center(), badge, 12.0, ui_theme::GOLD);
        ui.interact(
            badge_rect,
            egui::Id::new(("tile_control_badge", id.0, index)),
            egui::Sense::hover(),
        )
        .on_hover_text(*hint);
    }

    let title_rect = Rect::from_min_max(
        Pos2::new(thumbnail.min.x + 2.0, thumbnail.max.y + 4.0),
        Pos2::new(thumbnail.max.x, rect.max.y - CARD_PADDING + 2.0),
    );
    paint_icon(
        &painter,
        Pos2::new(title_rect.min.x + 7.0, title_rect.center().y),
        tile.icon,
        13.0,
        ui_theme::SECONDARY,
    );
    let title_color = if tile.hidden {
        ui_theme::SECONDARY
    } else {
        ui_theme::TEXT
    };
    let text_rect = Rect::from_min_max(
        Pos2::new(title_rect.min.x + 20.0, title_rect.center().y - 8.0),
        title_rect.max,
    );
    ui_theme::clipped_text(ui, text_rect, &tile.title, 13.0, title_color);

    // Sound: always visible, so a playing tile is easy to spot and silence.
    if let Some(sound) = tile.sound {
        let button = Rect::from_min_size(
            Pos2::new(thumbnail.max.x - 29.0, thumbnail.max.y - 29.0),
            Vec2::splat(24.0),
        );
        let (icon, color) = sound_icon(sound);
        let (action, hint) = match sound {
            Sound::Window { streaming } => (
                Action::ToggleStreamAudio(id),
                stream_audio_hint(streaming, snapshot.stream_monitor_ready),
            ),
            _ if sound.muted() == Some(true) => (Action::ToggleMute(id), "Unmute"),
            _ => (Action::ToggleMute(id), "Mute"),
        };
        if overlay_button(
            ui,
            button,
            egui::Id::new(("tile_control_sound", id.0)),
            icon,
            color,
        )
        .on_hover_text(hint)
        .clicked()
        {
            actions.push(action);
            return;
        }
    }

    if hovered || tile.hidden {
        let eye = Rect::from_min_size(
            Pos2::new(thumbnail.max.x - 29.0, thumbnail.min.y + 5.0),
            Vec2::splat(24.0),
        );
        let (icon, hint) = if tile.hidden {
            (icons::EYE, "Show on the canvas")
        } else if tile.group.is_some() {
            (icons::EYE_SLASH, "Hide this group from the canvas")
        } else {
            (icons::EYE_SLASH, "Hide from the canvas")
        };
        if overlay_button(
            ui,
            eye,
            egui::Id::new(("tile_control_eye", id.0)),
            icon,
            Color32::WHITE,
        )
        .on_hover_text(hint)
        .clicked()
        {
            actions.push(set_hidden_with_group(tile, snapshot, !tile.hidden));
            return;
        }
    }

    let response = response.on_hover_ui(|ui| {
        ui.label(egui::RichText::new(&tile.title).strong());
        ui.label(if focused {
            "Click to return to the canvas view"
        } else if tile.pinned {
            "Click to select it"
        } else {
            "Click to focus it on the canvas"
        });
        if drag_section.is_some() {
            ui.label("Drag to rearrange");
        }
    });
    if response.clicked() {
        actions.push(Action::Focus(id));
    }
    response.context_menu(|ui| tile_menu(ui, tile, snapshot, actions));
}

fn tile_menu(ui: &mut egui::Ui, tile: &TileView, snapshot: &Snapshot, actions: &mut Vec<Action>) {
    let id = tile.id;
    let focused = snapshot.focused == Some(id);
    ui_theme::menu(ui);
    ui.label(egui::RichText::new(&tile.title).strong());
    ui.separator();
    let focus_label = if focused {
        "Return from Focus"
    } else {
        "Focus on Canvas"
    };
    if ui
        .add_enabled(!tile.pinned || focused, egui::Button::new(focus_label))
        .on_disabled_hover_text("A pinned tile is always in view")
        .clicked()
    {
        actions.push(Action::Focus(id));
        ui.close_menu();
    }
    if ui.button("Select on Canvas").clicked() {
        actions.push(Action::Select(id));
        ui.close_menu();
    }
    let label = match (tile.hidden, tile.group.is_some()) {
        (true, true) => "Show Group",
        (true, false) => "Show Tile",
        (false, true) => "Hide Group",
        (false, false) => "Hide Tile",
    };
    if ui.button(label).clicked() {
        actions.push(set_hidden_with_group(tile, snapshot, !tile.hidden));
        ui.close_menu();
    }
    if let Some(sound) = tile.sound {
        ui.separator();
        match sound {
            Sound::Window { streaming } => {
                let mut on = streaming;
                if ui_theme::toggle(ui, &mut on, "Stream Audio")
                    .on_hover_text(stream_audio_hint(streaming, snapshot.stream_monitor_ready))
                    .clicked()
                {
                    actions.push(Action::ToggleStreamAudio(id));
                    ui.close_menu();
                }
            }
            _ => {
                let mut muted = sound.muted() == Some(true);
                if ui_theme::toggle(ui, &mut muted, "Mute").clicked() {
                    actions.push(Action::ToggleMute(id));
                    ui.close_menu();
                }
            }
        }
        if let Sound::Video { volume, .. } = sound {
            let mut volume = volume;
            ui.horizontal(|ui| {
                ui.label(icons::SPEAKER_LOW);
                let response =
                    ui.add(egui::Slider::new(&mut volume, 0.0..=100.0).show_value(false));
                ui.label(icons::SPEAKER_HIGH);
                if response.changed() {
                    actions.push(Action::SetVolume(id, f64::from(volume)));
                }
            });
        }
    }
    ui.separator();
    if tile.can_pop_out {
        let mut popped_out = tile.in_picture_in_picture;
        if ui_theme::toggle(ui, &mut popped_out, "Picture in Picture").clicked() {
            actions.push(Action::PictureInPicture(id));
            ui.close_menu();
        }
    }
    let mut pinned = tile.pinned;
    if ui_theme::toggle(ui, &mut pinned, "Pin to Viewport")
        .on_hover_text("Keep this tile fixed above the canvas while panning or zooming")
        .clicked()
    {
        actions.push(Action::TogglePin(id));
        ui.close_menu();
    }
    ui.separator();
    if ui.button("Bring to Front").clicked() {
        actions.push(Action::BringToFront(id));
        ui.close_menu();
    }
    if ui.button("Send to Back").clicked() {
        actions.push(Action::SendToBack(id));
        ui.close_menu();
    }
    if let Some(current) = tile.fps {
        ui.menu_button(format!("Frame Rate: {}", current.label()), |ui| {
            ui_theme::menu(ui);
            for preset in [FpsPreset::Low, FpsPreset::Medium, FpsPreset::High] {
                if ui
                    .selectable_label(current == preset, preset.label())
                    .clicked()
                {
                    actions.push(Action::SetFps(id, preset));
                    ui.close_menu();
                }
            }
        });
    }
}

/// Hiding or showing one member of a group affects the whole group, so
/// layouts never have holes in them.
fn set_hidden_with_group(tile: &TileView, snapshot: &Snapshot, hidden: bool) -> Action {
    let ids = match &tile.group {
        Some((group, ..)) => snapshot
            .tiles
            .iter()
            .filter(|other| other.group.as_ref().is_some_and(|(id, ..)| id == group))
            .map(|other| other.id)
            .collect(),
        None => vec![tile.id],
    };
    Action::SetHidden(ids, hidden)
}

fn map(
    ui: &mut egui::Ui,
    ui_state: &mut UiState,
    sections: &[Section],
    snapshot: &Snapshot,
    actions: &mut Vec<Action>,
) {
    // Pinned tiles float above the canvas, so they are drawn last, where
    // they cover it in the current view.
    let mut tiles: Vec<&TileView> = sections
        .iter()
        .flat_map(|section| section.tiles.iter().copied())
        .collect();
    tiles.sort_by_key(|tile| (tile.pinned, tile.z_order, tile.id.0));

    ui.horizontal(|ui| {
        for (mode, icon, label, hint) in [
            (
                MapMode::Tiles,
                icons::ARROWS_OUT_CARDINAL,
                "Move tiles",
                "Drag tiles to move them on the canvas; a group moves together",
            ),
            (
                MapMode::Camera,
                icons::HAND,
                "Move camera",
                "Drag to move the canvas view; click empty space to jump there",
            ),
        ] {
            if ui
                .selectable_label(ui_state.map_mode == mode, format!("{icon}  {label}"))
                .on_hover_text(hint)
                .clicked()
            {
                ui_state.map_mode = mode;
                ui_state.map_drag_bounds = None;
                ui_state.map_drag_origin = None;
            }
        }
    });
    ui.add_space(6.0);
    let moving_tiles = ui_state.map_mode == MapMode::Tiles;

    let area = ui.available_rect_before_wrap();
    let bounds = tiles
        .iter()
        .map(|tile| tile.rect)
        .chain(snapshot.viewport)
        .reduce(|a, b| a.union(b));
    let Some(bounds) = ui_state.map_drag_bounds.or(bounds) else {
        return;
    };
    let transform = MapTransform::new(bounds, area);
    // Claim the map so the resizable panel keeps its height.
    ui.allocate_rect(area, egui::Sense::hover());

    let background = ui.interact(
        area,
        egui::Id::new("tile_control_map"),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(area);
    painter.rect(
        transform.map,
        6.0,
        Color32::from_rgb(17, 17, 20),
        Stroke::new(1.0, ui_theme::BORDER),
    );

    // The background's own hint only shows away from tiles: with both it and a
    // tile's showing, the tile under the pointer stops receiving clicks.
    let mut over_tile = false;
    for tile in tiles {
        let id = tile.id;
        let rect = transform.map_rect(tile.rect);
        if !rect.intersects(area) {
            continue;
        }
        let sense = if moving_tiles {
            egui::Sense::click_and_drag()
        } else {
            egui::Sense::click()
        };
        let response = ui.interact(
            rect.intersect(area),
            egui::Id::new(("tile_control_map_tile", id.0)),
            sense,
        );
        let response = if !moving_tiles {
            response.on_hover_cursor(egui::CursorIcon::PointingHand)
        } else if response.dragged() {
            response.on_hover_and_drag_cursor(egui::CursorIcon::Grabbing)
        } else {
            response.on_hover_cursor(egui::CursorIcon::Grab)
        };
        over_tile |= response.contains_pointer();
        if moving_tiles {
            if response.drag_started() {
                // Hold the map's scale so the tile stays under the pointer.
                ui_state.map_drag_bounds = Some(bounds);
                ui_state.map_drag_origin = ui
                    .input(|input| input.pointer.press_origin())
                    .map(|press| (press, tile.rect.min));
                if !tile.hidden {
                    actions.push(Action::Select(id));
                }
            }
            let pointer = response.interact_pointer_pos();
            if let (true, Some((press, origin)), Some(pointer)) =
                (response.dragged(), ui_state.map_drag_origin, pointer)
            {
                let target = origin + (pointer - press) / transform.scale();
                actions.push(Action::MoveTileTo(id, target));
            }
        }
        let focused = snapshot.focused == Some(id);
        if tile.hidden {
            painter.rect_filled(rect, 2.0, Color32::from_white_alpha(6));
            let corners = [
                rect.left_top(),
                rect.right_top(),
                rect.right_bottom(),
                rect.left_bottom(),
                rect.left_top(),
            ];
            painter.extend(egui::Shape::dashed_line(
                &corners,
                Stroke::new(1.0, ui_theme::SECONDARY.gamma_multiply(0.6)),
                4.0,
                3.0,
            ));
            if rect.width() >= 22.0 && rect.height() >= 18.0 {
                paint_icon(
                    &painter,
                    rect.center(),
                    icons::EYE_SLASH,
                    (rect.height() * 0.4).clamp(10.0, 20.0),
                    ui_theme::SECONDARY.gamma_multiply(0.7),
                );
            }
        } else {
            painter.rect_filled(rect, 0.0, THUMBNAIL_BACKGROUND);
            if !paint_thumbnail(&painter, tile, rect)
                && rect.width() >= 18.0
                && rect.height() >= 18.0
            {
                paint_icon(
                    &painter,
                    rect.center(),
                    tile.icon,
                    (rect.height() * 0.4).clamp(10.0, 22.0),
                    ui_theme::BORDER,
                );
            }
        }
        // A playing tile is marked even when the map is small.
        if tile.sound.is_some_and(Sound::audible) && rect.width() >= 16.0 && rect.height() >= 12.0 {
            let mark =
                Rect::from_min_size(rect.right_top() + Vec2::new(-15.0, 2.0), Vec2::splat(13.0));
            painter.rect_filled(mark, 3.0, Color32::from_black_alpha(190));
            paint_icon(
                &painter,
                mark.center(),
                icons::SPEAKER_HIGH,
                10.0,
                ui_theme::GOLD,
            );
        }
        if tile.pinned && rect.width() >= 16.0 && rect.height() >= 12.0 {
            let mark =
                Rect::from_min_size(rect.left_top() + Vec2::new(2.0, 2.0), Vec2::splat(13.0));
            painter.rect_filled(mark, 3.0, Color32::from_black_alpha(190));
            paint_icon(
                &painter,
                mark.center(),
                icons::PUSH_PIN,
                10.0,
                ui_theme::GOLD,
            );
        }
        if focused || response.hovered() {
            painter.rect_stroke(rect, 2.0, Stroke::new(1.5, ui_theme::GOLD));
        } else if snapshot.selection.contains(&id) {
            painter.rect_stroke(
                rect,
                2.0,
                Stroke::new(1.0, ui_theme::GOLD.gamma_multiply(0.6)),
            );
        }
        // A pinned tile has no place on the canvas to resize it from when it
        // is small there, so its map tile gets a corner grip. Grouped tiles
        // resize with their group on the canvas.
        let resizable = moving_tiles && tile.pinned && !tile.hidden && tile.group.is_none();
        if resizable && rect.width() >= 24.0 && rect.height() >= 24.0 {
            let grip =
                Rect::from_min_size(rect.right_bottom() - Vec2::splat(14.0), Vec2::splat(14.0));
            let handle = ui
                .interact(
                    grip,
                    egui::Id::new(("tile_control_map_resize", id.0)),
                    egui::Sense::drag(),
                )
                .on_hover_and_drag_cursor(egui::CursorIcon::ResizeNwSe);
            over_tile |= handle.contains_pointer();
            if handle.drag_started() {
                ui_state.map_drag_bounds = Some(bounds);
                ui_state.map_drag_origin = ui
                    .input(|input| input.pointer.press_origin())
                    .map(|press| (press, tile.rect.size().to_pos2()));
                actions.push(Action::Select(id));
            }
            if let (true, Some((press, origin)), Some(pointer)) = (
                handle.dragged(),
                ui_state.map_drag_origin,
                handle.interact_pointer_pos(),
            ) {
                let size = origin.to_vec2() + (pointer - press) / transform.scale();
                actions.push(Action::ResizePinnedTo(id, size));
            }
            let color = if handle.hovered() || handle.dragged() {
                ui_theme::GOLD
            } else {
                ui_theme::GOLD.gamma_multiply(0.75)
            };
            painter.rect_filled(grip, 3.0, Color32::from_black_alpha(190));
            let corner = grip.right_bottom() - Vec2::splat(3.0);
            for step in [3.0, 6.0, 9.0] {
                painter.line_segment(
                    [corner - Vec2::new(step, 0.0), corner - Vec2::new(0.0, step)],
                    Stroke::new(1.2, color),
                );
            }
        }
        let response = response.on_hover_ui(|ui| {
            ui.label(egui::RichText::new(&tile.title).strong());
            if tile.pinned {
                ui.label("Pinned to the view, so it moves along with it");
            }
            // A pinned tile is always in view, so a click only selects it.
            ui.label(match (tile.hidden, tile.pinned, moving_tiles) {
                (true, false, false) => "Hidden · click to show and focus it",
                (true, true, false) => "Hidden · click to show and select it",
                (true, _, true) => "Hidden · drag to move it, click to show it",
                (false, false, false) => "Click to focus it on the canvas",
                (false, true, false) => "Click to select it on the canvas",
                (false, false, true) => "Drag to move it, click to focus it",
                (false, true, true) if resizable => {
                    "Drag to move it within the view or its corner to resize it, click to select it"
                }
                (false, true, true) => "Drag to move it within the view, click to select it",
            });
        });
        if response.clicked() {
            actions.push(Action::Focus(id));
        }
        response.context_menu(|ui| tile_menu(ui, tile, snapshot, actions));
    }

    // The part of the canvas the main window shows right now.
    if let Some(viewport) = snapshot.viewport {
        let rect = transform.map_rect(viewport);
        painter.rect_filled(rect, 3.0, ui_theme::GOLD.gamma_multiply(0.06));
        painter.rect_stroke(rect, 3.0, Stroke::new(1.5, ui_theme::GOLD));
    }

    if moving_tiles {
        if !over_tile {
            background.on_hover_text("Drag a tile to move it; click one to focus it");
        }
    } else {
        camera_drag(
            ui,
            ui_state,
            &background,
            (bounds, transform),
            !over_tile,
            snapshot,
            actions,
        );
    }
    if !ui.input(|input| input.pointer.any_down()) {
        ui_state.map_drag_bounds = None;
        ui_state.map_drag_origin = None;
    }
}

/// Move camera: drag to carry the view along from where it is (or from the
/// pointer, when the drag starts outside it). A click on empty space glides
/// there.
fn camera_drag(
    ui: &egui::Ui,
    ui_state: &mut UiState,
    background: &egui::Response,
    (bounds, transform): (Rect, MapTransform),
    show_hint: bool,
    snapshot: &Snapshot,
    actions: &mut Vec<Action>,
) {
    let Some(pointer) = background.interact_pointer_pos() else {
        if show_hint {
            background
                .clone()
                .on_hover_cursor(egui::CursorIcon::Grab)
                .on_hover_text("Drag to move the canvas view; click to jump there");
        }
        return;
    };
    if background.drag_started() {
        ui_state.map_drag_bounds = Some(bounds);
        let press = ui
            .input(|input| input.pointer.press_origin())
            .unwrap_or(pointer);
        // Grabbing the outlined view carries it along; anywhere else, the
        // view first jumps to the pointer.
        let origin = snapshot
            .viewport
            .filter(|viewport| transform.map_rect(*viewport).contains(press))
            .map_or_else(
                || transform.canvas_point(press),
                |viewport| viewport.center(),
            );
        ui_state.map_drag_origin = Some((press, origin));
    }
    if background.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        if let Some((press, origin)) = ui_state.map_drag_origin {
            actions.push(Action::LookAt {
                point: origin + (pointer - press) / transform.scale(),
                animate: false,
            });
        }
    } else if background.clicked() {
        actions.push(Action::LookAt {
            point: transform.canvas_point(pointer),
            animate: true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
    }

    fn tile(id: u64, rect: Rect) -> TileView {
        TileView {
            id: PreviewId(id),
            title: format!("tile {id}"),
            icon: icons::APP_WINDOW,
            list_order: id,
            rect,
            shown_size: rect.size(),
            z_order: id as u32,
            group: None,
            hidden: false,
            pinned: false,
            frozen: false,
            in_picture_in_picture: false,
            inactive: false,
            can_pop_out: true,
            fps: Some(FpsPreset::Medium),
            thumbnail: Thumbnail::None,
            page: None,
            sound: None,
        }
    }

    fn button(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// The pointer glides in over a few frames, as a real one does. egui
    /// only notices movement once it can measure the pointer's velocity;
    /// before that it treats the pointer as resting there for ages and shows
    /// the hovered widget's tooltip at once, over the target.
    fn arrive(at: Pos2) -> Vec<Vec<egui::Event>> {
        (0..4)
            .rev()
            .map(|step| {
                vec![egui::Event::PointerMoved(
                    at - Vec2::new(3.0 * step as f32, 0.0),
                )]
            })
            .collect()
    }

    fn click(at: Pos2) -> Vec<Vec<egui::Event>> {
        let mut frames = arrive(at);
        frames.push(vec![button(at, true)]);
        frames.push(vec![button(at, false)]);
        frames
    }

    fn drag(from: Pos2, by: Vec2) -> Vec<Vec<egui::Event>> {
        let mut frames = arrive(from);
        frames.push(vec![button(from, true)]);
        for step in 1..=4 {
            frames.push(vec![egui::Event::PointerMoved(
                from + by * step as f32 / 4.0,
            )]);
        }
        frames.push(vec![button(from + by, false)]);
        frames
    }

    /// Run `draw` through real egui frames, one batch of events per frame.
    fn run(
        frames: Vec<Vec<egui::Event>>,
        mut draw: impl FnMut(&mut egui::Ui, &mut Vec<Action>),
    ) -> Vec<Action> {
        let ctx = egui::Context::default();
        let mut actions = Vec::new();
        for (frame, events) in frames.into_iter().enumerate() {
            let input = egui::RawInput {
                screen_rect: Some(rect(0.0, 0.0, 400.0, 340.0)),
                // A steady 60 fps clock, as in the app.
                time: Some(frame as f64 / 60.0),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| draw(ui, &mut actions));
            });
        }
        actions
    }

    const CARD: Rect = Rect {
        min: Pos2::new(10.0, 10.0),
        max: Pos2::new(210.0, 160.0),
    };

    fn click_card(tile: &TileView, at: Pos2) -> Vec<Action> {
        let snapshot = Snapshot {
            tiles: vec![tile.clone()],
            ..Default::default()
        };
        run(click(at), |ui, actions| {
            card(ui, CARD, tile, &snapshot, Some(None), actions)
        })
    }

    #[test]
    fn eye_button_hides_a_shown_tile_and_shows_a_hidden_one() {
        // The eye button's center, from the card's top-right corner.
        let eye = Pos2::new(
            CARD.max.x - CARD_PADDING - 17.0,
            CARD.min.y + CARD_PADDING + 17.0,
        );
        for hidden in [false, true] {
            let mut view = tile(1, rect(0.0, 0.0, 160.0, 90.0));
            view.hidden = hidden;
            let actions = click_card(&view, eye);
            assert_eq!(
                actions,
                vec![Action::SetHidden(vec![PreviewId(1)], !hidden)],
                "hidden={hidden}"
            );
        }
    }

    #[test]
    fn sound_button_mutes_and_the_card_body_focuses() {
        let mut view = tile(1, rect(0.0, 0.0, 160.0, 90.0));
        view.sound = Some(Sound::Video {
            muted: false,
            playing: true,
            volume: 80.0,
        });
        let thumbnail_bottom = CARD.max.y - CARD_PADDING - CARD_TITLE_HEIGHT;
        let speaker = Pos2::new(CARD.max.x - CARD_PADDING - 17.0, thumbnail_bottom - 17.0);
        assert_eq!(
            click_card(&view, speaker),
            vec![Action::ToggleMute(PreviewId(1))]
        );
        assert_eq!(
            click_card(&view, CARD.center()),
            vec![Action::Focus(PreviewId(1))]
        );
    }

    #[test]
    fn a_window_card_toggles_stream_audio() {
        let mut view = tile(1, rect(0.0, 0.0, 160.0, 90.0));
        view.sound = Some(Sound::Window { streaming: false });
        let thumbnail_bottom = CARD.max.y - CARD_PADDING - CARD_TITLE_HEIGHT;
        let button = Pos2::new(CARD.max.x - CARD_PADDING - 17.0, thumbnail_bottom - 17.0);
        assert_eq!(
            click_card(&view, button),
            vec![Action::ToggleStreamAudio(PreviewId(1))]
        );
    }

    #[test]
    fn sound_states_read_at_a_glance() {
        let video = |muted, playing| Sound::Video {
            muted,
            playing,
            volume: 50.0,
        };
        assert!(video(false, true).audible());
        assert!(!video(true, true).audible());
        assert!(!video(false, false).audible());
        assert_eq!(sound_icon(video(true, true)).0, icons::SPEAKER_SLASH);
        assert_eq!(sound_icon(video(false, true)).0, icons::SPEAKER_HIGH);
        assert_eq!(sound_icon(video(false, false)).0, icons::SPEAKER_NONE);
        let window = |streaming| Sound::Window { streaming };
        assert_eq!(window(true).muted(), None);
        assert_eq!(sound_icon(window(true)).0, icons::WAVEFORM);
        assert_eq!(sound_icon(window(false)).0, icons::WAVEFORM_SLASH);
    }

    /// Run the map with one tile filling it.
    fn run_map(mode: MapMode, frames: Vec<Vec<egui::Event>>) -> Vec<Action> {
        run_map_with(vec![tile(1, rect(0.0, 0.0, 400.0, 300.0))], mode, frames)
    }

    fn run_map_with(
        tiles: Vec<TileView>,
        mode: MapMode,
        frames: Vec<Vec<egui::Event>>,
    ) -> Vec<Action> {
        let snapshot = Snapshot {
            tiles,
            viewport: Some(rect(0.0, 0.0, 400.0, 300.0)),
            ..Default::default()
        };
        let mut ui_state = UiState {
            map_mode: mode,
            ..Default::default()
        };
        run(frames, |ui, actions| {
            let sections = sections(&snapshot.tiles, "", false);
            map(ui, &mut ui_state, &sections, &snapshot, actions);
        })
    }

    const MAP_CENTER: Pos2 = Pos2::new(200.0, 190.0);

    #[test]
    fn move_tiles_drags_the_tile_to_follow_the_whole_pointer_offset() {
        let actions = run_map(MapMode::Tiles, drag(MAP_CENTER, Vec2::new(40.0, 0.0)));
        assert!(actions.contains(&Action::Select(PreviewId(1))));
        assert!(!actions
            .iter()
            .any(|action| matches!(action, Action::LookAt { .. })));
        let targets: Vec<Pos2> = actions
            .iter()
            .filter_map(|action| match action {
                Action::MoveTileTo(_, target) => Some(*target),
                _ => None,
            })
            .collect();
        // Targets are absolute and include the movement before the drag
        // threshold, so the last is the whole offset from the start.
        let last = *targets.last().expect("the drag should move the tile");
        assert!(
            last.x > targets[0].x && last.x > 0.0 && last.y.abs() < 1e-3,
            "{targets:?}"
        );
    }

    #[test]
    fn move_camera_drags_the_view_even_over_a_tile() {
        let actions = run_map(MapMode::Camera, drag(MAP_CENTER, Vec2::new(40.0, 0.0)));
        assert!(!actions
            .iter()
            .any(|action| matches!(action, Action::MoveTileTo(..))));
        let centers: Vec<Pos2> = actions
            .iter()
            .filter_map(|action| match action {
                Action::LookAt {
                    point,
                    animate: false,
                } => Some(*point),
                _ => None,
            })
            .collect();
        // Grabbed inside the view: it starts from its center and follows.
        let last = *centers.last().expect("the drag should move the view");
        assert!(
            last.x > 200.0 && (last.y - 150.0).abs() < 1e-3,
            "{centers:?}"
        );
    }

    /// A tile pinned over the middle of the view, above one filling it.
    fn pinned_over_another() -> Vec<TileView> {
        let mut pinned = tile(2, rect(150.0, 100.0, 100.0, 100.0));
        pinned.pinned = true;
        // Lower in the z-order, yet pinned tiles float above the canvas.
        pinned.z_order = 0;
        let mut below = tile(1, rect(0.0, 0.0, 400.0, 300.0));
        below.z_order = 5;
        vec![pinned, below]
    }

    #[test]
    fn a_pinned_tile_shows_on_the_map_above_the_canvas() {
        for mode in [MapMode::Camera, MapMode::Tiles] {
            let actions = run_map_with(pinned_over_another(), mode, click(MAP_CENTER));
            assert_eq!(actions, vec![Action::Focus(PreviewId(2))], "{mode:?}");
        }
        let actions = run_map_with(
            pinned_over_another(),
            MapMode::Tiles,
            drag(MAP_CENTER, Vec2::new(40.0, 0.0)),
        );
        assert!(actions
            .iter()
            .any(|action| matches!(action, Action::MoveTileTo(PreviewId(2), _))));
        assert!(!actions
            .iter()
            .any(|action| matches!(action, Action::MoveTileTo(PreviewId(1), _))));
    }

    #[test]
    fn a_pinned_tile_resizes_from_its_corner_grip_on_the_map() {
        // The grip sits inside the pinned tile's bottom-right corner.
        let grip = Pos2::new(236.0, 220.0);
        let actions = run_map_with(
            pinned_over_another(),
            MapMode::Tiles,
            drag(grip, Vec2::new(20.0, 10.0)),
        );
        assert!(actions.contains(&Action::Select(PreviewId(2))));
        assert!(!actions
            .iter()
            .any(|action| matches!(action, Action::MoveTileTo(..))));
        let sizes: Vec<Vec2> = actions
            .iter()
            .filter_map(|action| match action {
                Action::ResizePinnedTo(PreviewId(2), size) => Some(*size),
                _ => None,
            })
            .collect();
        let last = *sizes.last().expect("the drag should resize the tile");
        assert!(last.x > 110.0 && last.y > 105.0, "{sizes:?}");

        // Moving the camera leaves no grip to catch the pointer.
        let actions = run_map_with(pinned_over_another(), MapMode::Camera, click(grip));
        assert_eq!(actions, vec![Action::Focus(PreviewId(2))]);
    }

    #[test]
    fn the_map_moves_tiles_by_default() {
        assert_eq!(UiState::default().map_mode, MapMode::Tiles);
    }

    #[test]
    fn a_click_on_a_tile_focuses_it_in_both_modes() {
        for mode in [MapMode::Camera, MapMode::Tiles] {
            let actions = run_map(mode, click(MAP_CENTER));
            assert_eq!(actions, vec![Action::Focus(PreviewId(1))], "{mode:?}");
        }
    }

    #[test]
    fn sections_follow_the_users_order_not_the_canvas() {
        let mut tiles = vec![
            tile(1, rect(500.0, 20.0, 300.0, 200.0)),
            tile(2, rect(0.0, 400.0, 300.0, 200.0)),
            tile(3, rect(0.0, 0.0, 300.0, 200.0)),
            tile(4, rect(0.0, 900.0, 100.0, 100.0)),
            tile(5, rect(200.0, 900.0, 100.0, 100.0)),
        ];
        tiles[0].list_order = 30;
        tiles[1].list_order = 10;
        tiles[2].list_order = 20;
        tiles[0].title = "banana".to_owned();
        tiles[1].title = "Cherry".to_owned();
        tiles[2].title = "apple".to_owned();
        tiles[3].group = Some((9, "Cams".to_owned(), 0));
        tiles[4].group = Some((9, "Cams".to_owned(), 1));
        tiles[3].list_order = 50;
        tiles[4].list_order = 40;
        let ids = |section: &Section| section.tiles.iter().map(|t| t.id.0).collect::<Vec<_>>();

        let custom = sections(&tiles, "", false);
        assert_eq!(custom.len(), 2);
        assert_eq!(ids(&custom[0]), vec![2, 3, 1]);
        assert_eq!(custom[1].name, Some("Cams"));
        assert_eq!(ids(&custom[1]), vec![5, 4]);

        // By name, ignoring case.
        assert_eq!(ids(&sections(&tiles, "", true)[0]), vec![3, 1, 2]);
        // Searching matches titles and group names.
        assert_eq!(sections(&tiles, "cams", false).len(), 1);
        assert_eq!(ids(&sections(&tiles, "BAN", false)[0]), vec![1]);
    }

    #[test]
    fn moving_a_tile_places_it_next_to_another() {
        let ids = |list: &[u64]| list.iter().map(|id| PreviewId(*id)).collect::<Vec<_>>();
        let mut order = ids(&[1, 2, 3, 4]);
        assert!(move_in_order(
            &mut order,
            PreviewId(4),
            Place::Before(PreviewId(2))
        ));
        assert_eq!(order, ids(&[1, 4, 2, 3]));
        assert!(move_in_order(
            &mut order,
            PreviewId(1),
            Place::After(PreviewId(3))
        ));
        assert_eq!(order, ids(&[4, 2, 3, 1]));
        // Onto itself, or next to where it already is: nothing changes.
        assert!(!move_in_order(
            &mut order,
            PreviewId(2),
            Place::Before(PreviewId(2))
        ));
        assert!(!move_in_order(
            &mut order,
            PreviewId(2),
            Place::After(PreviewId(4))
        ));
        assert_eq!(order, ids(&[4, 2, 3, 1]));
    }

    #[test]
    fn dragging_a_card_past_another_moves_it_there() {
        let tiles = vec![
            tile(1, rect(0.0, 0.0, 160.0, 90.0)),
            tile(2, rect(0.0, 0.0, 160.0, 90.0)),
            tile(3, rect(0.0, 0.0, 160.0, 90.0)),
        ];
        let snapshot = Snapshot {
            tiles,
            ..Default::default()
        };
        // The test window is 400 points wide, inside the panel's 8-point
        // margins; drag the first card's middle to just past the second card.
        let (_, width) = grid_columns(400.0 - 16.0);
        let first = Pos2::new(8.0 + width / 2.0, 8.0 + 30.0);
        let past_second = Pos2::new(8.0 + width * 2.0 + CARD_GAP * 1.5 - 2.0, 8.0 + 30.0);
        let actions = run(drag(first, past_second - first), |ui, actions| {
            let sections = sections(&snapshot.tiles, "", false);
            grid(ui, &sections, &snapshot, true, actions);
        });
        let moves: Vec<_> = actions
            .iter()
            .filter(|action| matches!(action, Action::MoveInList(..)))
            .collect();
        assert_eq!(
            moves,
            vec![&Action::MoveInList(
                PreviewId(1),
                Place::After(PreviewId(2))
            )]
        );
    }

    #[test]
    fn the_default_window_shows_three_cards_in_a_row() {
        // Side margins, plus room for a scroll bar.
        let grid_width = DEFAULT_SIZE.x - 2.0 * 12.0 - 14.0;
        assert_eq!(grid_columns(grid_width).0, 3);
    }

    #[test]
    fn grid_fits_as_many_columns_as_the_width_allows() {
        assert_eq!(grid_columns(100.0).0, 1);
        let (columns, width) = grid_columns(470.0);
        assert_eq!(columns, 3);
        assert!((width * 3.0 + CARD_GAP * 2.0 - 470.0).abs() < 1e-3);
    }

    #[test]
    fn map_round_trips_canvas_points() {
        let transform = MapTransform::new(
            rect(-1000.0, -500.0, 2000.0, 1000.0),
            rect(0.0, 0.0, 400.0, 400.0),
        );
        // Letterboxed: a wide canvas fills the width and centers vertically.
        assert!((transform.map.width() - 400.0).abs() < 1e-3);
        assert!(transform.map.height() < 400.0);
        let point = Pos2::new(250.0, -125.0);
        let mapped = transform
            .map_rect(Rect::from_min_size(point, Vec2::ZERO))
            .min;
        let back = transform.canvas_point(mapped);
        assert!((back - point).length() < 1e-2);
    }
}
