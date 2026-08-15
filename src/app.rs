#[cfg(windows)]
use crate::browser::{
    self, normalize_url, scrub_url_for_storage, BrowserManager, ExtensionPreparationStatus,
};
use crate::canvas::{BrowserAction, CanvasState, PlaylistAction, VideoAction};
use crate::capture::CaptureCoordinator;
use crate::external_tools::{self, ExternalTools, ToolKind, ToolStatus};
#[cfg(windows)]
use crate::libmpv::VideoManager;
use crate::media;
use crate::overlay::RegionSelector;
use crate::persistence::{
    AppConfig, CanvasLayout, SavedLayout, Storage, WindowLayout, WorkspaceIndex,
};
use crate::playlist::{FolderPlaylist, FolderPlaylistLayout, ThumbnailState};
use crate::preview::{
    BrowserTileStatus, FpsPreset, PreviewId, PreviewLayout, PreviewManager, VideoPlaybackState,
    VideoSource, VideoTileStatus, VideoTrack, WindowHandle,
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
use windows::Win32::Foundation::HWND;
#[cfg(windows)]
use windows::Win32::UI::Shell::ShellExecuteW;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, SetForegroundWindow, SW_SHOWNORMAL,
};
use wry::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

/// How long after activating a browser we skip the focus-loss check, so the
/// WebView has time to actually take the foreground.
const BROWSER_FOCUS_GRACE: Duration = Duration::from_millis(300);

/// Restored pages can be much heavier than their placeholder tiles. Spread
/// WebView startup out so several media sites cannot saturate the UI at once.
#[cfg(windows)]
const RESTORED_BROWSER_START_INTERVAL: Duration = Duration::from_secs(2);
#[cfg(windows)]
const RESTORED_VIDEO_START_INTERVAL: Duration = Duration::from_secs(2);

/// How many recent browser URLs to keep for the Add Browser dialog.
const MAX_RECENT_URLS: usize = 8;

#[cfg(windows)]
#[derive(Clone, Copy)]
struct BrowserTilePlacement {
    /// Full tile rect, which determines the page's stable viewport and zoom.
    page_rect: egui::Rect,
    /// Portion of the tile allowed to become a native window inside the canvas.
    visible_rect: egui::Rect,
}

/// Initial image tile size, fitted inside 640×480 while preserving aspect.
fn media_tile_size(width: u32, height: u32) -> Vec2 {
    let width = width.max(1) as f32;
    let height = height.max(1) as f32;
    let scale = (640.0 / width).min(480.0 / height);
    Vec2::new(width * scale, height * scale)
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
    mpv_status: &ToolStatus,
    streamlink_status: &ToolStatus,
    start_paused: bool,
) -> Result<Option<VideoLaunch>, String> {
    let Some(mpv_path) = available_tool_path(mpv_status, "mpv")? else {
        return Ok(None);
    };
    let source = match source {
        VideoSource::LocalFile { path } => {
            if !path.is_file() {
                return Err(format!(
                    "The video file no longer exists: {}",
                    path.display()
                ));
            }
            video::VideoSource::LocalFile(path.clone())
        }
        VideoSource::Stream { url, quality } => {
            let Some(streamlink_path) = available_tool_path(streamlink_status, "Streamlink")?
            else {
                return Ok(None);
            };
            video::VideoSource::Stream {
                url: url.clone(),
                quality: quality.clone(),
                streamlink_path,
            }
        }
    };
    Ok(Some(VideoLaunch {
        mpv_path,
        source,
        start_paused,
    }))
}

#[cfg(windows)]
fn preview_playback_state(state: &video::VideoState) -> VideoPlaybackState {
    VideoPlaybackState {
        connected: state.connected,
        paused: state.pause,
        time_pos: state.time_pos,
        duration: state.duration,
        volume: state.volume,
        muted: state.mute,
        speed: state.speed,
        looping: !matches!(state.loop_file, video::LoopMode::Off),
        tracks: state
            .track_list
            .iter()
            .map(|track| VideoTrack {
                id: track.id,
                kind: track.kind.clone(),
                title: track.title.clone(),
                language: track.lang.clone(),
                selected: track.selected,
            })
            .collect(),
        audio_track: match state.audio_track {
            video::TrackSelection::Id(id) => Some(id),
            _ => None,
        },
        subtitle_track: match state.subtitle_track {
            video::TrackSelection::Id(id) => Some(id),
            _ => None,
        },
    }
}

#[cfg(windows)]
fn preview_video_status(state: &video::VideoState, paused_on_restore: bool) -> VideoTileStatus {
    if !state.connected {
        if paused_on_restore {
            VideoTileStatus::PausedOnRestore
        } else {
            VideoTileStatus::Starting
        }
    } else if state.paused_for_cache || (!state.pause && state.core_idle) {
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
    ConfirmDelete,
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
struct SeekPreviewJob {
    mpv_path: std::path::PathBuf,
    source: video::VideoThumbnailSource,
    requested_time: f64,
    due: Instant,
    delivered_time: Option<f64>,
    in_flight: Option<(f64, video::VideoThumbnailReceiver)>,
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
) -> bool {
    pending.start_paused
        && pending.shown_once
        && pending.retry_ready
        && tile_rect
            .zip(viewport)
            .is_some_and(|(tile, viewport)| tile.intersects(viewport))
}

#[cfg(windows)]
fn restored_browser_ready(
    pending: &PendingBrowserTile,
    tile_rect: Option<egui::Rect>,
    viewport: Option<egui::Rect>,
) -> bool {
    pending.shown_once
        && pending.restore_deferred
        && tile_rect
            .zip(viewport)
            .is_some_and(|(tile, viewport)| tile.intersects(viewport))
}

/// Main application state
pub struct PluriviewApp {
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
    /// Video placeholders waiting for optional tools to finish validation.
    #[cfg(windows)]
    pending_video_tiles: HashMap<PreviewId, PendingVideoTile>,
    /// Last restored video startup, used to stagger mpv/Streamlink processes.
    #[cfg(windows)]
    last_restored_video_start: Option<Instant>,
    /// Restored sessions stay visibly paused until mpv reports playback.
    #[cfg(windows)]
    restored_paused_videos: HashSet<PreviewId>,
    /// Debounced, background thumbnail decoders for seekable video tiles.
    #[cfg(windows)]
    seek_preview_jobs: HashMap<PreviewId, SeekPreviewJob>,
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

    /// Storage for persistence
    storage: Option<Storage>,

    /// App-global settings, kept outside workspace layouts.
    app_config: AppConfig,

    /// Optional mpv/Streamlink discovery and validation state.
    external_tools: ExternalTools,

    /// Show app-global Settings dialog.
    show_settings: bool,

    /// Last config persistence error, shown inside Settings.
    config_error: Option<String>,

    /// Named workspace catalog introduced in v0.5.
    workspaces: WorkspaceIndex,

    /// Create, duplicate, or rename dialog.
    workspace_dialog: Option<WorkspaceDialog>,

    /// Confirmation guard for deleting the active workspace.
    confirm_workspace_delete: bool,

    /// Last workspace persistence error, shown as a dismissible dialog.
    workspace_error: Option<String>,

    /// System tray manager
    tray_manager: Option<TrayManager>,

    /// Has the window HWND been set for the tray manager?
    hwnd_set: bool,

    /// Show About dialog
    show_about: bool,

    /// Show Keyboard Shortcuts dialog
    show_shortcuts: bool,

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

    /// Held state of the polled numpad shortcuts (`[Numpad 1, Numpad 2]`),
    /// kept so the toggles fire once per press instead of every frame.
    #[cfg(windows)]
    numpad_held: [bool; 2],
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
        let external_tools = ExternalTools::new(
            app_config.external_tools.mpv_path.clone(),
            app_config.external_tools.streamlink_path.clone(),
        );
        let (workspaces, workspace_error) = match &storage {
            Some(storage) => match storage.load_or_initialize_workspaces() {
                Ok(index) => (index, None),
                Err(error) => (
                    WorkspaceIndex::default(),
                    Some(format!("Could not load workspaces: {error}")),
                ),
            },
            None => (
                WorkspaceIndex::default(),
                Some("Workspace storage is unavailable.".to_owned()),
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
            canvas: CanvasState::default(),
            preview_manager: PreviewManager::new(),
            window_picker: WindowPicker::new(),
            capture_coordinator: CaptureCoordinator::new(),
            #[cfg(windows)]
            video_manager: VideoManager::new(),
            #[cfg(windows)]
            pending_video_tiles: HashMap::new(),
            #[cfg(windows)]
            last_restored_video_start: None,
            #[cfg(windows)]
            restored_paused_videos: HashSet::new(),
            #[cfg(windows)]
            seek_preview_jobs: HashMap::new(),
            #[cfg(windows)]
            playlist_thumbnail_queue: VecDeque::new(),
            #[cfg(windows)]
            playlist_thumbnail_jobs: Vec::new(),
            #[cfg(windows)]
            next_playlist_group: 1,
            picker_open: true,
            canvas_only: false,
            storage,
            app_config,
            external_tools,
            show_settings: false,
            config_error,
            workspaces,
            workspace_dialog: None,
            confirm_workspace_delete: false,
            workspace_error,
            tray_manager,
            hwnd_set: false,
            show_about: false,
            show_shortcuts: false,
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
            numpad_held: [false; 2],
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
    fn required_tool_paths(
        &self,
        action: &str,
        kinds: &[ToolKind],
    ) -> Result<Vec<std::path::PathBuf>, String> {
        let mut paths = Vec::with_capacity(kinds.len());
        let mut unavailable = Vec::new();
        for kind in kinds {
            match self.external_tools.status(*kind) {
                ToolStatus::Available { path, .. } => paths.push(path.clone()),
                _ => unavailable.push(kind.display_name()),
            }
        }
        if unavailable.is_empty() {
            Ok(paths)
        } else {
            Err(format!(
                "{action} requires {}. Configure the unavailable tool{} in Settings.",
                unavailable.join(" and "),
                if unavailable.len() == 1 { "" } else { "s" }
            ))
        }
    }

    #[cfg(windows)]
    fn add_local_video(&mut self, position: Pos2) {
        if let Err(error) = self.required_tool_paths("Adding a video", &[ToolKind::Mpv]) {
            self.external_tool_error = Some(error);
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
        self.required_tool_paths("Adding a video", &[ToolKind::Mpv])?;
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
        self.required_tool_paths("Adding a video folder", &[ToolKind::Mpv])?;
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
            Vec2::new(340.0, 360.0),
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
    ) -> Result<PreviewId, String> {
        let playlist = FolderPlaylist::from_layout(layout)?;
        Ok(self.preview_manager.add_folder_playlist(
            playlist,
            title,
            position,
            size,
            group,
            linked_video,
        ))
    }

    #[cfg(windows)]
    fn open_add_stream(&mut self, position: Pos2) {
        let paths = match self
            .required_tool_paths("Adding a stream", &[ToolKind::Mpv, ToolKind::Streamlink])
        {
            Ok(paths) => paths,
            Err(error) => {
                self.external_tool_error = Some(error);
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
            streamlink_path: paths[1].clone(),
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
            self.external_tools.status(ToolKind::Mpv),
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
                (!pending.start_paused && pending.retry_ready).then_some(*id)
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
                        !matches!(preview.video_status, VideoTileStatus::Failed(_))
                    });
                    restored_video_ready(pending, preview.map(|preview| preview.rect()), viewport)
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
                !matches!(preview.video_status, VideoTileStatus::Failed(_))
                    && restored_video_ready(pending, Some(preview.rect()), viewport)
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
            return;
        }

        let mut changed = HashSet::new();
        let mut errors = HashMap::new();
        let mut pause_changed = HashSet::new();
        let mut exited = HashSet::new();
        for (id, update) in updates {
            changed.insert(id);
            match update {
                VideoUpdate::Property(video::VideoProperty::Pause) => {
                    pause_changed.insert(id);
                }
                VideoUpdate::Error(error) => {
                    errors.insert(id, error);
                }
                VideoUpdate::Exited {
                    status,
                    unexpected,
                    stderr_tail,
                } => {
                    exited.insert(id);
                    errors.entry(id).or_insert_with(|| {
                        let detail = if stderr_tail.is_empty() {
                            String::new()
                        } else {
                            format!("\n{stderr_tail}")
                        };
                        if unexpected {
                            format!("The video process exited unexpectedly with {status}.{detail}")
                        } else {
                            format!("The video process exited with {status}.{detail}")
                        }
                    });
                }
                VideoUpdate::Connected | VideoUpdate::Property(_) | VideoUpdate::Event => {}
            }
        }

        let mut auto_advance = Vec::new();
        for id in changed {
            let Some(state) = self
                .video_manager
                .get(id)
                .map(|tile| tile.session.state().clone())
            else {
                continue;
            };
            if pause_changed.contains(&id) && !state.pause {
                self.restored_paused_videos.remove(&id);
            }
            if let Some(preview) = self.preview_manager.get_mut(id) {
                preview.video_playback = preview_playback_state(&state);
                if let Some(title) = state.media_title.as_ref().filter(|title| !title.is_empty()) {
                    preview.title.clone_from(title);
                }
                preview.video_status = errors
                    .remove(&id)
                    .map(VideoTileStatus::Failed)
                    .unwrap_or_else(|| {
                        preview_video_status(&state, self.restored_paused_videos.contains(&id))
                    });
            }
            if state.eof_reached && matches!(state.loop_file, video::LoopMode::Off) {
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

        for id in exited {
            self.capture_coordinator.stop_capture(id);
            self.video_manager.remove(id);
            self.restored_paused_videos.remove(&id);
        }
        ctx.request_repaint();
    }

    #[cfg(windows)]
    fn request_seek_preview(&mut self, id: PreviewId, time: f64) {
        if !time.is_finite() || time < 0.0 {
            return;
        }
        let Some(source) = self
            .preview_manager
            .get(id)
            .and_then(|preview| preview.video_source.clone())
        else {
            return;
        };
        let ToolStatus::Available { path: mpv_path, .. } =
            self.external_tools.status(ToolKind::Mpv)
        else {
            return;
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

        // A one-second bucket gives stable previews while still feeling exact,
        // and avoids launching a decoder for every sub-pixel pointer movement.
        let requested_time = time.round().max(0.0);
        let now = Instant::now();
        match self.seek_preview_jobs.entry(id) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let job = entry.get_mut();
                if (job.requested_time - requested_time).abs() > 0.01 {
                    job.requested_time = requested_time;
                    job.due = now + Duration::from_millis(140);
                }
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(SeekPreviewJob {
                    mpv_path: mpv_path.clone(),
                    source: thumbnail_source,
                    requested_time,
                    due: now + Duration::from_millis(140),
                    delivered_time: None,
                    in_flight: None,
                });
            }
        }
    }

    #[cfg(windows)]
    fn poll_seek_previews(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let ids: Vec<_> = self.seek_preview_jobs.keys().copied().collect();
        let mut completed = Vec::new();

        for id in ids {
            let Some(job) = self.seek_preview_jobs.get_mut(&id) else {
                continue;
            };
            let mut just_completed = false;
            if let Some((time, receiver)) = job.in_flight.as_ref() {
                match receiver.try_recv() {
                    Ok(result) => {
                        completed.push((id, *time, result));
                        job.in_flight = None;
                        just_completed = true;
                    }
                    Err(TryRecvError::Disconnected) => {
                        completed.push((
                            id,
                            *time,
                            Err("Timeline preview worker stopped unexpectedly".to_owned()),
                        ));
                        job.in_flight = None;
                        just_completed = true;
                    }
                    Err(TryRecvError::Empty) => {}
                }
            }

            let already_delivered = job
                .delivered_time
                .is_some_and(|time| (time - job.requested_time).abs() <= 0.01);
            if !just_completed && job.in_flight.is_none() && !already_delivered && now >= job.due {
                let time = job.requested_time;
                job.in_flight = Some((
                    time,
                    video::spawn_video_thumbnail(job.mpv_path.clone(), job.source.clone(), time),
                ));
            }
        }

        for (id, completed_time, result) in completed {
            let still_requested = self
                .seek_preview_jobs
                .get(&id)
                .is_some_and(|job| (job.requested_time - completed_time).abs() <= 0.01);
            match result {
                Ok(thumbnail) if still_requested => {
                    if let Some(job) = self.seek_preview_jobs.get_mut(&id) {
                        job.delivered_time = Some(completed_time);
                    }
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
                Ok(_) => {}
                Err(error) => {
                    if still_requested {
                        if let Some(job) = self.seek_preview_jobs.get_mut(&id) {
                            job.delivered_time = None;
                            job.due = Instant::now() + Duration::from_secs(2);
                        }
                    }
                    log::debug!("Timeline preview failed: {error}");
                }
            }
        }

        let previews = &self.preview_manager;
        self.seek_preview_jobs.retain(|id, _| {
            previews
                .get(*id)
                .is_some_and(|preview| preview.supports_seek_preview())
        });
        if self.seek_preview_jobs.values().any(|job| {
            job.in_flight.is_some()
                || (!job
                    .delivered_time
                    .is_some_and(|time| (time - job.requested_time).abs() <= 0.01)
                    && now < job.due)
        }) {
            ctx.request_repaint_after(Duration::from_millis(40));
        }
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
                    match FolderPlaylist::from_layout(&layout) {
                        Ok(playlist) => {
                            if let Some(preview) = self.preview_manager.get_mut(id) {
                                preview.folder_playlist = Some(playlist);
                            }
                            self.playlist_thumbnail_queue
                                .retain(|(playlist_id, _)| *playlist_id != id);
                        }
                        Err(error) => self.media_error = Some(error),
                    }
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

        self.seek_preview_jobs.remove(&video_id);
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
        let stale: Vec<_> = self
            .video_manager
            .ids()
            .filter(|id| {
                self.preview_manager
                    .get(*id)
                    .is_none_or(|preview| !preview.is_video())
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
                (pending.shown_once && !pending.restore_deferred).then_some(*id)
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
                        let tile_rect = self.preview_manager.get(*id).map(|preview| preview.rect());
                        restored_browser_ready(pending, tile_rect, viewport).then_some(*id)
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
                    let tile_rect = self.preview_manager.get(*id).map(|preview| preview.rect());
                    restored_browser_ready(pending, tile_rect, viewport)
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

    /// Import an external image into portable managed storage and create its tile.
    fn import_media_tile(
        &mut self,
        source: &std::path::Path,
        position: Pos2,
    ) -> Result<PreviewId, String> {
        // Decode first so an unsupported or damaged file is not copied into
        // managed storage as an unusable orphan.
        let asset = media::load(source)?;
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| "Pluriview storage is unavailable".to_owned())?;
        let managed_path = storage
            .import_media(source)
            .map_err(|error| format!("Could not copy image into managed storage: {error}"))?;
        let title = source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Image")
            .to_owned();
        let size = media_tile_size(asset.width, asset.height);
        Ok(self
            .preview_manager
            .add_media(managed_path, title, asset.frames, position, size))
    }

    /// Recreate a tile from a relative filename already in managed storage.
    fn restore_media_tile(
        &mut self,
        managed_path: &str,
        title: String,
        position: Pos2,
        size: Vec2,
    ) -> Result<PreviewId, String> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| "Pluriview storage is unavailable".to_owned())?;
        let path = storage
            .resolve_media(managed_path)
            .ok_or_else(|| "Saved image path is invalid".to_owned())?;
        if !path.is_file() {
            return Err(format!("Saved image is missing: {}", path.display()));
        }
        let asset = media::load(&path)?;
        Ok(self.preview_manager.add_media(
            managed_path.to_owned(),
            title,
            asset.frames,
            position,
            size,
        ))
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
                    let response = ui.add_sized(
                        [420.0, 24.0],
                        egui::TextEdit::singleline(&mut dialog.url)
                            .hint_text("twitch.tv/channel or https://kick.com/channel"),
                    );
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
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });

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
                    let response = ui.add_sized(
                        [460.0, 24.0],
                        egui::TextEdit::singleline(&mut dialog.url)
                            .hint_text("https://example.com/channel"),
                    );
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
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
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
        let rect = self
            .canvas
            .canvas_rect_to_screen(preview.rect(), canvas_rect);
        if !rect.intersects(canvas_rect) {
            return None;
        }
        let page_rect = rect;
        let visible_rect = page_rect.intersect(canvas_rect);
        (visible_rect.width() >= 1.0 && visible_rect.height() >= 1.0).then_some(
            BrowserTilePlacement {
                page_rect,
                visible_rect,
            },
        )
    }

    /// Keep the stream audio monitor alive while it's enabled and browser
    /// tiles exist, pointed at the current WebView2 browser process (which
    /// changes if the WebView2 runtime crashes and restarts).
    #[cfg(windows)]
    fn audio_monitor_upkeep(&mut self) {
        let enabled = self.monitor_device.is_some();
        if !enabled || self.browser.is_empty() {
            self.audio_monitor = None;
            self.audio_monitor_checked = None;
            return;
        }
        let due = self
            .audio_monitor_checked
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(2));
        if !due {
            return;
        }
        self.audio_monitor_checked = Some(Instant::now());
        let device_id = self.monitor_device.as_ref().map(|(id, _)| id.clone());
        let (Some(device_id), Some(pid)) = (device_id, self.browser.browser_process_id()) else {
            return;
        };
        let current = self
            .audio_monitor
            .as_ref()
            .map(|m| (m.browser_pid(), m.device_id().to_owned()));
        if current != Some((pid, device_id.clone())) {
            self.audio_monitor = Some(crate::audio::AudioMonitor::start(pid, device_id));
        }
    }

    /// Per-frame browser housekeeping. Runs after the canvas UI so tile
    /// rects and double-click state are fresh.
    #[cfg(windows)]
    fn browser_frame(&mut self, ctx: &egui::Context) {
        self.audio_monitor_upkeep();

        // Mirror page titles and current URLs onto the tiles so the hover
        // overlay shows "lofi hip hop radio..." instead of the raw URL and
        // layouts save where the user actually navigated.
        let mut updates = Vec::new();
        for (id, host) in self.browser.iter_mut() {
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
        // tile's model size, not the canvas zoom. This keeps previews sharp
        // without making responsive pages resize while the canvas zooms.
        let pixels_per_point = ctx.pixels_per_point();
        let ids: Vec<_> = self.browser.ids().collect();
        for id in ids {
            let Some(preview) = self.preview_manager.get(id) else {
                continue;
            };
            let physical_size = preview.size * pixels_per_point;
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
            let escape = browser::escape_pressed();
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

    /// Edge-detect the polled numpad shortcuts, returning `[Numpad 1, Numpad 2]`.
    ///
    /// These are polled instead of read from egui for two reasons: an
    /// interactive WebView swallows all keyboard input before egui sees it,
    /// and egui reports numpad digits identically to the number row. Polling is
    /// process-wide, so presses only count while Pluriview is in the
    /// foreground; the held state is still tracked when it is not, so a key
    /// pressed in another app doesn't fire on the way back.
    #[cfg(windows)]
    fn poll_numpad_shortcuts(&mut self, ctx: &egui::Context) -> [bool; 2] {
        let listening = self.owns_foreground() && !ctx.wants_keyboard_input();
        if listening {
            // Polling only sees keys still held at frame time, so tick fast
            // enough that a quick tap can't fall between two frames.
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        let mut pressed = [false; 2];
        for (index, digit) in [1u8, 2].into_iter().enumerate() {
            let down = browser::numpad_digit_down(digit);
            pressed[index] = down && !self.numpad_held[index] && listening;
            self.numpad_held[index] = down;
        }
        pressed
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

        egui::TopBottomPanel::top("custom_title_bar")
            .frame(egui::Frame::none().fill(bg))
            .exact_height(34.0)
            .show(ctx, |ui| {
                let title_bar_rect = ui.max_rect();

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
                        let (dot_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
                        ui.painter().circle_filled(
                            dot_rect.center(),
                            4.0,
                            egui::Color32::from_rgb(107, 170, 75),
                        );
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

                        let btn_size = Vec2::new(44.0, 34.0);

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
    }

    /// The File / View / Help menus. Rendered inline in the title bar next
    /// to the app name (Minimal Void: one unified dark strip, no separate
    /// menu-bar row).
    fn menu_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let workspace_entries = self.workspaces.workspaces.clone();
        let active_workspace_id = self.workspaces.active_workspace_id.clone();
        let active_workspace_name = self
            .workspaces
            .active()
            .map(|workspace| workspace.name.clone())
            .unwrap_or_else(|| "Default".to_owned());
        let mut workspace_action = None;

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
                if ui.button("Settings...").clicked() {
                    self.show_settings = true;
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
                if ui
                    .checkbox(&mut self.picker_open, "Window Picker")
                    .clicked()
                {
                    ui.close_menu();
                }
                if ui
                    .checkbox(&mut self.canvas.show_grid, "Show Grid (G)")
                    .clicked()
                {
                    ui.close_menu();
                }
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
                WorkspaceMenuAction::ConfirmDelete => self.confirm_workspace_delete = true,
            }
        }
    }

    fn settings_ui(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }

        let mut open = self.show_settings;
        let mut close = false;
        let mut action = None;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(600.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::window(&ctx.style()).fill(egui::Color32::from_rgb(25, 25, 28)),
            )
            .show(ctx, |ui| {
                ui.heading("External tools");
                ui.label(
                    egui::RichText::new(
                        "Optional helpers are detected automatically. They are not required to use Pluriview.",
                    )
                    .weak(),
                );
                ui.add_space(8.0);

                for kind in ToolKind::ALL {
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

                if let Some(error) = &self.config_error {
                    ui.colored_label(egui::Color32::from_rgb(235, 120, 120), error);
                    ui.add_space(6.0);
                }

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
            ToolKind::Mpv => self.app_config.external_tools.mpv_path = path.clone(),
            ToolKind::Streamlink => self.app_config.external_tools.streamlink_path = path.clone(),
        }
        self.external_tools.set_override(kind, path);

        self.config_error = match &self.storage {
            Some(storage) => storage
                .save_config(&self.app_config)
                .err()
                .map(|error| format!("Could not save settings: {error}")),
            None => Some("Settings storage is unavailable.".to_owned()),
        };
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
            ui.label("Replay tile audio to a device so Discord/OBS\nwindow shares carry sound. Pick one you don't\nlisten to (virtual cable, unused output).");
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
        let title_bar_height = if self.canvas_only { 0.0 } else { 34.0 };
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

    /// Load the active named workspace if it has been saved before.
    fn load_active_workspace(&mut self) {
        let Some(storage) = &self.storage else {
            return;
        };
        let id = self.workspaces.active_workspace_id.clone();
        match storage.load_workspace(&id) {
            Ok(layout) => {
                self.apply_layout(&layout);
                #[cfg(debug_assertions)]
                println!(
                    "Loaded workspace {id} with {} previews",
                    layout.previews.len()
                );
            }
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
            Err(error) => {
                self.workspace_error =
                    Some(format!("Could not load the active workspace: {error}"));
            }
        }
    }

    /// Save the current workspace and mirror it to legacy autosave.json.
    fn save_active_workspace(&self) -> Result<(), String> {
        let Some(storage) = &self.storage else {
            return Err("Workspace storage is unavailable.".to_owned());
        };
        let layout = self.create_layout();
        storage
            .save_active_workspace(&self.workspaces, &layout)
            .map_err(|error| format!("Could not save workspace: {error}"))?;
        #[cfg(debug_assertions)]
        println!(
            "Saved workspace {} with {} previews",
            self.workspaces.active_workspace_id,
            layout.previews.len()
        );
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
        self.save_active_workspace()?;

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
        Ok(())
    }

    fn create_workspace(&mut self, name: &str, duplicate: bool) -> Result<(), String> {
        let name = self.valid_workspace_name(name, None)?;
        self.save_active_workspace()?;
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
        if let Err(error) = storage.delete_workspace(&deleted_id) {
            self.workspace_error = Some(format!(
                "The workspace was removed from the list, but its old file could not be deleted: {error}"
            ));
        }
        self.apply_layout(&layout);
        Ok(())
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
        // Clear existing state
        self.capture_coordinator.stop_all();
        #[cfg(windows)]
        {
            self.video_manager.clear();
            self.pending_video_tiles.clear();
            self.last_restored_video_start = None;
            self.restored_paused_videos.clear();
            self.seek_preview_jobs.clear();
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
        self.canvas.clear_preview_animations();
        #[cfg(windows)]
        {
            self.browser.clear();
            self.pending_browser_tiles.clear();
            self.last_restored_browser_start = None;
        }

        // Restore canvas state
        self.canvas.pan = Vec2::new(layout.canvas.pan.0, layout.canvas.pan.1);
        self.canvas.zoom = layout.canvas.zoom;
        self.canvas.show_grid = layout.canvas.show_grid;
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
                            // Restored tiles appear instantly, no spawn animation.
                            preview.created_at = Instant::now() - Duration::from_secs(1);
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
                            preview.lock_aspect_ratio = preview_layout.lock_aspect_ratio;
                            preview.crop_uv = preview_layout.crop_uv;
                            preview.created_at = Instant::now() - Duration::from_secs(1);
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
                match self.restore_folder_playlist(
                    folder_playlist,
                    preview_layout.window_title.clone(),
                    Pos2::new(preview_layout.position.0, preview_layout.position.1),
                    Vec2::new(preview_layout.size.0, preview_layout.size.1),
                    group,
                    None,
                ) {
                    Ok(id) => {
                        self.preview_manager.set_z_order(id, preview_layout.z_order);
                        if let Some(preview) = self.preview_manager.get_mut(id) {
                            preview.created_at = Instant::now() - Duration::from_secs(1);
                        }
                    }
                    Err(error) => {
                        log::error!("Failed to restore folder playlist: {error}");
                        self.media_error = Some(error);
                    }
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
                    preview.lock_aspect_ratio = preview_layout.lock_aspect_ratio;
                    preview.crop_uv = preview_layout.crop_uv;
                    preview.playlist_group = preview_layout.playlist_group;
                    preview.created_at = Instant::now() - Duration::from_secs(1);
                }
                continue;
            }

            // Try to find a matching window by title
            let matching_window = current_windows
                .iter()
                .find(|w| w.title == preview_layout.window_title);

            if let Some(window_info) = matching_window {
                // Create preview with saved position/size
                let id = self.preview_manager.add_with_window(
                    window_info.title.clone(),
                    Pos2::new(preview_layout.position.0, preview_layout.position.1),
                    Vec2::new(preview_layout.size.0, preview_layout.size.1),
                    window_info.hwnd,
                    preview_layout.fps_preset,
                    preview_layout.z_order,
                );

                // Start capture
                self.capture_coordinator.start_capture(
                    id,
                    window_info.hwnd,
                    window_info.title.clone(),
                    preview_layout.fps_preset.as_u32(),
                );

                // Restore crop region if it was saved
                if let Some(crop) = preview_layout.crop_uv {
                    if let Some(preview) = self.preview_manager.get_mut(id) {
                        preview.crop_uv = Some(crop);
                    }
                }

                #[cfg(debug_assertions)]
                println!(
                    "Restored preview: {}",
                    privacy::redact_title(&window_info.title)
                );
            } else {
                #[cfg(debug_assertions)]
                println!(
                    "Window not found: {}",
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
        preview_playback_state, preview_video_status, restored_browser_ready, restored_video_ready,
        video_launch_for_source, FpsPreset, PendingBrowserTile, PendingVideoTile, VideoSource,
        VideoTileStatus,
    };
    use crate::external_tools::{DiscoverySource, ToolStatus};
    use crate::video::{LoopMode, TrackInfo, TrackSelection, VideoState};
    use eframe::egui::{Pos2, Rect, Vec2};
    use std::path::PathBuf;

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
            Some(viewport)
        ));
        assert!(restored_browser_ready(
            &restored_pending(true),
            Some(visible),
            Some(viewport)
        ));
        assert!(!restored_browser_ready(
            &restored_pending(true),
            Some(offscreen),
            Some(viewport)
        ));
        assert!(!restored_browser_ready(
            &restored_pending(true),
            Some(visible),
            None
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
        ));
        assert!(restored_video_ready(
            &PendingVideoTile {
                start_paused: true,
                shown_once: true,
                retry_ready: true,
            },
            Some(visible),
            Some(viewport),
        ));
        assert!(!restored_video_ready(
            &PendingVideoTile {
                start_paused: true,
                shown_once: true,
                retry_ready: true,
            },
            Some(offscreen),
            Some(viewport),
        ));
        assert!(!restored_video_ready(
            &PendingVideoTile {
                start_paused: false,
                shown_once: true,
                retry_ready: true,
            },
            Some(visible),
            Some(viewport),
        ));
    }

    #[test]
    fn video_launches_only_with_validated_available_tools() {
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
        assert!(video_launch_for_source(&source, &checking, &missing, true)
            .unwrap()
            .is_none());

        let invalid = ToolStatus::Invalid {
            path: PathBuf::from(r"C:\broken\mpv.exe"),
            error: "version check failed".to_owned(),
            source: DiscoverySource::Override,
        };
        assert!(video_launch_for_source(&source, &invalid, &missing, true).is_err());

        let available = ToolStatus::Available {
            path: PathBuf::from(r"C:\mpv\mpv.exe"),
            version: "mpv test".to_owned(),
            source: DiscoverySource::Path,
        };
        let launch = video_launch_for_source(&source, &available, &missing, true)
            .unwrap()
            .unwrap();
        assert!(launch.start_paused);
        assert!(matches!(
            launch.source,
            crate::video::VideoSource::LocalFile(path)
                if path == media_path
        ));

        let stream = VideoSource::Stream {
            url: "https://example.test/live".to_owned(),
            quality: "best".to_owned(),
        };
        assert!(video_launch_for_source(&stream, &available, &missing, false).is_err());
        std::fs::remove_file(media_path).unwrap();
    }

    #[test]
    fn video_runtime_state_maps_to_preview_models() {
        let mut state = VideoState {
            connected: true,
            pause: true,
            time_pos: Some(12.0),
            duration: Some(60.0),
            volume: 45.0,
            mute: true,
            speed: 1.25,
            loop_file: LoopMode::Infinite,
            audio_track: TrackSelection::Id(2),
            subtitle_track: TrackSelection::Disabled,
            media_title: Some("Mapped title".to_owned()),
            ..Default::default()
        };
        state.track_list.push(TrackInfo {
            id: 2,
            kind: "audio".to_owned(),
            title: Some("Commentary".to_owned()),
            lang: Some("en".to_owned()),
            selected: true,
            external: false,
            codec: Some("aac".to_owned()),
        });

        let mapped = preview_playback_state(&state);
        assert!(mapped.connected);
        assert!(mapped.paused);
        assert!(mapped.looping);
        assert_eq!(mapped.audio_track, Some(2));
        assert_eq!(mapped.subtitle_track, None);
        assert_eq!(mapped.tracks[0].language.as_deref(), Some("en"));
        assert_eq!(
            preview_video_status(&state, true),
            VideoTileStatus::PausedOnRestore
        );

        state.pause = false;
        state.paused_for_cache = true;
        assert_eq!(
            preview_video_status(&state, false),
            VideoTileStatus::Buffering
        );
        state.paused_for_cache = false;
        assert_eq!(preview_video_status(&state, false), VideoTileStatus::Ready);
    }
}

impl Drop for PluriviewApp {
    fn drop(&mut self) {
        self.capture_coordinator.stop_all();
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
        self.poll_video_manager(ctx);
        #[cfg(windows)]
        self.poll_seek_previews(ctx);

        // Custom title bar + manual resize border (decorations are off)
        self.handle_frameless_resize(ctx);
        if !self.canvas_only {
            self.title_bar_ui(ctx);
        }

        // Process any pending captured frames
        self.capture_coordinator
            .process_frames(&mut self.preview_manager);

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
                        if let Some(preview) = self.preview_manager.get_mut(preview_id) {
                            // Get source dimensions from frame if available
                            if let Some((w, h)) = preview.frame_size {
                                let crop_uv = selection.to_uv(w, h);
                                preview.crop_uv = Some(crop_uv);
                                // Update aspect ratio for the cropped region
                                let crop_w = (crop_uv.2 - crop_uv.0) * w as f32;
                                let crop_h = (crop_uv.3 - crop_uv.1) * h as f32;
                                if crop_h > 0.0 {
                                    preview.source_aspect_ratio = crop_w / crop_h;
                                }
                            }
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
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(egui::Color32::from_rgb(13, 13, 13)))
            .show(ctx, |ui| {
                self.canvas.ui(
                    ui,
                    &mut self.preview_manager,
                    &mut self.capture_coordinator,
                    ctx,
                    !self.canvas_only,
                );
            });

        self.file_drop_overlay(ctx);
        self.import_dropped_files(ctx);

        #[cfg(windows)]
        if !self.pending_browser_tiles.is_empty() {
            for pending in self.pending_browser_tiles.values_mut() {
                pending.shown_once = true;
            }
            // Guarantee at least one painted placeholder frame before any
            // WebView creation can occupy the UI thread.
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
            let [numpad_interact, numpad_focus] = self.poll_numpad_shortcuts(ctx);

            let browser_double_clicked = self
                .canvas
                .last_double_clicked
                .filter(|id| self.browser.contains(*id));

            let browser_shortcut = ((!ctx.wants_keyboard_input()
                && ctx.input(|input| input.modifiers.ctrl && input.key_pressed(egui::Key::B)))
                || numpad_interact)
                .then(|| {
                    // The live tile first, so Numpad 1 toggles interaction back off
                    // even if the selection drifted while the page had focus.
                    self.browser.active_id().or_else(|| {
                        self.canvas
                            .selection
                            .iter()
                            .copied()
                            .find(|id| self.browser.contains(*id))
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
                        // Bring to front + select so the accent outline shows
                        // around the live window's inset edge.
                        self.preview_manager.bring_to_front(id);
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

            // Numpad 2: fit the current tile to the canvas, or restore the
            // pre-focus view if a tile is already focused (same as Escape).
            if numpad_focus && !self.canvas.exit_focus() {
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
            self.browser_frame(ctx);
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

        // Image files are copied into `pluriview_data/media` so the saved
        // relative path remains valid when a portable install is moved.
        if let Some(position) = self.canvas.pending_media_add.take() {
            if let Some(path) = media::pick_file(self.main_hwnd) {
                if let Err(error) = self.import_media_tile(&path, position) {
                    log::error!("Failed to import image tile: {error}");
                    self.media_error = Some(error);
                }
            }
        }

        #[cfg(windows)]
        {
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

            // "Undo" on a removed browser tile: recreate the WebView from
            // its saved URL (the original host window is already destroyed).
            if let Some(info) = self.canvas.pending_browser_restore.take() {
                if let Some(url) = info.browser_url.clone() {
                    match self.create_browser_tile(&url, info.position, info.size, info.fps_preset)
                    {
                        Ok(id) => self.apply_browser_mute(id, info.browser_muted),
                        Err(error) => log::error!("Failed to restore browser tile: {error}"),
                    }
                }
            }
        }

        if let Some(info) = self.canvas.pending_media_restore.take() {
            if let Some(media_path) = info.media_path.clone() {
                match self.restore_media_tile(&media_path, info.title, info.position, info.size) {
                    Ok(id) => {
                        if let Some(preview) = self.preview_manager.get_mut(id) {
                            preview.crop_uv = info.crop_uv;
                        }
                    }
                    Err(error) => {
                        log::error!("Failed to undo image tile removal: {error}");
                        self.media_error = Some(error);
                    }
                }
            }
        }

        #[cfg(windows)]
        if let Some(info) = self.canvas.pending_video_restore.take() {
            if let Some(source) = info.video_source.clone() {
                let id = self.create_video_tile(
                    source,
                    info.title,
                    info.position,
                    info.size,
                    info.fps_preset,
                    true,
                );
                if let Some(preview) = self.preview_manager.get_mut(id) {
                    preview.crop_uv = info.crop_uv;
                }
            }
        }

        #[cfg(windows)]
        if let Some(info) = self.canvas.pending_playlist_restore.take() {
            if let (Some(playlist), Some(group)) =
                (info.folder_playlist.as_ref(), info.playlist_group)
            {
                let layout = playlist.layout();
                let linked = self.preview_manager.all().find_map(|preview| {
                    (preview.playlist_group == Some(group) && preview.is_video())
                        .then_some(preview.id)
                });
                match self.restore_folder_playlist(
                    &layout,
                    info.title,
                    info.position,
                    info.size,
                    group,
                    linked,
                ) {
                    Ok(id) => self.canvas.selection = vec![id],
                    Err(error) => self.media_error = Some(error),
                }
            }
        }

        self.quick_add_ui(ctx);
        #[cfg(windows)]
        self.add_browser_ui(ctx);
        #[cfg(windows)]
        self.add_stream_ui(ctx);
        self.external_tool_error_ui(ctx);
        self.video_action_error_ui(ctx);
        self.settings_ui(ctx);
        self.workspace_dialog_ui(ctx);
        self.workspace_delete_confirmation_ui(ctx);

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

        // Handle global keyboard shortcuts (skip while typing in a text field)
        if !ctx.wants_keyboard_input() {
            ctx.input(|i| {
                // G - Toggle grid
                if i.key_pressed(egui::Key::G) && !i.modifiers.ctrl && !i.modifiers.alt {
                    self.canvas.show_grid = !self.canvas.show_grid;
                }
                // H - Toggle canvas-only mode
                if i.key_pressed(egui::Key::H) && !i.modifiers.ctrl && !i.modifiers.alt {
                    self.canvas_only = !self.canvas_only;
                }
                // F1 - Show keyboard shortcuts
                if i.key_pressed(egui::Key::F1) {
                    self.show_shortcuts = true;
                }
            });
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
                            ui.label(egui::RichText::new("G").weak());
                            ui.end_row();

                            ui.label("Canvas-only mode");
                            ui.label(egui::RichText::new("H").weak());
                            ui.end_row();

                            ui.label("Focus current tile");
                            ui.label(egui::RichText::new("Numpad 2").weak());
                            ui.end_row();

                            ui.label("Exit tile focus");
                            ui.label(egui::RichText::new("Numpad 2 / Esc").weak());
                            ui.end_row();

                            ui.add_space(10.0);
                            ui.end_row();

                            ui.label(egui::RichText::new("Selection").strong());
                            ui.label("");
                            ui.end_row();

                            ui.label("Select all");
                            ui.label(egui::RichText::new("Ctrl+A").weak());
                            ui.end_row();

                            ui.label("Multi-select");
                            ui.label(egui::RichText::new("Ctrl+Click").weak());
                            ui.end_row();

                            ui.label("Delete selected");
                            ui.label(egui::RichText::new("Delete").weak());
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
                                egui::RichText::new("Numpad 1 / Double-click / Ctrl+B").weak(),
                            );
                            ui.end_row();

                            ui.label("Exit interaction");
                            ui.label(egui::RichText::new("Numpad 1 / Esc / click outside").weak());
                            ui.end_row();

                            ui.label(
                                egui::RichText::new("Numpad shortcuts need NumLock on").weak(),
                            );
                            ui.label("");
                            ui.end_row();

                            ui.add_space(10.0);
                            ui.end_row();

                            ui.label(egui::RichText::new("Other").strong());
                            ui.label("");
                            ui.end_row();

                            ui.label("Show this help");
                            ui.label(egui::RichText::new("F1").weak());
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
