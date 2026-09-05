#[cfg(windows)]
use crate::browser::{
    self, normalize_url, scrub_url_for_storage, BrowserManager, ExtensionPreparationStatus,
};
use crate::canvas::{
    BrowserAction, CanvasKeyboardInput, CanvasState, CanvasWallpaper, MediaAction, PlaylistAction,
    TileActivityAction, VideoAction, WallpaperSource, WALLPAPER_VIDEO_ID,
};
use crate::capture::CaptureCoordinator;
use crate::external_tools::{self, ExternalTools, ToolKind, ToolStatus};
use crate::hotkeys::{Hotkey, HotkeyBindings, HotkeySlot, HotkeyTracker};
#[cfg(windows)]
use crate::libmpv::{SeekPreviewManager, VideoManager, VideoSnapshot};
use crate::media;
use crate::overlay::RegionSelector;
use crate::persistence::{
    AppConfig, CanvasLayout, CanvasView, SavedLayout, Storage, WallpaperLayout, WindowLayout,
    WorkspaceIndex, WorkspaceSaveState,
};
use crate::playlist::{FolderPlaylist, FolderPlaylistLayout, ThumbnailState};
use crate::preview::{
    is_usable_media_title, video_tile_title, BrowserTileStatus, FpsPreset, Preview, PreviewId,
    PreviewLayout, PreviewManager, RemovedPreviewInfo, VideoPlaybackState, VideoSource,
    VideoTileStatus, WindowHandle,
};
#[cfg(debug_assertions)]
use crate::privacy;
use crate::tray::TrayManager;
use crate::video::{self, VideoLaunch, VideoUpdate};
use crate::window_picker::{enumerate_windows, spawn_preview, WindowInfo, WindowPicker};
use eframe::egui::{self, Pos2, Vec2};
#[cfg(windows)]
use std::collections::{HashMap, HashSet, VecDeque};
#[cfg(windows)]
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};
#[cfg(windows)]
use windows::core::HSTRING;
#[cfg(windows)]
use windows::Win32::Foundation::{HGLOBAL, HWND};
#[cfg(windows)]
use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
};
#[cfg(windows)]
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
#[cfg(windows)]
use windows::Win32::UI::Shell::ShellExecuteW;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow, SW_SHOWNORMAL,
};
use wry::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

/// How long after activating a browser we skip the focus-loss check, so the
/// WebView has time to actually take the foreground.
const BROWSER_FOCUS_GRACE: Duration = Duration::from_millis(300);

/// Restored pages can be much heavier than their placeholder tiles. Spread
/// WebView startup out so several media sites cannot saturate the UI at once.
#[cfg(windows)]
const RESTORED_BROWSER_START_INTERVAL: Duration = Duration::from_secs(2);
/// Transparent overlay pages can legitimately take a long time to show their
/// first visible pixel. Probe them slowly until there is content to display.
#[cfg(windows)]
const TRANSPARENT_BROWSER_STARTUP_FPS: u32 = 5;
#[cfg(windows)]
const RESTORED_VIDEO_START_INTERVAL: Duration = Duration::from_secs(2);
/// Polling cadence for saved Windows tiles whose applications are not open.
#[cfg(windows)]
const INACTIVE_WINDOW_SCAN_INTERVAL: Duration = Duration::from_secs(2);
/// Pause a hidden video wallpaper immediately, then release its libmpv core
/// after a grace period so quick minimize/restore cycles do not churn decoders.
#[cfg(windows)]
const WALLPAPER_HIBERNATE_AFTER: Duration = Duration::from_secs(10);

/// How many recent browser URLs to keep for the Add Browser dialog.
const MAX_RECENT_URLS: usize = 8;
/// Persist a changed workspace at most this long after the previous check.
const WORKSPACE_AUTOSAVE_INTERVAL: Duration = Duration::from_secs(60);
/// Recoverable deletion window for a named canvas view.
const CANVAS_VIEW_UNDO_SECS: f32 = 4.0;

#[cfg(windows)]
fn clipboard_text() -> Result<String, &'static str> {
    // CF_UNICODETEXT is always UTF-16 and avoids lossy ANSI URL conversion.
    const CF_UNICODETEXT: u32 = 13;

    unsafe {
        IsClipboardFormatAvailable(CF_UNICODETEXT)
            .map_err(|_| "The clipboard does not contain text.")?;
        OpenClipboard(None).map_err(|_| "The clipboard is temporarily unavailable.")?;

        let result = (|| {
            let handle = GetClipboardData(CF_UNICODETEXT)
                .map_err(|_| "The clipboard text could not be read.")?;
            let memory = HGLOBAL(handle.0);
            let data = GlobalLock(memory);
            if data.is_null() {
                return Err("The clipboard text could not be read.");
            }

            let text = {
                let len = GlobalSize(memory) / std::mem::size_of::<u16>();
                let utf16 = std::slice::from_raw_parts(data.cast::<u16>(), len);
                let nul = utf16.iter().position(|unit| *unit == 0).unwrap_or(len);
                String::from_utf16(&utf16[..nul])
                    .map_err(|_| "The clipboard contains invalid text.")
            };
            let _ = GlobalUnlock(memory);
            text
        })();

        let _ = CloseClipboard();
        result
    }
}

/// Custom title bar height when fully visible (also the window-control hit size).
const TITLE_BAR_HEIGHT: f32 = 34.0;
/// Leftover strip while auto-hidden so the top window edge stays an egui hit
/// target (native tiles cannot steal those pixels).
const TITLE_BAR_COLLAPSED_HEIGHT: f32 = 2.0;
/// How close the pointer must be to the top of the window to reveal the bar.
const TITLE_BAR_HOVER_ZONE: f32 = 12.0;
/// Slide/fade duration when the title bar auto-hides or comes back.
const TITLE_BAR_ANIM_SECS: f32 = 0.16;

#[cfg(windows)]
fn find_saved_window<'a>(
    saved_title: &str,
    saved_exe: Option<&str>,
    windows: &'a [WindowInfo],
    claimed_hwnds: &HashSet<isize>,
) -> Option<&'a WindowInfo> {
    let available = |window: &&WindowInfo| !claimed_hwnds.contains(&window.hwnd);
    let usable_exe = saved_exe
        .filter(|name| !name.trim().is_empty() && !name.trim().eq_ignore_ascii_case("unknown"));

    if let Some(saved_exe) = usable_exe {
        if let Some(window) = windows.iter().filter(available).find(|window| {
            window.exe_name.eq_ignore_ascii_case(saved_exe) && window.title == saved_title
        }) {
            return Some(window);
        }
        if let Some(window) = windows
            .iter()
            .filter(available)
            .find(|window| window.exe_name.eq_ignore_ascii_case(saved_exe))
        {
            return Some(window);
        }
    }

    windows
        .iter()
        .filter(available)
        .find(|window| window.title == saved_title)
}

#[cfg(windows)]
fn browser_capture_fps(preview: &Preview) -> u32 {
    let requested = preview.target_fps.max(1);
    if preview.browser_waiting_for_content {
        requested.min(TRANSPARENT_BROWSER_STARTUP_FPS)
    } else {
        requested
    }
}

fn title_bar_ease(t: f32) -> f32 {
    let inv = 1.0 - t;
    1.0 - inv * inv * inv
}

#[cfg(windows)]
#[derive(Clone, Copy)]
struct BrowserTilePlacement {
    /// Full tile rect, which determines the page's stable viewport and zoom.
    page_rect: egui::Rect,
    /// Portion of the tile allowed to become a native window inside the canvas.
    visible_rect: egui::Rect,
}

#[cfg(windows)]
fn browser_tile_screen_rect(
    canvas: &CanvasState,
    preview: &Preview,
    canvas_rect: egui::Rect,
) -> egui::Rect {
    preview
        .viewport_pin
        .map(|pin| pin.rect(canvas_rect))
        .unwrap_or_else(|| canvas.canvas_rect_to_screen(preview.rect(), canvas_rect))
}

/// Recover the full page rectangle behind a cropped browser tile. The visible
/// tile represents `crop_uv` within this rectangle, so keeping the native
/// WebView at the full size preserves page layout and aligns pointer input with
/// the captured texture.
#[cfg(windows)]
fn uncropped_rect(visible_rect: egui::Rect, crop_uv: Option<(f32, f32, f32, f32)>) -> egui::Rect {
    let Some((min_u, min_v, max_u, max_v)) = crop_uv else {
        return visible_rect;
    };
    let u_span = (max_u - min_u).max(0.001);
    let v_span = (max_v - min_v).max(0.001);
    let full_size = egui::vec2(
        visible_rect.width() / u_span,
        visible_rect.height() / v_span,
    );
    egui::Rect::from_min_size(
        visible_rect.min - egui::vec2(min_u * full_size.x, min_v * full_size.y),
        full_size,
    )
}

#[cfg(windows)]
fn browser_page_screen_rect(
    canvas: &CanvasState,
    preview: &Preview,
    canvas_rect: egui::Rect,
) -> egui::Rect {
    uncropped_rect(
        browser_tile_screen_rect(canvas, preview, canvas_rect),
        preview.crop_uv,
    )
}

#[cfg(windows)]
fn browser_capture_display_size(preview: &Preview) -> Vec2 {
    let visible_size = preview
        .viewport_pin
        .map(|pin| pin.size_vec2())
        .unwrap_or(preview.size);
    uncropped_rect(
        egui::Rect::from_min_size(Pos2::ZERO, visible_size),
        preview.crop_uv,
    )
    .size()
}

fn restore_layout_tile_state(preview: &mut Preview, saved: &PreviewLayout) {
    preview.lock_aspect_ratio = saved.lock_aspect_ratio;
    preview.crop_uv = saved.crop_uv;
    preview.stream_audio = saved.stream_audio;
    preview.playlist_group = saved.playlist_group;
    preview.viewport_pin = saved.viewport_pin;
    preview.created_at = Instant::now() - Duration::from_secs(1);
    preview.left_click_disabled = saved.left_click_disabled;
    preview.manually_frozen = saved.manually_frozen;
    preview.capture_paused = saved.manually_frozen;
    preview.capture_hibernated = false;
    preview.capture_offscreen_since = None;
    if preview.is_video() && saved.manually_frozen {
        preview.video_playback.paused = true;
    }
}

fn restore_removed_tile_state(preview: &mut Preview, removed: &RemovedPreviewInfo) {
    preview.set_fps_preset(removed.fps_preset);
    preview.crop_uv = removed.crop_uv;
    preview.stream_audio = removed.stream_audio;
    preview.viewport_pin = removed.viewport_pin;
    preview.left_click_disabled = removed.left_click_disabled;
    preview.z_order = removed.z_order;
    preview.lock_aspect_ratio = removed.lock_aspect_ratio;
    preview.manually_frozen = removed.manually_frozen;
    preview.capture_paused = removed.manually_frozen;
    preview.playlist_group = removed.playlist_group;
}

#[cfg(windows)]
fn restore_browser_geometry(preview: &mut Preview, saved: &PreviewLayout) {
    restore_layout_tile_state(preview, saved);
}

fn original_image_path(source: &std::path::Path) -> Result<std::path::PathBuf, String> {
    if !source.is_file() {
        return Err(format!("Image file does not exist: {}", source.display()));
    }
    std::path::absolute(source)
        .map_err(|error| format!("Could not resolve the original image path: {error}"))
}

/// Initial image tile size, fitted inside 640×480 while preserving aspect.
fn media_tile_size(width: u32, height: u32) -> Vec2 {
    let width = width.max(1) as f32;
    let height = height.max(1) as f32;
    let scale = (640.0 / width).min(480.0 / height);
    Vec2::new(width * scale, height * scale)
}

#[cfg(windows)]
fn hwnd_process_id(hwnd: isize) -> u32 {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut pid));
    }
    pid
}

#[cfg(windows)]
fn available_tool_path(
    status: &ToolStatus,
    name: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    match status {
        ToolStatus::Checking => Ok(None),
        ToolStatus::Available { path, .. } => Ok(Some(path.clone())),
        ToolStatus::Invalid { error, .. } => Err(format!("{name} is invalid: {error}")),
        ToolStatus::Missing => Err(format!("{name} was not found. Configure it in Settings.")),
    }
}

#[cfg(windows)]
fn video_launch_for_source(
    source: &VideoSource,
    streamlink_status: &ToolStatus,
    start_paused: bool,
) -> Result<Option<VideoLaunch>, String> {
    let streamlink_path = match source {
        VideoSource::LocalFile { path } => {
            if !path.is_file() {
                return Err(format!(
                    "The video file no longer exists: {}",
                    path.display()
                ));
            }
            None
        }
        VideoSource::Stream { .. } => {
            let Some(streamlink_path) = available_tool_path(streamlink_status, "Streamlink")?
            else {
                return Ok(None);
            };
            Some(streamlink_path)
        }
    };
    Ok(Some(VideoLaunch {
        source: source.clone(),
        streamlink_path,
        start_paused,
        wallpaper: false,
    }))
}

#[cfg(windows)]
fn resumable_video_position(playback: &VideoPlaybackState) -> Option<f64> {
    playback.time_pos.filter(|position| {
        position.is_finite()
            && playback
                .duration
                .is_some_and(|duration| duration.is_finite() && duration > 0.0)
    })
}

#[cfg(windows)]
fn preview_video_status(state: &VideoPlaybackState, paused_on_restore: bool) -> VideoTileStatus {
    if !state.connected {
        if paused_on_restore {
            VideoTileStatus::PausedOnRestore
        } else {
            VideoTileStatus::Starting
        }
    } else if state.paused_for_cache || (!state.paused && state.core_idle) {
        VideoTileStatus::Buffering
    } else if paused_on_restore {
        VideoTileStatus::PausedOnRestore
    } else {
        VideoTileStatus::Ready
    }
}

/// Canvas right-click "Add Window..." popup: a small searchable list shown
/// at the click position so windows can be added without the sidebar.
struct QuickAddPopup {
    /// Where to place the new preview (canvas coordinates).
    canvas_pos: Pos2,
    /// Where to anchor the popup (screen coordinates).
    screen_pos: Pos2,
    /// Snapshot of open windows, taken when the popup was opened.
    windows: Vec<WindowInfo>,
    search: String,
}

struct AddBrowserDialog {
    position: Pos2,
    url: String,
    error: Option<String>,
    /// Existing browser tile being retargeted ("Change URL..."); None = create new.
    target: Option<PreviewId>,
    /// The URL field grabs focus once when the dialog opens.
    focused: bool,
}

#[cfg(windows)]
struct AddStreamDialog {
    position: Pos2,
    url: String,
    quality: String,
    qualities: Vec<String>,
    error: Option<String>,
    probe_error: Option<String>,
    probe_due: Option<Instant>,
    probe_receiver: Option<external_tools::StreamQualityProbe>,
    probing_url: String,
    streamlink_path: std::path::PathBuf,
    focused: bool,
}

#[derive(Clone, Copy)]
enum WorkspaceDialogKind {
    Create,
    Duplicate,
    Rename,
}

struct WorkspaceDialog {
    kind: WorkspaceDialogKind,
    name: String,
    focused: bool,
}

enum WorkspaceMenuAction {
    Switch(String),
    OpenDialog(WorkspaceDialogKind),
    RestorePreviousSave,
    ConfirmDelete,
}

#[derive(Clone, Copy)]
enum CanvasViewDialogKind {
    Create,
    Rename(usize),
}

struct CanvasViewDialog {
    kind: CanvasViewDialogKind,
    name: String,
    focused: bool,
    error: Option<String>,
}

struct DeletedCanvasView {
    removed_at: Instant,
    index: usize,
    view: CanvasView,
    was_active: bool,
}

enum CanvasViewMenuAction {
    Jump(usize),
    Previous,
    Next,
    OpenCreateDialog,
    OpenRenameDialog(usize),
    Update(usize),
    Delete(usize),
}

fn canvas_views_menu_contents(
    ui: &mut egui::Ui,
    canvas_views: &[CanvasView],
    active_canvas_view: Option<usize>,
    active_canvas_view_modified: bool,
    keyboard_shortcuts: &HotkeyBindings,
) -> Option<CanvasViewMenuAction> {
    let mut action = None;
    if canvas_views.is_empty() {
        ui.label(egui::RichText::new("No saved viewpoints").weak());
    } else {
        let previous_label = format!(
            "Previous Viewpoint ({})",
            keyboard_shortcuts
                .get(HotkeySlot::PreviousCanvasView)
                .display()
        );
        if ui.button(previous_label).clicked() {
            action = Some(CanvasViewMenuAction::Previous);
            ui.close_menu();
        }
        let next_label = format!(
            "Next Viewpoint ({})",
            keyboard_shortcuts.get(HotkeySlot::NextCanvasView).display()
        );
        if ui.button(next_label).clicked() {
            action = Some(CanvasViewMenuAction::Next);
            ui.close_menu();
        }
        ui.separator();
        for (index, view) in canvas_views.iter().enumerate() {
            ui.push_id(index, |ui| {
                ui.horizontal(|ui| {
                    let is_active = active_canvas_view == Some(index);
                    let mut label = if is_active {
                        format!("● {}", view.name)
                    } else {
                        view.name.clone()
                    };
                    if is_active && active_canvas_view_modified {
                        label.push_str(" (modified)");
                    }
                    if let Some(slot) = HotkeySlot::CANVAS_VIEWS.get(index) {
                        label.push_str(&format!("  {}", keyboard_shortcuts.get(*slot).display()));
                    }
                    let text = if is_active {
                        egui::RichText::new(label).color(if active_canvas_view_modified {
                            egui::Color32::from_rgb(220, 180, 95)
                        } else {
                            egui::Color32::from_rgb(125, 185, 255)
                        })
                    } else {
                        egui::RichText::new(label)
                    };
                    if ui
                        .button(text)
                        .on_hover_text("Move smoothly to this saved viewpoint")
                        .clicked()
                    {
                        action = Some(CanvasViewMenuAction::Jump(index));
                        ui.close_menu();
                    }
                    ui.menu_button(egui_phosphor::regular::DOTS_THREE, |ui| {
                        if ui.button("Update from Current Viewpoint").clicked() {
                            action = Some(CanvasViewMenuAction::Update(index));
                            ui.close_menu();
                        }
                        if ui.button("Rename...").clicked() {
                            action = Some(CanvasViewMenuAction::OpenRenameDialog(index));
                            ui.close_menu();
                        }
                        if ui.button("Delete").clicked() {
                            action = Some(CanvasViewMenuAction::Delete(index));
                            ui.close_menu();
                        }
                    });
                });
            });
        }
        ui.separator();
    }
    if ui.button("Save Current Viewpoint...").clicked() {
        action = Some(CanvasViewMenuAction::OpenCreateDialog);
        ui.close_menu();
    }
    action
}

enum SettingsAction {
    Browse(ToolKind),
    UseAutoDetected(ToolKind),
    Rescan(ToolKind),
}

#[cfg(windows)]
struct PendingBrowserTile {
    url: String,
    fps: FpsPreset,
    muted: bool,
    shown_once: bool,
    /// Layout restores wait for a visible viewport and are rate-limited.
    /// Direct user additions and undo actions start immediately.
    restore_deferred: bool,
}

#[cfg(windows)]
fn mark_pending_browser_placeholders_shown(
    pending_tiles: &mut HashMap<PreviewId, PendingBrowserTile>,
) -> bool {
    let awaiting_first_paint = pending_tiles.values().any(|pending| !pending.shown_once);
    for pending in pending_tiles.values_mut() {
        pending.shown_once = true;
    }
    awaiting_first_paint
}

#[cfg(windows)]
#[derive(Clone, Copy)]
struct PendingVideoTile {
    start_paused: bool,
    /// Restored tiles wait until their placeholder has been painted once.
    shown_once: bool,
    /// Cleared after a definitive launch failure and re-enabled by tool changes
    /// or an explicit play request.
    retry_ready: bool,
}

#[cfg(windows)]
struct PendingVideoFreeze {
    receiver: std::sync::mpsc::Receiver<Option<VideoSnapshot>>,
    requested_at: Instant,
}

#[cfg(windows)]
#[derive(Clone)]
struct FrozenVideoCheckpoint {
    playback: VideoPlaybackState,
    seek_position: Option<f64>,
}

#[cfg(windows)]
struct PlaylistThumbnailJob {
    playlist_id: PreviewId,
    path: std::path::PathBuf,
    receiver: video::VideoThumbnailReceiver,
}

#[cfg(windows)]
fn restored_video_ready(
    pending: &PendingVideoTile,
    tile_rect: Option<egui::Rect>,
    viewport: Option<egui::Rect>,
    viewport_pinned: bool,
) -> bool {
    pending.start_paused
        && pending.shown_once
        && pending.retry_ready
        && (viewport_pinned
            || tile_rect
                .zip(viewport)
                .is_some_and(|(tile, viewport)| tile.intersects(viewport)))
}

fn video_session_is_stale(
    id: PreviewId,
    wallpaper_video_active: bool,
    preview_is_video: bool,
) -> bool {
    if id == WALLPAPER_VIDEO_ID {
        !wallpaper_video_active
    } else {
        !preview_is_video
    }
}

#[cfg(windows)]
fn wallpaper_hibernation_due(since: Instant, now: Instant) -> bool {
    now.saturating_duration_since(since) >= WALLPAPER_HIBERNATE_AFTER
}

fn adjacent_canvas_view_index(
    active: Option<usize>,
    view_count: usize,
    forward: bool,
) -> Option<usize> {
    if view_count == 0 {
        return None;
    }
    match (active.filter(|index| *index < view_count), forward) {
        (Some(index), true) => Some((index + 1) % view_count),
        (Some(index), false) => Some((index + view_count - 1) % view_count),
        (None, true) => Some(0),
        (None, false) => Some(view_count - 1),
    }
}

fn active_canvas_view_after_deletion(active: Option<usize>, deleted_index: usize) -> Option<usize> {
    match active {
        Some(index) if index == deleted_index => None,
        Some(index) if index > deleted_index => Some(index - 1),
        other => other,
    }
}

fn active_canvas_view_after_insertion(
    active: Option<usize>,
    inserted_index: usize,
) -> Option<usize> {
    active.map(|index| {
        if index >= inserted_index {
            index + 1
        } else {
            index
        }
    })
}

fn camera_matches_canvas_view(center: Pos2, zoom: f32, view: &CanvasView) -> bool {
    let target = Pos2::new(view.center.0, view.center.1);
    let screen_distance = center.distance(target) * zoom.max(0.01);
    let zoom_distance = (zoom.max(0.01) / view.zoom.max(0.01)).ln().abs();
    screen_distance <= 1.0 && zoom_distance <= 0.001
}

#[cfg(windows)]
fn restored_browser_ready(
    pending: &PendingBrowserTile,
    tile_rect: Option<egui::Rect>,
    viewport: Option<egui::Rect>,
    viewport_pinned: bool,
) -> bool {
    pending.shown_once
        && pending.restore_deferred
        && (viewport_pinned
            || tile_rect
                .zip(viewport)
                .is_some_and(|(tile, viewport)| tile.intersects(viewport)))
}

/// Main application state
pub struct PluriviewApp {
    brand_icon: egui::TextureHandle,
    /// Canvas state (pan, zoom, selection)
    pub canvas: CanvasState,

    /// Manages all preview windows
    pub preview_manager: PreviewManager,

    /// Window picker panel state
    pub window_picker: WindowPicker,

    /// Capture coordinator for managing window captures
    pub capture_coordinator: CaptureCoordinator,

    /// Windows-only in-process libmpv playback cores and renderers.
    #[cfg(windows)]
    video_manager: VideoManager,
    /// Minimized-window grace period before the muted wallpaper decoder is
    /// destroyed. While present, playback is already paused.
    #[cfg(windows)]
    wallpaper_sleep_since: Option<Instant>,
    /// Prevent `ensure_wallpaper_video` from immediately recreating a decoder
    /// intentionally released while the window is minimized.
    #[cfg(windows)]
    wallpaper_hibernated: bool,
    /// Video placeholders waiting for optional tools to finish validation.
    #[cfg(windows)]
    pending_video_tiles: HashMap<PreviewId, PendingVideoTile>,
    /// Last restored video startup, used to stagger mpv/Streamlink processes.
    #[cfg(windows)]
    last_restored_video_start: Option<Instant>,
    /// Restored sessions stay visibly paused until mpv reports playback.
    #[cfg(windows)]
    restored_paused_videos: HashSet<PreviewId>,
    /// Direct renderers waiting for their one-frame GL readback before their
    /// libmpv cores can be destroyed.
    #[cfg(windows)]
    pending_video_freezes: HashMap<PreviewId, PendingVideoFreeze>,
    /// Playback state retained while libmpv is fully unloaded.
    #[cfg(windows)]
    frozen_video_checkpoints: HashMap<PreviewId, FrozenVideoCheckpoint>,
    /// Checkpoints waiting to be applied after a recreated source is ready.
    #[cfg(windows)]
    video_resume_checkpoints: HashMap<PreviewId, FrozenVideoCheckpoint>,
    /// Persistent in-process timeline preview cores for seekable video tiles.
    #[cfg(windows)]
    seek_preview_manager: SeekPreviewManager,
    /// Lazy playlist poster decoding. Only two helper processes may run at once.
    #[cfg(windows)]
    playlist_thumbnail_queue: VecDeque<(PreviewId, std::path::PathBuf)>,
    #[cfg(windows)]
    playlist_thumbnail_jobs: Vec<PlaylistThumbnailJob>,
    /// Persistent relationship key allocated to each folder/video pair.
    #[cfg(windows)]
    next_playlist_group: u64,

    /// Is the window picker panel open?
    pub picker_open: bool,

    /// Hide app chrome without changing the user's sidebar preference.
    canvas_only: bool,

    /// Current animated title-bar height, used by the frameless resize border.
    title_bar_shown_height: f32,

    /// True while a File/View/Help menu is open, so auto-hide cannot collapse
    /// the bar out from under the dropdown.
    title_bar_menu_open: bool,

    /// Storage for persistence
    storage: Option<Storage>,

    /// App-global settings, kept outside workspace layouts.
    app_config: AppConfig,

    /// Optional Streamlink discovery. mpv.exe is scanned silently for playlist
    /// thumbnails only; playback uses bundled libmpv.
    external_tools: ExternalTools,

    /// Show app-global Settings dialog.
    show_settings: bool,

    /// Last config persistence error, shown inside Settings.
    config_error: Option<String>,

    /// Named workspace catalog introduced in v0.5.
    workspaces: WorkspaceIndex,

    /// Create, duplicate, or rename dialog.
    workspace_dialog: Option<WorkspaceDialog>,

    /// Named camera positions belonging to the active workspace.
    canvas_views: Vec<CanvasView>,

    /// Last selected view remains active while its camera is moving; manual
    /// navigation leaves it selected but marks it modified in the menu.
    active_canvas_view: Option<usize>,

    /// Create or rename a workspace-local canvas view.
    canvas_view_dialog: Option<CanvasViewDialog>,

    /// Most recently deleted view, retained briefly for a recoverable Undo.
    deleted_canvas_view: Option<DeletedCanvasView>,

    /// Confirmation guard for deleting the active workspace.
    confirm_workspace_delete: bool,

    /// Last workspace persistence error, shown as a dismissible dialog.
    workspace_error: Option<String>,

    /// A corrupt catalog must never be replaced by a blank in-memory fallback.
    workspace_persistence_ready: bool,

    /// Periodic saves compare serialized persisted state instead of writing
    /// unchanged JSON or requiring every canvas mutation to set a dirty flag.
    workspace_save: WorkspaceSaveState,
    last_workspace_autosave_check: Instant,

    /// System tray manager
    tray_manager: Option<TrayManager>,

    /// Has the window HWND been set for the tray manager?
    hwnd_set: bool,

    /// Show About dialog
    show_about: bool,

    /// Show Keyboard Shortcuts dialog
    show_shortcuts: bool,

    /// Low-level keyboard tracker keeps Numpad keys distinct and continues to
    /// receive configured shortcuts while an interactive WebView has focus.
    hotkey_tracker: HotkeyTracker,

    /// Settings row currently waiting for the user's next one- or two-key chord.
    hotkey_recording: Option<HotkeySlot>,

    /// First key held while the recorder waits to see whether this is a pair.
    hotkey_recording_first: Option<u16>,

    /// Validation feedback for the keyboard shortcut editor.
    hotkey_error: Option<String>,

    /// Active region selector overlay (if any)
    region_selector: Option<RegionSelector>,

    /// Preview ID that the region selector is for
    region_select_preview_id: Option<PreviewId>,

    /// Active canvas right-click "Add Window..." popup, if any.
    quick_add: Option<QuickAddPopup>,

    /// Last image import/decode error, shown as a dismissible dialog.
    media_error: Option<String>,

    /// Missing optional tools encountered by a video creation action.
    external_tool_error: Option<String>,

    /// Last mpv control command that could not be delivered.
    video_action_error: Option<String>,

    /// Main window HWND, cached from eframe on the first frame.
    main_hwnd: Option<isize>,

    /// Main window geometry, mirrored every frame so the layout can be saved
    /// from `on_exit`, where no `Context` is available.
    window_layout: WindowLayout,

    /// Set when the layout was saved maximized; applied on the first frame,
    /// since the window has to be *built* at its restored geometry.
    pending_maximize: bool,

    /// Recently added browser URLs, newest first.
    recent_urls: Vec<String>,

    #[cfg(windows)]
    browser: BrowserManager,
    #[cfg(windows)]
    pending_browser_tiles: HashMap<PreviewId, PendingBrowserTile>,
    /// Last restored WebView startup, used to keep heavyweight pages staggered.
    #[cfg(windows)]
    last_restored_browser_start: Option<Instant>,
    #[cfg(windows)]
    add_browser: Option<AddBrowserDialog>,
    #[cfg(windows)]
    add_stream: Option<AddStreamDialog>,
    /// When the current browser interaction mode started (focus grace period).
    #[cfg(windows)]
    browser_activated_at: Option<Instant>,

    /// OBS-style stream audio monitor target: `(device id, friendly name)`.
    /// None = off (default). When set, browser-tile audio is replayed from
    /// this process to that device so Discord/OBS per-app capture hears it.
    #[cfg(windows)]
    monitor_device: Option<(String, String)>,
    /// Running monitor pipeline, present while tiles exist and a device is set.
    #[cfg(windows)]
    audio_monitor: Option<crate::audio::AudioMonitor>,
    /// Last time the monitor checked the WebView2 browser process PID.
    #[cfg(windows)]
    audio_monitor_checked: Option<Instant>,
    /// Per-window process loopbacks for tiles with SA enabled.
    #[cfg(windows)]
    window_audio_monitors: HashMap<u32, crate::audio::AudioMonitor>,

    /// Last scan for applications backing inactive saved window tiles.
    #[cfg(windows)]
    last_inactive_window_scan: Option<Instant>,
}

impl PluriviewApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        // Register phosphor icon glyphs alongside the default font so we can
        // use crisp vector icons instead of emoji/text glyphs in the UI.
        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        _cc.egui_ctx.set_fonts(fonts);

        let storage = Storage::new();
        let (app_config, config_error) = match &storage {
            Some(storage) => match storage.load_config() {
                Ok(config) => (config, None),
                Err(error) => (
                    AppConfig::default(),
                    Some(format!("Could not load settings: {error}")),
                ),
            },
            None => (
                AppConfig::default(),
                Some("Settings storage is unavailable.".to_owned()),
            ),
        };
        let external_tools = ExternalTools::new(app_config.external_tools.streamlink_path.clone());
        let (workspaces, workspace_error, workspace_persistence_ready) = match &storage {
            Some(storage) => match storage.load_or_initialize_workspaces() {
                Ok(index) => (index, None, true),
                Err(error) => (
                    WorkspaceIndex::default(),
                    Some(format!("Could not load workspaces: {error}")),
                    false,
                ),
            },
            None => (
                WorkspaceIndex::default(),
                Some("Workspace storage is unavailable.".to_owned()),
                false,
            ),
        };
        let tray_manager = TrayManager::new();

        #[cfg(debug_assertions)]
        if tray_manager.is_some() {
            println!("System tray initialized");
        } else {
            eprintln!("Failed to initialize system tray");
        }

        let mut app = Self {
            brand_icon: _cc.egui_ctx.load_texture(
                "pluriview-brand-icon",
                egui::ColorImage::from_rgba_unmultiplied(
                    [256, 256],
                    &crate::tray::create_app_icon_rgba(256),
                ),
                egui::TextureOptions::LINEAR,
            ),
            canvas: CanvasState::default(),
            preview_manager: PreviewManager::new(),
            window_picker: WindowPicker::new(),
            capture_coordinator: CaptureCoordinator::new(),
            #[cfg(windows)]
            video_manager: VideoManager::new(),
            #[cfg(windows)]
            wallpaper_sleep_since: None,
            #[cfg(windows)]
            wallpaper_hibernated: false,
            #[cfg(windows)]
            pending_video_tiles: HashMap::new(),
            #[cfg(windows)]
            last_restored_video_start: None,
            #[cfg(windows)]
            restored_paused_videos: HashSet::new(),
            #[cfg(windows)]
            pending_video_freezes: HashMap::new(),
            #[cfg(windows)]
            frozen_video_checkpoints: HashMap::new(),
            #[cfg(windows)]
            video_resume_checkpoints: HashMap::new(),
            #[cfg(windows)]
            seek_preview_manager: SeekPreviewManager::new(),
            #[cfg(windows)]
            playlist_thumbnail_queue: VecDeque::new(),
            #[cfg(windows)]
            playlist_thumbnail_jobs: Vec::new(),
            #[cfg(windows)]
            next_playlist_group: 1,
            picker_open: true,
            canvas_only: false,
            title_bar_shown_height: TITLE_BAR_HEIGHT,
            title_bar_menu_open: false,
            storage,
            app_config,
            external_tools,
            show_settings: false,
            config_error,
            workspaces,
            workspace_dialog: None,
            canvas_views: Vec::new(),
            active_canvas_view: None,
            canvas_view_dialog: None,
            deleted_canvas_view: None,
            confirm_workspace_delete: false,
            workspace_error,
            workspace_persistence_ready,
            workspace_save: WorkspaceSaveState::default(),
            last_workspace_autosave_check: Instant::now(),
            tray_manager,
            hwnd_set: false,
            show_about: false,
            show_shortcuts: false,
            hotkey_tracker: HotkeyTracker::default(),
            hotkey_recording: None,
            hotkey_recording_first: None,
            hotkey_error: None,
            region_selector: None,
            region_select_preview_id: None,
            quick_add: None,
            media_error: None,
            external_tool_error: None,
            video_action_error: None,
            main_hwnd: None,
            window_layout: WindowLayout::default(),
            pending_maximize: false,
            recent_urls: Vec::new(),
            #[cfg(windows)]
            browser: BrowserManager::new(),
            #[cfg(windows)]
            pending_browser_tiles: HashMap::new(),
            #[cfg(windows)]
            last_restored_browser_start: None,
            #[cfg(windows)]
            add_browser: None,
            #[cfg(windows)]
            add_stream: None,
            #[cfg(windows)]
            browser_activated_at: None,
            #[cfg(windows)]
            monitor_device: None,
            #[cfg(windows)]
            audio_monitor: None,
            #[cfg(windows)]
            audio_monitor_checked: None,
            #[cfg(windows)]
            window_audio_monitors: HashMap::new(),
            #[cfg(windows)]
            last_inactive_window_scan: None,
        };

        // Restore the active named workspace (or the migrated legacy autosave).
        app.load_active_workspace();
        app.pending_maximize = app.window_layout.maximized;

        app
    }

    /// Reserve a persisted video tile and activate it once its tools are ready.
    #[cfg(windows)]
    fn create_video_tile(
        &mut self,
        source: VideoSource,
        title: String,
        position: Pos2,
        size: Vec2,
        fps: FpsPreset,
        start_paused: bool,
    ) -> PreviewId {
        let title = video_tile_title(&source, Some(&title));
        let id = self.preview_manager.add_video_placeholder(
            source,
            title,
            position,
            size,
            fps,
            start_paused,
        );
        self.pending_video_tiles.insert(
            id,
            PendingVideoTile {
                start_paused,
                shown_once: !start_paused,
                retry_ready: true,
            },
        );
        if !start_paused {
            if let Err(error) = self.try_activate_video_tile(id) {
                self.mark_video_launch_failed(id, error);
            }
        }
        id
    }

    #[cfg(windows)]
    fn require_libmpv(action: &str) -> Result<(), String> {
        if crate::libmpv::runtime_is_available() {
            Ok(())
        } else {
            Err(format!(
                "{action} needs libmpv-2.dll in the lib folder next to pluriview.exe."
            ))
        }
    }

    #[cfg(windows)]
    fn add_local_video(&mut self, position: Pos2) {
        if let Err(error) = Self::require_libmpv("Adding a video") {
            self.media_error = Some(error);
            return;
        }
        let Some(path) = media::pick_video_file(self.main_hwnd) else {
            return;
        };
        match self.add_local_video_path(path, position) {
            Ok(id) => self.canvas.selection = vec![id],
            Err(error) => self.media_error = Some(error),
        }
    }

    /// Create a playing MPV tile for a local path supplied by the picker or
    /// an OS drag-and-drop operation.
    #[cfg(windows)]
    fn add_local_video_path(
        &mut self,
        path: std::path::PathBuf,
        position: Pos2,
    ) -> Result<PreviewId, String> {
        Self::require_libmpv("Adding a video")?;
        if !path.is_file() {
            return Err(format!("Video file does not exist: {}", path.display()));
        }
        let title = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| path.display().to_string());
        Ok(self.create_video_tile(
            VideoSource::LocalFile { path },
            title,
            position,
            Vec2::new(640.0, 360.0),
            FpsPreset::Medium,
            false,
        ))
    }

    /// Create a linked video/player pair from a dropped folder. Scanning is
    /// intentionally top-level so an unexpectedly deep media archive cannot
    /// flood the canvas or thumbnail queue.
    #[cfg(windows)]
    fn add_video_folder(
        &mut self,
        folder: std::path::PathBuf,
        position: Pos2,
    ) -> Result<(PreviewId, PreviewId), String> {
        Self::require_libmpv("Adding a video folder")?;
        let playlist = FolderPlaylist::scan(folder.clone(), None)?;
        let first = playlist
            .selected
            .clone()
            .ok_or_else(|| "The video folder is empty".to_owned())?;
        let group = self.next_playlist_group;
        self.next_playlist_group = self.next_playlist_group.saturating_add(1);

        let video_id = self.add_local_video_path(first, position)?;
        if let Some(preview) = self.preview_manager.get_mut(video_id) {
            preview.playlist_group = Some(group);
        }

        let title = folder
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("Video folder")
            .to_owned();
        let playlist_id = self.preview_manager.add_folder_playlist(
            playlist,
            title,
            position + Vec2::new(664.0, 0.0),
            Vec2::new(380.0, 360.0),
            group,
            Some(video_id),
        );
        Ok((video_id, playlist_id))
    }

    #[cfg(windows)]
    fn restore_folder_playlist(
        &mut self,
        layout: &FolderPlaylistLayout,
        title: String,
        position: Pos2,
        size: Vec2,
        group: u64,
        linked_video: Option<PreviewId>,
    ) -> PreviewId {
        let playlist = FolderPlaylist::from_layout(layout);
        self.preview_manager.add_folder_playlist(
            playlist,
            title,
            position,
            size,
            group,
            linked_video,
        )
    }

    #[cfg(windows)]
    fn open_add_stream(&mut self, position: Pos2) {
        if let Err(error) = Self::require_libmpv("Adding a stream") {
            self.media_error = Some(error);
            return;
        }
        let streamlink_path = match self.external_tools.status(ToolKind::Streamlink) {
            ToolStatus::Available { path, .. } => path.clone(),
            _ => {
                self.external_tool_error = Some(
                    "Adding a stream requires Streamlink. Configure the unavailable tool in Settings."
                        .to_owned(),
                );
                return;
            }
        };
        self.add_stream = Some(AddStreamDialog {
            position,
            url: String::new(),
            quality: "best".to_owned(),
            qualities: Vec::new(),
            error: None,
            probe_error: None,
            probe_due: None,
            probe_receiver: None,
            probing_url: String::new(),
            streamlink_path,
            focused: false,
        });
    }

    /// Attach an in-process libmpv renderer to a video placeholder.
    #[cfg(windows)]
    fn try_activate_video_tile(&mut self, id: PreviewId) -> Result<bool, String> {
        if self.video_manager.contains(id) {
            self.pending_video_tiles.remove(&id);
            return Ok(true);
        }
        let Some(preview) = self.preview_manager.get(id) else {
            self.pending_video_tiles.remove(&id);
            return Ok(false);
        };
        let Some(source) = preview.video_source.clone() else {
            self.pending_video_tiles.remove(&id);
            return Ok(false);
        };
        let target_fps = preview.fps_preset.as_u32();
        let start_paused = self
            .pending_video_tiles
            .get(&id)
            .is_some_and(|pending| pending.start_paused);
        let Some(launch) = video_launch_for_source(
            &source,
            self.external_tools.status(ToolKind::Streamlink),
            start_paused,
        )?
        else {
            if let Some(preview) = self.preview_manager.get_mut(id) {
                preview.video_status = if start_paused {
                    VideoTileStatus::PausedOnRestore
                } else {
                    VideoTileStatus::Starting
                };
            }
            return Ok(false);
        };

        if let Some(preview) = self.preview_manager.get_mut(id) {
            preview.video_status = if start_paused {
                VideoTileStatus::PausedOnRestore
            } else {
                VideoTileStatus::Starting
            };
        }
        let renderer = self.video_manager.launch(id, launch, target_fps)?;
        if let Some(preview) = self.preview_manager.get_mut(id) {
            preview.window_handle = None;
            preview.video_renderer = Some(renderer);
        }
        self.pending_video_tiles.remove(&id);
        if start_paused {
            self.restored_paused_videos.insert(id);
        }
        Ok(true)
    }

    #[cfg(windows)]
    fn mark_video_launch_failed(&mut self, id: PreviewId, error: String) {
        if let Some(preview) = self.preview_manager.get_mut(id) {
            preview.video_status = VideoTileStatus::Failed(error);
        }
        if let Some(pending) = self.pending_video_tiles.get_mut(&id) {
            pending.retry_ready = false;
        }
    }

    /// Reload an mpv/Streamlink tile from the beginning while preserving its
    /// playback settings. Normal Play never takes this process-restart path.
    #[cfg(windows)]
    fn reload_video_tile(&mut self, id: PreviewId) -> Result<(), String> {
        let playback = self
            .preview_manager
            .get(id)
            .filter(|preview| preview.is_video())
            .map(|preview| preview.video_playback.clone())
            .ok_or_else(|| "The video tile no longer exists".to_owned())?;

        self.capture_coordinator.stop_capture(id);
        self.pending_video_freezes.remove(&id);
        self.frozen_video_checkpoints.remove(&id);
        self.video_resume_checkpoints.remove(&id);
        self.video_manager.remove(id);
        self.pending_video_tiles.insert(
            id,
            PendingVideoTile {
                start_paused: false,
                shown_once: true,
                retry_ready: true,
            },
        );
        self.restored_paused_videos.remove(&id);
        if let Some(preview) = self.preview_manager.get_mut(id) {
            preview.window_handle = None;
            preview.video_renderer = None;
            preview.video_status = VideoTileStatus::Starting;
            preview.video_playback.connected = false;
            preview.video_playback.paused = true;
            preview.video_playback.time_pos = None;
            preview.video_playback.duration = None;
        }

        if self.try_activate_video_tile(id)? {
            let tile = self
                .video_manager
                .get_mut(id)
                .ok_or_else(|| "The reloaded video session was not created".to_owned())?;
            tile.session.set_volume(playback.volume)?;
            tile.session.set_muted(playback.muted)?;
            tile.session.set_speed(playback.speed)?;
            tile.session.set_looping(playback.looping)?;
            if let Some(track) = playback.audio_track {
                tile.session.select_audio_track(track)?;
            }
            if let Some(track) = playback.subtitle_track {
                tile.session.select_subtitle_track(track)?;
            } else {
                tile.session.disable_subtitles()?;
            }
        }
        Ok(())
    }

    #[cfg(windows)]
    fn pending_video_upkeep(&mut self, ctx: &egui::Context) {
        let immediate: Vec<_> = self
            .pending_video_tiles
            .iter()
            .filter_map(|(id, pending)| {
                let live = self
                    .preview_manager
                    .get(*id)
                    .is_some_and(|preview| !preview.manually_frozen);
                (!pending.start_paused && pending.retry_ready && live).then_some(*id)
            })
            .collect();
        for id in immediate {
            if let Err(error) = self.try_activate_video_tile(id) {
                self.mark_video_launch_failed(id, error);
            }
        }

        let viewport = self
            .canvas
            .last_screen_rect
            .map(|screen_rect| self.canvas.get_viewport(screen_rect));
        let slot_available = self
            .last_restored_video_start
            .is_none_or(|started| started.elapsed() >= RESTORED_VIDEO_START_INTERVAL);
        let restored_id = slot_available
            .then(|| {
                self.pending_video_tiles.iter().find_map(|(id, pending)| {
                    let preview = self.preview_manager.get(*id);
                    let eligible = preview.is_some_and(|preview| {
                        !preview.manually_frozen
                            && !matches!(preview.video_status, VideoTileStatus::Failed(_))
                    });
                    restored_video_ready(
                        pending,
                        preview.map(|preview| preview.rect()),
                        viewport,
                        preview.is_some_and(|preview| preview.viewport_pin.is_some()),
                    )
                    .then_some(*id)
                    .filter(|_| eligible)
                })
            })
            .flatten();

        if let Some(id) = restored_id {
            match self.try_activate_video_tile(id) {
                Ok(true) => self.last_restored_video_start = Some(Instant::now()),
                Ok(false) => {}
                Err(error) => self.mark_video_launch_failed(id, error),
            }
        }

        let visible_restored_pending = self.pending_video_tiles.iter().any(|(id, pending)| {
            let preview = self.preview_manager.get(*id);
            preview.is_some_and(|preview| {
                !preview.manually_frozen
                    && !matches!(preview.video_status, VideoTileStatus::Failed(_))
                    && restored_video_ready(
                        pending,
                        Some(preview.rect()),
                        viewport,
                        preview.viewport_pin.is_some(),
                    )
            })
        });
        if visible_restored_pending {
            let delay = self
                .last_restored_video_start
                .map(|started| RESTORED_VIDEO_START_INTERVAL.saturating_sub(started.elapsed()))
                .unwrap_or(Duration::ZERO);
            ctx.request_repaint_after(delay.max(Duration::from_millis(50)));
        }
    }

    /// Drain mpv IPC without waiting and mirror the latest state onto previews.
    #[cfg(windows)]
    fn poll_video_manager(&mut self, ctx: &egui::Context) {
        let updates = self.video_manager.poll();
        if updates.is_empty() {
            self.restore_ready_video_checkpoints();
            return;
        }

        let mut changed = HashSet::new();
        let mut errors = HashMap::new();
        let mut pause_changed = HashSet::new();
        for (id, update) in updates {
            changed.insert(id);
            match update {
                VideoUpdate::PauseChanged => {
                    pause_changed.insert(id);
                }
                VideoUpdate::Error(error) => {
                    errors.insert(id, error);
                }
                VideoUpdate::Connected | VideoUpdate::Event => {}
            }
        }

        let mut auto_advance = Vec::new();
        for id in changed {
            if id == WALLPAPER_VIDEO_ID {
                if let Some(error) = errors.remove(&id) {
                    log::error!("Wallpaper video failed: {error}");
                    if let Some(wallpaper) = self.canvas.wallpaper.as_mut() {
                        wallpaper.error = Some(error.clone());
                    }
                    self.media_error = Some(error);
                }
                continue;
            }
            let Some(state) = self
                .video_manager
                .get(id)
                .map(|tile| tile.session.state().clone())
            else {
                continue;
            };
            if pause_changed.contains(&id) && !state.paused {
                self.restored_paused_videos.remove(&id);
            }
            if let Some(preview) = self.preview_manager.get_mut(id) {
                preview.video_playback.clone_from(&state);
                if let Some(title) = state
                    .media_title
                    .as_ref()
                    .filter(|title| is_usable_media_title(title))
                {
                    preview.title.clone_from(title);
                } else if !is_usable_media_title(&preview.title) {
                    if let Some(source) = preview.video_source.as_ref() {
                        preview.title = video_tile_title(source, None);
                    }
                }
                preview.video_status = errors
                    .remove(&id)
                    .map(VideoTileStatus::Failed)
                    .unwrap_or_else(|| {
                        preview_video_status(&state, self.restored_paused_videos.contains(&id))
                    });
            }
            if state.eof_reached && !state.looping {
                if let Some((playlist_id, path)) = self.preview_manager.all().find_map(|preview| {
                    let playlist = preview.folder_playlist.as_ref()?;
                    (preview.playlist_linked_video == Some(id) && playlist.autoplay)
                        .then(|| playlist.adjacent_path(1).map(|path| (preview.id, path)))
                        .flatten()
                }) {
                    auto_advance.push((playlist_id, path));
                }
            }
        }

        for (playlist_id, path) in auto_advance {
            if let Err(error) = self.play_playlist_path(playlist_id, path) {
                log::error!("Playlist autoplay failed: {error}");
                self.video_action_error = Some(format!("Playlist autoplay failed: {error}"));
            }
        }

        self.restore_ready_video_checkpoints();
        ctx.request_repaint();
    }

    #[cfg(windows)]
    fn request_seek_preview(&mut self, id: PreviewId, time: f64) {
        if !time.is_finite() || time < 0.0 {
            return;
        }
        let requested_time = time.round().max(0.0);
        let (source, already_showing, duration) = {
            let Some(preview) = self.preview_manager.get(id) else {
                return;
            };
            if !preview.supports_seek_preview() {
                return;
            }
            let Some(source) = preview.video_source.clone() else {
                return;
            };
            let already_showing = preview
                .seek_preview_time()
                .is_some_and(|current| (current - requested_time).abs() <= 0.5);
            (source, already_showing, preview.video_playback.duration)
        };
        let thumbnail_source = match source {
            VideoSource::LocalFile { path } => video::VideoThumbnailSource::LocalFile(path),
            VideoSource::Stream { url, quality } => {
                let ToolStatus::Available {
                    path: streamlink_path,
                    ..
                } = self.external_tools.status(ToolKind::Streamlink)
                else {
                    return;
                };
                video::VideoThumbnailSource::Stream {
                    streamlink_path: streamlink_path.clone(),
                    url,
                    quality,
                }
            }
        };

        if let Some(thumbnail) = self.seek_preview_manager.cached_frame(id, requested_time) {
            if !already_showing {
                if let Some(preview) = self.preview_manager.get_mut(id) {
                    preview.update_seek_preview(
                        thumbnail.time,
                        thumbnail.width,
                        thumbnail.height,
                        thumbnail.rgba,
                    );
                }
            }
        }
        if let Err(error) =
            self.seek_preview_manager
                .request(id, thumbnail_source, requested_time, duration)
        {
            log::debug!("Timeline preview could not start: {error}");
        }
    }

    #[cfg(windows)]
    fn poll_seek_previews(&mut self, ctx: &egui::Context) {
        for id in self.seek_preview_manager.expire_idle() {
            if let Some(preview) = self.preview_manager.get_mut(id) {
                preview.clear_seek_preview();
            }
        }
        for (id, thumbnail) in self.seek_preview_manager.poll() {
            if let Some(preview) = self.preview_manager.get_mut(id) {
                preview.update_seek_preview(
                    thumbnail.time,
                    thumbnail.width,
                    thumbnail.height,
                    thumbnail.rgba,
                );
            }
            ctx.request_repaint();
        }

        let active: HashSet<PreviewId> = self
            .preview_manager
            .all()
            .filter(|preview| preview.supports_seek_preview())
            .map(|preview| preview.id)
            .collect();
        self.seek_preview_manager.retain(|id| active.contains(&id));
        if self.seek_preview_manager.needs_repaint() {
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }

    #[cfg(windows)]
    fn poll_video_freezes(&mut self, ctx: &egui::Context) {
        let ids: Vec<_> = self.pending_video_freezes.keys().copied().collect();
        for id in ids {
            let completed = self.pending_video_freezes.get(&id).and_then(|pending| {
                match pending.receiver.try_recv() {
                    Ok(snapshot) => Some(snapshot),
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(None),
                    Err(std::sync::mpsc::TryRecvError::Empty)
                        if pending.requested_at.elapsed() >= Duration::from_secs(1) =>
                    {
                        // Resource release is more important than retaining a
                        // frame if painting was interrupted (for example while
                        // the app was minimized during the freeze request).
                        Some(None)
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                }
            });
            let Some(snapshot) = completed else {
                continue;
            };

            if let Some(snapshot) = snapshot {
                if let Some(preview) = self.preview_manager.get_mut(id) {
                    preview.update_frame(snapshot.width, snapshot.height, snapshot.rgba);
                }
            }
            if let Some(preview) = self.preview_manager.get_mut(id) {
                preview.video_renderer = None;
                preview.video_playback.connected = false;
                preview.video_playback.paused = true;
            }
            self.video_manager.remove(id);
            self.restored_paused_videos.remove(&id);
            self.pending_video_freezes.remove(&id);
            ctx.request_repaint();
        }
    }

    #[cfg(windows)]
    fn restore_ready_video_checkpoints(&mut self) {
        let ready: Vec<_> = self
            .video_resume_checkpoints
            .keys()
            .copied()
            .filter(|id| {
                self.video_manager
                    .get(*id)
                    .is_some_and(|tile| tile.session.source_ready())
            })
            .collect();

        for id in ready {
            let Some(checkpoint) = self.video_resume_checkpoints.get(&id).cloned() else {
                continue;
            };
            let mut failures = Vec::new();
            if let Some(tile) = self.video_manager.get_mut(id) {
                let session = &mut tile.session;
                let mut apply = |label: &str, result: Result<(), String>| {
                    if let Err(error) = result {
                        failures.push(format!("{label}: {error}"));
                    }
                };
                apply("volume", session.set_volume(checkpoint.playback.volume));
                apply("mute", session.set_muted(checkpoint.playback.muted));
                apply("speed", session.set_speed(checkpoint.playback.speed));
                apply("loop", session.set_looping(checkpoint.playback.looping));
                if let Some(position) = checkpoint.seek_position {
                    apply("position", session.seek_absolute(position));
                }
                if let Some(track) = checkpoint.playback.audio_track {
                    apply("audio track", session.select_audio_track(track));
                }
                if let Some(track) = checkpoint.playback.subtitle_track {
                    apply("subtitle track", session.select_subtitle_track(track));
                } else {
                    apply("subtitles", session.disable_subtitles());
                }
                if !checkpoint.playback.paused {
                    apply("playback", session.play());
                }
            }
            if !failures.is_empty() {
                log::warn!(
                    "Some resumed video settings could not be restored: {}",
                    failures.join("; ")
                );
            }
            self.video_resume_checkpoints.remove(&id);
            self.restored_paused_videos.remove(&id);
            if let Some(preview) = self.preview_manager.get_mut(id) {
                preview.video_playback = checkpoint.playback;
                preview.video_playback.connected = true;
            }
        }
    }

    #[cfg(windows)]
    fn handle_tile_activity_action(
        &mut self,
        ctx: &egui::Context,
        id: PreviewId,
        action: TileActivityAction,
    ) {
        let Some((already_frozen, is_browser, is_video, window, spout_sender, title, fps)) =
            self.preview_manager.get(id).map(|preview| {
                (
                    preview.manually_frozen,
                    preview.is_browser(),
                    preview.is_video(),
                    preview.window_handle.clone(),
                    preview.spout_sender.clone(),
                    preview.title.clone(),
                    preview.target_fps,
                )
            })
        else {
            return;
        };

        let freeze = action == TileActivityAction::Freeze;
        if already_frozen == freeze {
            return;
        }

        if freeze {
            if is_video {
                // Hover thumbnails own a separate libmpv core. A frozen video
                // must release that auxiliary decoder as well as playback.
                self.seek_preview_manager.remove(id);
                if let Some(preview) = self.preview_manager.get_mut(id) {
                    preview.clear_seek_preview();
                }
                let pending_was_playing = self
                    .pending_video_tiles
                    .get(&id)
                    .is_some_and(|pending| !pending.start_paused);
                let mut playback = self
                    .preview_manager
                    .get(id)
                    .map(|preview| preview.video_playback.clone())
                    .unwrap_or_default();
                if pending_was_playing {
                    playback.paused = false;
                }
                let seek_position = resumable_video_position(&playback);
                self.frozen_video_checkpoints.insert(
                    id,
                    FrozenVideoCheckpoint {
                        playback,
                        seek_position,
                    },
                );
                if let Some(pending) = self.pending_video_tiles.get_mut(&id) {
                    pending.start_paused = true;
                }
                if let Some(tile) = self.video_manager.get_mut(id) {
                    if let Err(error) = tile.session.set_paused(true) {
                        log::warn!("Could not pause a frozen video tile: {error}");
                    }
                }
                if let Some(receiver) = self.video_manager.schedule_snapshot(id, ctx) {
                    self.pending_video_freezes.insert(
                        id,
                        PendingVideoFreeze {
                            receiver,
                            requested_at: Instant::now(),
                        },
                    );
                }
            } else {
                // A manually frozen capture is fully stopped, not merely
                // throttled like an off-screen capture.
                self.capture_coordinator.stop_capture(id);
                if let Some(preview) = self.preview_manager.get_mut(id) {
                    preview.discard_pending_capture_frame();
                }
                if is_browser {
                    if let Some(host) = self.browser.get_mut(id) {
                        if let Err(error) = host.suspend() {
                            log::warn!("Could not suspend a frozen browser tile: {error}");
                        }
                    }
                    self.browser_activated_at = None;
                }
            }
            if let Some(preview) = self.preview_manager.get_mut(id) {
                preview.manually_frozen = true;
                preview.capture_paused = true;
                preview.capture_hibernated = false;
                preview.capture_offscreen_since = None;
                if is_video {
                    preview.video_playback.paused = true;
                }
            }
        } else {
            if let Some(preview) = self.preview_manager.get_mut(id) {
                preview.manually_frozen = false;
                preview.capture_paused = false;
                preview.capture_hibernated = false;
                preview.capture_offscreen_since = None;
            }

            if is_video {
                let checkpoint = self.frozen_video_checkpoints.remove(&id);
                let canceled_readback = self.pending_video_freezes.remove(&id).is_some();
                if canceled_readback && self.video_manager.contains(id) {
                    if checkpoint
                        .as_ref()
                        .is_some_and(|checkpoint| !checkpoint.playback.paused)
                    {
                        if let Some(tile) = self.video_manager.get_mut(id) {
                            if let Err(error) = tile.session.play() {
                                log::warn!("Could not resume a frozen video tile: {error}");
                            }
                        }
                    }
                } else if let Some(checkpoint) = checkpoint {
                    self.video_resume_checkpoints.insert(id, checkpoint);
                    self.pending_video_tiles.insert(
                        id,
                        PendingVideoTile {
                            start_paused: true,
                            shown_once: true,
                            retry_ready: true,
                        },
                    );
                    if let Err(error) = self.try_activate_video_tile(id) {
                        self.mark_video_launch_failed(id, error);
                    }
                }
            } else if is_browser {
                let resumed_hwnd = self
                    .browser
                    .get_mut(id)
                    .and_then(|host| match host.resume() {
                        Ok(()) => Some(host.hwnd()),
                        Err(error) => {
                            log::warn!("Could not resume a frozen browser tile: {error}");
                            None
                        }
                    });
                if let Some(hwnd) = resumed_hwnd {
                    self.capture_coordinator.start_capture(id, hwnd, title, fps);
                }
            } else if let Some(sender) = spout_sender {
                self.capture_coordinator
                    .start_spout_capture(id, sender, fps);
            } else if window.is_some() {
                if let Some(preview) = self.preview_manager.get_mut(id) {
                    preview.mark_window_inactive();
                }
            }
        }
        ctx.request_repaint();
    }

    #[cfg(windows)]
    fn handle_video_action(&mut self, ctx: &egui::Context, id: PreviewId, action: VideoAction) {
        if action == VideoAction::OpenSettings {
            self.show_settings = true;
            ctx.request_repaint();
            return;
        }

        if action == VideoAction::Reload {
            if let Err(error) = self.reload_video_tile(id) {
                log::error!("Video reload failed: {error}");
                self.mark_video_launch_failed(id, error.clone());
                self.video_action_error = Some(format!("Could not reload this video: {error}"));
            }
            ctx.request_repaint();
            return;
        }

        if let VideoAction::RequestSeekPreview(time) = action {
            self.request_seek_preview(id, time);
            ctx.request_repaint_after(Duration::from_millis(40));
            return;
        }

        if action == VideoAction::SetPaused(false)
            && !self.video_manager.contains(id)
            && self.pending_video_tiles.contains_key(&id)
        {
            if let Some(pending) = self.pending_video_tiles.get_mut(&id) {
                pending.start_paused = false;
                pending.shown_once = true;
                pending.retry_ready = true;
            }
            self.restored_paused_videos.remove(&id);
            if let Some(preview) = self.preview_manager.get_mut(id) {
                preview.video_status = VideoTileStatus::Starting;
            }
            if let Err(error) = self.try_activate_video_tile(id) {
                self.mark_video_launch_failed(id, error);
            }
            ctx.request_repaint();
            return;
        }

        let Some(playback) = self
            .preview_manager
            .get(id)
            .filter(|preview| preview.is_video())
            .map(|preview| preview.video_playback.clone())
        else {
            return;
        };

        let result = self
            .video_manager
            .get_mut(id)
            .ok_or_else(|| "The video session is not connected".to_owned())
            .and_then(|tile| match action {
                VideoAction::SetPaused(false) => tile.session.play(),
                VideoAction::SetPaused(true) => tile.session.set_paused(true),
                VideoAction::Reload => Ok(()),
                VideoAction::RequestSeekPreview(_) => Ok(()),
                VideoAction::SeekAbsolute(seconds) => tile.session.seek_absolute(seconds),
                VideoAction::SetVolume(volume) => tile.session.set_volume(volume),
                VideoAction::ToggleMute => tile.session.set_muted(!playback.muted),
                VideoAction::SetSpeed(speed) => tile.session.set_speed(speed),
                VideoAction::ToggleLoop => tile.session.set_looping(!playback.looping),
                VideoAction::SelectAudioTrack(track) => tile.session.select_audio_track(track),
                VideoAction::SelectSubtitleTrack(Some(track)) => {
                    tile.session.select_subtitle_track(track)
                }
                VideoAction::SelectSubtitleTrack(None) => tile.session.disable_subtitles(),
                VideoAction::OpenSettings => Ok(()),
            });

        if result.is_ok() {
            if let Some(preview) = self.preview_manager.get_mut(id) {
                match action {
                    VideoAction::SetPaused(paused) => {
                        preview.video_playback.paused = paused;
                    }
                    VideoAction::SetVolume(volume) => {
                        preview.video_playback.volume = volume.clamp(0.0, 100.0);
                    }
                    _ => {}
                }
            }
        }

        if let Err(error) = result {
            log::error!("Video control failed: {error}");
            if let Some(preview) = self.preview_manager.get_mut(id) {
                preview.video_status = VideoTileStatus::Failed(error.clone());
            }
            self.video_action_error = Some(format!("Could not control this video: {error}"));
        }
        ctx.request_repaint();
    }

    #[cfg(windows)]
    fn handle_playlist_action(
        &mut self,
        ctx: &egui::Context,
        id: PreviewId,
        action: PlaylistAction,
    ) {
        match action {
            PlaylistAction::RequestThumbnail(path) => {
                let key_exists = self
                    .playlist_thumbnail_queue
                    .iter()
                    .any(|queued| queued.0 == id && queued.1 == path)
                    || self
                        .playlist_thumbnail_jobs
                        .iter()
                        .any(|job| job.playlist_id == id && job.path == path);
                if !key_exists {
                    self.playlist_thumbnail_queue.push_back((id, path));
                }
            }
            PlaylistAction::ToggleAutoplay
            | PlaylistAction::ToggleShuffle
            | PlaylistAction::ToggleRepeat => {
                if let Some(playlist) = self
                    .preview_manager
                    .get_mut(id)
                    .and_then(|preview| preview.folder_playlist.as_mut())
                {
                    match action {
                        PlaylistAction::ToggleAutoplay => playlist.autoplay = !playlist.autoplay,
                        PlaylistAction::ToggleShuffle => playlist.shuffle = !playlist.shuffle,
                        PlaylistAction::ToggleRepeat => playlist.repeat = !playlist.repeat,
                        _ => unreachable!(),
                    }
                }
            }
            PlaylistAction::Rescan => {
                let layout = self
                    .preview_manager
                    .get(id)
                    .and_then(|preview| preview.folder_playlist.as_ref())
                    .map(FolderPlaylist::layout);
                if let Some(layout) = layout {
                    let playlist = FolderPlaylist::from_layout(&layout);
                    if let Some(preview) = self.preview_manager.get_mut(id) {
                        preview.folder_playlist = Some(playlist);
                    }
                    self.playlist_thumbnail_queue
                        .retain(|(playlist_id, _)| *playlist_id != id);
                }
            }
            PlaylistAction::Select(path) => {
                if let Err(error) = self.play_playlist_path(id, path) {
                    self.video_action_error =
                        Some(format!("Could not play this folder video: {error}"));
                }
            }
            PlaylistAction::Previous | PlaylistAction::Next => {
                let direction = if action == PlaylistAction::Previous {
                    -1
                } else {
                    1
                };
                let path = self
                    .preview_manager
                    .get(id)
                    .and_then(|preview| preview.folder_playlist.as_ref())
                    .and_then(|playlist| playlist.adjacent_path(direction));
                if let Some(path) = path {
                    if let Err(error) = self.play_playlist_path(id, path) {
                        self.video_action_error =
                            Some(format!("Could not change the folder video: {error}"));
                    }
                }
            }
        }
        ctx.request_repaint();
    }

    #[cfg(windows)]
    fn play_playlist_path(
        &mut self,
        playlist_id: PreviewId,
        path: std::path::PathBuf,
    ) -> Result<(), String> {
        let (group, linked_video, playlist_position, valid_path) = self
            .preview_manager
            .get(playlist_id)
            .and_then(|preview| {
                preview.folder_playlist.as_ref().map(|playlist| {
                    (
                        preview.playlist_group,
                        preview.playlist_linked_video,
                        preview.position,
                        playlist.contains(&path),
                    )
                })
            })
            .ok_or_else(|| "The folder playlist no longer exists".to_owned())?;
        if !valid_path {
            return Err("The selected file is not part of this folder playlist".to_owned());
        }
        if !path.is_file() {
            return Err(format!(
                "The video file no longer exists: {}",
                path.display()
            ));
        }

        let existing_video = linked_video.filter(|id| {
            self.preview_manager
                .get(*id)
                .is_some_and(|preview| preview.is_video())
        });
        let video_id = if let Some(video_id) = existing_video {
            if let Some(tile) = self.video_manager.get_mut(video_id) {
                tile.session.load_local_file(&path)?;
            } else {
                if let Some(preview) = self.preview_manager.get_mut(video_id) {
                    preview.video_source = Some(VideoSource::LocalFile { path: path.clone() });
                    preview.video_status = VideoTileStatus::Starting;
                }
                self.pending_video_tiles.insert(
                    video_id,
                    PendingVideoTile {
                        start_paused: false,
                        shown_once: true,
                        retry_ready: true,
                    },
                );
                self.try_activate_video_tile(video_id)?;
            }
            video_id
        } else {
            let video_id =
                self.add_local_video_path(path.clone(), playlist_position - Vec2::new(664.0, 0.0))?;
            if let Some(preview) = self.preview_manager.get_mut(video_id) {
                preview.playlist_group = group;
            }
            if let Some(preview) = self.preview_manager.get_mut(playlist_id) {
                preview.playlist_linked_video = Some(video_id);
            }
            video_id
        };

        self.seek_preview_manager.remove(video_id);
        if let Some(preview) = self.preview_manager.get_mut(video_id) {
            preview.clear_seek_preview();
        }
        let title = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Video")
            .to_owned();
        if let Some(preview) = self.preview_manager.get_mut(video_id) {
            preview.video_source = Some(VideoSource::LocalFile { path: path.clone() });
            preview.title = title;
            preview.video_status = VideoTileStatus::Starting;
            preview.video_playback.paused = false;
            preview.video_playback.time_pos = None;
            preview.video_playback.duration = None;
        }
        if let Some(playlist) = self
            .preview_manager
            .get_mut(playlist_id)
            .and_then(|preview| preview.folder_playlist.as_mut())
        {
            playlist.selected = Some(path);
            playlist.error = None;
        }
        Ok(())
    }

    #[cfg(windows)]
    fn pump_playlist_thumbnails(&mut self, ctx: &egui::Context) {
        let mut index = 0;
        while index < self.playlist_thumbnail_jobs.len() {
            let result = match self.playlist_thumbnail_jobs[index].receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err("Thumbnail worker stopped".to_owned())),
            };
            let Some(result) = result else {
                index += 1;
                continue;
            };
            let job = self.playlist_thumbnail_jobs.swap_remove(index);
            if let Some(entry) = self
                .preview_manager
                .get_mut(job.playlist_id)
                .and_then(|preview| preview.folder_playlist.as_mut())
                .and_then(|playlist| {
                    playlist
                        .entries
                        .iter_mut()
                        .find(|entry| entry.path == job.path)
                })
            {
                match result {
                    Ok(thumbnail) => {
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [thumbnail.width as usize, thumbnail.height as usize],
                            &thumbnail.rgba,
                        );
                        entry.thumbnail = Some(ctx.load_texture(
                            format!("playlist_{}_{}", job.playlist_id.0, entry.name),
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                    Err(error) => {
                        entry.thumbnail_state = ThumbnailState::Failed;
                        log::debug!("Playlist thumbnail failed: {error}");
                    }
                }
            }
        }

        while self.playlist_thumbnail_jobs.len() < 2 {
            let Some((playlist_id, path)) = self.playlist_thumbnail_queue.pop_front() else {
                break;
            };
            let still_needed = self
                .preview_manager
                .get(playlist_id)
                .and_then(|preview| preview.folder_playlist.as_ref())
                .is_some_and(|playlist| {
                    playlist.entries.iter().any(|entry| {
                        entry.path == path
                            && entry.thumbnail.is_none()
                            && entry.thumbnail_state == ThumbnailState::Loading
                    })
                });
            if !still_needed {
                continue;
            }
            let ToolStatus::Available { path: mpv_path, .. } =
                self.external_tools.status(ToolKind::Mpv)
            else {
                if let Some(entry) = self
                    .preview_manager
                    .get_mut(playlist_id)
                    .and_then(|preview| preview.folder_playlist.as_mut())
                    .and_then(|playlist| {
                        playlist.entries.iter_mut().find(|entry| entry.path == path)
                    })
                {
                    entry.thumbnail_state = ThumbnailState::Failed;
                }
                continue;
            };
            self.playlist_thumbnail_jobs.push(PlaylistThumbnailJob {
                playlist_id,
                path: path.clone(),
                receiver: video::spawn_video_thumbnail(
                    mpv_path.clone(),
                    video::VideoThumbnailSource::LocalFile(path),
                    1.0,
                ),
            });
        }

        if !self.playlist_thumbnail_jobs.is_empty() || !self.playlist_thumbnail_queue.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    /// Drop video hosts only after their capture sessions have been stopped.
    #[cfg(windows)]
    fn prune_video_tiles(&mut self) {
        let wallpaper_video_active = self
            .canvas
            .wallpaper
            .as_ref()
            .is_some_and(|wallpaper| matches!(wallpaper.source, WallpaperSource::Video { .. }));
        let stale: Vec<_> = self
            .video_manager
            .ids()
            .filter(|id| {
                let preview_is_video = self
                    .preview_manager
                    .get(*id)
                    .is_some_and(|preview| preview.is_video());
                video_session_is_stale(*id, wallpaper_video_active, preview_is_video)
            })
            .collect();
        for id in stale {
            self.capture_coordinator.stop_capture(id);
            self.video_manager.remove(id);
            self.restored_paused_videos.remove(&id);
        }
        let previews = &self.preview_manager;
        self.pending_video_tiles
            .retain(|id, _| previews.get(*id).is_some_and(|preview| preview.is_video()));
        self.restored_paused_videos
            .retain(|id| previews.get(*id).is_some_and(|preview| preview.is_video()));
        self.pending_video_freezes
            .retain(|id, _| previews.get(*id).is_some_and(|preview| preview.is_video()));
        self.frozen_video_checkpoints
            .retain(|id, _| previews.get(*id).is_some_and(|preview| preview.is_video()));
        self.video_resume_checkpoints
            .retain(|id, _| previews.get(*id).is_some_and(|preview| preview.is_video()));
    }

    /// Create a browser tile: WebView host, preview, and capture session.
    /// Used by the Add Browser dialog, layout restore, and undo.
    #[cfg(windows)]
    fn create_browser_tile(
        &mut self,
        url: &str,
        position: Pos2,
        size: Vec2,
        fps: FpsPreset,
    ) -> Result<PreviewId, String> {
        let url = normalize_url(url).map_err(str::to_owned)?;

        // Reserve the visual tile immediately. If uBOL needs extraction, the
        // app can render progress here while filesystem work runs off-thread.
        let id = self
            .preview_manager
            .add_browser_placeholder(url.clone(), position, size, fps);
        self.remember_recent_url(&url);

        if !self.browser.can_create_browser() {
            self.browser.start_extension_preparation();
        }

        self.pending_browser_tiles.insert(
            id,
            PendingBrowserTile {
                url,
                fps,
                muted: false,
                shown_once: false,
                restore_deferred: false,
            },
        );
        Ok(id)
    }

    #[cfg(windows)]
    fn activate_browser_tile(
        &mut self,
        id: PreviewId,
        url: &str,
        fps: FpsPreset,
    ) -> Result<(), String> {
        if let Some(preview) = self.preview_manager.get_mut(id) {
            preview.browser_status = BrowserTileStatus::Starting;
        }
        let hwnd = self.browser.create(id, url)?;
        if let Some(preview) = self.preview_manager.get_mut(id) {
            preview.window_handle = Some(WindowHandle {
                hwnd,
                process_id: std::process::id(),
            });
            preview.browser_status = BrowserTileStatus::Ready;
            preview.set_fps_preset(fps);
        }
        self.capture_coordinator
            .start_capture(id, hwnd, url.to_owned(), fps.as_u32());
        Ok(())
    }

    #[cfg(windows)]
    fn browser_preparation_upkeep(&mut self, ctx: &egui::Context) {
        let status = self.browser.poll_extension_preparation();
        let progress = match &status {
            ExtensionPreparationStatus::Preparing(progress) => *progress,
            ExtensionPreparationStatus::Ready => 1.0,
            ExtensionPreparationStatus::Idle | ExtensionPreparationStatus::Failed(_) => 0.0,
        };

        let can_create = self.browser.can_create_browser();
        for id in self.pending_browser_tiles.keys().copied() {
            if let Some(preview) = self.preview_manager.get_mut(id) {
                preview.browser_status = if can_create {
                    BrowserTileStatus::Starting
                } else {
                    match &status {
                        ExtensionPreparationStatus::Failed(error) => {
                            BrowserTileStatus::Failed(error.clone())
                        }
                        _ => BrowserTileStatus::PreparingAdblock { progress },
                    }
                };
            }
        }

        if can_create {
            let immediate_id = self.pending_browser_tiles.iter().find_map(|(id, pending)| {
                let live = self
                    .preview_manager
                    .get(*id)
                    .is_some_and(|preview| !preview.manually_frozen);
                (pending.shown_once && !pending.restore_deferred && live).then_some(*id)
            });
            let restored_slot_available = self
                .last_restored_browser_start
                .is_none_or(|started| started.elapsed() >= RESTORED_BROWSER_START_INTERVAL);
            let viewport = self
                .canvas
                .last_screen_rect
                .map(|screen_rect| self.canvas.get_viewport(screen_rect));
            let restored_id = restored_slot_available
                .then(|| {
                    self.pending_browser_tiles.iter().find_map(|(id, pending)| {
                        let preview = self.preview_manager.get(*id);
                        let tile_rect = preview.map(|preview| preview.rect());
                        (preview.is_some_and(|preview| !preview.manually_frozen)
                            && restored_browser_ready(
                                pending,
                                tile_rect,
                                viewport,
                                preview.is_some_and(|preview| preview.viewport_pin.is_some()),
                            ))
                        .then_some(*id)
                    })
                })
                .flatten();

            // User-created tiles start immediately. Restored tiles start only
            // when visible and at a human-scale interval so media-heavy pages
            // cannot all compete for WebView2, capture, CPU, and GPU at once.
            if let Some(id) = immediate_id.or(restored_id) {
                if let Some(pending) = self.pending_browser_tiles.remove(&id) {
                    if self.preview_manager.get(id).is_some() {
                        if pending.restore_deferred {
                            self.last_restored_browser_start = Some(Instant::now());
                        }
                        match self.activate_browser_tile(id, &pending.url, pending.fps) {
                            Ok(()) => self.apply_browser_mute(id, pending.muted),
                            Err(error) => {
                                log::error!("Failed to start browser tile: {error}");
                                if let Some(preview) = self.preview_manager.get_mut(id) {
                                    preview.browser_status = BrowserTileStatus::Failed(error);
                                }
                            }
                        }
                    }
                }
            }
            self.browser
                .initialize_prepared_extension_for_existing_host();

            let visible_restored_pending =
                self.pending_browser_tiles.iter().any(|(id, pending)| {
                    let preview = self.preview_manager.get(*id);
                    let tile_rect = preview.map(|preview| preview.rect());
                    preview.is_some_and(|preview| !preview.manually_frozen)
                        && restored_browser_ready(
                            pending,
                            tile_rect,
                            viewport,
                            preview.is_some_and(|preview| preview.viewport_pin.is_some()),
                        )
                });
            if visible_restored_pending {
                let delay = self
                    .last_restored_browser_start
                    .map(|started| {
                        RESTORED_BROWSER_START_INTERVAL.saturating_sub(started.elapsed())
                    })
                    .unwrap_or(Duration::ZERO);
                ctx.request_repaint_after(delay);
            }
        }

        self.browser.poll_adblock_settle();

        if matches!(status, ExtensionPreparationStatus::Preparing(_))
            || self.browser.adblock_settling()
        {
            ctx.request_repaint();
        }
    }

    /// Reapply a saved mute state to a freshly created browser tile
    /// (WebView2 mute is per-session, so restores must set it again).
    #[cfg(windows)]
    fn apply_browser_mute(&mut self, id: PreviewId, muted: bool) {
        if !muted {
            return;
        }
        if let Some(pending) = self.pending_browser_tiles.get_mut(&id) {
            pending.muted = true;
            if let Some(preview) = self.preview_manager.get_mut(id) {
                preview.browser_muted = true;
            }
            return;
        }
        if let Some(host) = self.browser.get_mut(id) {
            if host.set_muted(true).is_ok() {
                if let Some(preview) = self.preview_manager.get_mut(id) {
                    preview.browser_muted = true;
                }
            }
        }
    }

    fn remember_recent_url(&mut self, url: &str) {
        let Some(url) = scrub_url_for_storage(url) else {
            return;
        };
        self.recent_urls.retain(|u| u != &url);
        self.recent_urls.insert(0, url);
        self.recent_urls.truncate(MAX_RECENT_URLS);
    }

    /// Decode an external image in place and retain its original local path.
    fn import_media_tile(
        &mut self,
        source: &std::path::Path,
        position: Pos2,
    ) -> Result<PreviewId, String> {
        let path = original_image_path(source)?;
        let asset = media::load(&path)?;
        let title = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Image")
            .to_owned();
        let size = media_tile_size(asset.width, asset.height);
        Ok(self
            .preview_manager
            .add_media(path, title, asset.frames, position, size))
    }

    /// Recreate a tile from its original path. Relative filenames from older
    /// layouts still resolve through the legacy managed-media directory.
    fn restore_media_tile(
        &mut self,
        saved_path: &std::path::Path,
        title: String,
        position: Pos2,
        size: Vec2,
    ) -> Result<PreviewId, String> {
        let path = self.resolve_saved_image_path(saved_path)?;
        let (frames, error) = media::load_or_placeholder(&path);
        let id = self
            .preview_manager
            .add_media(path, title, frames, position, size);
        if let Some(error) = error {
            if let Some(preview) = self.preview_manager.get_mut(id) {
                preview.set_capture_error(error);
            }
        }
        Ok(id)
    }

    fn handle_media_action(&mut self, id: PreviewId, action: MediaAction) {
        let Some(path) = self
            .preview_manager
            .get(id)
            .filter(|preview| preview.is_media())
            .and_then(|preview| preview.media_path.clone())
        else {
            return;
        };
        let result = match action {
            MediaAction::Retry => self.resolve_saved_image_path(&path),
            MediaAction::Locate => {
                let Some(path) = media::pick_file(self.main_hwnd) else {
                    return;
                };
                original_image_path(&path)
            }
        };
        let path = match result {
            Ok(path) => path,
            Err(error) => {
                self.media_error = Some(error);
                return;
            }
        };
        let (frames, error) = media::load_or_placeholder(&path);
        if let Some(preview) = self.preview_manager.get_mut(id) {
            // Repairing a source must not move, unpin, uncrop, or unfreeze it.
            preview.set_media(path, frames);
            if let Some(error) = error {
                preview.set_capture_error(error);
            }
        }
    }

    #[cfg(windows)]
    fn restore_removed_preview(&mut self, info: RemovedPreviewInfo) -> Option<PreviewId> {
        let id = if let Some(playlist) = info.folder_playlist.as_ref() {
            let group = info.playlist_group?;
            let linked = self.preview_manager.all().find_map(|preview| {
                (preview.playlist_group == Some(group) && preview.is_video()).then_some(preview.id)
            });
            self.restore_folder_playlist(
                &playlist.layout(),
                info.title.clone(),
                info.position,
                info.size,
                group,
                linked,
            )
        } else if let Some(url) = info.browser_url.as_deref() {
            match self.create_browser_tile(url, info.position, info.size, info.fps_preset) {
                Ok(id) => {
                    self.apply_browser_mute(id, info.browser_muted);
                    id
                }
                Err(error) => {
                    self.media_error = Some(error);
                    return None;
                }
            }
        } else if let Some(media_path) = info.media_path.as_deref() {
            match self.restore_media_tile(media_path, info.title.clone(), info.position, info.size)
            {
                Ok(id) => id,
                Err(error) => {
                    self.media_error = Some(error);
                    return None;
                }
            }
        } else if let Some(source) = info.video_source.clone() {
            let id = self.create_video_tile(
                source,
                info.title.clone(),
                info.position,
                info.size,
                info.fps_preset,
                true,
            );
            let checkpoint = FrozenVideoCheckpoint {
                playback: info.video_playback.clone(),
                seek_position: resumable_video_position(&info.video_playback),
            };
            if info.manually_frozen {
                self.frozen_video_checkpoints.insert(id, checkpoint);
            } else {
                self.video_resume_checkpoints.insert(id, checkpoint);
            }
            id
        } else if let Some(sender) = info.spout_sender.as_ref() {
            let id = self.preview_manager.add_for_spout(
                sender.clone(),
                info.position,
                info.size,
                info.fps_preset,
            );
            if !info.manually_frozen {
                self.capture_coordinator.start_spout_capture(
                    id,
                    sender.clone(),
                    info.fps_preset.as_u32(),
                );
            }
            id
        } else if info.window_waiting_for_match || info.window_handle.is_some() {
            // HWNDs can be recycled while the Undo toast is visible. Reconnect
            // through the approved window list instead of trusting the old handle.
            self.preview_manager.add_inactive_window(
                info.title.clone(),
                info.window_exe.clone(),
                info.position,
                info.size,
                info.fps_preset,
                info.z_order,
            )
        } else {
            return None;
        };
        if let Some(preview) = self.preview_manager.get_mut(id) {
            restore_removed_tile_state(preview, &info);
        }
        Some(id)
    }

    #[cfg(windows)]
    fn restore_removed_previews(&mut self, mut removed: Vec<RemovedPreviewInfo>) {
        let groups: HashSet<_> = removed
            .iter()
            .filter_map(|info| info.playlist_group)
            .collect();
        // Recreate video sources before rebuilding their playlist links.
        removed.sort_by_key(|info| (info.folder_playlist.is_some(), info.z_order));
        self.canvas.selection.clear();
        for info in removed {
            let selectable = !info.left_click_disabled;
            if let Some(id) = self.restore_removed_preview(info) {
                if selectable {
                    self.canvas.selection.push(id);
                }
            }
        }
        for group in groups {
            let linked = self.preview_manager.all().find_map(|preview| {
                (preview.playlist_group == Some(group) && preview.is_video()).then_some(preview.id)
            });
            for preview in self.preview_manager.all_mut() {
                if preview.is_playlist() && preview.playlist_group == Some(group) {
                    preview.playlist_linked_video = linked;
                }
            }
        }
    }

    fn resolve_saved_image_path(
        &self,
        saved_path: &std::path::Path,
    ) -> Result<std::path::PathBuf, String> {
        if saved_path.is_absolute() {
            return Ok(saved_path.to_owned());
        }
        let filename = saved_path
            .to_str()
            .ok_or_else(|| "Saved legacy image path is invalid".to_owned())?;
        self.storage
            .as_ref()
            .ok_or_else(|| "Pluriview storage is unavailable".to_owned())?
            .resolve_media(filename)
            .ok_or_else(|| "Saved legacy image path is invalid".to_owned())
    }

    fn pick_wallpaper(&mut self) {
        let Some(path) = media::pick_wallpaper_file(self.main_hwnd) else {
            return;
        };
        if let Err(error) = self.set_wallpaper_from_path(path) {
            log::error!("Failed to set wallpaper: {error}");
            self.media_error = Some(error);
        }
    }

    fn clear_wallpaper(&mut self) {
        #[cfg(windows)]
        {
            if self.video_manager.contains(WALLPAPER_VIDEO_ID) {
                self.video_manager.remove(WALLPAPER_VIDEO_ID);
            }
            self.wallpaper_sleep_since = None;
            self.wallpaper_hibernated = false;
        }
        self.canvas.wallpaper = None;
    }

    fn set_wallpaper_from_path(&mut self, path: std::path::PathBuf) -> Result<(), String> {
        if media::is_supported_video_path(&path) {
            #[cfg(windows)]
            Self::require_libmpv("Setting a video wallpaper")?;
            self.set_video_wallpaper(path)
        } else if media::is_supported_image_path(&path) {
            self.set_image_wallpaper(&path)
        } else {
            Err(format!("Unsupported wallpaper file: {}", path.display()))
        }
    }

    fn set_image_wallpaper(&mut self, source: &std::path::Path) -> Result<(), String> {
        let path = original_image_path(source)?;
        let asset = media::load(&path)?;
        self.clear_wallpaper();
        self.canvas.wallpaper = Some(CanvasWallpaper::from_image(path, asset.frames));
        Ok(())
    }

    fn restore_image_wallpaper(&mut self, saved_path: &std::path::Path) -> Result<(), String> {
        let path = self.resolve_saved_image_path(saved_path)?;
        if !path.is_file() {
            self.clear_wallpaper();
            let mut wallpaper = CanvasWallpaper::from_image(path, Vec::new());
            wallpaper.error = Some(
                "Can't find wallpaper image\nThe original file may have moved or its path changed"
                    .to_owned(),
            );
            self.canvas.wallpaper = Some(wallpaper);
            return Ok(());
        }
        let asset = media::load(&path)?;
        self.clear_wallpaper();
        self.canvas.wallpaper = Some(CanvasWallpaper::from_image(path, asset.frames));
        Ok(())
    }

    #[cfg(windows)]
    fn set_video_wallpaper(&mut self, path: std::path::PathBuf) -> Result<(), String> {
        if !path.is_file() {
            return Err(format!("Video file does not exist: {}", path.display()));
        }
        self.clear_wallpaper();
        self.canvas.wallpaper = Some(CanvasWallpaper::from_video(path));
        match self.ensure_wallpaper_video() {
            Ok(_) => Ok(()),
            Err(error) => {
                if let Some(wallpaper) = self.canvas.wallpaper.as_mut() {
                    wallpaper.error = Some(error.clone());
                }
                Err(error)
            }
        }
    }

    #[cfg(not(windows))]
    fn set_video_wallpaper(&mut self, path: std::path::PathBuf) -> Result<(), String> {
        Err(format!(
            "Video wallpaper requires Windows and libmpv: {}",
            path.display()
        ))
    }

    fn restore_wallpaper(&mut self, layout: WallpaperLayout) -> Result<(), String> {
        match layout {
            WallpaperLayout::Image { path } => self.restore_image_wallpaper(&path),
            WallpaperLayout::Video { path } => self.set_video_wallpaper(path),
        }
    }

    /// Attach libmpv to the current video wallpaper when the file is ready.
    #[cfg(windows)]
    fn ensure_wallpaper_video(&mut self) -> Result<bool, String> {
        if self.wallpaper_hibernated || self.wallpaper_sleep_since.is_some() {
            return Ok(false);
        }
        let path = match self.canvas.wallpaper.as_ref() {
            Some(wallpaper) if wallpaper.video_renderer.is_none() => match &wallpaper.source {
                WallpaperSource::Video { path } => path.clone(),
                WallpaperSource::Image { .. } => return Ok(false),
            },
            _ => return Ok(false),
        };
        if let Some(tile) = self.video_manager.get(WALLPAPER_VIDEO_ID) {
            let renderer = tile.session.renderer();
            if let Some(wallpaper) = self.canvas.wallpaper.as_mut() {
                wallpaper.video_renderer = Some(renderer);
                wallpaper.video_sleeping = false;
            }
            return Ok(true);
        }
        if self
            .canvas
            .wallpaper
            .as_ref()
            .is_some_and(|wallpaper| wallpaper.error.is_some())
        {
            return Ok(false);
        }
        let source = VideoSource::LocalFile { path };
        let Some(mut launch) = video_launch_for_source(
            &source,
            self.external_tools.status(ToolKind::Streamlink),
            false,
        )?
        else {
            return Ok(false);
        };
        launch.wallpaper = true;
        let renderer =
            self.video_manager
                .launch(WALLPAPER_VIDEO_ID, launch, FpsPreset::Medium.as_u32())?;
        if let Some(tile) = self.video_manager.get_mut(WALLPAPER_VIDEO_ID) {
            let _ = tile.session.set_looping(true);
            let _ = tile.session.set_muted(true);
            let _ = tile.session.set_fill_frame(true);
        }
        if let Some(wallpaper) = self.canvas.wallpaper.as_mut() {
            wallpaper.video_renderer = Some(renderer);
            wallpaper.video_sleeping = false;
            wallpaper.error = None;
        }
        Ok(true)
    }

    /// Keep a muted video wallpaper from consuming decoder/GPU resources while
    /// the window cannot display it. This reuses normal libmpv cleanup and adds
    /// no polling thread; the existing egui upkeep tick drives the deadline.
    #[cfg(windows)]
    fn wallpaper_memory_upkeep(&mut self, ctx: &egui::Context) {
        let is_video = self
            .canvas
            .wallpaper
            .as_ref()
            .is_some_and(|wallpaper| matches!(wallpaper.source, WallpaperSource::Video { .. }));
        if !is_video {
            self.wallpaper_sleep_since = None;
            self.wallpaper_hibernated = false;
            return;
        }

        let minimized = ctx.input(|input| input.viewport().minimized.unwrap_or(false));
        let hidden = minimized || self.canvas.is_focusing_tile();
        if !hidden {
            self.wallpaper_sleep_since = None;
            self.wallpaper_hibernated = false;
            if let Some(wallpaper) = self.canvas.wallpaper.as_mut() {
                wallpaper.video_sleeping = false;
            }
            return;
        }

        let now = Instant::now();
        let since = *self.wallpaper_sleep_since.get_or_insert(now);
        if let Some(wallpaper) = self.canvas.wallpaper.as_mut() {
            wallpaper.video_sleeping = true;
        }
        if let Err(error) = self.video_manager.set_paused(WALLPAPER_VIDEO_ID, true) {
            log::warn!("Could not pause minimized wallpaper: {error}");
        }
        if !self.wallpaper_hibernated && wallpaper_hibernation_due(since, now) {
            if let Some(wallpaper) = self.canvas.wallpaper.as_mut() {
                wallpaper.video_renderer = None;
            }
            self.video_manager.remove(WALLPAPER_VIDEO_ID);
            self.wallpaper_hibernated = true;
        }
    }

    /// Video wallpaper keeps decoding while focused tiles cover it. Pause the
    /// player (and skip painting) until tile focus is cleared.
    #[cfg(windows)]
    fn sync_wallpaper_under_tile_focus(&mut self) {
        let covered = self.canvas.is_focusing_tile() || self.wallpaper_sleep_since.is_some();
        if let Err(error) = self.video_manager.set_paused(WALLPAPER_VIDEO_ID, covered) {
            log::warn!("Could not pause wallpaper under a focused tile: {error}");
        }
    }

    /// Open files dropped anywhere over the app and place them where the
    /// pointer meets the canvas. Images use managed storage; videos launch as
    /// MPV-backed tiles. Multiple files fan out so each remains selectable.
    fn import_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }

        let Some(canvas_rect) = self.canvas.last_screen_rect else {
            return;
        };
        let pointer = ctx
            .input(|input| input.pointer.hover_pos())
            .unwrap_or_else(|| canvas_rect.center());
        let pointer = Pos2::new(
            pointer.x.clamp(canvas_rect.left(), canvas_rect.right()),
            pointer.y.clamp(canvas_rect.top(), canvas_rect.bottom()),
        );
        let base_position = self.canvas.screen_to_canvas(pointer, canvas_rect);
        let mut imported = Vec::new();
        let mut errors = Vec::new();

        for (index, file) in dropped.into_iter().enumerate() {
            let Some(path) = file.path else {
                let name = if file.name.is_empty() {
                    "dragged item"
                } else {
                    &file.name
                };
                errors.push(format!(
                    "{name}: this drag source did not provide a local file"
                ));
                continue;
            };
            let offset = Vec2::splat(index as f32 * 24.0 / self.canvas.zoom.max(0.1));
            let position = base_position + offset;
            let result: Result<Vec<PreviewId>, String> = if path.is_dir() {
                #[cfg(windows)]
                {
                    self.add_video_folder(path.clone(), position)
                        .map(|(video, playlist)| vec![video, playlist])
                }
                #[cfg(not(windows))]
                {
                    Err("Video playback is currently available only on Windows".to_owned())
                }
            } else if media::is_supported_video_path(&path) {
                #[cfg(windows)]
                {
                    self.add_local_video_path(path.clone(), position)
                        .map(|id| vec![id])
                }
                #[cfg(not(windows))]
                {
                    Err("Video playback is currently available only on Windows".to_owned())
                }
            } else {
                self.import_media_tile(&path, position).map(|id| vec![id])
            };
            match result {
                Ok(ids) => imported.extend(ids),
                Err(error) => errors.push(format!("{}: {error}", path.display())),
            }
        }

        if !imported.is_empty() {
            self.canvas.selection = imported;
        }
        if !errors.is_empty() {
            self.media_error = Some(errors.join("\n\n"));
        }
    }

    /// Highlight the canvas while Windows is carrying files over the app.
    fn file_drop_overlay(&self, ctx: &egui::Context) {
        if !ctx.input(|input| !input.raw.hovered_files.is_empty()) {
            return;
        }
        let Some(rect) = self.canvas.last_screen_rect else {
            return;
        };
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("media_drop_overlay"),
        ));
        painter.rect_filled(
            rect,
            8.0,
            egui::Color32::from_rgba_unmultiplied(35, 95, 55, 90),
        );
        painter.rect_stroke(
            rect.shrink(8.0),
            8.0,
            egui::Stroke::new(2.0, egui::Color32::from_rgb(107, 200, 110)),
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Drop images, videos, or a video folder onto the canvas",
            egui::FontId::proportional(18.0),
            egui::Color32::WHITE,
        );
        ctx.request_repaint();
    }

    #[cfg(windows)]
    fn add_browser_ui(&mut self, ctx: &egui::Context) {
        let mut submit = None;
        let mut cancel = false;

        if let Some(dialog) = self.add_browser.as_mut() {
            let editing = dialog.target.is_some();
            let recent_urls = &self.recent_urls;
            egui::Window::new(if editing { "Change URL" } else { "Add Browser" })
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("Paste a website URL");
                    let mut paste_requested = false;
                    let response = ui.add_sized(
                        [420.0, 24.0],
                        egui::TextEdit::singleline(&mut dialog.url)
                            .hint_text("twitch.tv/channel or https://kick.com/channel"),
                    );
                    response.context_menu(|ui| {
                        if ui.button("Paste").clicked() {
                            paste_requested = true;
                            ui.close_menu();
                        }
                    });
                    if !dialog.focused {
                        response.request_focus();
                        dialog.focused = true;
                    }
                    if let Some(error) = &dialog.error {
                        ui.colored_label(egui::Color32::from_rgb(255, 100, 100), error);
                    }

                    // Pressing Enter in a TextEdit surrenders focus that same
                    // frame, so lost_focus + Enter is the reliable submit check.
                    let submitted = response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));

                    ui.horizontal(|ui| {
                        let label = if editing { "Load" } else { "Add" };
                        if ui.button(label).clicked() || submitted {
                            submit = Some((dialog.url.clone(), dialog.position, dialog.target));
                        }
                        if ui.button("Paste").clicked() {
                            paste_requested = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });

                    if paste_requested {
                        match clipboard_text() {
                            Ok(text) if !text.trim().is_empty() => {
                                dialog.url = text.trim().to_owned();
                                dialog.error = None;
                                response.request_focus();
                            }
                            Ok(_) => dialog.error = Some("The clipboard is empty.".to_owned()),
                            Err(error) => dialog.error = Some(error.to_owned()),
                        }
                    }

                    if !recent_urls.is_empty() {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("Recent").weak().small());
                        for url in recent_urls.iter().take(5) {
                            if ui
                                .add(
                                    egui::Button::new(egui::RichText::new(url).size(11.5))
                                        .frame(false),
                                )
                                .clicked()
                            {
                                submit = Some((url.clone(), dialog.position, dialog.target));
                            }
                        }
                    }
                });
        }

        if cancel {
            self.add_browser = None;
        } else if let Some((url, position, target)) = submit {
            let result = match target {
                // Retarget an existing tile: navigate its WebView in place.
                Some(id) => normalize_url(&url).map_err(str::to_owned).map(|url| {
                    if let Some(host) = self.browser.get(id) {
                        host.load(&url);
                    }
                    if let Some(pending) = self.pending_browser_tiles.get_mut(&id) {
                        pending.url = url.clone();
                    }
                    if let Some(preview) = self.preview_manager.get_mut(id) {
                        preview.browser_url = Some(url.clone());
                        preview.title = url.clone();
                    }
                    self.remember_recent_url(&url);
                }),
                None => self
                    .create_browser_tile(&url, position, Vec2::new(640.0, 360.0), FpsPreset::Medium)
                    .map(|_| ()),
            };
            match result {
                Ok(()) => self.add_browser = None,
                Err(error) => {
                    if let Some(dialog) = self.add_browser.as_mut() {
                        dialog.error = Some(error);
                        // Put the caret back so the user can correct the URL.
                        dialog.focused = false;
                    }
                }
            }
        }
    }

    #[cfg(windows)]
    fn add_stream_ui(&mut self, ctx: &egui::Context) {
        let mut submit = false;
        let mut cancel = false;

        if let Some(dialog) = self.add_stream.as_mut() {
            let probe_result =
                dialog
                    .probe_receiver
                    .as_ref()
                    .and_then(|receiver| match receiver.try_recv() {
                        Ok(result) => Some(result),
                        Err(TryRecvError::Disconnected) => {
                            Some(Err("The quality probe stopped unexpectedly.".to_owned()))
                        }
                        Err(TryRecvError::Empty) => None,
                    });
            if let Some(result) = probe_result {
                dialog.probe_receiver = None;
                if dialog.probing_url == dialog.url.trim() {
                    match result {
                        Ok(qualities) => {
                            dialog.qualities = qualities;
                            dialog.probe_error = None;
                        }
                        Err(error) => {
                            dialog.probe_error =
                                Some(format!("{error} You can still enter a quality and add."));
                        }
                    }
                }
            }

            if let Some(due) = dialog.probe_due {
                let remaining = due.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    dialog.probe_due = None;
                    let url = dialog.url.trim().to_owned();
                    if !url.is_empty() {
                        dialog.probing_url = url.clone();
                        dialog.probe_error = None;
                        dialog.probe_receiver = Some(external_tools::probe_stream_qualities(
                            dialog.streamlink_path.clone(),
                            url,
                        ));
                    }
                } else {
                    ctx.request_repaint_after(remaining);
                }
            }
            if dialog.probe_receiver.is_some() {
                ctx.request_repaint_after(Duration::from_millis(100));
            }

            egui::Window::new("Add Stream")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("Stream URL");
                    let mut paste_requested = false;
                    let response = ui.add_sized(
                        [460.0, 24.0],
                        egui::TextEdit::singleline(&mut dialog.url)
                            .hint_text("https://example.com/channel"),
                    );
                    response.context_menu(|ui| {
                        if ui.button("Paste").clicked() {
                            paste_requested = true;
                            ui.close_menu();
                        }
                    });
                    if !dialog.focused {
                        response.request_focus();
                        dialog.focused = true;
                    }
                    if response.changed() {
                        dialog.error = None;
                        dialog.probe_error = None;
                        if dialog.url.trim().is_empty() {
                            dialog.probe_due = None;
                            dialog.probe_receiver = None;
                            dialog.probing_url.clear();
                            dialog.qualities.clear();
                        } else {
                            dialog.probe_due = Some(Instant::now() + Duration::from_millis(400));
                        }
                    }

                    ui.add_space(6.0);
                    ui.label("Quality");
                    ui.add_sized(
                        [220.0, 24.0],
                        egui::TextEdit::singleline(&mut dialog.quality).hint_text("best"),
                    );
                    if !dialog.qualities.is_empty() {
                        egui::ComboBox::from_id_salt("stream_quality_choices")
                            .selected_text("Detected qualities")
                            .show_ui(ui, |ui| {
                                for quality in &dialog.qualities {
                                    if ui.selectable_label(false, quality).clicked() {
                                        dialog.quality = quality.clone();
                                    }
                                }
                            });
                    }
                    if dialog.probe_receiver.is_some() {
                        ui.label(egui::RichText::new("Checking available qualities...").weak());
                    } else if let Some(error) = &dialog.probe_error {
                        ui.colored_label(egui::Color32::from_rgb(235, 170, 100), error);
                    }
                    if let Some(error) = &dialog.error {
                        ui.colored_label(egui::Color32::from_rgb(255, 100, 100), error);
                    }

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Add").clicked() {
                            submit = true;
                        }
                        if ui.button("Paste").clicked() {
                            paste_requested = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });

                    if paste_requested {
                        match clipboard_text() {
                            Ok(text) if !text.trim().is_empty() => {
                                dialog.url = text.trim().to_owned();
                                dialog.error = None;
                                dialog.probe_error = None;
                                dialog.probe_receiver = None;
                                dialog.probing_url.clear();
                                dialog.qualities.clear();
                                dialog.probe_due =
                                    Some(Instant::now() + Duration::from_millis(400));
                                response.request_focus();
                            }
                            Ok(_) => dialog.error = Some("The clipboard is empty.".to_owned()),
                            Err(error) => dialog.error = Some(error.to_owned()),
                        }
                    }
                });
        }

        if cancel {
            self.add_stream = None;
        } else if submit {
            let Some(dialog) = self.add_stream.as_mut() else {
                return;
            };
            let url = dialog.url.trim().to_owned();
            if url.is_empty() {
                dialog.error = Some("Enter a stream URL.".to_owned());
                dialog.focused = false;
                return;
            }
            let quality = external_tools::normalize_stream_quality(&dialog.quality);
            let position = dialog.position;
            let id = self.create_video_tile(
                VideoSource::Stream {
                    url: url.clone(),
                    quality,
                },
                url,
                position,
                Vec2::new(640.0, 360.0),
                FpsPreset::Medium,
                false,
            );
            self.canvas.selection = vec![id];
            self.add_stream = None;
        }
    }

    /// Apply an action queued by a browser tile's hover controls / context menu.
    #[cfg(windows)]
    fn handle_browser_action(&mut self, ctx: &egui::Context, id: PreviewId, action: BrowserAction) {
        match action {
            BrowserAction::Back => {
                if let Some(host) = self.browser.get(id) {
                    host.go_back();
                }
            }
            BrowserAction::Forward => {
                if let Some(host) = self.browser.get(id) {
                    host.go_forward();
                }
            }
            BrowserAction::Reload => {
                if let Some(host) = self.browser.get(id) {
                    host.reload();
                }
            }
            BrowserAction::ToggleMute => {
                if let Some(host) = self.browser.get_mut(id) {
                    let muted = !host.is_muted();
                    if host.set_muted(muted).is_ok() {
                        if let Some(preview) = self.preview_manager.get_mut(id) {
                            preview.browser_muted = muted;
                        }
                    }
                }
            }
            BrowserAction::CopyUrl => {
                if let Some(host) = self.browser.get(id) {
                    ctx.copy_text(host.current_url());
                }
            }
            BrowserAction::OpenExternal => {
                if let Some(host) = self.browser.get(id) {
                    if let Ok(url) = normalize_url(&host.current_url()) {
                        let url = HSTRING::from(url);
                        unsafe {
                            ShellExecuteW(
                                None,
                                windows::core::w!("open"),
                                &url,
                                None,
                                None,
                                SW_SHOWNORMAL,
                            );
                        }
                    }
                }
            }
            BrowserAction::EditUrl => {
                let current = self
                    .browser
                    .get(id)
                    .map(|host| host.current_url())
                    .or_else(|| {
                        self.preview_manager
                            .get(id)
                            .and_then(|preview| preview.browser_url.clone())
                    })
                    .unwrap_or_default();
                self.add_browser = Some(AddBrowserDialog {
                    position: Pos2::ZERO,
                    url: current,
                    error: None,
                    target: Some(id),
                    focused: false,
                });
            }
        }
    }

    /// Full and canvas-clipped browser geometry for tile `id`, in egui points
    /// (window client coordinates). The page uses the exact tile rect so live
    /// interaction does not introduce an inset/size change.
    #[cfg(windows)]
    fn browser_tile_placement(
        &self,
        id: PreviewId,
        canvas_rect: egui::Rect,
    ) -> Option<BrowserTilePlacement> {
        let preview = self.preview_manager.get(id)?;
        let tile_rect = browser_tile_screen_rect(&self.canvas, preview, canvas_rect);
        if !tile_rect.intersects(canvas_rect) {
            return None;
        }
        let page_rect = browser_page_screen_rect(&self.canvas, preview, canvas_rect);
        let visible_rect = tile_rect.intersect(canvas_rect);
        (visible_rect.width() >= 1.0 && visible_rect.height() >= 1.0).then_some(
            BrowserTilePlacement {
                page_rect,
                visible_rect,
            },
        )
    }

    /// Keep the stream audio monitor alive while it's enabled and browser
    /// tiles exist, pointed at the current WebView2 browser process (which
    /// changes if the WebView2 runtime crashes and restarts). Window tiles
    /// with SA enabled get their own process-loopback copies on the same device.
    #[cfg(windows)]
    fn audio_monitor_upkeep(&mut self) {
        let enabled = self.monitor_device.is_some();
        let has_live_browser = self.preview_manager.all().any(|preview| {
            preview.is_browser() && !preview.manually_frozen && preview.removing.is_none()
        });
        if !enabled || !has_live_browser {
            self.audio_monitor = None;
            self.audio_monitor_checked = None;
        } else {
            let due = self
                .audio_monitor_checked
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(2));
            if due {
                self.audio_monitor_checked = Some(Instant::now());
                let device_id = self.monitor_device.as_ref().map(|(id, _)| id.clone());
                let (Some(device_id), Some(pid)) = (device_id, self.browser.browser_process_id())
                else {
                    self.window_audio_monitor_upkeep();
                    return;
                };
                let current = self
                    .audio_monitor
                    .as_ref()
                    .map(|m| (m.pid(), m.device_id().to_owned()));
                if current != Some((pid, device_id.clone())) {
                    self.audio_monitor = Some(crate::audio::AudioMonitor::start(pid, device_id));
                }
            }
        }
        self.window_audio_monitor_upkeep();
    }

    #[cfg(windows)]
    fn window_audio_monitor_upkeep(&mut self) {
        let Some((device_id, _)) = self.monitor_device.clone() else {
            self.window_audio_monitors.clear();
            return;
        };

        for preview in self.preview_manager.all_mut() {
            if preview.stream_audio && preview.is_window_capture() {
                if let Some(handle) = preview.window_handle.as_mut() {
                    if handle.process_id == 0 {
                        handle.process_id = hwnd_process_id(handle.hwnd);
                    }
                }
            }
        }

        let wanted: HashSet<u32> = self
            .preview_manager
            .all()
            .filter(|preview| {
                preview.stream_audio && preview.is_window_capture() && preview.removing.is_none()
            })
            .filter_map(|preview| {
                preview
                    .window_handle
                    .as_ref()
                    .map(|handle| handle.process_id)
                    .filter(|pid| *pid != 0)
            })
            .collect();

        self.window_audio_monitors
            .retain(|pid, monitor| wanted.contains(pid) && monitor.device_id() == device_id);
        for pid in wanted {
            self.window_audio_monitors
                .entry(pid)
                .or_insert_with(|| crate::audio::AudioMonitor::start(pid, device_id.clone()));
        }
    }

    /// Reconnect saved window tiles when their application later exposes a
    /// capturable top-level window. Enumeration uses the same privacy filters
    /// as the window picker.
    #[cfg(windows)]
    fn inactive_window_upkeep(&mut self, ctx: &egui::Context) {
        let has_inactive = self.preview_manager.all().any(|preview| {
            (preview.is_inactive_window()
                || (preview.is_window_capture() && preview.capture_error.is_some()))
                && !preview.manually_frozen
                && preview.removing.is_none()
        });
        if !has_inactive {
            self.last_inactive_window_scan = None;
            return;
        }

        ctx.request_repaint_after(INACTIVE_WINDOW_SCAN_INTERVAL);
        if self
            .last_inactive_window_scan
            .is_some_and(|last| last.elapsed() < INACTIVE_WINDOW_SCAN_INTERVAL)
        {
            return;
        }
        self.last_inactive_window_scan = Some(Instant::now());

        let windows = enumerate_windows();
        for preview in self.preview_manager.all_mut() {
            if preview.is_window_capture()
                && preview.capture_error.is_some()
                && !preview.manually_frozen
                && preview.removing.is_none()
                && !windows.iter().any(|window| {
                    preview.window_handle.as_ref().is_some_and(|handle| {
                        handle.hwnd == window.hwnd && handle.process_id == window.process_id
                    })
                })
            {
                self.capture_coordinator.stop_capture(preview.id);
                preview.mark_window_inactive();
            }
        }
        let mut claimed_hwnds: HashSet<isize> = self
            .preview_manager
            .all()
            .filter_map(|preview| preview.window_handle.as_ref().map(|handle| handle.hwnd))
            .collect();
        let inactive = self
            .preview_manager
            .all()
            .filter(|preview| {
                preview.is_inactive_window()
                    && !preview.manually_frozen
                    && preview.removing.is_none()
            })
            .map(|preview| {
                (
                    preview.id,
                    preview.title.clone(),
                    preview.window_exe.clone(),
                    preview.fps_preset,
                )
            })
            .collect::<Vec<_>>();

        for (id, saved_title, saved_exe, fps_preset) in inactive {
            let Some(window) =
                find_saved_window(&saved_title, saved_exe.as_deref(), &windows, &claimed_hwnds)
            else {
                continue;
            };
            let hwnd = window.hwnd;
            let process_id = window.process_id;
            let title = window.title.clone();
            let exe_name = window.exe_name.clone();
            claimed_hwnds.insert(hwnd);

            if let Some(preview) = self.preview_manager.get_mut(id) {
                preview.window_handle = Some(WindowHandle { hwnd, process_id });
                preview.window_exe = Some(exe_name);
                preview.window_waiting_for_match = false;
                preview.capture_paused = false;
                preview.capture_hibernated = false;
                preview.capture_offscreen_since = None;
                preview.title.clone_from(&title);
                preview.clear_capture_error();
            }
            self.capture_coordinator
                .start_capture(id, hwnd, title, fps_preset.as_u32());
            ctx.request_repaint();
        }
    }

    /// Fully transparent browser captures are hidden behind a loading card.
    /// Keep probing for visible content without copying frames at full speed.
    #[cfg(windows)]
    fn browser_capture_frame_rate_upkeep(&mut self) {
        for preview in self
            .preview_manager
            .all()
            .filter(|preview| preview.is_browser() && preview.removing.is_none())
        {
            self.capture_coordinator
                .set_target_fps(preview.id, browser_capture_fps(preview));
        }
    }

    /// Per-frame browser housekeeping. Runs after the canvas UI so tile
    /// rects and double-click state are fresh.
    #[cfg(windows)]
    fn browser_frame(&mut self, ctx: &egui::Context, exit_hotkey_pressed: bool) {
        self.audio_monitor_upkeep();

        let frozen: HashSet<_> = self
            .preview_manager
            .all()
            .filter(|preview| preview.is_browser() && preview.manually_frozen)
            .map(|preview| preview.id)
            .collect();

        // Mirror page titles and current URLs onto the tiles so the hover
        // overlay shows "lofi hip hop radio..." instead of the raw URL and
        // layouts save where the user actually navigated.
        let mut updates = Vec::new();
        for (id, host) in self.browser.iter_mut() {
            if frozen.contains(id) {
                continue;
            }
            let update = host.poll();
            if update.title.is_some() || update.url.is_some() {
                updates.push((*id, update));
            }
        }
        for (id, update) in updates {
            if let Some(preview) = self.preview_manager.get_mut(id) {
                if let Some(title) = update.title {
                    if !title.is_empty() {
                        preview.title = title;
                    }
                }
                if let Some(url) = update.url {
                    preview.browser_url = Some(url);
                }
            }
        }

        // Give parked browsers a stable supersampled backing derived from the
        // tile's model size, not the canvas zoom. A viewport-pinned browser
        // uses its fixed screen size. This keeps previews sharp without making
        // responsive pages resize while the canvas zooms.
        let pixels_per_point = ctx.pixels_per_point();
        let ids: Vec<_> = self.browser.ids().collect();
        for id in ids {
            if frozen.contains(&id) {
                continue;
            }
            let Some(preview) = self.preview_manager.get(id) else {
                continue;
            };
            let display_size = browser_capture_display_size(preview);
            let physical_size = display_size * pixels_per_point;
            let capture_size = browser::capture_size_for_tile(
                physical_size.x.round() as i32,
                physical_size.y.round() as i32,
            );
            if let Some(host) = self.browser.get_mut(id) {
                host.sync_capture_size(capture_size.0, capture_size.1);
            }
        }

        // Interaction-mode upkeep for the (single) active host.
        if let Some(active_id) = self.browser.active_id() {
            let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
            let escape = exit_hotkey_pressed;
            let in_grace = self
                .browser_activated_at
                .is_some_and(|at| at.elapsed() < BROWSER_FOCUS_GRACE);
            let owns_focus = self
                .browser
                .get(active_id)
                .is_some_and(|host| host.owns_foreground());
            let tile_rect = self
                .canvas
                .last_screen_rect
                .and_then(|canvas_rect| self.browser_tile_placement(active_id, canvas_rect));

            let should_park =
                escape || minimized || tile_rect.is_none() || (!owns_focus && !in_grace);

            if should_park {
                if let Some(host) = self.browser.get_mut(active_id) {
                    host.park();
                }
                self.capture_coordinator.resume_capture(active_id);
                self.browser_activated_at = None;
                if escape {
                    self.canvas.exit_focus();
                }
                // On Escape the (now offscreen) WebView still holds focus;
                // hand it back to the main window so keyboard input works.
                if escape && !minimized {
                    if let Some(hwnd) = self.main_hwnd {
                        unsafe {
                            let _ = SetForegroundWindow(HWND(hwnd as *mut _));
                        }
                    }
                }
            } else if let (Some(hwnd), Some(placement)) = (self.main_hwnd, tile_rect) {
                // Glue the live host to its tile so panning/zooming the
                // canvas or moving the window keeps them in lockstep.
                let preparing = if let Some(host) = self.browser.get_mut(active_id) {
                    host.place(
                        HWND(hwnd as *mut _),
                        placement.page_rect,
                        placement.visible_rect,
                        ctx.pixels_per_point(),
                        false,
                    );
                    host.is_preparing_interaction()
                } else {
                    false
                };
                if preparing {
                    ctx.request_repaint_after(Duration::from_millis(16));
                } else {
                    // The live host now covers the preview, so capture can
                    // continue underneath without exposing its resize frames.
                    self.capture_coordinator.resume_capture(active_id);
                }
            }
        }

        // Let the canvas outline the interactive tile in the accent color.
        self.canvas.interactive_browser = self.browser.active_id();
    }

    /// True when Pluriview owns keyboard focus, either through its main window
    /// or through the WebView host of an interactive browser tile.
    #[cfg(windows)]
    fn owns_foreground(&self) -> bool {
        let foreground = unsafe { GetForegroundWindow() };
        if self.main_hwnd == Some(foreground.0 as isize) {
            return true;
        }
        self.browser
            .active_id()
            .and_then(|id| self.browser.get(id))
            .is_some_and(|host| host.owns_foreground())
    }

    /// Set the window HWND for the tray manager (call once after window is created)
    fn setup_tray_hwnd(&mut self) {
        if self.hwnd_set || self.tray_manager.is_none() {
            return;
        }

        if let Some(hwnd) = self.main_hwnd {
            TrayManager::set_window_hwnd(hwnd);
            self.hwnd_set = true;
            #[cfg(debug_assertions)]
            println!("Set tray HWND: {:?}", hwnd);
        }
    }

    /// Custom title bar (we run with `with_decorations(false)` so the OS
    /// doesn't draw its own white title bar over our dark theme).
    fn title_bar_ui(&mut self, ctx: &egui::Context) {
        let bg = egui::Color32::from_rgb(13, 13, 13);
        let is_maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let workspace_name = self
            .workspaces
            .active()
            .map(|workspace| workspace.name.clone())
            .unwrap_or_else(|| "Default".to_owned());

        let screen = ctx.input(|i| i.screen_rect());
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let hover_zone = TITLE_BAR_HOVER_ZONE.max(self.title_bar_shown_height);
        let hovering = pointer.is_some_and(|pos| {
            pos.x >= screen.min.x
                && pos.x <= screen.max.x
                && pos.y >= screen.min.y
                && pos.y <= screen.min.y + hover_zone
        });
        let want_visible =
            self.app_config.always_show_title_bar || hovering || self.title_bar_menu_open;
        let t = ctx.animate_bool_with_time_and_easing(
            egui::Id::new("title_bar_reveal"),
            want_visible,
            TITLE_BAR_ANIM_SECS,
            title_bar_ease,
        );
        let height = egui::lerp(TITLE_BAR_COLLAPSED_HEIGHT..=TITLE_BAR_HEIGHT, t);
        self.title_bar_shown_height = height;

        egui::TopBottomPanel::top("custom_title_bar")
            .frame(egui::Frame::none().fill(bg))
            .exact_height(height)
            .show(ctx, |ui| {
                let panel_rect = ui.max_rect();
                ui.set_clip_rect(panel_rect);
                // Keep the chrome laid out at full height and slide it with the
                // panel so the bar eases in from above instead of popping.
                let title_bar_rect = egui::Rect::from_min_size(
                    egui::Pos2::new(panel_rect.left(), panel_rect.bottom() - TITLE_BAR_HEIGHT),
                    Vec2::new(panel_rect.width(), TITLE_BAR_HEIGHT),
                );
                ui.multiply_opacity(t);

                // Background drag handle, allocated FIRST so the buttons
                // (added after) take interaction priority where they overlap.
                let drag_response = ui.interact(
                    title_bar_rect,
                    egui::Id::new("title_bar_drag"),
                    egui::Sense::click_and_drag(),
                );
                if drag_response.double_clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maximized));
                } else if drag_response.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }

                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(title_bar_rect), |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.add_space(10.0);
                        ui.image((self.brand_icon.id(), Vec2::splat(20.0)));
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("Pluriview")
                                .size(13.0)
                                .color(egui::Color32::from_rgb(170, 170, 175)),
                        );
                        ui.add_space(7.0);
                        ui.label(
                            egui::RichText::new(format!("/ {workspace_name}"))
                                .size(12.0)
                                .color(egui::Color32::from_rgb(105, 105, 112)),
                        );
                        ui.add_space(16.0);
                        // File / View / Help, inline next to the app name.
                        self.menu_bar(ui, ctx);
                    });
                });

                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(title_bar_rect), |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.visuals_mut().widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;

                        let btn_size = Vec2::new(44.0, TITLE_BAR_HEIGHT);

                        let close = ui.add_sized(
                            btn_size,
                            egui::Button::new(
                                egui::RichText::new(egui_phosphor::regular::X).size(14.0),
                            )
                            .frame(false),
                        );
                        if close.hovered() {
                            ui.painter().rect_filled(
                                close.rect,
                                0.0,
                                egui::Color32::from_rgb(196, 43, 28),
                            );
                            ui.painter().text(
                                close.rect.center(),
                                egui::Align2::CENTER_CENTER,
                                egui_phosphor::regular::X,
                                egui::FontId::proportional(14.0),
                                egui::Color32::WHITE,
                            );
                        }
                        if close.clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }

                        let max_icon = if is_maximized {
                            egui_phosphor::regular::COPY
                        } else {
                            egui_phosphor::regular::SQUARE
                        };
                        let maximize = ui.add_sized(
                            btn_size,
                            egui::Button::new(egui::RichText::new(max_icon).size(12.0))
                                .frame(false),
                        );
                        if maximize.clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maximized));
                        }

                        let minimize = ui.add_sized(
                            btn_size,
                            egui::Button::new(
                                egui::RichText::new(egui_phosphor::regular::MINUS).size(14.0),
                            )
                            .frame(false),
                        );
                        if minimize.clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                    });
                });
            });

        self.title_bar_menu_open = want_visible && ctx.memory(|m| m.any_popup_open());
    }

    /// The File / View / Help menus. Rendered inline in the title bar next
    /// to the app name (Minimal Void: one unified dark strip, no separate
    /// menu-bar row).
    fn menu_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let workspace_entries = self.workspaces.workspaces.clone();
        let canvas_views = self.canvas_views.clone();
        let active_canvas_view = self.active_canvas_view;
        let active_canvas_view_modified =
            active_canvas_view.is_some_and(|index| self.canvas_view_is_modified(index));
        let active_workspace_id = self.workspaces.active_workspace_id.clone();
        let active_workspace_name = self
            .workspaces
            .active()
            .map(|workspace| workspace.name.clone())
            .unwrap_or_else(|| "Default".to_owned());
        let previous_save_available = self
            .storage
            .as_ref()
            .is_some_and(|storage| storage.workspace_backup_exists(&active_workspace_id));
        let mut workspace_action = None;
        let mut canvas_view_action = None;

        ui.visuals_mut().widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
        ui.visuals_mut().widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(30, 30, 35);
        ui.visuals_mut().widgets.active.weak_bg_fill = egui::Color32::from_rgb(40, 40, 45);

        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Add Image...").clicked() {
                    let position = self
                        .canvas
                        .last_screen_rect
                        .map(|rect| self.canvas.screen_to_canvas(rect.center(), rect))
                        .unwrap_or(Pos2::ZERO);
                    self.canvas.pending_media_add = Some(position);
                    ui.close_menu();
                }
                #[cfg(windows)]
                if ui.button("Add Video...").clicked() {
                    let position = self
                        .canvas
                        .last_screen_rect
                        .map(|rect| self.canvas.screen_to_canvas(rect.center(), rect))
                        .unwrap_or(Pos2::ZERO);
                    self.canvas.pending_video_add = Some(position);
                    ui.close_menu();
                }
                #[cfg(windows)]
                if ui.button("Add Stream...").clicked() {
                    let position = self
                        .canvas
                        .last_screen_rect
                        .map(|rect| self.canvas.screen_to_canvas(rect.center(), rect))
                        .unwrap_or(Pos2::ZERO);
                    self.canvas.pending_stream_add = Some(position);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Save Workspace Now").clicked() {
                    if let Err(error) = self.save_active_workspace() {
                        self.workspace_error = Some(error);
                    }
                    ui.close_menu();
                }
                if ui.button("Reload Workspace").clicked() {
                    self.load_active_workspace();
                    ui.close_menu();
                }
                ui.separator();
                if self.tray_manager.is_some() {
                    if ui.button("Minimize to Tray").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        ui.close_menu();
                    }
                    ui.separator();
                }
                if ui.button("Exit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });

            ui.menu_button("Workspace", |ui| {
                ui.label(egui::RichText::new(format!("Current: {active_workspace_name}")).strong());
                ui.separator();
                for workspace in &workspace_entries {
                    let selected = workspace.id == active_workspace_id;
                    if ui.selectable_label(selected, &workspace.name).clicked() {
                        workspace_action = Some(WorkspaceMenuAction::Switch(workspace.id.clone()));
                        ui.close_menu();
                    }
                }
                ui.separator();
                if ui.button("New Workspace...").clicked() {
                    workspace_action =
                        Some(WorkspaceMenuAction::OpenDialog(WorkspaceDialogKind::Create));
                    ui.close_menu();
                }
                if ui.button("Duplicate Workspace...").clicked() {
                    workspace_action = Some(WorkspaceMenuAction::OpenDialog(
                        WorkspaceDialogKind::Duplicate,
                    ));
                    ui.close_menu();
                }
                if ui.button("Rename Workspace...").clicked() {
                    workspace_action =
                        Some(WorkspaceMenuAction::OpenDialog(WorkspaceDialogKind::Rename));
                    ui.close_menu();
                }
                if ui
                    .add_enabled(
                        previous_save_available,
                        egui::Button::new("Restore Previous Save as New Workspace"),
                    )
                    .on_hover_text(
                        "Opens the previous valid save as a separate workspace and preserves the current one.",
                    )
                    .clicked()
                {
                    workspace_action = Some(WorkspaceMenuAction::RestorePreviousSave);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(
                        workspace_entries.len() > 1,
                        egui::Button::new("Delete Workspace..."),
                    )
                    .clicked()
                {
                    workspace_action = Some(WorkspaceMenuAction::ConfirmDelete);
                    ui.close_menu();
                }
            });

            ui.menu_button("View", |ui| {
                let window_picker_label = format!(
                    "Window Picker ({})",
                    self.app_config
                        .keyboard_shortcuts
                        .get(HotkeySlot::ToggleWindowPicker)
                        .display()
                );
                if ui
                    .checkbox(&mut self.picker_open, window_picker_label)
                    .clicked()
                {
                    ui.close_menu();
                }
                let grid_label = format!(
                    "Show Grid ({})",
                    self.app_config
                        .keyboard_shortcuts
                        .get(HotkeySlot::ToggleGrid)
                        .display()
                );
                if ui
                    .checkbox(&mut self.canvas.show_grid, grid_label)
                    .clicked()
                {
                    ui.close_menu();
                }
                if ui
                    .checkbox(
                        &mut self.app_config.always_show_title_bar,
                        "Always Show Top Bar",
                    )
                    .on_hover_text(
                        "When off, the top bar hides until you move the mouse to the top of the window.",
                    )
                    .clicked()
                {
                    self.save_app_config();
                    ui.close_menu();
                }
                if ui.button("Set Wallpaper...").clicked() {
                    self.canvas.pending_wallpaper_pick = true;
                    ui.close_menu();
                }
                if self.canvas.wallpaper.is_some() && ui.button("Clear Wallpaper").clicked() {
                    self.canvas.pending_wallpaper_clear = true;
                    ui.close_menu();
                }
                ui.separator();
                ui.menu_button("Saved Viewpoints", |ui| {
                    canvas_view_action = canvas_views_menu_contents(
                        ui,
                        &canvas_views,
                        active_canvas_view,
                        active_canvas_view_modified,
                        &self.app_config.keyboard_shortcuts,
                    );
                });
                ui.separator();
                if ui.button("Reset View").clicked() {
                    self.canvas.reset();
                    ui.close_menu();
                }
                #[cfg(windows)]
                {
                    ui.separator();
                    let mut enabled = self.browser.adblock_enabled();
                    let response = ui
                        .checkbox(&mut enabled, "Block Ads & Trackers (uBOL)")
                        .on_hover_text(self.browser.adblock_status_text());
                    if response.clicked() {
                        if let Err(error) = self.browser.set_adblock_enabled(enabled) {
                            log::error!("Could not change ad blocker state: {error}");
                        } else {
                            if enabled {
                                self.browser.start_extension_preparation();
                            }
                            if let Err(error) = self.save_active_workspace() {
                                self.workspace_error = Some(error);
                            }
                        }
                        ui.close_menu();
                    }
                    ui.label(
                        egui::RichText::new(self.browser.adblock_status_text())
                            .small()
                            .weak(),
                    );
                    ui.separator();
                    self.stream_audio_menu(ui);
                }
                ui.separator();
                if ui.button("Settings...").clicked() {
                    self.show_settings = true;
                    ui.close_menu();
                }
            });

            ui.menu_button("Help", |ui| {
                if ui.button("Keyboard Shortcuts").clicked() {
                    self.show_shortcuts = true;
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("About").clicked() {
                    self.show_about = true;
                    ui.close_menu();
                }
            });
        });

        if let Some(action) = canvas_view_action {
            self.handle_canvas_view_menu_action(action, ctx);
        }

        if let Some(action) = workspace_action {
            match action {
                WorkspaceMenuAction::Switch(id) => {
                    if let Err(error) = self.switch_workspace(&id) {
                        self.workspace_error = Some(error);
                    }
                }
                WorkspaceMenuAction::OpenDialog(kind) => {
                    let name = match kind {
                        WorkspaceDialogKind::Create => "Untitled Workspace".to_owned(),
                        WorkspaceDialogKind::Duplicate => format!("{active_workspace_name} Copy"),
                        WorkspaceDialogKind::Rename => active_workspace_name,
                    };
                    self.workspace_dialog = Some(WorkspaceDialog {
                        kind,
                        name,
                        focused: false,
                    });
                }
                WorkspaceMenuAction::RestorePreviousSave => {
                    if let Err(error) = self.restore_previous_workspace_save() {
                        self.workspace_error = Some(error);
                    }
                }
                WorkspaceMenuAction::ConfirmDelete => self.confirm_workspace_delete = true,
            }
        }
    }

    fn hotkey_capture_upkeep(&mut self, ctx: &egui::Context) {
        let Some(slot) = self.hotkey_recording else {
            return;
        };
        ctx.request_repaint_after(Duration::from_millis(30));

        #[cfg(windows)]
        if !self.owns_foreground() {
            return;
        }

        if let Some(first_key) = self.hotkey_recording_first {
            if let Some(second_key) = self
                .hotkey_tracker
                .newly_pressed_key_except(Some(first_key))
            {
                self.assign_recorded_hotkey(slot, Hotkey::pair(first_key, second_key));
            } else if !self.hotkey_tracker.is_down(first_key) {
                self.assign_recorded_hotkey(slot, Hotkey::key(first_key));
            }
        } else if let Some(first_key) = self.hotkey_tracker.newly_pressed_key_except(None) {
            let simultaneous_second = self
                .hotkey_tracker
                .newly_pressed_key_except(Some(first_key))
                .filter(|second| self.hotkey_tracker.is_down(*second));
            if let Some(second_key) = simultaneous_second {
                self.assign_recorded_hotkey(slot, Hotkey::pair(first_key, second_key));
            } else if self.hotkey_tracker.is_down(first_key) {
                self.hotkey_recording_first = Some(first_key);
                self.hotkey_error = None;
            } else {
                // A quick tap can begin and end between polls; Windows retains
                // the pressed edge, so it is still a valid single-key binding.
                self.assign_recorded_hotkey(slot, Hotkey::key(first_key));
            }
        }
    }

    fn assign_recorded_hotkey(&mut self, slot: HotkeySlot, hotkey: Hotkey) {
        if let Some(conflict) = self.app_config.keyboard_shortcuts.conflict(slot, hotkey) {
            self.hotkey_error = Some(format!(
                "{} is already assigned to {}.",
                hotkey.display(),
                conflict.label()
            ));
            self.hotkey_recording_first = None;
        } else {
            self.app_config.keyboard_shortcuts.set(slot, hotkey);
            self.hotkey_recording = None;
            self.hotkey_recording_first = None;
            self.hotkey_error = None;
            self.save_app_config();
        }
    }

    fn shortcut_settings_row(&mut self, ui: &mut egui::Ui, slot: HotkeySlot) {
        ui.label(slot.label());
        let recording = self.hotkey_recording == Some(slot);
        let text = if recording {
            self.hotkey_recording_first.map_or_else(
                || "Press a key…".to_owned(),
                |first| format!("{} + …", Hotkey::key(first).display()),
            )
        } else {
            self.app_config.keyboard_shortcuts.get(slot).display()
        };
        let button = egui::Button::new(text).min_size(Vec2::new(180.0, 24.0));
        if ui.add(button).clicked() {
            self.hotkey_recording = Some(slot);
            self.hotkey_recording_first = None;
            self.hotkey_error = None;
        }
        ui.end_row();
    }

    fn settings_ui(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            self.hotkey_recording = None;
            self.hotkey_recording_first = None;
            return;
        }

        self.hotkey_capture_upkeep(ctx);

        let mut open = self.show_settings;
        let mut close = false;
        let mut action = None;
        let mut restore_hotkeys = false;
        let is_viewpoint_shortcut = |slot: HotkeySlot| {
            matches!(
                slot,
                HotkeySlot::PreviousCanvasView | HotkeySlot::NextCanvasView
            ) || HotkeySlot::CANVAS_VIEWS.contains(&slot)
        };
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(600.0)
            .default_height(680.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::window(&ctx.style()).fill(egui::Color32::from_rgb(25, 25, 28)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(580.0)
                    .show(ui, |ui| {
                        ui.heading("External tools");
                        ui.label(
                            egui::RichText::new(
                                "Video files play with bundled libmpv. Streamlink is only needed for live stream URLs.",
                            )
                            .weak(),
                        );
                        ui.add_space(8.0);
                        for kind in ToolKind::SETTINGS {
                            ui.group(|ui| {
                                Self::external_tool_settings_row(
                                    ui,
                                    kind,
                                    &self.external_tools,
                                    &mut action,
                                );
                            });
                            ui.add_space(8.0);
                        }
                        ui.separator();
                        ui.add_space(12.0);
                        ui.heading("Keyboard shortcuts");
                        ui.label(
                            egui::RichText::new(
                                "Click a shortcut, then tap one key, or hold one key and press a second. Mouse controls are not changed here.",
                            )
                            .weak(),
                        );
                        ui.add_space(8.0);

                        egui::Grid::new("settings_hotkeys_grid")
                            .num_columns(2)
                            .spacing([24.0, 7.0])
                            .show(ui, |ui| {
                                for slot in HotkeySlot::ALL.into_iter().filter(|slot| !is_viewpoint_shortcut(*slot)) {
                                    self.shortcut_settings_row(ui, slot);
                                }
                            });

                        ui.add_space(12.0);
                        let viewpoints = egui::CollapsingHeader::new("Saved Viewpoints")
                            .id_salt("settings_saved_viewpoint_shortcuts")
                            .default_open(false)
                            .show(ui, |ui| {
                                egui::Grid::new("settings_viewpoint_hotkeys_grid")
                                    .num_columns(2)
                                    .spacing([24.0, 7.0])
                                    .show(ui, |ui| {
                                        for slot in HotkeySlot::ALL.into_iter().filter(|slot| is_viewpoint_shortcut(*slot)) {
                                            self.shortcut_settings_row(ui, slot);
                                        }
                                    });
                            });
                        if viewpoints.body_response.is_none()
                            && self.hotkey_recording.is_some_and(is_viewpoint_shortcut)
                        {
                            self.hotkey_recording = None;
                            self.hotkey_recording_first = None;
                            self.hotkey_error = None;
                        }

                        if let Some(error) = &self.hotkey_error {
                            ui.add_space(6.0);
                            ui.colored_label(egui::Color32::from_rgb(235, 120, 120), error);
                        } else if self.hotkey_recording.is_some() {
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new(
                                    "Listening… release for one key, or hold it and press another for a two-key shortcut.",
                                )
                                .weak(),
                            );
                        }

                        ui.add_space(8.0);
                        if ui.button("Restore Default Keys").clicked() {
                            restore_hotkeys = true;
                        }

                        if let Some(error) = &self.config_error {
                            ui.colored_label(egui::Color32::from_rgb(235, 120, 120), error);
                            ui.add_space(6.0);
                        }
                    });

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Re-scan All").clicked() {
                        self.external_tools.rescan_all();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                });
            });
        self.show_settings = open && !close;
        if !self.show_settings {
            self.hotkey_recording = None;
            self.hotkey_recording_first = None;
            self.hotkey_error = None;
        }

        if restore_hotkeys {
            self.app_config.keyboard_shortcuts = HotkeyBindings::default();
            self.hotkey_recording = None;
            self.hotkey_recording_first = None;
            self.hotkey_error = None;
            self.save_app_config();
        }

        match action {
            Some(SettingsAction::Browse(kind)) => {
                if let Some(path) = external_tools::pick_executable(self.main_hwnd, kind) {
                    self.set_external_tool_override(kind, Some(path));
                }
            }
            Some(SettingsAction::UseAutoDetected(kind)) => {
                self.set_external_tool_override(kind, None);
            }
            Some(SettingsAction::Rescan(kind)) => self.external_tools.rescan(kind),
            None => {}
        }
    }

    fn external_tool_error_ui(&mut self, ctx: &egui::Context) {
        let Some(message) = self.external_tool_error.clone() else {
            return;
        };
        let mut open_settings = false;
        let mut close = false;
        egui::Window::new("External Tools Required")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(message);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Open Settings").clicked() {
                        open_settings = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        if open_settings {
            self.external_tool_error = None;
            self.show_settings = true;
        } else if close {
            self.external_tool_error = None;
        }
    }

    fn video_action_error_ui(&mut self, ctx: &egui::Context) {
        let Some(message) = self.video_action_error.clone() else {
            return;
        };
        let mut dismiss = false;
        egui::Window::new("Video Control Error")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(message);
                ui.add_space(8.0);
                if ui.button("Dismiss").clicked() {
                    dismiss = true;
                }
            });
        if dismiss {
            self.video_action_error = None;
        }
    }

    fn external_tool_settings_row(
        ui: &mut egui::Ui,
        kind: ToolKind,
        tools: &ExternalTools,
        action: &mut Option<SettingsAction>,
    ) {
        let status = tools.status(kind);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(kind.display_name()).strong().size(16.0));
            let (label, color) = match status {
                ToolStatus::Checking => ("Checking...", egui::Color32::from_rgb(140, 170, 235)),
                ToolStatus::Available { .. } => {
                    ("Available", egui::Color32::from_rgb(110, 205, 135))
                }
                ToolStatus::Invalid { .. } => ("Invalid", egui::Color32::from_rgb(235, 120, 120)),
                ToolStatus::Missing => ("Not found", egui::Color32::from_rgb(220, 180, 95)),
            };
            ui.colored_label(color, label);
        });

        let displayed_path = status.path().or_else(|| tools.override_path(kind));
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Path:").strong());
            ui.label(
                displayed_path
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "No executable detected".to_owned()),
            );
        });

        match status {
            ToolStatus::Available {
                version, source, ..
            } => {
                ui.label(format!("Version: {version}"));
                ui.label(egui::RichText::new(source.label()).small().weak());
            }
            ToolStatus::Invalid { error, source, .. } => {
                ui.colored_label(egui::Color32::from_rgb(235, 120, 120), error);
                ui.label(egui::RichText::new(source.label()).small().weak());
            }
            ToolStatus::Checking => {
                ui.label(
                    egui::RichText::new("Running --version off the UI thread")
                        .small()
                        .weak(),
                );
            }
            ToolStatus::Missing => {
                ui.label(
                    egui::RichText::new(
                        "Checked beside Pluriview, PATH, and common Windows install locations.",
                    )
                    .small()
                    .weak(),
                );
            }
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Browse...").clicked() {
                *action = Some(SettingsAction::Browse(kind));
            }
            if ui
                .add_enabled(
                    tools.override_path(kind).is_some(),
                    egui::Button::new("Use Auto-detected"),
                )
                .clicked()
            {
                *action = Some(SettingsAction::UseAutoDetected(kind));
            }
            if ui.button("Re-scan").clicked() {
                *action = Some(SettingsAction::Rescan(kind));
            }
        });
    }

    fn set_external_tool_override(&mut self, kind: ToolKind, path: Option<std::path::PathBuf>) {
        match kind {
            ToolKind::Mpv => {}
            ToolKind::Streamlink => self.app_config.external_tools.streamlink_path = path.clone(),
        }
        self.external_tools.set_override(kind, path);
        self.save_app_config();
    }

    fn save_app_config(&mut self) {
        self.config_error = match &self.storage {
            Some(storage) => storage
                .save_config(&self.app_config)
                .err()
                .map(|error| format!("Could not save settings: {error}")),
            None => Some("Settings storage is unavailable.".to_owned()),
        };
    }

    fn validate_canvas_view_name(
        &self,
        name: &str,
        renamed_index: Option<usize>,
    ) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Viewpoint name cannot be empty.".to_owned());
        }
        if name.chars().count() > 60 {
            return Err("Viewpoint names can contain at most 60 characters.".to_owned());
        }
        if self.canvas_views.iter().enumerate().any(|(index, view)| {
            Some(index) != renamed_index && view.name.eq_ignore_ascii_case(name)
        }) {
            return Err("A saved viewpoint with that name already exists.".to_owned());
        }
        Ok(name.to_owned())
    }

    fn current_canvas_view(&self, name: String) -> Result<CanvasView, String> {
        let canvas_rect = self
            .canvas
            .last_screen_rect
            .ok_or_else(|| "The canvas is not ready yet.".to_owned())?;
        let center = self.canvas.camera_center(canvas_rect);
        Ok(CanvasView {
            name,
            center: (center.x, center.y),
            zoom: self.canvas.zoom,
        })
    }

    fn canvas_view_is_modified(&self, index: usize) -> bool {
        if self.active_canvas_view != Some(index) || self.canvas.is_camera_transitioning() {
            return false;
        }
        let Some(view) = self.canvas_views.get(index) else {
            return false;
        };
        let Some(canvas_rect) = self.canvas.last_screen_rect else {
            return false;
        };
        !camera_matches_canvas_view(
            self.canvas.camera_center(canvas_rect),
            self.canvas.zoom,
            view,
        )
    }

    fn jump_to_canvas_view(&mut self, index: usize, ctx: &egui::Context) {
        let Some(view) = self.canvas_views.get(index).cloned() else {
            return;
        };
        let Some(canvas_rect) = self.canvas.last_screen_rect else {
            return;
        };
        let now = ctx.input(|input| input.time);
        self.canvas.start_camera_transition(
            Pos2::new(view.center.0, view.center.1),
            view.zoom,
            canvas_rect,
            now,
        );
        self.active_canvas_view = Some(index);
        ctx.request_repaint();
    }

    fn jump_to_adjacent_canvas_view(&mut self, forward: bool, ctx: &egui::Context) {
        if let Some(index) =
            adjacent_canvas_view_index(self.active_canvas_view, self.canvas_views.len(), forward)
        {
            self.jump_to_canvas_view(index, ctx);
        }
    }

    fn persist_canvas_view_change(&mut self) {
        if let Err(error) = self.save_active_workspace() {
            self.workspace_error = Some(error);
        }
    }

    fn handle_canvas_view_menu_action(
        &mut self,
        action: CanvasViewMenuAction,
        ctx: &egui::Context,
    ) {
        match action {
            CanvasViewMenuAction::Jump(index) => self.jump_to_canvas_view(index, ctx),
            CanvasViewMenuAction::Previous => self.jump_to_adjacent_canvas_view(false, ctx),
            CanvasViewMenuAction::Next => self.jump_to_adjacent_canvas_view(true, ctx),
            CanvasViewMenuAction::OpenCreateDialog => {
                self.canvas_view_dialog = Some(CanvasViewDialog {
                    kind: CanvasViewDialogKind::Create,
                    name: format!("Viewpoint {}", self.canvas_views.len() + 1),
                    focused: false,
                    error: None,
                });
            }
            CanvasViewMenuAction::OpenRenameDialog(index) => {
                if let Some(view) = self.canvas_views.get(index) {
                    self.canvas_view_dialog = Some(CanvasViewDialog {
                        kind: CanvasViewDialogKind::Rename(index),
                        name: view.name.clone(),
                        focused: false,
                        error: None,
                    });
                }
            }
            CanvasViewMenuAction::Update(index) => {
                let Some(name) = self.canvas_views.get(index).map(|view| view.name.clone()) else {
                    return;
                };
                match self.current_canvas_view(name) {
                    Ok(view) => {
                        self.canvas_views[index] = view;
                        self.active_canvas_view = Some(index);
                        self.persist_canvas_view_change();
                    }
                    Err(error) => self.workspace_error = Some(error),
                }
            }
            CanvasViewMenuAction::Delete(index) => {
                if index < self.canvas_views.len() {
                    let was_active = self.active_canvas_view == Some(index);
                    let view = self.canvas_views.remove(index);
                    if was_active {
                        self.canvas.cancel_camera_transition();
                    }
                    self.active_canvas_view =
                        active_canvas_view_after_deletion(self.active_canvas_view, index);
                    self.deleted_canvas_view = Some(DeletedCanvasView {
                        removed_at: Instant::now(),
                        index,
                        view,
                        was_active,
                    });
                    self.persist_canvas_view_change();
                    ctx.request_repaint();
                }
            }
        }
    }

    fn canvas_view_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.canvas_view_dialog else {
            return;
        };
        let (title, submit_label) = match dialog.kind {
            CanvasViewDialogKind::Create => ("Save Viewpoint", "Save"),
            CanvasViewDialogKind::Rename(_) => ("Rename Viewpoint", "Rename"),
        };
        let mut submit = false;
        let mut cancel = false;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Viewpoint name");
                let response =
                    ui.add(egui::TextEdit::singleline(&mut dialog.name).desired_width(300.0));
                if !dialog.focused {
                    response.request_focus();
                    dialog.focused = true;
                }
                let enter =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if let Some(error) = &dialog.error {
                    ui.add_space(4.0);
                    ui.colored_label(egui::Color32::from_rgb(235, 120, 120), error);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(submit_label).clicked() || enter {
                        submit = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if cancel {
            self.canvas_view_dialog = None;
        } else if submit {
            let mut dialog = self
                .canvas_view_dialog
                .take()
                .expect("canvas view dialog exists");
            let renamed_index = match dialog.kind {
                CanvasViewDialogKind::Create => None,
                CanvasViewDialogKind::Rename(index) => Some(index),
            };
            match self.validate_canvas_view_name(&dialog.name, renamed_index) {
                Ok(name) => match dialog.kind {
                    CanvasViewDialogKind::Create => match self.current_canvas_view(name) {
                        Ok(view) => {
                            self.canvas_views.push(view);
                            self.active_canvas_view = self.canvas_views.len().checked_sub(1);
                            self.persist_canvas_view_change();
                        }
                        Err(error) => {
                            dialog.error = Some(error);
                            self.canvas_view_dialog = Some(dialog);
                        }
                    },
                    CanvasViewDialogKind::Rename(index) => {
                        if let Some(view) = self.canvas_views.get_mut(index) {
                            view.name = name;
                            self.persist_canvas_view_change();
                        }
                    }
                },
                Err(error) => {
                    dialog.error = Some(error);
                    self.canvas_view_dialog = Some(dialog);
                }
            }
        }
    }

    fn canvas_view_undo_toast_ui(&mut self, ctx: &egui::Context) {
        if self.canvas_only {
            return;
        }
        let Some(deleted) = self.deleted_canvas_view.as_ref() else {
            return;
        };
        let age = deleted.removed_at.elapsed().as_secs_f32();
        if age >= CANVAS_VIEW_UNDO_SECS {
            self.deleted_canvas_view = None;
            return;
        }
        let Some(canvas_rect) = self.canvas.last_screen_rect else {
            return;
        };

        let fade_in = (age / 0.15).clamp(0.0, 1.0);
        let fade_out = ((CANVAS_VIEW_UNDO_SECS - age) / 0.5).clamp(0.0, 1.0);
        let fade = fade_in.min(fade_out);
        let name = if deleted.view.name.chars().count() > 32 {
            format!(
                "{}...",
                deleted.view.name.chars().take(29).collect::<String>()
            )
        } else {
            deleted.view.name.clone()
        };
        let position = Pos2::new(canvas_rect.center().x - 130.0, canvas_rect.top() + 16.0);
        let mut undo = false;
        egui::Area::new(egui::Id::new("canvas_view_undo_toast"))
            .order(egui::Order::Foreground)
            .fixed_pos(position)
            .show(ctx, |ui| {
                ui.multiply_opacity(fade);
                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(24, 24, 28))
                    .rounding(10.0)
                    .inner_margin(egui::Margin::symmetric(12.0, 7.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(format!("Deleted “{name}”"));
                            if ui
                                .button(
                                    egui::RichText::new("Undo")
                                        .color(egui::Color32::from_rgb(125, 185, 255)),
                                )
                                .clicked()
                            {
                                undo = true;
                            }
                        });
                    });
            });
        ctx.request_repaint();

        if undo {
            let deleted = self
                .deleted_canvas_view
                .take()
                .expect("deleted canvas view exists");
            if self
                .canvas_views
                .iter()
                .any(|view| view.name.eq_ignore_ascii_case(&deleted.view.name))
            {
                self.workspace_error = Some(format!(
                    "Could not restore “{}” because a saved viewpoint now uses that name.",
                    deleted.view.name
                ));
                return;
            }
            let index = deleted.index.min(self.canvas_views.len());
            self.active_canvas_view =
                active_canvas_view_after_insertion(self.active_canvas_view, index);
            self.canvas_views.insert(index, deleted.view);
            if deleted.was_active && self.active_canvas_view.is_none() {
                self.active_canvas_view = Some(index);
            }
            self.persist_canvas_view_change();
        }
    }

    fn workspace_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.workspace_dialog else {
            return;
        };
        let (title, submit_label) = match dialog.kind {
            WorkspaceDialogKind::Create => ("New Workspace", "Create"),
            WorkspaceDialogKind::Duplicate => ("Duplicate Workspace", "Duplicate"),
            WorkspaceDialogKind::Rename => ("Rename Workspace", "Rename"),
        };
        let mut submit = false;
        let mut cancel = false;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Workspace name");
                let response =
                    ui.add(egui::TextEdit::singleline(&mut dialog.name).desired_width(300.0));
                if !dialog.focused {
                    response.request_focus();
                    dialog.focused = true;
                }
                let enter =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(submit_label).clicked() || enter {
                        submit = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if cancel {
            self.workspace_dialog = None;
        } else if submit {
            let dialog = self
                .workspace_dialog
                .take()
                .expect("workspace dialog exists");
            let result = match dialog.kind {
                WorkspaceDialogKind::Create => self.create_workspace(&dialog.name, false),
                WorkspaceDialogKind::Duplicate => self.create_workspace(&dialog.name, true),
                WorkspaceDialogKind::Rename => self.rename_active_workspace(&dialog.name),
            };
            if let Err(error) = result {
                self.workspace_error = Some(error);
            }
        }
    }

    fn workspace_delete_confirmation_ui(&mut self, ctx: &egui::Context) {
        if !self.confirm_workspace_delete {
            return;
        }
        let name = self
            .workspaces
            .active()
            .map(|workspace| workspace.name.clone())
            .unwrap_or_else(|| "this workspace".to_owned());
        let mut delete = false;
        let mut cancel = false;
        egui::Window::new("Delete Workspace")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("Delete “{name}”? This cannot be undone."));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        delete = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if delete {
            self.confirm_workspace_delete = false;
            if let Err(error) = self.delete_active_workspace() {
                self.workspace_error = Some(error);
            }
        } else if cancel {
            self.confirm_workspace_delete = false;
        }
    }

    /// "Stream Audio Monitor" submenu: pick the output device that receives
    /// a copy of the browser tiles' audio (so Discord/OBS window capture can
    /// hear them), or Off. Pick a device you don't listen to — a virtual
    /// cable or an unconnected output — or you'll hear tiles twice.
    #[cfg(windows)]
    fn stream_audio_menu(&mut self, ui: &mut egui::Ui) {
        let current_name = self.monitor_device.as_ref().map(|(_, name)| name.clone());
        let label = match &current_name {
            Some(name) => format!("Stream Audio Monitor: {name}"),
            None => "Stream Audio Monitor: Off".to_owned(),
        };
        ui.menu_button(label, |ui| {
            ui.label("Replay tile audio to a device so Discord/OBS\nwindow shares carry sound. Pick one you don't\nlisten to (virtual cable, unused output).\nBrowser tiles replay automatically; window\ntiles use the Stream Audio toggle on hover.");
            if let Some(monitor) = &self.audio_monitor {
                let status = monitor.status();
                ui.label(format!("Browser audio: {}", status.label())).on_hover_text(status.detail());
            }
            for (pid, monitor) in &self.window_audio_monitors {
                let name = self.preview_manager.all().find(|preview| {
                    preview.is_window_capture() && preview.window_handle.as_ref()
                        .is_some_and(|handle| handle.process_id == *pid)
                }).map(|preview| preview.title.as_str()).unwrap_or("Window audio");
                let status = monitor.status();
                ui.label(format!("{name}: {}", status.label())).on_hover_text(status.detail());
            }
            ui.separator();
            if ui
                .radio(self.monitor_device.is_none(), "Off")
                .clicked()
            {
                self.monitor_device = None;
                ui.close_menu();
            }
            for device in crate::audio::render_devices() {
                if device.is_default {
                    // Monitoring to the device you're hearing = double audio.
                    continue;
                }
                let selected = self
                    .monitor_device
                    .as_ref()
                    .is_some_and(|(id, _)| *id == device.id);
                if ui.radio(selected, &device.name).clicked() {
                    self.monitor_device = Some((device.id, device.name));
                    ui.close_menu();
                }
            }
        });
    }

    /// We turned off OS decorations for the custom title bar, which also
    /// removes the native resize border. Re-implement it: a thin hit-band
    /// along each edge that shows a resize cursor and starts an OS-driven
    /// resize drag (so resizing still feels native).
    fn handle_frameless_resize(&self, ctx: &egui::Context) {
        use egui::viewport::ResizeDirection as RD;

        if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
            return;
        }
        // Don't fight with widgets that already want the pointer (e.g. a
        // preview's own resize handles) by only acting near the window edge.
        let border = 6.0;
        // The custom title bar owns the entire top strip (drag-to-move plus
        // the min/max/close buttons) — never treat that area as a resize
        // zone, or a click on a title bar button can also start a native
        // resize drag and leave the window stuck at a tiny size.
        let title_bar_height = if self.canvas_only {
            0.0
        } else {
            self.title_bar_shown_height
        };
        let rect = ctx.input(|i| i.screen_rect());
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
            return;
        };

        if pos.y < rect.min.y + title_bar_height {
            return;
        }

        let on_left = pos.x <= rect.min.x + border;
        let on_right = pos.x >= rect.max.x - border;
        let on_bottom = pos.y >= rect.max.y - border;

        let direction = match (on_left, on_right, on_bottom) {
            (true, _, true) => Some(RD::SouthWest),
            (_, true, true) => Some(RD::SouthEast),
            (true, false, false) => Some(RD::West),
            (false, true, false) => Some(RD::East),
            (false, false, true) => Some(RD::South),
            _ => None,
        };

        let Some(direction) = direction else {
            return;
        };

        let cursor = match direction {
            RD::NorthWest | RD::SouthEast => egui::CursorIcon::ResizeNwSe,
            RD::NorthEast | RD::SouthWest => egui::CursorIcon::ResizeNeSw,
            RD::North | RD::South => egui::CursorIcon::ResizeVertical,
            RD::East | RD::West => egui::CursorIcon::ResizeHorizontal,
        };
        ctx.set_cursor_icon(cursor);

        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
        }
    }

    /// Render the canvas right-click "Add Window..." popup, if open.
    fn quick_add_ui(&mut self, ctx: &egui::Context) {
        let Some(popup) = &mut self.quick_add else {
            return;
        };

        // Read this before drawing the popup: the focused search box's
        // TextEdit consumes the Escape key itself (to drop focus), so
        // checking afterwards would always see it as already consumed.
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let mut clicked_index = None;

        let area_response = egui::Area::new(egui::Id::new("quick_add_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(popup.screen_pos)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(22, 22, 26))
                    .rounding(8.0)
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(45, 45, 52)))
                    .inner_margin(egui::Margin::same(10.0))
                    .show(ui, |ui| {
                        ui.set_width(240.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(egui_phosphor::regular::MAGNIFYING_GLASS)
                                    .size(13.0)
                                    .color(egui::Color32::from_rgb(140, 140, 150)),
                            );
                            ui.add_space(6.0);
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut popup.search)
                                    .desired_width(ui.available_width())
                                    .hint_text("Search windows...")
                                    .frame(false),
                            );
                            resp.request_focus();
                        });

                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(4.0);

                        let filter = popup.search.to_lowercase();
                        let matches = |w: &WindowInfo| w.matches_filter(&filter);

                        egui::ScrollArea::vertical()
                            .max_height(260.0)
                            .show(ui, |ui| {
                                let mut any = false;
                                for (idx, window) in popup.windows.iter().enumerate() {
                                    if !matches(window) {
                                        continue;
                                    }
                                    any = true;

                                    let label = if window.title.is_empty() {
                                        &window.exe_name
                                    } else {
                                        &window.title
                                    };
                                    let resp = ui.add_sized(
                                        Vec2::new(ui.available_width(), 22.0),
                                        egui::Button::new(egui::RichText::new(label).size(12.5))
                                            .frame(false),
                                    );
                                    if resp.clicked() {
                                        clicked_index = Some(idx);
                                    }
                                }

                                if !any {
                                    ui.add_space(8.0);
                                    ui.label(
                                        egui::RichText::new("No matching windows")
                                            .size(11.5)
                                            .color(egui::Color32::from_rgb(120, 120, 128)),
                                    );
                                }
                            });
                    });
            });

        if ctx.input(|i| i.pointer.any_click()) {
            if let Some(click_pos) = ctx.input(|i| i.pointer.interact_pos()) {
                if !area_response.response.rect.contains(click_pos) {
                    close = true;
                }
            }
        }

        if let Some(idx) = clicked_index {
            if let Some(popup) = &self.quick_add {
                if let Some(window) = popup.windows.get(idx) {
                    spawn_preview(
                        window,
                        &mut self.preview_manager,
                        &mut self.capture_coordinator,
                        popup.canvas_pos,
                        Vec2::new(320.0, 240.0),
                    );
                }
            }
            close = true;
        }

        if close {
            self.quick_add = None;
        }
    }

    /// Keep source editing out of a failed-load canvas until the user recovers
    /// it or explicitly opens a different workspace.
    fn workspace_recovery_ui(&mut self, ctx: &egui::Context) {
        self.handle_frameless_resize(ctx);
        egui::TopBottomPanel::top("workspace_recovery_title").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let title = ui.label("Pluriview — Workspace Recovery");
                if title.interact(egui::Sense::drag()).drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("Minimize").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                });
            });
        });
        let mut retry = false;
        let mut recover = false;
        let mut create = false;
        let mut switch = None;
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(24.0);
                ui.heading("This workspace could not be opened");
                ui.label("Editing and saving are paused to protect your saved files.");
                ui.add_space(12.0);
                if let Some(error) = self
                    .workspace_error
                    .as_deref()
                    .or(self.workspace_save.load_error())
                {
                    ui.label(error);
                }
                ui.add_space(16.0);
                retry = ui.button("Retry Load").clicked();
                if self.workspace_persistence_ready {
                    let backup_available = self.storage.as_ref().is_some_and(|storage| {
                        storage.workspace_backup_exists(&self.workspaces.active_workspace_id)
                    });
                    recover = ui
                        .add_enabled(
                            backup_available,
                            egui::Button::new("Restore Previous Save as New Workspace"),
                        )
                        .clicked();
                    create = ui.button("Open New Workspace…").clicked();
                    ui.add_space(16.0);
                    for workspace in &self.workspaces.workspaces {
                        if workspace.id != self.workspaces.active_workspace_id
                            && ui.button(format!("Open {}", workspace.name)).clicked()
                        {
                            switch = Some(workspace.id.clone());
                        }
                    }
                }
            });
        });
        if retry {
            if !self.workspace_persistence_ready {
                if let Some(storage) = &self.storage {
                    match storage.load_or_initialize_workspaces() {
                        Ok(index) => {
                            self.workspaces = index;
                            self.workspace_persistence_ready = true;
                        }
                        Err(error) => self.workspace_error = Some(error.to_string()),
                    }
                }
            }
            self.load_active_workspace();
        } else if recover {
            if let Err(error) = self.restore_previous_workspace_save() {
                self.workspace_error = Some(error);
            }
        } else if let Some(id) = switch {
            if let Err(error) = self.switch_workspace(&id) {
                self.workspace_error = Some(error);
            }
        } else if create {
            self.workspace_dialog = Some(WorkspaceDialog {
                kind: WorkspaceDialogKind::Create,
                name: "Untitled Workspace".to_owned(),
                focused: false,
            });
        }
        self.workspace_dialog_ui(ctx);
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    /// Load the active named workspace if it has been saved before.
    fn load_active_workspace(&mut self) {
        if !self.workspace_persistence_ready {
            return;
        }
        let Some(storage) = &self.storage else {
            return;
        };
        let id = self.workspaces.active_workspace_id.clone();
        match self.workspace_save.load(storage, &id) {
            Ok(layout) => {
                self.apply_layout(&layout);
                self.remember_saved_workspace(&layout);
                #[cfg(debug_assertions)]
                println!(
                    "Loaded workspace {id} with {} previews",
                    layout.previews.len()
                );
            }
            Err(error) => {
                self.workspace_error = Some(error);
            }
        }
    }

    /// Save the current workspace and mirror it to legacy autosave.json.
    fn save_active_workspace(&mut self) -> Result<(), String> {
        if !self.workspace_persistence_ready {
            return Err(
                "Workspace saving is disabled because the catalog could not be loaded safely. Restart after repairing the workspace files."
                    .to_owned(),
            );
        }
        let Some(storage) = &self.storage else {
            return Err("Workspace storage is unavailable.".to_owned());
        };
        let layout = self.create_layout();
        self.workspace_save
            .save(storage, &self.workspaces, &layout)?;
        self.last_workspace_autosave_check = Instant::now();
        #[cfg(debug_assertions)]
        println!(
            "Saved workspace {} with {} previews",
            self.workspaces.active_workspace_id,
            layout.previews.len()
        );
        Ok(())
    }

    fn remember_saved_workspace(&mut self, layout: &SavedLayout) {
        self.workspace_save.remember(layout);
        self.workspace_error = None;
        self.last_workspace_autosave_check = Instant::now();
    }

    fn workspace_autosave_upkeep(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_workspace_autosave_check);
        if elapsed < WORKSPACE_AUTOSAVE_INTERVAL {
            ctx.request_repaint_after(WORKSPACE_AUTOSAVE_INTERVAL - elapsed);
            return;
        }
        self.last_workspace_autosave_check = now;
        if !self.workspace_persistence_ready || !self.workspace_save.is_ready() {
            return;
        }
        if let Err(error) = self.save_active_workspace() {
            self.workspace_error = Some(error);
        }
    }

    fn save_before_workspace_change(&mut self) -> Result<(), String> {
        if !self.workspace_persistence_ready {
            return Err("The workspace catalog must be recovered first.".to_owned());
        }
        if self.workspace_save.is_ready() {
            self.save_active_workspace()?;
        }
        Ok(())
    }

    fn blank_workspace_layout(&self) -> SavedLayout {
        let mut layout = self.create_layout();
        layout.canvas = CanvasLayout::default();
        layout.previews.clear();
        layout
    }

    fn switch_workspace(&mut self, id: &str) -> Result<(), String> {
        if id == self.workspaces.active_workspace_id {
            return Ok(());
        }
        if self
            .workspaces
            .workspaces
            .iter()
            .all(|workspace| workspace.id != id)
        {
            return Err("That workspace no longer exists.".to_owned());
        }
        self.save_before_workspace_change()?;

        let layout = self
            .storage
            .as_ref()
            .ok_or_else(|| "Workspace storage is unavailable.".to_owned())?
            .load_workspace(id)
            .map_err(|error| format!("Could not load workspace: {error}"))?;
        let previous_id =
            std::mem::replace(&mut self.workspaces.active_workspace_id, id.to_owned());
        if let Some(storage) = &self.storage {
            if let Err(error) = storage.save_workspace_index(&self.workspaces) {
                self.workspaces.active_workspace_id = previous_id;
                return Err(format!("Could not select workspace: {error}"));
            }
            if let Err(error) = storage.save_autosave(&layout) {
                self.workspaces.active_workspace_id = previous_id;
                let _ = storage.save_workspace_index(&self.workspaces);
                return Err(format!(
                    "Could not update the compatibility autosave: {error}"
                ));
            }
        }
        self.apply_layout(&layout);
        self.remember_saved_workspace(&layout);
        Ok(())
    }

    fn create_workspace(&mut self, name: &str, duplicate: bool) -> Result<(), String> {
        let name = self.valid_workspace_name(name, None)?;
        if duplicate && !self.workspace_save.is_ready() {
            return Err("Load or recover the workspace before duplicating it.".to_owned());
        }
        self.save_before_workspace_change()?;
        let layout = if duplicate {
            self.create_layout()
        } else {
            self.blank_workspace_layout()
        };
        let previous_index = self.workspaces.clone();
        let id = self.workspaces.add(name);
        self.workspaces.active_workspace_id = id;

        let Some(storage) = &self.storage else {
            self.workspaces = previous_index;
            return Err("Workspace storage is unavailable.".to_owned());
        };
        if let Err(error) = storage.save_active_workspace(&self.workspaces, &layout) {
            self.workspaces = previous_index;
            return Err(format!("Could not create workspace: {error}"));
        }
        self.apply_layout(&layout);
        self.remember_saved_workspace(&layout);
        Ok(())
    }

    fn rename_active_workspace(&mut self, name: &str) -> Result<(), String> {
        let active_id = self.workspaces.active_workspace_id.clone();
        let name = self.valid_workspace_name(name, Some(&active_id))?;
        let previous_index = self.workspaces.clone();
        if !self.workspaces.rename(&active_id, name) {
            return Err("The active workspace no longer exists.".to_owned());
        }
        let result = self
            .storage
            .as_ref()
            .ok_or_else(|| "Workspace storage is unavailable.".to_owned())?
            .save_workspace_index(&self.workspaces);
        if let Err(error) = result {
            self.workspaces = previous_index;
            return Err(format!("Could not rename workspace: {error}"));
        }
        Ok(())
    }

    fn delete_active_workspace(&mut self) -> Result<(), String> {
        if self.workspaces.workspaces.len() <= 1 {
            return Err("The only workspace cannot be deleted.".to_owned());
        }
        let deleted_id = self.workspaces.active_workspace_id.clone();
        let previous_index = self.workspaces.clone();
        if !self.workspaces.remove(&deleted_id) {
            return Err("The active workspace could not be removed.".to_owned());
        }
        let next_id = self.workspaces.active_workspace_id.clone();
        let Some(storage) = &self.storage else {
            self.workspaces = previous_index;
            return Err("Workspace storage is unavailable.".to_owned());
        };
        let layout = match storage.load_workspace(&next_id) {
            Ok(layout) => layout,
            Err(error) => {
                self.workspaces = previous_index;
                return Err(format!("Could not open the next workspace: {error}"));
            }
        };
        if let Err(error) = storage.save_workspace_index(&self.workspaces) {
            self.workspaces = previous_index;
            return Err(format!("Could not update the workspace list: {error}"));
        }
        if let Err(error) = storage.save_autosave(&layout) {
            self.workspaces = previous_index;
            let _ = storage.save_workspace_index(&self.workspaces);
            return Err(format!(
                "Could not update the compatibility autosave: {error}"
            ));
        }
        let deletion_error = storage.delete_workspace(&deleted_id).err().map(|error| {
            format!(
                "The workspace was removed from the list, but its old file could not be deleted: {error}"
            )
        });
        self.apply_layout(&layout);
        self.remember_saved_workspace(&layout);
        self.workspace_error = deletion_error;
        Ok(())
    }

    fn restore_previous_workspace_save(&mut self) -> Result<(), String> {
        if !self.workspace_persistence_ready {
            return Err(
                "Workspace saving is unavailable until the catalog is repaired.".to_owned(),
            );
        }
        let active_id = self.workspaces.active_workspace_id.clone();
        let active_name = self
            .workspaces
            .active()
            .map(|workspace| workspace.name.clone())
            .unwrap_or_else(|| "Workspace".to_owned());
        let recovered_layout = self
            .storage
            .as_ref()
            .ok_or_else(|| "Workspace storage is unavailable.".to_owned())?
            .load_workspace_backup(&active_id)
            .map_err(|error| format!("Could not load the previous workspace save: {error}"))?;

        // Preserve the current state first. The old backup is already in memory,
        // so rotating the current primary cannot destroy the recovery candidate.
        self.save_before_workspace_change()?;
        let previous_index = self.workspaces.clone();
        let recovered_name = self.unique_recovered_workspace_name(&active_name);
        let recovered_id = self.workspaces.add(recovered_name);
        self.workspaces.active_workspace_id = recovered_id;
        let save_result = self
            .storage
            .as_ref()
            .expect("workspace storage checked above")
            .save_active_workspace(&self.workspaces, &recovered_layout);
        if let Err(error) = save_result {
            self.workspaces = previous_index;
            return Err(format!("Could not create the recovered workspace: {error}"));
        }
        self.apply_layout(&recovered_layout);
        self.remember_saved_workspace(&recovered_layout);
        Ok(())
    }

    fn unique_recovered_workspace_name(&self, active_name: &str) -> String {
        let shortened = active_name.chars().take(38).collect::<String>();
        let base = format!("{shortened} Recovered");
        if self
            .workspaces
            .workspaces
            .iter()
            .all(|workspace| !workspace.name.eq_ignore_ascii_case(&base))
        {
            return base;
        }
        for suffix in 2.. {
            let candidate = format!("{shortened} Recovered {suffix}");
            if self
                .workspaces
                .workspaces
                .iter()
                .all(|workspace| !workspace.name.eq_ignore_ascii_case(&candidate))
            {
                return candidate;
            }
        }
        unreachable!("an unused recovered workspace name always exists")
    }

    fn valid_workspace_name(&self, name: &str, except_id: Option<&str>) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Enter a workspace name.".to_owned());
        }
        if name.chars().count() > 60 {
            return Err("Workspace names can contain at most 60 characters.".to_owned());
        }
        if self.workspaces.workspaces.iter().any(|workspace| {
            Some(workspace.id.as_str()) != except_id && workspace.name.eq_ignore_ascii_case(name)
        }) {
            return Err("A workspace with that name already exists.".to_owned());
        }
        Ok(name.to_owned())
    }

    /// Mirror the window's geometry into [`Self::window_layout`]. Minimized
    /// and maximized windows report the geometry they currently occupy, not
    /// the one to reopen at, so only the restored size and position are kept.
    fn remember_window_geometry(&mut self, ctx: &egui::Context) {
        let (position, size, maximized, minimized) = ctx.input(|i| {
            let viewport = i.viewport();
            (
                viewport.outer_rect.map(|rect| (rect.min.x, rect.min.y)),
                viewport
                    .inner_rect
                    .map(|rect| (rect.width(), rect.height())),
                viewport.maximized.unwrap_or(false),
                viewport.minimized.unwrap_or(false),
            )
        });
        if minimized {
            return;
        }

        self.window_layout.maximized = maximized;
        if maximized {
            return;
        }
        if let Some(position) = position {
            self.window_layout.position = Some(position);
        }
        if let Some(size) = size {
            self.window_layout.size = size;
        }
    }

    /// Create a SavedLayout from current state
    fn create_layout(&self) -> SavedLayout {
        let mut layout = SavedLayout::new();

        // Save canvas state
        layout.canvas = CanvasLayout {
            pan: (self.canvas.pan.x, self.canvas.pan.y),
            zoom: self.canvas.zoom,
            show_grid: self.canvas.show_grid,
            wallpaper: self
                .canvas
                .wallpaper
                .as_ref()
                .map(CanvasWallpaper::to_layout),
            views: self.canvas_views.clone(),
            active_view: self.active_canvas_view,
        };

        // Save all previews
        layout.previews = self
            .preview_manager
            .all()
            .map(|p| {
                let mut saved = PreviewLayout::from(p);
                #[cfg(windows)]
                {
                    // Ask the live WebView where it actually is, so the tile
                    // reopens on the last page browsed rather than the one it
                    // was created with. Falls back to the mirrored URL for
                    // tiles whose host is gone.
                    saved.browser_url = self
                        .browser
                        .get(p.id)
                        .and_then(|host| host.live_url())
                        .or(saved.browser_url)
                        .as_deref()
                        .and_then(scrub_url_for_storage);
                }
                saved
            })
            .collect();

        layout.recent_browser_urls = self.recent_urls.clone();
        layout.picker_open = self.picker_open;
        layout.window = Some(self.window_layout);
        #[cfg(windows)]
        {
            layout.monitor_device = self.monitor_device.clone();
            layout.adblock_enabled = self.browser.adblock_enabled();
        }

        layout
    }

    /// Apply a SavedLayout to restore state
    fn apply_layout(&mut self, layout: &SavedLayout) {
        self.canvas_view_dialog = None;
        self.active_canvas_view = None;
        self.deleted_canvas_view = None;
        self.clear_wallpaper();
        // Clear existing state
        self.capture_coordinator.stop_all();
        #[cfg(windows)]
        {
            self.video_manager.clear();
            self.pending_video_tiles.clear();
            self.last_restored_video_start = None;
            self.restored_paused_videos.clear();
            self.pending_video_freezes.clear();
            self.frozen_video_checkpoints.clear();
            self.video_resume_checkpoints.clear();
            self.seek_preview_manager.clear();
            self.playlist_thumbnail_queue.clear();
            self.playlist_thumbnail_jobs.clear();
            self.next_playlist_group = layout
                .previews
                .iter()
                .filter_map(|preview| preview.playlist_group)
                .max()
                .unwrap_or(0)
                .saturating_add(1);
        }
        self.preview_manager.clear();
        self.canvas.clear_preview_interactions();
        #[cfg(windows)]
        {
            self.browser.clear();
            self.pending_browser_tiles.clear();
            self.last_restored_browser_start = None;
            self.audio_monitor = None;
            self.audio_monitor_checked = None;
            self.window_audio_monitors.clear();
            self.last_inactive_window_scan = None;
        }

        // Restore canvas state
        self.canvas.pan = Vec2::new(layout.canvas.pan.0, layout.canvas.pan.1);
        self.canvas.zoom = layout.canvas.zoom;
        self.canvas.show_grid = layout.canvas.show_grid;
        self.canvas.cancel_camera_transition();
        self.canvas_views = layout.canvas.views.clone();
        self.active_canvas_view = layout
            .canvas
            .active_view
            .filter(|index| *index < self.canvas_views.len());
        if let Some(wallpaper) = layout.canvas.wallpaper.clone() {
            if let Err(error) = self.restore_wallpaper(wallpaper) {
                log::error!("Failed to restore wallpaper: {error}");
                self.media_error = Some(error);
            }
        }
        self.picker_open = layout.picker_open;
        // The window is not moved from here — main.rs opens it at the saved
        // geometry, and reloading a layout mid-session should not move the
        // window out from under the user. Mirroring the value only keeps
        // `pending_maximize` (read once, in `new`) in sync.
        self.window_layout = layout.window.unwrap_or(self.window_layout);

        #[cfg(windows)]
        {
            self.recent_urls = layout
                .recent_browser_urls
                .iter()
                .filter_map(|url| scrub_url_for_storage(url))
                .collect();
        }
        #[cfg(not(windows))]
        {
            self.recent_urls = layout.recent_browser_urls.clone();
        }
        #[cfg(windows)]
        {
            self.monitor_device = layout.monitor_device.clone();
            if let Err(error) = self.browser.set_adblock_enabled(layout.adblock_enabled) {
                log::error!("Could not restore ad blocker state: {error}");
            }
        }

        // Enumerate current windows to find matching ones
        let current_windows = enumerate_windows();
        let mut claimed_hwnds = HashSet::new();

        // Restore previews
        for preview_layout in &layout.previews {
            // Browser tiles restore by recreating their WebView at the saved
            // URL; a failed host creation skips just this tile.
            #[cfg(windows)]
            if let Some(url) = &preview_layout.browser_url {
                match self.create_browser_tile(
                    url,
                    Pos2::new(preview_layout.position.0, preview_layout.position.1),
                    Vec2::new(preview_layout.size.0, preview_layout.size.1),
                    preview_layout.fps_preset,
                ) {
                    Ok(id) => {
                        if let Some(pending) = self.pending_browser_tiles.get_mut(&id) {
                            pending.restore_deferred = true;
                        }
                        self.preview_manager.set_z_order(id, preview_layout.z_order);
                        if let Some(preview) = self.preview_manager.get_mut(id) {
                            restore_browser_geometry(preview, preview_layout);
                        }
                        self.apply_browser_mute(id, preview_layout.browser_muted);
                    }
                    Err(error) => {
                        log::error!("Failed to restore browser tile: {error}");
                    }
                }
                continue;
            }

            if let Some(media_path) = &preview_layout.media_path {
                match self.restore_media_tile(
                    media_path,
                    preview_layout.window_title.clone(),
                    Pos2::new(preview_layout.position.0, preview_layout.position.1),
                    Vec2::new(preview_layout.size.0, preview_layout.size.1),
                ) {
                    Ok(id) => {
                        self.preview_manager.set_z_order(id, preview_layout.z_order);
                        if let Some(preview) = self.preview_manager.get_mut(id) {
                            restore_layout_tile_state(preview, preview_layout);
                        }
                    }
                    Err(error) => {
                        log::error!("Failed to restore image tile: {error}");
                        self.media_error = Some(error);
                    }
                }
                continue;
            }

            #[cfg(windows)]
            if let Some(folder_playlist) = &preview_layout.folder_playlist {
                let group = preview_layout.playlist_group.unwrap_or_else(|| {
                    let group = self.next_playlist_group;
                    self.next_playlist_group = self.next_playlist_group.saturating_add(1);
                    group
                });
                let id = self.restore_folder_playlist(
                    folder_playlist,
                    preview_layout.window_title.clone(),
                    Pos2::new(preview_layout.position.0, preview_layout.position.1),
                    Vec2::new(preview_layout.size.0, preview_layout.size.1),
                    group,
                    None,
                );
                self.preview_manager.set_z_order(id, preview_layout.z_order);
                if let Some(preview) = self.preview_manager.get_mut(id) {
                    restore_layout_tile_state(preview, preview_layout);
                }
                continue;
            }

            #[cfg(windows)]
            if let Some(source) = &preview_layout.video_source {
                let id = self.create_video_tile(
                    source.clone(),
                    preview_layout.window_title.clone(),
                    Pos2::new(preview_layout.position.0, preview_layout.position.1),
                    Vec2::new(preview_layout.size.0, preview_layout.size.1),
                    preview_layout.fps_preset,
                    true,
                );
                self.preview_manager.set_z_order(id, preview_layout.z_order);
                if let Some(preview) = self.preview_manager.get_mut(id) {
                    restore_layout_tile_state(preview, preview_layout);
                }
                continue;
            }

            if let Some(sender) = &preview_layout.spout_sender {
                let id = self.preview_manager.add_for_spout(
                    sender.clone(),
                    Pos2::new(preview_layout.position.0, preview_layout.position.1),
                    Vec2::new(preview_layout.size.0, preview_layout.size.1),
                    preview_layout.fps_preset,
                );
                self.preview_manager.set_z_order(id, preview_layout.z_order);
                if let Some(preview) = self.preview_manager.get_mut(id) {
                    restore_layout_tile_state(preview, preview_layout);
                }
                if !preview_layout.manually_frozen {
                    self.capture_coordinator.start_spout_capture(
                        id,
                        sender.clone(),
                        preview_layout.fps_preset.as_u32(),
                    );
                }
                continue;
            }

            // Prefer the exact saved title, but use the executable identity
            // when dynamic titles (Spotify tracks, documents, etc.) changed.
            let matching_window = find_saved_window(
                &preview_layout.window_title,
                preview_layout.window_exe.as_deref(),
                &current_windows,
                &claimed_hwnds,
            );

            if let Some(window_info) = matching_window {
                claimed_hwnds.insert(window_info.hwnd);
                // Create preview with saved position/size
                let id = self.preview_manager.add_with_window(
                    window_info.title.clone(),
                    Pos2::new(preview_layout.position.0, preview_layout.position.1),
                    Vec2::new(preview_layout.size.0, preview_layout.size.1),
                    WindowHandle {
                        hwnd: window_info.hwnd,
                        process_id: window_info.process_id,
                    },
                    preview_layout.fps_preset,
                    preview_layout.z_order,
                );

                // Restore crop region if it was saved
                if let Some(preview) = self.preview_manager.get_mut(id) {
                    preview.window_exe = Some(window_info.exe_name.clone());
                    preview.window_waiting_for_match = false;
                    restore_layout_tile_state(preview, preview_layout);
                }

                if !preview_layout.manually_frozen {
                    self.capture_coordinator.start_capture(
                        id,
                        window_info.hwnd,
                        window_info.title.clone(),
                        preview_layout.fps_preset.as_u32(),
                    );
                }

                #[cfg(debug_assertions)]
                println!(
                    "Restored preview: {}",
                    privacy::redact_title(&window_info.title)
                );
            } else {
                let id = self.preview_manager.add_inactive_window(
                    preview_layout.window_title.clone(),
                    preview_layout.window_exe.clone(),
                    Pos2::new(preview_layout.position.0, preview_layout.position.1),
                    Vec2::new(preview_layout.size.0, preview_layout.size.1),
                    preview_layout.fps_preset,
                    preview_layout.z_order,
                );
                if let Some(preview) = self.preview_manager.get_mut(id) {
                    restore_layout_tile_state(preview, preview_layout);
                }
                #[cfg(debug_assertions)]
                println!(
                    "Window inactive; keeping saved tile: {}",
                    privacy::redact_title(&preview_layout.window_title)
                );
            }
        }

        #[cfg(windows)]
        {
            let group_to_video = self
                .preview_manager
                .all()
                .filter_map(|preview| {
                    preview
                        .playlist_group
                        .filter(|_| preview.is_video())
                        .map(|group| (group, preview.id))
                })
                .collect::<HashMap<_, _>>();
            for preview in self.preview_manager.all_mut() {
                if preview.is_playlist() {
                    preview.playlist_linked_video = preview
                        .playlist_group
                        .and_then(|group| group_to_video.get(&group).copied());
                }
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::{
        active_canvas_view_after_deletion, active_canvas_view_after_insertion,
        adjacent_canvas_view_index, browser_capture_display_size, browser_capture_fps,
        browser_page_screen_rect, browser_tile_screen_rect, camera_matches_canvas_view,
        find_saved_window, mark_pending_browser_placeholders_shown, preview_video_status,
        restore_browser_geometry, restored_browser_ready, restored_video_ready,
        resumable_video_position, video_launch_for_source, video_session_is_stale,
        wallpaper_hibernation_due, CanvasView, FpsPreset, PendingBrowserTile, PendingVideoTile,
        VideoSource, VideoTileStatus, WALLPAPER_HIBERNATE_AFTER, WALLPAPER_VIDEO_ID,
    };
    use crate::canvas::CanvasState;
    use crate::external_tools::{DiscoverySource, ToolStatus};
    use crate::preview::{
        Preview, PreviewId, PreviewLayout, VideoPlaybackState, VideoTrack, ViewportPin,
    };
    use crate::window_picker::WindowInfo;
    use eframe::egui::{Pos2, Rect, Vec2};
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    #[test]
    fn wallpaper_hibernation_keeps_a_quick_minimize_warm() {
        let now = Instant::now();
        assert!(!wallpaper_hibernation_due(now, now));
        assert!(!wallpaper_hibernation_due(
            now - WALLPAPER_HIBERNATE_AFTER + Duration::from_millis(1),
            now
        ));
        assert!(wallpaper_hibernation_due(
            now - WALLPAPER_HIBERNATE_AFTER,
            now
        ));
    }

    #[test]
    fn adjacent_canvas_views_wrap_and_start_at_the_nearest_end() {
        assert_eq!(adjacent_canvas_view_index(None, 3, true), Some(0));
        assert_eq!(adjacent_canvas_view_index(None, 3, false), Some(2));
        assert_eq!(adjacent_canvas_view_index(Some(2), 3, true), Some(0));
        assert_eq!(adjacent_canvas_view_index(Some(0), 3, false), Some(2));
        assert_eq!(adjacent_canvas_view_index(Some(8), 3, true), Some(0));
        assert_eq!(adjacent_canvas_view_index(None, 0, true), None);
    }

    #[test]
    fn active_canvas_view_index_survives_delete_and_undo_reordering() {
        assert_eq!(active_canvas_view_after_deletion(Some(3), 1), Some(2));
        assert_eq!(active_canvas_view_after_deletion(Some(1), 1), None);
        assert_eq!(active_canvas_view_after_deletion(Some(0), 1), Some(0));
        assert_eq!(active_canvas_view_after_insertion(Some(2), 1), Some(3));
        assert_eq!(active_canvas_view_after_insertion(Some(0), 1), Some(0));
    }

    #[test]
    fn active_canvas_view_uses_screen_and_proportional_zoom_tolerances() {
        let view = CanvasView {
            name: "Overview".to_owned(),
            center: (100.0, 200.0),
            zoom: 1.0,
        };
        assert!(camera_matches_canvas_view(
            Pos2::new(100.5, 200.0),
            1.0,
            &view
        ));
        assert!(!camera_matches_canvas_view(
            Pos2::new(102.0, 200.0),
            1.0,
            &view
        ));
        assert!(!camera_matches_canvas_view(
            Pos2::new(100.0, 200.0),
            1.01,
            &view
        ));
    }

    #[test]
    fn transparent_browser_uses_probe_fps_until_visible_content_arrives() {
        let mut preview = Preview::new(
            PreviewId(1),
            "browser".to_owned(),
            Pos2::ZERO,
            Vec2::splat(100.0),
        );
        preview.browser_url = Some("https://example.com".to_owned());
        preview.target_fps = 30;
        preview.browser_waiting_for_content = true;

        assert_eq!(browser_capture_fps(&preview), 5);
        preview.browser_waiting_for_content = false;
        assert_eq!(browser_capture_fps(&preview), 30);
    }

    #[test]
    fn undo_restores_every_persisted_common_tile_setting() {
        let mut previews = crate::preview::PreviewManager::new();
        let id = previews.add_browser_placeholder(
            "https://example.com".to_owned(),
            Pos2::new(20.0, 30.0),
            Vec2::new(200.0, 150.0),
            FpsPreset::High,
        );
        let preview = previews.get_mut(id).unwrap();
        preview.crop_uv = Some((0.1, 0.2, 0.9, 0.8));
        preview.manually_frozen = true;
        preview.lock_aspect_ratio = false;
        preview.stream_audio = true;
        preview.left_click_disabled = true;
        preview.playlist_group = Some(3);
        preview.viewport_pin = Some(crate::preview::ViewportPin::from_rect(
            preview.rect(),
            Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0)),
        ));
        let before = PreviewLayout::from(&*preview);
        previews.start_removal(id);
        previews.get_mut(id).unwrap().removing = Some(Instant::now() - Duration::from_secs(1));
        let info = previews.finalize_removals().pop().unwrap().pop().unwrap();
        let restored = previews.add_browser_placeholder(
            info.browser_url.clone().unwrap(),
            info.position,
            info.size,
            FpsPreset::Low,
        );
        let preview = previews.get_mut(restored).unwrap();
        super::restore_removed_tile_state(preview, &info);
        assert!(preview.capture_paused);
        assert_eq!(
            serde_json::to_value(PreviewLayout::from(&*preview)).unwrap(),
            serde_json::to_value(before).unwrap()
        );
    }

    #[test]
    fn frozen_pending_browser_requests_only_its_initial_placeholder_paint() {
        let id = PreviewId(1);
        let mut pending = HashMap::from([(id, restored_pending(false))]);

        assert!(mark_pending_browser_placeholders_shown(&mut pending));
        assert!(pending.get(&id).unwrap().shown_once);
        assert!(!mark_pending_browser_placeholders_shown(&mut pending));
        assert!(pending.contains_key(&id));
    }

    #[test]
    fn saved_window_matching_survives_dynamic_titles() {
        let windows = vec![WindowInfo::new(
            100,
            "A different song".to_owned(),
            42,
            "Spotify.exe".to_owned(),
        )];

        let matched = find_saved_window(
            "The song from last session",
            Some("spotify.EXE"),
            &windows,
            &HashSet::new(),
        );
        assert_eq!(matched.map(|window| window.hwnd), Some(100));
    }

    #[test]
    fn saved_window_matching_never_claims_one_window_twice() {
        let windows = vec![WindowInfo::new(
            100,
            "Spotify".to_owned(),
            42,
            "Spotify.exe".to_owned(),
        )];
        let claimed = HashSet::from([100]);

        assert!(find_saved_window("Spotify", Some("Spotify.exe"), &windows, &claimed).is_none());
    }

    #[test]
    fn pinned_browser_uses_viewport_geometry_for_capture_and_interaction() {
        let canvas = CanvasState::default();
        let canvas_rect = Rect::from_min_size(Pos2::new(20.0, 30.0), Vec2::new(1000.0, 700.0));
        let pinned_rect = Rect::from_min_size(Pos2::new(80.0, 90.0), Vec2::new(640.0, 360.0));
        let mut preview = Preview::new(
            PreviewId(1),
            "browser".to_owned(),
            Pos2::new(200.0, 150.0),
            Vec2::new(320.0, 180.0),
        );
        preview.browser_url = Some("https://example.com".to_owned());
        preview.viewport_pin = Some(ViewportPin::from_rect(pinned_rect, canvas_rect));

        assert_eq!(
            browser_tile_screen_rect(&canvas, &preview, canvas_rect),
            pinned_rect
        );
        assert_eq!(browser_capture_display_size(&preview), pinned_rect.size());
    }

    #[test]
    fn cropped_browser_keeps_the_full_page_geometry_for_capture_and_interaction() {
        let canvas = CanvasState::default();
        let canvas_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0));
        let mut preview = Preview::new(
            PreviewId(1),
            "browser".to_owned(),
            Pos2::new(200.0, 150.0),
            Vec2::new(320.0, 180.0),
        );
        preview.browser_url = Some("https://example.com".to_owned());
        preview.crop_uv = Some((0.25, 0.25, 0.75, 0.75));

        assert_eq!(
            browser_capture_display_size(&preview),
            Vec2::new(640.0, 360.0)
        );
        assert_eq!(
            browser_page_screen_rect(&canvas, &preview, canvas_rect),
            Rect::from_min_size(Pos2::new(40.0, 60.0), Vec2::new(640.0, 360.0))
        );
    }

    #[test]
    fn restored_browser_keeps_its_saved_crop() {
        let mut saved = Preview::new(
            PreviewId(1),
            "browser".to_owned(),
            Pos2::new(200.0, 150.0),
            Vec2::new(240.0, 180.0),
        );
        saved.browser_url = Some("https://example.com".to_owned());
        saved.lock_aspect_ratio = false;
        saved.crop_uv = Some((0.25, 0.0, 1.0, 1.0));
        saved.manually_frozen = true;
        saved.left_click_disabled = true;
        saved.viewport_pin = Some(ViewportPin::from_rect(
            Rect::from_min_size(Pos2::new(20.0, 30.0), Vec2::new(240.0, 180.0)),
            Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0)),
        ));
        let layout = PreviewLayout::from(&saved);
        let mut restored = Preview::new(
            PreviewId(2),
            "browser".to_owned(),
            Pos2::new(layout.position.0, layout.position.1),
            Vec2::new(layout.size.0, layout.size.1),
        );

        restore_browser_geometry(&mut restored, &layout);

        assert_eq!(restored.crop_uv, saved.crop_uv);
        assert_eq!(restored.viewport_pin, saved.viewport_pin);
        assert_eq!(restored.lock_aspect_ratio, saved.lock_aspect_ratio);
        assert!(restored.manually_frozen);
        assert!(restored.left_click_disabled);
        assert!(restored.capture_paused);
    }

    fn restored_pending(shown_once: bool) -> PendingBrowserTile {
        PendingBrowserTile {
            url: "https://example.com".to_owned(),
            fps: FpsPreset::Medium,
            muted: false,
            shown_once,
            restore_deferred: true,
        }
    }

    #[test]
    fn restored_browsers_wait_for_a_painted_visible_tile() {
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0));
        let visible = Rect::from_min_size(Pos2::new(50.0, 50.0), Vec2::splat(100.0));
        let offscreen = Rect::from_min_size(Pos2::new(800.0, 800.0), Vec2::splat(100.0));

        assert!(!restored_browser_ready(
            &restored_pending(false),
            Some(visible),
            Some(viewport),
            false,
        ));
        assert!(restored_browser_ready(
            &restored_pending(true),
            Some(visible),
            Some(viewport),
            false,
        ));
        assert!(!restored_browser_ready(
            &restored_pending(true),
            Some(offscreen),
            Some(viewport),
            false,
        ));
        assert!(!restored_browser_ready(
            &restored_pending(true),
            Some(visible),
            None,
            false,
        ));
        assert!(restored_browser_ready(
            &restored_pending(true),
            Some(offscreen),
            Some(viewport),
            true,
        ));
    }

    #[test]
    fn restored_videos_wait_for_a_painted_visible_tile() {
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0));
        let visible = Rect::from_min_size(Pos2::new(50.0, 50.0), Vec2::splat(100.0));
        let offscreen = Rect::from_min_size(Pos2::new(800.0, 800.0), Vec2::splat(100.0));

        assert!(!restored_video_ready(
            &PendingVideoTile {
                start_paused: true,
                shown_once: false,
                retry_ready: true,
            },
            Some(visible),
            Some(viewport),
            false,
        ));
        assert!(restored_video_ready(
            &PendingVideoTile {
                start_paused: true,
                shown_once: true,
                retry_ready: true,
            },
            Some(visible),
            Some(viewport),
            false,
        ));
        assert!(!restored_video_ready(
            &PendingVideoTile {
                start_paused: true,
                shown_once: true,
                retry_ready: true,
            },
            Some(offscreen),
            Some(viewport),
            false,
        ));
        assert!(!restored_video_ready(
            &PendingVideoTile {
                start_paused: false,
                shown_once: true,
                retry_ready: true,
            },
            Some(visible),
            Some(viewport),
            false,
        ));
        assert!(restored_video_ready(
            &PendingVideoTile {
                start_paused: true,
                shown_once: true,
                retry_ready: true,
            },
            Some(offscreen),
            Some(viewport),
            true,
        ));
    }

    #[test]
    fn active_wallpaper_video_session_is_not_pruned_as_an_orphan_tile() {
        assert!(!video_session_is_stale(WALLPAPER_VIDEO_ID, true, false));
        assert!(video_session_is_stale(WALLPAPER_VIDEO_ID, false, false));
        assert!(!video_session_is_stale(
            crate::preview::PreviewId(7),
            false,
            true
        ));
        assert!(video_session_is_stale(
            crate::preview::PreviewId(7),
            true,
            false
        ));
    }

    #[test]
    fn video_launches_local_files_without_mpv_and_streams_need_streamlink() {
        let media_path = std::env::temp_dir().join(format!(
            "pluriview-video-source-test-{}.mp4",
            std::process::id()
        ));
        std::fs::write(&media_path, b"test").unwrap();
        let source = VideoSource::LocalFile {
            path: media_path.clone(),
        };
        let checking = ToolStatus::Checking;
        let missing = ToolStatus::Missing;
        let launch = video_launch_for_source(&source, &missing, true)
            .unwrap()
            .unwrap();
        assert!(launch.start_paused);
        assert!(matches!(
            launch.source,
            VideoSource::LocalFile { path } if path == media_path
        ));
        assert!(launch.streamlink_path.is_none());

        let invalid = ToolStatus::Invalid {
            path: PathBuf::from(r"C:\broken\streamlink.exe"),
            error: "version check failed".to_owned(),
            source: DiscoverySource::Override,
        };
        let stream = VideoSource::Stream {
            url: "https://example.test/live".to_owned(),
            quality: "best".to_owned(),
        };
        assert!(video_launch_for_source(&stream, &checking, false)
            .unwrap()
            .is_none());
        assert!(video_launch_for_source(&stream, &missing, false).is_err());
        assert!(video_launch_for_source(&stream, &invalid, false).is_err());

        let available = ToolStatus::Available {
            path: PathBuf::from(r"C:\Tools\streamlink.exe"),
            version: "streamlink test".to_owned(),
            source: DiscoverySource::Path,
        };
        let launch = video_launch_for_source(&stream, &available, false)
            .unwrap()
            .unwrap();
        assert!(!launch.start_paused);
        assert!(matches!(
            launch.source,
            VideoSource::Stream { ref url, .. } if url == "https://example.test/live"
        ));
        assert_eq!(
            launch.streamlink_path,
            Some(PathBuf::from(r"C:\Tools\streamlink.exe"))
        );
        std::fs::remove_file(media_path).unwrap();
    }

    #[test]
    fn video_runtime_state_drives_preview_status() {
        let mut state = VideoPlaybackState {
            connected: true,
            paused: true,
            time_pos: Some(12.0),
            duration: Some(60.0),
            volume: 45.0,
            muted: true,
            speed: 1.25,
            looping: true,
            audio_track: Some(2),
            subtitle_track: None,
            media_title: Some("Mapped title".to_owned()),
            seekable: true,
            ..Default::default()
        };
        state.tracks.push(VideoTrack {
            id: 2,
            kind: "audio".to_owned(),
            title: Some("Commentary".to_owned()),
            language: Some("en".to_owned()),
            selected: true,
        });

        assert_eq!(state.tracks[0].language.as_deref(), Some("en"));
        assert_eq!(
            preview_video_status(&state, true),
            VideoTileStatus::PausedOnRestore
        );

        state.paused = false;
        state.paused_for_cache = true;
        assert_eq!(
            preview_video_status(&state, false),
            VideoTileStatus::Buffering
        );
        state.paused_for_cache = false;
        assert_eq!(preview_video_status(&state, false), VideoTileStatus::Ready);
    }

    #[test]
    fn frozen_vod_resumes_at_position_but_live_stream_reconnects() {
        let vod = VideoPlaybackState {
            time_pos: Some(42.5),
            duration: Some(120.0),
            ..Default::default()
        };
        assert_eq!(resumable_video_position(&vod), Some(42.5));

        let live = VideoPlaybackState {
            time_pos: Some(1_000.0),
            duration: None,
            ..Default::default()
        };
        assert_eq!(resumable_video_position(&live), None);

        let invalid = VideoPlaybackState {
            time_pos: Some(f64::NAN),
            duration: Some(120.0),
            ..Default::default()
        };
        assert_eq!(resumable_video_position(&invalid), None);
    }
}

impl Drop for PluriviewApp {
    fn drop(&mut self) {
        self.capture_coordinator.stop_all();
        #[cfg(windows)]
        self.seek_preview_manager.clear();
        #[cfg(windows)]
        self.video_manager.clear();
    }
}

impl eframe::App for PluriviewApp {
    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        // Auto-save the active named workspace on exit.
        if let Err(error) = self.save_active_workspace() {
            eprintln!("{error}");
        }
        self.capture_coordinator.stop_all();
        #[cfg(windows)]
        self.seek_preview_manager.clear();
        #[cfg(windows)]
        if let Some(gl) = gl {
            self.video_manager.cleanup_all(gl);
        } else {
            self.video_manager.clear();
        }
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Cache our window handle once; used by the tray and for positioning
        // browser host windows (no more FindWindowW-by-title every frame).
        if self.main_hwnd.is_none() {
            if let Ok(handle) = frame.window_handle() {
                if let RawWindowHandle::Win32(win32) = handle.as_raw() {
                    self.main_hwnd = Some(win32.hwnd.get());
                }
            }
        }

        if !self.workspace_persistence_ready || !self.workspace_save.is_ready() {
            self.workspace_recovery_ui(ctx);
            return;
        }

        self.hotkey_tracker.sample(
            &self.app_config.keyboard_shortcuts,
            self.hotkey_recording.is_some(),
        );
        #[cfg(windows)]
        let owns_foreground = self.owns_foreground();
        #[cfg(not(windows))]
        let owns_foreground = true;
        let shortcut_listening = owns_foreground
            && !ctx.wants_keyboard_input()
            && !self.show_settings
            && !self.show_shortcuts
            && !self.show_about
            && self.canvas_view_dialog.is_none()
            && self.workspace_dialog.is_none()
            && !self.confirm_workspace_delete
            && self.hotkey_recording.is_none();
        let shortcut_presses = self
            .hotkey_tracker
            .presses(&self.app_config.keyboard_shortcuts, shortcut_listening);

        #[cfg(windows)]
        let webview_active = self.browser.active_id().is_some();
        #[cfg(not(windows))]
        let webview_active = false;
        if owns_foreground && (self.hotkey_recording.is_some() || webview_active) {
            // Native WebViews do not wake egui for keyboard events. A short
            // poll interval keeps remapped shortcuts responsive there and in
            // the shortcut recorder.
            ctx.request_repaint_after(Duration::from_millis(30));
        }

        if shortcut_presses.pressed(HotkeySlot::ToggleGrid) {
            self.canvas.show_grid = !self.canvas.show_grid;
        }
        if shortcut_presses.pressed(HotkeySlot::ToggleWindowPicker) {
            self.picker_open = !self.picker_open;
        }
        if shortcut_presses.pressed(HotkeySlot::ToggleCanvasOnly) {
            self.canvas_only = !self.canvas_only;
        }
        if shortcut_presses.pressed(HotkeySlot::ShowShortcutHelp) {
            self.show_shortcuts = true;
        }
        let numbered_canvas_view = HotkeySlot::CANVAS_VIEWS
            .iter()
            .position(|slot| shortcut_presses.pressed(*slot));
        if let Some(index) = numbered_canvas_view {
            self.jump_to_canvas_view(index, ctx);
        } else if shortcut_presses.pressed(HotkeySlot::PreviousCanvasView) {
            self.jump_to_adjacent_canvas_view(false, ctx);
        } else if shortcut_presses.pressed(HotkeySlot::NextCanvasView) {
            self.jump_to_adjacent_canvas_view(true, ctx);
        }
        self.canvas.set_keyboard_input(CanvasKeyboardInput {
            delete_selected: shortcut_presses.pressed(HotkeySlot::DeleteSelected),
            select_all: shortcut_presses.pressed(HotkeySlot::SelectAll),
            exit_tile_focus: shortcut_presses.pressed(HotkeySlot::ExitTileOrBrowser),
        });

        if self.external_tools.poll() {
            #[cfg(windows)]
            for (id, pending) in &mut self.pending_video_tiles {
                pending.retry_ready = true;
                if let Some(preview) = self.preview_manager.get_mut(*id) {
                    if matches!(preview.video_status, VideoTileStatus::Failed(_)) {
                        preview.video_status = if pending.start_paused {
                            VideoTileStatus::PausedOnRestore
                        } else {
                            VideoTileStatus::Starting
                        };
                    }
                }
            }
            ctx.request_repaint();
        }
        if self.external_tools.is_scanning() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        #[cfg(windows)]
        self.pending_video_upkeep(ctx);

        // Set up tray HWND on first frame (window now exists)
        self.setup_tray_hwnd();

        if self.pending_maximize {
            self.pending_maximize = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        }
        self.remember_window_geometry(ctx);

        // Never block first paint on extension extraction/integrity work.
        // Pending browser tiles render their progress while this polls the
        // background worker and activates them when preparation completes.
        #[cfg(windows)]
        self.browser_preparation_upkeep(ctx);
        #[cfg(windows)]
        self.poll_video_freezes(ctx);
        #[cfg(windows)]
        self.wallpaper_memory_upkeep(ctx);
        #[cfg(windows)]
        self.poll_video_manager(ctx);
        #[cfg(windows)]
        if let Err(error) = self.ensure_wallpaper_video() {
            log::error!("Could not start wallpaper video: {error}");
            if let Some(wallpaper) = self.canvas.wallpaper.as_mut() {
                wallpaper.error = Some(error.clone());
            }
            self.media_error = Some(error);
        }
        #[cfg(windows)]
        self.poll_seek_previews(ctx);

        // Custom title bar + manual resize border (decorations are off)
        self.handle_frameless_resize(ctx);
        if !self.canvas_only {
            self.title_bar_ui(ctx);
        }

        #[cfg(windows)]
        self.inactive_window_upkeep(ctx);

        // Process any pending captured frames
        self.capture_coordinator
            .process_frames(&mut self.preview_manager);
        #[cfg(windows)]
        self.browser_capture_frame_rate_upkeep();

        // Handle pending region selection request (from context menu in canvas)
        if let Some(preview_id) = self.canvas.pending_region_select.take() {
            if let Some(preview) = self.preview_manager.get(preview_id) {
                if let Some(ref handle) = preview.window_handle {
                    // Start the region selector overlay
                    if let Some(selector) = RegionSelector::show_for_window(handle.hwnd) {
                        self.region_selector = Some(selector);
                        self.region_select_preview_id = Some(preview_id);
                    }
                }
            }
        }

        // Poll for region selection result
        if let Some(ref mut selector) = self.region_selector {
            if let Some(result) = selector.poll_result() {
                if let Some(selection) = result {
                    // Apply the crop to the preview
                    if let Some(preview_id) = self.region_select_preview_id {
                        let crop_uv = self.preview_manager.get(preview_id).and_then(|preview| {
                            preview
                                .source_frame_size
                                .or(preview.frame_size)
                                .map(|(width, height)| selection.to_uv(width, height))
                        });
                        if let (Some(crop_uv), Some(canvas_rect)) =
                            (crop_uv, self.canvas.last_screen_rect)
                        {
                            self.canvas.set_preview_crop(
                                preview_id,
                                Some(crop_uv),
                                canvas_rect,
                                &mut self.preview_manager,
                            );
                        }
                    }
                }
                // Clear the selector (whether successful or cancelled)
                self.region_selector = None;
                self.region_select_preview_id = None;
            }
        }

        // Menu bar (File / View / Help) now lives inline in the custom
        // title bar; see `title_bar_ui` / `menu_bar`.

        // Minimal Void: Dark sidebar
        if self.picker_open && !self.canvas_only {
            egui::SidePanel::left("window_picker_panel")
                .default_width(250.0)
                .min_width(200.0)
                .max_width(400.0)
                .frame(
                    egui::Frame::none()
                        .fill(egui::Color32::from_rgb(18, 18, 18))
                        .inner_margin(egui::Margin::same(8.0)),
                )
                .show(ctx, |ui| {
                    self.window_picker.ui(
                        ui,
                        &mut self.preview_manager,
                        &mut self.capture_coordinator,
                        &self.canvas,
                    );
                });
        }

        // Minimal Void: No status bar - floating indicator is drawn in the canvas

        // Minimal Void: Main canvas area with dark background
        #[cfg(windows)]
        {
            self.canvas.stream_monitor_ready = self.monitor_device.is_some();
        }
        let active_canvas_view = self.active_canvas_view;
        let active_canvas_view_modified =
            active_canvas_view.is_some_and(|index| self.canvas_view_is_modified(index));
        let canvas_views = &self.canvas_views;
        let keyboard_shortcuts = &self.app_config.keyboard_shortcuts;
        let show_canvas_overlays = !self.canvas_only;
        let canvas = &mut self.canvas;
        let preview_manager = &mut self.preview_manager;
        let capture_coordinator = &mut self.capture_coordinator;
        let mut canvas_view_action = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(egui::Color32::from_rgb(13, 13, 13)))
            .show(ctx, |ui| {
                canvas.ui_with_canvas_views_menu(
                    ui,
                    preview_manager,
                    capture_coordinator,
                    ctx,
                    show_canvas_overlays,
                    |ui| {
                        canvas_view_action = canvas_views_menu_contents(
                            ui,
                            canvas_views,
                            active_canvas_view,
                            active_canvas_view_modified,
                            keyboard_shortcuts,
                        );
                        canvas_view_action.is_some()
                    },
                );
            });
        if let Some(action) = canvas_view_action {
            self.handle_canvas_view_menu_action(action, ctx);
        }

        #[cfg(windows)]
        self.sync_wallpaper_under_tile_focus();

        self.file_drop_overlay(ctx);
        self.import_dropped_files(ctx);

        #[cfg(windows)]
        if mark_pending_browser_placeholders_shown(&mut self.pending_browser_tiles) {
            // Guarantee at least one painted placeholder frame before any
            // WebView creation can occupy the UI thread. Restored frozen
            // browsers stay pending until resumed, so subsequent frames must
            // not keep repainting just because the queue is non-empty.
            ctx.request_repaint();
        }
        #[cfg(windows)]
        if !self.pending_video_tiles.is_empty() {
            let awaiting_first_paint = self
                .pending_video_tiles
                .values()
                .any(|pending| !pending.shown_once);
            for pending in self.pending_video_tiles.values_mut() {
                pending.shown_once = true;
            }
            if awaiting_first_paint {
                ctx.request_repaint();
            }
        }

        #[cfg(windows)]
        {
            let browser_double_clicked = self
                .canvas
                .last_double_clicked
                .filter(|id| self.browser.contains(*id));

            let browser_shortcut = (shortcut_presses.pressed(HotkeySlot::InteractBrowser)
                || shortcut_presses.pressed(HotkeySlot::InteractBrowserAlternate))
            .then(|| {
                // The live tile first, so the configured shortcut toggles
                // interaction off even if selection drifted while the page
                // had focus.
                self.browser.active_id().or_else(|| {
                    self.canvas.selection.iter().copied().find(|id| {
                        self.browser.contains(*id)
                            && self
                                .preview_manager
                                .get(*id)
                                .is_some_and(|preview| !preview.left_click_disabled)
                    })
                })
            })
            .flatten();

            if let Some(id) = browser_double_clicked.or(browser_shortcut) {
                let active = self.browser.get(id).is_some_and(|host| host.is_active());
                if active {
                    if let Some(host) = self.browser.get_mut(id) {
                        host.park();
                    }
                    self.capture_coordinator.resume_capture(id);
                    self.browser_activated_at = None;
                    // The parked WebView still holds keyboard focus; hand it
                    // back so the next shortcut reaches us.
                    if let Some(hwnd) = self.main_hwnd {
                        unsafe {
                            let _ = SetForegroundWindow(HWND(hwnd as *mut _));
                        }
                    }
                } else if let (Some(hwnd), Some(canvas_rect)) =
                    (self.main_hwnd, self.canvas.last_screen_rect)
                {
                    if let Some(placement) = self.browser_tile_placement(id, canvas_rect) {
                        if let Some(active_id) = self.browser.active_id() {
                            self.capture_coordinator.resume_capture(active_id);
                        }
                        self.browser.park_all();
                        // Hold the last correct captured frame onscreen while
                        // WebView2 prepares its interactive viewport offscreen.
                        self.capture_coordinator.pause_capture(id);
                        // Select for the accent outline without changing the
                        // saved canvas stacking order.
                        self.canvas.selection = vec![id];
                        if let Some(host) = self.browser.get_mut(id) {
                            host.place(
                                HWND(hwnd as *mut _),
                                placement.page_rect,
                                placement.visible_rect,
                                ctx.pixels_per_point(),
                                true,
                            );
                        }
                        self.browser_activated_at = Some(Instant::now());
                    }
                }
            }
            self.canvas.last_double_clicked = None;

            // Fit the current tile to the canvas, or restore the pre-focus
            // view if a tile is already focused.
            if shortcut_presses.pressed(HotkeySlot::FocusCurrentTile) && !self.canvas.exit_focus() {
                let target = self
                    .browser
                    .active_id()
                    .or_else(|| self.canvas.selection.first().copied());
                let tile_rect = target.and_then(|id| {
                    self.preview_manager
                        .get(id)
                        .filter(|preview| preview.removing.is_none())
                        .map(|preview| preview.rect())
                });
                if let (Some(id), Some(rect), Some(canvas_rect)) =
                    (target, tile_rect, self.canvas.last_screen_rect)
                {
                    self.canvas.focus_on_tile(id, rect, canvas_rect);
                }
            }

            // Per-frame browser housekeeping: mirror page titles/URLs onto
            // tiles, exit interaction mode on Escape/focus loss/minimize,
            // and keep the live host glued to its tile through pan/zoom
            // and window moves.
            self.browser_frame(ctx, shortcut_presses.pressed(HotkeySlot::ExitTileOrBrowser));
        }

        // Canvas right-click "Add Window..." was selected: open the
        // quick-add popup at that spot with a fresh window snapshot.
        if let Some((canvas_pos, screen_pos)) = self.canvas.pending_quick_add.take() {
            self.quick_add = Some(QuickAddPopup {
                canvas_pos,
                screen_pos,
                windows: enumerate_windows(),
                search: String::new(),
            });
        }

        // Image tiles retain their validated original path. If that source is
        // later moved, the saved tile remains as a recoverable placeholder.
        if let Some(position) = self.canvas.pending_media_add.take() {
            if let Some(path) = media::pick_file(self.main_hwnd) {
                if let Err(error) = self.import_media_tile(&path, position) {
                    log::error!("Failed to import image tile: {error}");
                    self.media_error = Some(error);
                }
            }
        }

        if self.canvas.pending_wallpaper_pick {
            self.canvas.pending_wallpaper_pick = false;
            self.pick_wallpaper();
            ctx.request_repaint();
        }
        if self.canvas.pending_wallpaper_clear {
            self.canvas.pending_wallpaper_clear = false;
            self.clear_wallpaper();
        }

        #[cfg(windows)]
        {
            for (id, action) in std::mem::take(&mut self.canvas.pending_tile_activity_actions) {
                self.handle_tile_activity_action(ctx, id, action);
            }

            if let Some(position) = self.canvas.pending_video_add.take() {
                self.add_local_video(position);
            }

            if let Some(position) = self.canvas.pending_stream_add.take() {
                self.open_add_stream(position);
            }

            if let Some(position) = self.canvas.pending_browser_add.take() {
                self.add_browser = Some(AddBrowserDialog {
                    position,
                    url: String::new(),
                    error: None,
                    target: None,
                    focused: false,
                });
            }

            // Actions queued by browser tile hover controls / context menus.
            for (id, action) in std::mem::take(&mut self.canvas.pending_browser_actions) {
                self.handle_browser_action(ctx, id, action);
            }

            for (id, action) in std::mem::take(&mut self.canvas.pending_video_actions) {
                self.handle_video_action(ctx, id, action);
            }

            for (id, action) in std::mem::take(&mut self.canvas.pending_playlist_actions) {
                self.handle_playlist_action(ctx, id, action);
            }
            self.pump_playlist_thumbnails(ctx);

            if let Some(info) = self.canvas.pending_removed_restore.take() {
                self.restore_removed_previews(info);
            }
        }

        self.workspace_autosave_upkeep(ctx);

        for (id, action) in std::mem::take(&mut self.canvas.pending_media_actions) {
            self.handle_media_action(id, action);
        }

        self.quick_add_ui(ctx);
        #[cfg(windows)]
        self.add_browser_ui(ctx);
        #[cfg(windows)]
        self.add_stream_ui(ctx);
        self.external_tool_error_ui(ctx);
        self.video_action_error_ui(ctx);
        self.settings_ui(ctx);
        self.canvas_view_dialog_ui(ctx);
        self.workspace_dialog_ui(ctx);
        self.workspace_delete_confirmation_ui(ctx);
        self.canvas_view_undo_toast_ui(ctx);

        if self.media_error.is_some() {
            let mut dismiss = false;
            egui::Window::new("Media Tile Error")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    if let Some(error) = &self.media_error {
                        ui.label(error);
                    }
                    ui.add_space(8.0);
                    if ui.button("OK").clicked() {
                        dismiss = true;
                    }
                });
            if dismiss {
                self.media_error = None;
            }
        }

        if let Some(error) = self.workspace_error.clone() {
            let mut dismiss = false;
            egui::Window::new("Workspace Error")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(error);
                    ui.add_space(8.0);
                    if ui.button("OK").clicked() {
                        dismiss = true;
                    }
                });
            if dismiss {
                self.workspace_error = None;
            }
        }

        #[cfg(windows)]
        {
            let previews = &self.preview_manager;
            self.browser.retain(|id| previews.get(id).is_some());
            self.pending_browser_tiles
                .retain(|id, _| previews.get(*id).is_some());
            self.prune_video_tiles();
            self.video_manager.schedule_cleanup(ctx);
        }

        // About dialog
        if self.show_about {
            egui::Window::new("About Pluriview")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .frame(egui::Frame::window(&ctx.style()).fill(egui::Color32::from_rgb(25, 25, 28)))
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(10.0);
                        ui.image((self.brand_icon.id(), Vec2::splat(80.0)));
                        ui.heading("Pluriview");
                        ui.label(concat!("Version ", env!("CARGO_PKG_VERSION")));
                        ui.add_space(10.0);
                        ui.label("Live window preview application");
                        ui.label("with infinite canvas");
                        ui.add_space(15.0);
                        ui.label(egui::RichText::new("Created by Spatpit").weak());
                        ui.add_space(15.0);
                        if ui.button("Close").clicked() {
                            self.show_about = false;
                        }
                        ui.add_space(5.0);
                    });
                });
        }

        // Keyboard Shortcuts dialog
        if self.show_shortcuts {
            egui::Window::new("Keyboard Shortcuts")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .frame(egui::Frame::window(&ctx.style()).fill(egui::Color32::from_rgb(25, 25, 28)))
                .show(ctx, |ui| {
                    ui.add_space(5.0);

                    egui::Grid::new("shortcuts_grid")
                        .num_columns(2)
                        .spacing([40.0, 8.0])
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("Navigation").strong());
                            ui.label("");
                            ui.end_row();

                            ui.label("Pan canvas");
                            ui.label(egui::RichText::new("Middle Mouse / Alt+Drag").weak());
                            ui.end_row();

                            ui.label("Zoom");
                            ui.label(egui::RichText::new("Scroll Wheel").weak());
                            ui.end_row();

                            ui.label("Toggle grid");
                            ui.label(
                                egui::RichText::new(
                                    self.app_config
                                        .keyboard_shortcuts
                                        .get(HotkeySlot::ToggleGrid)
                                        .display(),
                                )
                                .weak(),
                            );
                            ui.end_row();

                            ui.label("Toggle Window Picker");
                            ui.label(
                                egui::RichText::new(
                                    self.app_config
                                        .keyboard_shortcuts
                                        .get(HotkeySlot::ToggleWindowPicker)
                                        .display(),
                                )
                                .weak(),
                            );
                            ui.end_row();

                            ui.label("Canvas-only mode");
                            ui.label(
                                egui::RichText::new(
                                    self.app_config
                                        .keyboard_shortcuts
                                        .get(HotkeySlot::ToggleCanvasOnly)
                                        .display(),
                                )
                                .weak(),
                            );
                            ui.end_row();

                            ui.label("Focus current tile");
                            ui.label(
                                egui::RichText::new(
                                    self.app_config
                                        .keyboard_shortcuts
                                        .get(HotkeySlot::FocusCurrentTile)
                                        .display(),
                                )
                                .weak(),
                            );
                            ui.end_row();

                            ui.label("Exit tile focus");
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} / {}",
                                    self.app_config
                                        .keyboard_shortcuts
                                        .get(HotkeySlot::FocusCurrentTile)
                                        .display(),
                                    self.app_config
                                        .keyboard_shortcuts
                                        .get(HotkeySlot::ExitTileOrBrowser)
                                        .display()
                                ))
                                .weak(),
                            );
                            ui.end_row();

                            ui.label("Previous saved viewpoint");
                            ui.label(
                                egui::RichText::new(
                                    self.app_config
                                        .keyboard_shortcuts
                                        .get(HotkeySlot::PreviousCanvasView)
                                        .display(),
                                )
                                .weak(),
                            );
                            ui.end_row();

                            ui.label("Next saved viewpoint");
                            ui.label(
                                egui::RichText::new(
                                    self.app_config
                                        .keyboard_shortcuts
                                        .get(HotkeySlot::NextCanvasView)
                                        .display(),
                                )
                                .weak(),
                            );
                            ui.end_row();

                            ui.label("Saved viewpoints 1–9");
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} … {}",
                                    self.app_config
                                        .keyboard_shortcuts
                                        .get(HotkeySlot::CanvasView1)
                                        .display(),
                                    self.app_config
                                        .keyboard_shortcuts
                                        .get(HotkeySlot::CanvasView9)
                                        .display()
                                ))
                                .weak(),
                            );
                            ui.end_row();

                            ui.add_space(10.0);
                            ui.end_row();

                            ui.label(egui::RichText::new("Selection").strong());
                            ui.label("");
                            ui.end_row();

                            ui.label("Select all");
                            ui.label(
                                egui::RichText::new(
                                    self.app_config
                                        .keyboard_shortcuts
                                        .get(HotkeySlot::SelectAll)
                                        .display(),
                                )
                                .weak(),
                            );
                            ui.end_row();

                            ui.label("Multi-select");
                            ui.label(egui::RichText::new("Ctrl+Click").weak());
                            ui.end_row();

                            ui.label("Delete selected");
                            ui.label(
                                egui::RichText::new(
                                    self.app_config
                                        .keyboard_shortcuts
                                        .get(HotkeySlot::DeleteSelected)
                                        .display(),
                                )
                                .weak(),
                            );
                            ui.end_row();

                            ui.add_space(10.0);
                            ui.end_row();

                            ui.label(egui::RichText::new("Editing").strong());
                            ui.label("");
                            ui.end_row();

                            ui.label("Resize preview");
                            ui.label(egui::RichText::new("Drag corners/edges").weak());
                            ui.end_row();

                            ui.label("Crop preview");
                            ui.label(egui::RichText::new("Alt+Drag corners").weak());
                            ui.end_row();

                            ui.label("Context menu");
                            ui.label(egui::RichText::new("Right-click").weak());
                            ui.end_row();

                            ui.add_space(10.0);
                            ui.end_row();

                            ui.label(egui::RichText::new("Browser Tiles").strong());
                            ui.label("");
                            ui.end_row();

                            ui.label("Add browser");
                            ui.label(egui::RichText::new("Right-click canvas").weak());
                            ui.end_row();

                            ui.label("Interact with page");
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} / {} / Double-click",
                                    self.app_config
                                        .keyboard_shortcuts
                                        .get(HotkeySlot::InteractBrowser)
                                        .display(),
                                    self.app_config
                                        .keyboard_shortcuts
                                        .get(HotkeySlot::InteractBrowserAlternate)
                                        .display()
                                ))
                                .weak(),
                            );
                            ui.end_row();

                            ui.label("Exit interaction");
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} / {} / {} / click outside",
                                    self.app_config
                                        .keyboard_shortcuts
                                        .get(HotkeySlot::InteractBrowser)
                                        .display(),
                                    self.app_config
                                        .keyboard_shortcuts
                                        .get(HotkeySlot::InteractBrowserAlternate)
                                        .display(),
                                    self.app_config
                                        .keyboard_shortcuts
                                        .get(HotkeySlot::ExitTileOrBrowser)
                                        .display()
                                ))
                                .weak(),
                            );
                            ui.end_row();

                            ui.add_space(10.0);
                            ui.end_row();

                            ui.label(egui::RichText::new("Other").strong());
                            ui.label("");
                            ui.end_row();

                            ui.label("Show this help");
                            ui.label(
                                egui::RichText::new(
                                    self.app_config
                                        .keyboard_shortcuts
                                        .get(HotkeySlot::ShowShortcutHelp)
                                        .display(),
                                )
                                .weak(),
                            );
                            ui.end_row();
                        });

                    ui.add_space(15.0);
                    ui.vertical_centered(|ui| {
                        if ui.button("Close").clicked() {
                            self.show_shortcuts = false;
                        }
                    });
                    ui.add_space(5.0);
                });
        }

        // Schedule the next repaint instead of spinning at uncapped frame rate.
        // When captures are live, match the fastest active preview's requested
        // FPS; otherwise tick slowly, which is still frequent enough to process
        // tray events while keeping the app near-idle on the CPU.
        // (egui repaints immediately on input regardless of this hint.)
        #[cfg(windows)]
        let direct_video_fps = self.video_manager.repaint_fps();
        #[cfg(not(windows))]
        let direct_video_fps: Option<u32> = None;
        let repaint_after = self
            .capture_coordinator
            .max_live_fps()
            .into_iter()
            .chain(direct_video_fps)
            .max()
            .map(|fps| Duration::from_secs_f64(1.0 / f64::from(fps)))
            .unwrap_or_else(|| Duration::from_millis(250));
        ctx.request_repaint_after(repaint_after);
    }
}
