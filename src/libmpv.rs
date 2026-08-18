//! In-process libmpv playback and OpenGL rendering.
//!
//! The old player path embedded a separate `mpv.exe` window and copied that
//! window back through Windows Graphics Capture. Besides an avoidable GPU to
//! CPU round-trip, that left playback control and the displayed frame in two
//! independent asynchronous systems. This module keeps decoding, audio,
//! controls, and drawing in one libmpv core per tile.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::{c_char, c_int, c_void, CStr, CString, OsString},
    path::{Path, PathBuf},
    ptr,
    sync::{
        mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use eframe::{egui, glow};
use glow::HasContext as _;
use libloading::Library;
use parking_lot::Mutex;

use crate::{
    preview::PreviewId,
    video::{
        self, LoopMode, TrackInfo, TrackSelection, VideoLaunch, VideoProperty, VideoState,
        VideoUpdate,
    },
};

type MpvHandle = c_void;
type MpvRenderContext = c_void;

#[repr(C)]
struct MpvEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct MpvEventProperty {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}

#[repr(C)]
union MpvNodeValue {
    string: *mut c_char,
    flag: c_int,
    int64: i64,
    double_value: f64,
    list: *mut MpvNodeList,
    byte_array: *mut c_void,
}

#[repr(C)]
struct MpvNode {
    value: MpvNodeValue,
    format: c_int,
}

#[repr(C)]
struct MpvNodeList {
    count: c_int,
    values: *mut MpvNode,
    keys: *mut *mut c_char,
}

#[repr(C)]
struct MpvRenderParam {
    kind: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct MpvOpenGlInitParams {
    get_proc_address: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void>,
    get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
struct MpvOpenGlFbo {
    fbo: c_int,
    w: c_int,
    h: c_int,
    internal_format: c_int,
}

const MPV_RENDER_PARAM_API_TYPE: c_int = 1;
const MPV_RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
const MPV_RENDER_PARAM_OPENGL_FBO: c_int = 3;
const MPV_RENDER_PARAM_FLIP_Y: c_int = 4;
const MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME: c_int = 12;
const MPV_RENDER_PARAM_SW_SIZE: c_int = 17;
const MPV_RENDER_PARAM_SW_FORMAT: c_int = 18;
const MPV_RENDER_PARAM_SW_STRIDE: c_int = 19;
const MPV_RENDER_PARAM_SW_POINTER: c_int = 20;
const MPV_RENDER_PARAM_INVALID: c_int = 0;
const MPV_RENDER_UPDATE_FRAME: u64 = 1;

const MPV_EVENT_NONE: c_int = 0;
const MPV_EVENT_SHUTDOWN: c_int = 1;
const MPV_EVENT_START_FILE: c_int = 6;
const MPV_EVENT_END_FILE: c_int = 7;
const MPV_EVENT_FILE_LOADED: c_int = 8;
const MPV_EVENT_SEEK: c_int = 20;
const MPV_EVENT_PLAYBACK_RESTART: c_int = 21;
const MPV_EVENT_PROPERTY_CHANGE: c_int = 22;

const SEEK_PREVIEW_WIDTH: i32 = 320;
const SEEK_PREVIEW_HEIGHT: i32 = 180;
const SEEK_PREVIEW_CACHE_LEN: usize = 20;
const SEEK_PREVIEW_COLD_DEBOUNCE: Duration = Duration::from_millis(140);
const SEEK_PREVIEW_WARM_DEBOUNCE: Duration = Duration::from_millis(50);
const SEEK_PREVIEW_PREFETCH_OFFSETS: [i64; 4] = [-1, 1, -2, 2];
const SEEK_PREVIEW_PREFETCH_GRACE: Duration = Duration::from_millis(250);
const SEEK_PREVIEW_MATCH_SECS: f64 = 0.5;

const MPV_FORMAT_NONE: c_int = 0;
const MPV_FORMAT_STRING: c_int = 1;
const MPV_FORMAT_FLAG: c_int = 3;
const MPV_FORMAT_INT64: c_int = 4;
const MPV_FORMAT_DOUBLE: c_int = 5;
const MPV_FORMAT_NODE: c_int = 6;
const MPV_FORMAT_NODE_ARRAY: c_int = 7;
const MPV_FORMAT_NODE_MAP: c_int = 8;

const OBSERVED_PROPERTIES: [(&str, c_int); 15] = [
    ("pause", MPV_FORMAT_FLAG),
    ("time-pos", MPV_FORMAT_DOUBLE),
    ("duration", MPV_FORMAT_DOUBLE),
    ("volume", MPV_FORMAT_DOUBLE),
    ("mute", MPV_FORMAT_FLAG),
    ("speed", MPV_FORMAT_DOUBLE),
    ("loop-file", MPV_FORMAT_STRING),
    ("track-list", MPV_FORMAT_NODE),
    ("aid", MPV_FORMAT_STRING),
    ("sid", MPV_FORMAT_STRING),
    ("media-title", MPV_FORMAT_STRING),
    ("paused-for-cache", MPV_FORMAT_FLAG),
    ("core-idle", MPV_FORMAT_FLAG),
    ("eof-reached", MPV_FORMAT_FLAG),
    ("seekable", MPV_FORMAT_FLAG),
];

struct MpvApi {
    _library: Library,
    create: unsafe extern "C" fn() -> *mut MpvHandle,
    initialize: unsafe extern "C" fn(*mut MpvHandle) -> c_int,
    terminate_destroy: unsafe extern "C" fn(*mut MpvHandle),
    set_option_string: unsafe extern "C" fn(*mut MpvHandle, *const c_char, *const c_char) -> c_int,
    set_property_string:
        unsafe extern "C" fn(*mut MpvHandle, *const c_char, *const c_char) -> c_int,
    get_property: unsafe extern "C" fn(*mut MpvHandle, *const c_char, c_int, *mut c_void) -> c_int,
    #[cfg(test)]
    get_property_string: unsafe extern "C" fn(*mut MpvHandle, *const c_char) -> *mut c_char,
    command: unsafe extern "C" fn(*mut MpvHandle, *const *const c_char) -> c_int,
    wait_event: unsafe extern "C" fn(*mut MpvHandle, f64) -> *mut MpvEvent,
    observe_property: unsafe extern "C" fn(*mut MpvHandle, u64, *const c_char, c_int) -> c_int,
    #[cfg(test)]
    free: unsafe extern "C" fn(*mut c_void),
    error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    render_context_create: unsafe extern "C" fn(
        *mut *mut MpvRenderContext,
        *mut MpvHandle,
        *mut MpvRenderParam,
    ) -> c_int,
    render_context_update: unsafe extern "C" fn(*mut MpvRenderContext) -> u64,
    render_context_render:
        unsafe extern "C" fn(*mut MpvRenderContext, *mut MpvRenderParam) -> c_int,
    render_context_free: unsafe extern "C" fn(*mut MpvRenderContext),
}

unsafe impl Send for MpvApi {}
unsafe impl Sync for MpvApi {}

impl MpvApi {
    fn load(mpv_path: &Path) -> Result<Arc<Self>, String> {
        let path = find_libmpv(mpv_path).ok_or_else(|| {
            "libmpv-2.dll is missing. Reinstall Pluriview or place libmpv-2.dll beside pluriview.exe."
                .to_owned()
        })?;
        let library = unsafe { Library::new(&path) }
            .map_err(|error| format!("Could not load {}: {error}", path.display()))?;

        unsafe fn symbol<T: Copy>(library: &Library, name: &[u8]) -> Result<T, String> {
            library
                .get::<T>(name)
                .map(|symbol| *symbol)
                .map_err(|error| {
                    format!(
                        "The bundled libmpv runtime is missing {}: {error}",
                        String::from_utf8_lossy(name).trim_end_matches('\0')
                    )
                })
        }

        // Load every entry point before moving the Library into the owner.
        let create = unsafe { symbol(&library, b"mpv_create\0")? };
        let initialize = unsafe { symbol(&library, b"mpv_initialize\0")? };
        let terminate_destroy = unsafe { symbol(&library, b"mpv_terminate_destroy\0")? };
        let set_option_string = unsafe { symbol(&library, b"mpv_set_option_string\0")? };
        let set_property_string = unsafe { symbol(&library, b"mpv_set_property_string\0")? };
        let get_property = unsafe { symbol(&library, b"mpv_get_property\0")? };
        #[cfg(test)]
        let get_property_string = unsafe { symbol(&library, b"mpv_get_property_string\0")? };
        let command = unsafe { symbol(&library, b"mpv_command\0")? };
        let wait_event = unsafe { symbol(&library, b"mpv_wait_event\0")? };
        let observe_property = unsafe { symbol(&library, b"mpv_observe_property\0")? };
        #[cfg(test)]
        let free = unsafe { symbol(&library, b"mpv_free\0")? };
        let error_string = unsafe { symbol(&library, b"mpv_error_string\0")? };
        let render_context_create = unsafe { symbol(&library, b"mpv_render_context_create\0")? };
        let render_context_update = unsafe { symbol(&library, b"mpv_render_context_update\0")? };
        let render_context_render = unsafe { symbol(&library, b"mpv_render_context_render\0")? };
        let render_context_free = unsafe { symbol(&library, b"mpv_render_context_free\0")? };

        Ok(Arc::new(Self {
            _library: library,
            create,
            initialize,
            terminate_destroy,
            set_option_string,
            set_property_string,
            get_property,
            #[cfg(test)]
            get_property_string,
            command,
            wait_event,
            observe_property,
            #[cfg(test)]
            free,
            error_string,
            render_context_create,
            render_context_update,
            render_context_render,
            render_context_free,
        }))
    }

    fn error(&self, code: c_int) -> String {
        let text = unsafe { (self.error_string)(code) };
        if text.is_null() {
            format!("libmpv error {code}")
        } else {
            unsafe { CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned()
        }
    }
}

fn find_libmpv(mpv_path: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            candidates.push(directory.join("libmpv-2.dll"));
        }
    }
    if let Ok(directory) = std::env::current_dir() {
        candidates.push(directory.join("vendor").join("libmpv-2.dll"));
        candidates.push(directory.join("libmpv-2.dll"));
    }
    if let Some(directory) = mpv_path.parent() {
        candidates.push(directory.join("libmpv-2.dll"));
    }
    candidates.into_iter().find(|path| path.is_file())
}

#[derive(Default)]
struct EventChanges {
    any: bool,
    pause: bool,
}

unsafe fn apply_property_event(state: &mut VideoState, property: &MpvEventProperty) -> bool {
    if property.name.is_null() {
        return false;
    }
    let name = CStr::from_ptr(property.name).to_string_lossy();
    if property.format == MPV_FORMAT_NONE || property.data.is_null() {
        match name.as_ref() {
            "time-pos" => state.time_pos = None,
            "duration" => state.duration = None,
            "media-title" => state.media_title = None,
            "track-list" => state.track_list.clear(),
            "aid" => state.audio_track = TrackSelection::Disabled,
            "sid" => state.subtitle_track = TrackSelection::Disabled,
            _ => return false,
        }
        return true;
    }

    match name.as_ref() {
        "pause" => event_flag(property).map(|value| state.pause = value),
        "time-pos" => event_double(property).map(|value| state.time_pos = Some(value)),
        "duration" => event_double(property).map(|value| state.duration = Some(value)),
        "volume" => event_double(property).map(|value| state.volume = value),
        "mute" => event_flag(property).map(|value| state.mute = value),
        "speed" => event_double(property).map(|value| state.speed = value),
        "loop-file" => event_string(property).map(|value| {
            state.loop_file = match value.as_str() {
                "inf" | "yes" => LoopMode::Infinite,
                _ => value
                    .parse::<u64>()
                    .ok()
                    .filter(|count| *count > 0)
                    .map(LoopMode::Count)
                    .unwrap_or(LoopMode::Off),
            }
        }),
        "track-list" => event_node(property).map(|node| {
            state.track_list = parse_track_list_node(node);
        }),
        "aid" => event_string(property)
            .map(|value| state.audio_track = parse_track_selection(Some(value))),
        "sid" => event_string(property)
            .map(|value| state.subtitle_track = parse_track_selection(Some(value))),
        "media-title" => event_string(property).map(|value| state.media_title = Some(value)),
        "paused-for-cache" => event_flag(property).map(|value| state.paused_for_cache = value),
        "core-idle" => event_flag(property).map(|value| state.core_idle = value),
        "eof-reached" => event_flag(property).map(|value| state.eof_reached = value),
        "seekable" => event_flag(property).map(|value| state.seekable = value),
        _ => None,
    }
    .is_some()
}

unsafe fn event_flag(property: &MpvEventProperty) -> Option<bool> {
    (property.format == MPV_FORMAT_FLAG).then(|| *property.data.cast::<c_int>() != 0)
}

unsafe fn event_double(property: &MpvEventProperty) -> Option<f64> {
    (property.format == MPV_FORMAT_DOUBLE).then(|| *property.data.cast::<f64>())
}

unsafe fn event_string(property: &MpvEventProperty) -> Option<String> {
    if property.format != MPV_FORMAT_STRING {
        return None;
    }
    let value = *property.data.cast::<*mut c_char>();
    (!value.is_null()).then(|| CStr::from_ptr(value).to_string_lossy().into_owned())
}

unsafe fn event_node(property: &MpvEventProperty) -> Option<&MpvNode> {
    (property.format == MPV_FORMAT_NODE).then(|| &*property.data.cast::<MpvNode>())
}

unsafe fn parse_track_list_node(node: &MpvNode) -> Vec<TrackInfo> {
    if node.format != MPV_FORMAT_NODE_ARRAY || node.value.list.is_null() {
        return Vec::new();
    }
    let list = &*node.value.list;
    if list.count <= 0 || list.values.is_null() {
        return Vec::new();
    }
    (0..list.count.min(128) as usize)
        .filter_map(|index| {
            let track = &*list.values.add(index);
            let id = node_map_value(track, "id").and_then(|node| node_i64(node))?;
            Some(TrackInfo {
                id,
                kind: node_map_value(track, "type")
                    .and_then(|node| node_string(node))
                    .unwrap_or_default(),
                title: node_map_value(track, "title").and_then(|node| node_string(node)),
                lang: node_map_value(track, "lang").and_then(|node| node_string(node)),
                selected: node_map_value(track, "selected")
                    .and_then(|node| node_flag(node))
                    .unwrap_or(false),
                external: node_map_value(track, "external")
                    .and_then(|node| node_flag(node))
                    .unwrap_or(false),
                codec: node_map_value(track, "codec").and_then(|node| node_string(node)),
            })
        })
        .collect()
}

unsafe fn node_map_value<'a>(node: &'a MpvNode, key: &str) -> Option<&'a MpvNode> {
    if node.format != MPV_FORMAT_NODE_MAP || node.value.list.is_null() {
        return None;
    }
    let list = &*node.value.list;
    if list.count <= 0 || list.values.is_null() || list.keys.is_null() {
        return None;
    }
    for index in 0..list.count as usize {
        let candidate = *list.keys.add(index);
        if !candidate.is_null() && CStr::from_ptr(candidate).to_bytes() == key.as_bytes() {
            return Some(&*list.values.add(index));
        }
    }
    None
}

unsafe fn node_i64(node: &MpvNode) -> Option<i64> {
    match node.format {
        MPV_FORMAT_INT64 => Some(node.value.int64),
        MPV_FORMAT_DOUBLE => Some(node.value.double_value as i64),
        _ => None,
    }
}

unsafe fn node_flag(node: &MpvNode) -> Option<bool> {
    (node.format == MPV_FORMAT_FLAG).then(|| node.value.flag != 0)
}

unsafe fn node_string(node: &MpvNode) -> Option<String> {
    if node.format != MPV_FORMAT_STRING || node.value.string.is_null() {
        None
    } else {
        Some(
            CStr::from_ptr(node.value.string)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

struct GlTarget {
    framebuffer: glow::NativeFramebuffer,
    texture: glow::NativeTexture,
    width: i32,
    height: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BlitDestination {
    left: i32,
    bottom: i32,
    right: i32,
    top: i32,
}

fn video_blit_destination(
    viewport: egui::Rect,
    pixels_per_point: f32,
    screen_height: i32,
) -> BlitDestination {
    let left = (viewport.min.x * pixels_per_point).round() as i32;
    let top_from_screen = (viewport.min.y * pixels_per_point).round() as i32;
    let right = (viewport.max.x * pixels_per_point).round() as i32;
    let bottom_from_screen = (viewport.max.y * pixels_per_point).round() as i32;
    let bottom = screen_height - bottom_from_screen;
    BlitDestination {
        left,
        bottom,
        right,
        top: bottom + (bottom_from_screen - top_from_screen),
    }
}

fn flip_rgba_rows(rgba: &mut [u8], width: usize, height: usize) {
    let Some(stride) = width.checked_mul(4) else {
        return;
    };
    if rgba.len() != stride.saturating_mul(height) {
        return;
    }
    for row in 0..(height / 2) {
        let opposite = height - 1 - row;
        let top = row * stride;
        let bottom = opposite * stride;
        for column in 0..stride {
            rgba.swap(top + column, bottom + column);
        }
    }
}

struct MpvCore {
    api: Arc<MpvApi>,
    handle: *mut MpvHandle,
    render_context: *mut MpvRenderContext,
    target: Option<GlTarget>,
    has_rendered_frame: bool,
    pending_source: Option<OsString>,
    source_loaded: bool,
    file_loaded: bool,
    wallpaper: bool,
    destroyed: bool,
    error: Option<String>,
}

// libmpv's client API is thread safe. Access to each handle and render context
// is additionally serialized by VideoRenderer's mutex; all GL work is invoked
// by egui on the one context-owning paint thread.
unsafe impl Send for MpvCore {}

impl MpvCore {
    fn new(
        api: Arc<MpvApi>,
        start_paused: bool,
        network_source: bool,
        wallpaper: bool,
    ) -> Result<Self, String> {
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err("libmpv could not create a playback core".to_owned());
        }

        let mut core = Self {
            api,
            handle,
            render_context: ptr::null_mut(),
            target: None,
            has_rendered_frame: false,
            pending_source: None,
            source_loaded: false,
            file_loaded: false,
            wallpaper,
            destroyed: false,
            error: None,
        };
        for (name, value) in [
            ("config", "no"),
            ("terminal", "no"),
            ("osc", "no"),
            ("input-default-bindings", "no"),
            ("keep-open", "yes"),
            ("vo", "libmpv"),
            // The WGL render context cannot directly import D3D11 surfaces,
            // but copy-mode hardware decoding is compatible and substantially
            // lowers CPU use compared with forcing software decoding.
            ("hwdec", "auto-copy-safe"),
        ] {
            if let Err(error) = core.set_option(name, value) {
                core.destroy_without_render_context();
                return Err(error);
            }
        }
        if wallpaper {
            // Background video should fill its rectangle, loop, and never
            // touch the user's audio devices. These are best-effort: a
            // rejected option must not prevent the wallpaper from starting.
            for (name, value) in [
                ("loop-file", "inf"),
                ("mute", "yes"),
                ("ao", "null"),
                ("aid", "no"),
                ("sid", "no"),
                ("panscan", "1.0"),
            ] {
                if let Err(error) = core.set_option(name, value) {
                    log::warn!("Wallpaper mpv option {name}={value} was rejected: {error}");
                }
            }
        }
        if network_source {
            // Give network streams enough forward data to ride through normal
            // HLS/CDN jitter. These limits are maxima, not eager allocations.
            for (name, value) in [
                ("cache", "yes"),
                ("cache-secs", "30"),
                ("demuxer-readahead-secs", "30"),
                ("demuxer-max-bytes", "128MiB"),
                ("demuxer-max-back-bytes", "16MiB"),
                ("cache-pause", "yes"),
                ("cache-pause-wait", "2"),
                ("cache-pause-initial", "yes"),
            ] {
                if let Err(error) = core.set_option(name, value) {
                    core.destroy_without_render_context();
                    return Err(error);
                }
            }
        }
        let result = unsafe { (core.api.initialize)(core.handle) };
        if result < 0 {
            let error = core.api.error(result);
            core.destroy_without_render_context();
            return Err(format!("Could not initialize libmpv: {error}"));
        }
        if let Err(error) = core.observe_properties() {
            core.destroy_without_render_context();
            return Err(error);
        }
        core.set_property("pause", if start_paused { "yes" } else { "no" })?;
        if wallpaper {
            let _ = core.set_property("loop-file", "inf");
            let _ = core.set_property("mute", "yes");
            let _ = core.set_property("panscan", "1.0");
            let _ = core.set_property("aid", "no");
            let _ = core.set_property("sid", "no");
        }
        Ok(core)
    }

    fn observe_properties(&self) -> Result<(), String> {
        for (name, format) in OBSERVED_PROPERTIES {
            let name = cstring(name)?;
            let result =
                unsafe { (self.api.observe_property)(self.handle, 0, name.as_ptr(), format) };
            self.check(result, "observe an mpv property")?;
        }
        Ok(())
    }

    fn set_option(&self, name: &str, value: &str) -> Result<(), String> {
        let name = cstring(name)?;
        let value = cstring(value)?;
        let result =
            unsafe { (self.api.set_option_string)(self.handle, name.as_ptr(), value.as_ptr()) };
        self.check(result, "set an mpv option")
    }

    fn set_property(&self, name: &str, value: &str) -> Result<(), String> {
        if self.destroyed {
            return Err("The video session has already closed".to_owned());
        }
        let name = cstring(name)?;
        let value = cstring(value)?;
        let result =
            unsafe { (self.api.set_property_string)(self.handle, name.as_ptr(), value.as_ptr()) };
        self.check(result, "change an mpv property")
    }

    fn command(&self, arguments: &[String]) -> Result<(), String> {
        if self.destroyed {
            return Err("The video session has already closed".to_owned());
        }
        let strings = arguments
            .iter()
            .map(|argument| cstring(argument))
            .collect::<Result<Vec<_>, _>>()?;
        let mut pointers = strings
            .iter()
            .map(|argument| argument.as_ptr())
            .collect::<Vec<_>>();
        pointers.push(ptr::null());
        let result = unsafe { (self.api.command)(self.handle, pointers.as_ptr()) };
        self.check(result, "run an mpv command")
    }

    fn check(&self, result: c_int, action: &str) -> Result<(), String> {
        if result < 0 {
            Err(format!("Could not {action}: {}", self.api.error(result)))
        } else {
            Ok(())
        }
    }

    fn load_source(&mut self, source: OsString) -> Result<(), String> {
        self.pending_source = Some(source);
        self.source_loaded = false;
        self.file_loaded = false;
        self.has_rendered_frame = false;
        if !self.render_context.is_null() {
            self.start_pending_source()?;
        }
        Ok(())
    }

    fn start_pending_source(&mut self) -> Result<(), String> {
        let Some(source) = self.pending_source.take() else {
            return Ok(());
        };
        let source = source.to_string_lossy().into_owned();
        self.command(&["loadfile".to_owned(), source, "replace".to_owned()])?;
        self.source_loaded = true;
        Ok(())
    }

    #[cfg(test)]
    fn property(&self, name: &str) -> Option<String> {
        if self.destroyed {
            return None;
        }
        let name = CString::new(name).ok()?;
        let value = unsafe { (self.api.get_property_string)(self.handle, name.as_ptr()) };
        if value.is_null() {
            return None;
        }
        let result = unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned();
        unsafe { (self.api.free)(value.cast()) };
        Some(result)
    }

    fn drain_events(&mut self, state: &mut VideoState) -> EventChanges {
        let mut changes = EventChanges::default();
        for _ in 0..256 {
            let event = unsafe { (self.api.wait_event)(self.handle, 0.0) };
            if event.is_null() {
                break;
            }
            match unsafe { (*event).event_id } {
                MPV_EVENT_NONE => break,
                MPV_EVENT_SHUTDOWN => {
                    self.error = Some("The libmpv playback core shut down unexpectedly".to_owned());
                    break;
                }
                MPV_EVENT_START_FILE => {
                    changes.any = true;
                }
                MPV_EVENT_END_FILE => {
                    changes.any = true;
                    if self.wallpaper && !self.file_loaded {
                        self.error = Some("The wallpaper video could not be opened".to_owned());
                    }
                }
                MPV_EVENT_FILE_LOADED => {
                    self.file_loaded = true;
                    changes.any = true;
                }
                MPV_EVENT_PROPERTY_CHANGE => {
                    let property = unsafe { (*event).data.cast::<MpvEventProperty>().as_ref() };
                    if let Some(property) = property {
                        if unsafe { apply_property_event(state, property) } {
                            changes.any = true;
                            changes.pause |= unsafe {
                                !property.name.is_null()
                                    && CStr::from_ptr(property.name).to_bytes() == b"pause"
                            };
                        }
                    }
                }
                _ => {}
            }
        }
        changes
    }

    unsafe fn ensure_render_context(&mut self) -> Result<(), String> {
        if !self.render_context.is_null() {
            return Ok(());
        }
        let api_type = b"opengl\0";
        let mut init = MpvOpenGlInitParams {
            get_proc_address: Some(get_gl_proc_address),
            get_proc_address_ctx: ptr::null_mut(),
        };
        let mut params = [
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_API_TYPE,
                data: api_type.as_ptr() as *mut c_void,
            },
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                data: (&mut init as *mut MpvOpenGlInitParams).cast(),
            },
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];
        let result = (self.api.render_context_create)(
            &mut self.render_context,
            self.handle,
            params.as_mut_ptr(),
        );
        self.check(result, "create the libmpv OpenGL renderer")?;
        self.start_pending_source()
    }

    unsafe fn ensure_target(
        &mut self,
        gl: &glow::Context,
        width: i32,
        height: i32,
    ) -> Result<bool, String> {
        if self
            .target
            .as_ref()
            .is_some_and(|target| target.width == width && target.height == height)
        {
            return Ok(false);
        }
        if let Some(target) = self.target.take() {
            gl.delete_framebuffer(target.framebuffer);
            gl.delete_texture(target.texture);
        }
        let texture = gl
            .create_texture()
            .map_err(|error| format!("Could not create the video texture: {error}"))?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            width,
            height,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            None,
        );
        let framebuffer = gl
            .create_framebuffer()
            .map_err(|error| format!("Could not create the video framebuffer: {error}"))?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(texture),
            0,
        );
        if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
            gl.delete_framebuffer(framebuffer);
            gl.delete_texture(texture);
            return Err("The OpenGL video framebuffer is incomplete".to_owned());
        }
        self.target = Some(GlTarget {
            framebuffer,
            texture,
            width,
            height,
        });
        Ok(true)
    }

    unsafe fn render_to_fbo(
        &mut self,
        gl: &glow::Context,
        width: i32,
        height: i32,
    ) -> Result<(), String> {
        if self.destroyed {
            return Ok(());
        }
        let width = width.max(1);
        let height = height.max(1);
        let previous_draw = gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING);
        let previous_read = gl.get_parameter_framebuffer(glow::READ_FRAMEBUFFER_BINDING);
        let mut previous_viewport = [0; 4];
        gl.get_parameter_i32_slice(glow::VIEWPORT, &mut previous_viewport);
        let scissor_enabled = gl.is_enabled(glow::SCISSOR_TEST);
        self.ensure_render_context()?;
        let target_changed = self.ensure_target(gl, width, height)?;
        let target = self.target.as_ref().expect("target was just created");

        let mut fbo = MpvOpenGlFbo {
            fbo: target.framebuffer.0.get() as c_int,
            w: target.width,
            h: target.height,
            internal_format: glow::RGBA8 as c_int,
        };
        let mut flip_y: c_int = 1;
        let mut block_for_target_time: c_int = 0;
        let mut params = [
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_OPENGL_FBO,
                data: (&mut fbo as *mut MpvOpenGlFbo).cast(),
            },
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_FLIP_Y,
                data: (&mut flip_y as *mut c_int).cast(),
            },
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME,
                data: (&mut block_for_target_time as *mut c_int).cast(),
            },
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];
        // egui_glow sets the window viewport to the callback rect. libmpv
        // must render into our FBO at (0,0), not into that window rectangle.
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target.framebuffer));
        gl.viewport(0, 0, target.width, target.height);
        if scissor_enabled {
            gl.disable(glow::SCISSOR_TEST);
        }
        let update = (self.api.render_context_update)(self.render_context);
        let render_result = if target_changed
            || !self.has_rendered_frame
            || update & MPV_RENDER_UPDATE_FRAME != 0
        {
            let result = (self.api.render_context_render)(self.render_context, params.as_mut_ptr());
            let checked = self.check(result, "render the video frame");
            if checked.is_ok() {
                self.has_rendered_frame = true;
            }
            checked
        } else {
            Ok(())
        };
        if scissor_enabled {
            gl.enable(glow::SCISSOR_TEST);
        }
        gl.viewport(
            previous_viewport[0],
            previous_viewport[1],
            previous_viewport[2],
            previous_viewport[3],
        );
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, previous_draw);
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, previous_read);
        render_result
    }

    unsafe fn paint(
        &mut self,
        info: egui::PaintCallbackInfo,
        gl: &glow::Context,
    ) -> Result<(), String> {
        if self.destroyed {
            return Ok(());
        }
        // PaintCallbackInfo's convenience viewport is clamped to the screen.
        // Using it as the render-target size made the FBO resize for every
        // pixel of movement across a screen edge. Keep a stable full-tile
        // target instead and let OpenGL clip the destination blit.
        let pixels_per_point = info.pixels_per_point;
        let destination = video_blit_destination(
            info.viewport,
            pixels_per_point,
            info.screen_size_px[1] as i32,
        );
        let viewport_width = destination.right - destination.left;
        let viewport_height = destination.top - destination.bottom;
        if viewport_width <= 0 || viewport_height <= 0 {
            return Ok(());
        }

        // During an interactive resize, only reallocate at 32-pixel steps.
        // The final blit scales the cached frame to the exact tile rectangle.
        let target_width = ((viewport_width + 31) / 32) * 32;
        let target_height = ((viewport_height + 31) / 32) * 32;

        self.render_to_fbo(gl, target_width, target_height)?;
        let Some(target) = self.target.as_ref() else {
            return Ok(());
        };
        let previous_draw = gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING);
        let previous_read = gl.get_parameter_framebuffer(glow::READ_FRAMEBUFFER_BINDING);

        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(target.framebuffer));
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, previous_draw);
        let clip = info.clip_rect_in_pixels();
        let screen_w = info.screen_size_px[0] as i32;
        let screen_h = info.screen_size_px[1] as i32;
        let scissor_x = clip.left_px.clamp(0, screen_w);
        let scissor_y = clip.from_bottom_px.clamp(0, screen_h);
        let scissor_w = clip.width_px.min(screen_w.saturating_sub(scissor_x)).max(0);
        let scissor_h = clip
            .height_px
            .min(screen_h.saturating_sub(scissor_y))
            .max(0);
        if scissor_w > 0 && scissor_h > 0 {
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(scissor_x, scissor_y, scissor_w, scissor_h);
            // Keep the full, potentially offscreen destination rectangle.
            // OpenGL clips it against the framebuffer/scissor while retaining
            // the original source-to-destination scale. Clamping these edges
            // first squashed the whole video into its visible remainder.
            gl.blit_framebuffer(
                0,
                0,
                target.width,
                target.height,
                destination.left,
                destination.bottom,
                destination.right,
                destination.top,
                glow::COLOR_BUFFER_BIT,
                glow::LINEAR,
            );
        }
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, previous_read);
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, previous_draw);
        Ok(())
    }

    unsafe fn snapshot(&self, gl: &glow::Context) -> Option<VideoSnapshot> {
        if self.destroyed || !self.has_rendered_frame {
            return None;
        }
        let target = self.target.as_ref()?;
        let width = usize::try_from(target.width).ok()?;
        let height = usize::try_from(target.height).ok()?;
        let byte_len = width.checked_mul(height)?.checked_mul(4)?;
        let mut rgba = vec![0; byte_len];

        let previous_read = gl.get_parameter_framebuffer(glow::READ_FRAMEBUFFER_BINDING);
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(target.framebuffer));
        gl.read_pixels(
            0,
            0,
            target.width,
            target.height,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(&mut rgba),
        );
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, previous_read);

        // OpenGL readback starts at the bottom row; egui ColorImage expects
        // the first row to be the top of the image.
        flip_rgba_rows(&mut rgba, width, height);

        Some(VideoSnapshot {
            width: target.width as u32,
            height: target.height as u32,
            rgba,
        })
    }

    unsafe fn cleanup(&mut self, gl: &glow::Context) {
        if self.destroyed {
            return;
        }
        if let Some(target) = self.target.take() {
            gl.delete_framebuffer(target.framebuffer);
            gl.delete_texture(target.texture);
        }
        if !self.render_context.is_null() {
            (self.api.render_context_free)(self.render_context);
            self.render_context = ptr::null_mut();
        }
        (self.api.terminate_destroy)(self.handle);
        self.handle = ptr::null_mut();
        self.destroyed = true;
    }

    fn destroy_without_render_context(&mut self) {
        if !self.destroyed && self.render_context.is_null() && !self.handle.is_null() {
            unsafe { (self.api.terminate_destroy)(self.handle) };
            self.handle = ptr::null_mut();
            self.destroyed = true;
        }
    }
}

/// Paintable handle retained by the Preview and its controlling session.
pub struct VideoRenderer {
    core: Mutex<MpvCore>,
}

impl VideoRenderer {
    fn new(
        api: Arc<MpvApi>,
        start_paused: bool,
        network_source: bool,
        wallpaper: bool,
    ) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            core: Mutex::new(MpvCore::new(api, start_paused, network_source, wallpaper)?),
        }))
    }

    pub fn paint(&self, info: egui::PaintCallbackInfo, gl: &glow::Context) {
        let mut core = self.core.lock();
        let result = unsafe { core.paint(info, gl) };
        if let Err(error) = result {
            if core.error.as_deref() != Some(&error) {
                log::error!("Direct libmpv rendering failed: {error}");
            }
            core.error = Some(error);
        }
    }

    pub fn snapshot(&self, gl: &glow::Context) -> Option<VideoSnapshot> {
        let core = self.core.lock();
        unsafe { core.snapshot(gl) }
    }

    fn load_source(&self, source: OsString) -> Result<(), String> {
        self.core.lock().load_source(source)
    }

    fn stop(&self) {
        let core = self.core.lock();
        let _ = core.command(&["stop".to_owned()]);
        let _ = core.set_property("pause", "yes");
    }

    fn cleanup(&self, gl: &glow::Context) {
        unsafe { self.core.lock().cleanup(gl) };
    }
}

/// A one-frame CPU copy used while replacing an unloaded libmpv tile with an
/// ordinary lightweight egui texture.
pub struct VideoSnapshot {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct VideoSession {
    renderer: Arc<VideoRenderer>,
    state: VideoState,
    first_poll: bool,
    stream_receiver: Option<mpsc::Receiver<Result<OsString, String>>>,
}

impl VideoSession {
    fn new(renderer: Arc<VideoRenderer>, launch: &VideoLaunch) -> Result<Self, String> {
        let stream_receiver = match &launch.source {
            video::VideoSource::LocalFile(path) => {
                renderer.load_source(path.as_os_str().to_owned())?;
                None
            }
            video::VideoSource::Stream {
                url,
                quality,
                streamlink_path,
            } => {
                let (sender, receiver) = mpsc::channel();
                let url = url.clone();
                let quality = quality.clone();
                let streamlink_path = streamlink_path.clone();
                thread::spawn(move || {
                    let result = video::resolve_stream_url(&streamlink_path, &url, &quality);
                    let _ = sender.send(result);
                });
                Some(receiver)
            }
        };
        Ok(Self {
            renderer,
            state: VideoState {
                connected: true,
                pause: launch.start_paused,
                volume: 100.0,
                speed: 1.0,
                core_idle: true,
                ..Default::default()
            },
            first_poll: true,
            stream_receiver,
        })
    }

    pub fn state(&self) -> &VideoState {
        &self.state
    }

    pub fn source_ready(&self) -> bool {
        self.stream_receiver.is_none() && self.renderer.core.lock().file_loaded
    }

    pub fn renderer(&self) -> Arc<VideoRenderer> {
        self.renderer.clone()
    }

    fn poll(&mut self) -> Vec<VideoUpdate> {
        let mut updates = Vec::new();
        if self.first_poll {
            self.first_poll = false;
            updates.push(VideoUpdate::Connected);
        }
        if let Some(receiver) = &self.stream_receiver {
            match receiver.try_recv() {
                Ok(Ok(source)) => {
                    self.stream_receiver = None;
                    if let Err(error) = self.renderer.load_source(source) {
                        updates.push(VideoUpdate::Error(error));
                    } else {
                        updates.push(VideoUpdate::Event);
                    }
                }
                Ok(Err(error)) => {
                    self.stream_receiver = None;
                    updates.push(VideoUpdate::Error(format!(
                        "Streamlink could not resolve this stream: {error}"
                    )));
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.stream_receiver = None;
                    updates.push(VideoUpdate::Error(
                        "Streamlink stopped before returning a playable URL".to_owned(),
                    ));
                }
            }
        }

        let mut core = self.renderer.core.lock();
        let changes = core.drain_events(&mut self.state);
        if let Some(error) = core.error.take() {
            updates.push(VideoUpdate::Error(error));
        }
        if changes.pause {
            updates.push(VideoUpdate::Property(VideoProperty::Pause));
        }
        if changes.any {
            updates.push(VideoUpdate::Event);
        }
        updates
    }

    pub fn set_paused(&mut self, paused: bool) -> Result<(), String> {
        self.renderer
            .core
            .lock()
            .set_property("pause", if paused { "yes" } else { "no" })?;
        self.state.pause = paused;
        Ok(())
    }

    pub fn play(&mut self) -> Result<(), String> {
        if self.state.eof_reached {
            self.seek_absolute(0.0)?;
        }
        self.set_paused(false)
    }

    /// Replace the current local file without recreating the libmpv core.
    /// Playback properties such as volume, speed, tracks, and the renderer
    /// remain attached to the same canvas tile.
    pub fn load_local_file(&mut self, path: &Path) -> Result<(), String> {
        if !path.is_file() {
            return Err(format!(
                "The video file no longer exists: {}",
                path.display()
            ));
        }
        self.renderer
            .core
            .lock()
            .load_source(path.as_os_str().to_os_string())?;
        self.state.time_pos = None;
        self.state.duration = None;
        self.state.eof_reached = false;
        self.state.core_idle = false;
        self.state.paused_for_cache = false;
        self.set_paused(false)
    }

    pub fn seek_absolute(&mut self, seconds: f64) -> Result<(), String> {
        if !seconds.is_finite() {
            return Err("Seek time must be finite".to_owned());
        }
        self.renderer.core.lock().command(&[
            "seek".to_owned(),
            seconds.max(0.0).to_string(),
            "absolute+exact".to_owned(),
        ])
    }

    pub fn set_volume(&mut self, volume: f64) -> Result<(), String> {
        if !volume.is_finite() {
            return Err("Volume must be finite".to_owned());
        }
        let volume = volume.clamp(0.0, 100.0);
        self.renderer
            .core
            .lock()
            .set_property("volume", &volume.to_string())?;
        self.state.volume = volume;
        Ok(())
    }

    pub fn set_muted(&mut self, muted: bool) -> Result<(), String> {
        self.renderer
            .core
            .lock()
            .set_property("mute", if muted { "yes" } else { "no" })?;
        self.state.mute = muted;
        Ok(())
    }

    pub fn set_speed(&mut self, speed: f64) -> Result<(), String> {
        if !speed.is_finite() || speed <= 0.0 {
            return Err("Playback speed must be a positive finite number".to_owned());
        }
        self.renderer
            .core
            .lock()
            .set_property("speed", &speed.to_string())?;
        self.state.speed = speed;
        Ok(())
    }

    pub fn set_looping(&mut self, enabled: bool) -> Result<(), String> {
        self.renderer
            .core
            .lock()
            .set_property("loop-file", if enabled { "inf" } else { "no" })?;
        self.state.loop_file = if enabled {
            LoopMode::Infinite
        } else {
            LoopMode::Off
        };
        Ok(())
    }

    /// Crop the video to fill its paint rectangle instead of letterboxing.
    pub fn set_fill_frame(&mut self, enabled: bool) -> Result<(), String> {
        self.renderer
            .core
            .lock()
            .set_property("panscan", if enabled { "1.0" } else { "0.0" })?;
        Ok(())
    }

    pub fn select_audio_track(&mut self, id: i64) -> Result<(), String> {
        self.renderer
            .core
            .lock()
            .set_property("aid", &id.to_string())?;
        self.state.audio_track = TrackSelection::Id(id);
        Ok(())
    }

    pub fn select_subtitle_track(&mut self, id: i64) -> Result<(), String> {
        self.renderer
            .core
            .lock()
            .set_property("sid", &id.to_string())?;
        self.state.subtitle_track = TrackSelection::Id(id);
        Ok(())
    }

    pub fn disable_subtitles(&mut self) -> Result<(), String> {
        self.renderer.core.lock().set_property("sid", "no")?;
        self.state.subtitle_track = TrackSelection::Disabled;
        Ok(())
    }
}

pub struct VideoTile {
    pub session: VideoSession,
    target_fps: u32,
}

#[derive(Default)]
pub struct VideoManager {
    api: Option<Arc<MpvApi>>,
    tiles: std::collections::HashMap<PreviewId, VideoTile>,
    cleanup_queue: Arc<Mutex<Vec<Arc<VideoRenderer>>>>,
}

impl VideoManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn launch(
        &mut self,
        id: PreviewId,
        launch: VideoLaunch,
        target_fps: u32,
    ) -> Result<Arc<VideoRenderer>, String> {
        if self.tiles.contains_key(&id) {
            return Err(format!("A video session already exists for {id:?}"));
        }
        if self.api.is_none() {
            self.api = Some(MpvApi::load(&launch.mpv_path)?);
        }
        let renderer = VideoRenderer::new(
            self.api.as_ref().expect("libmpv was loaded").clone(),
            launch.start_paused,
            matches!(&launch.source, video::VideoSource::Stream { .. }),
            launch.wallpaper,
        )?;
        let session = VideoSession::new(renderer.clone(), &launch)?;
        self.tiles.insert(
            id,
            VideoTile {
                session,
                target_fps: target_fps.clamp(1, 60),
            },
        );
        Ok(renderer)
    }

    pub fn contains(&self, id: PreviewId) -> bool {
        self.tiles.contains_key(&id)
    }

    pub fn set_paused(&mut self, id: PreviewId, paused: bool) -> Result<(), String> {
        let Some(tile) = self.tiles.get_mut(&id) else {
            return Ok(());
        };
        if tile.session.state.pause == paused {
            return Ok(());
        }
        tile.session.set_paused(paused)
    }

    pub fn repaint_fps(&self) -> Option<u32> {
        self.tiles
            .values()
            .filter(|tile| tile.session.state.connected && !tile.session.state.pause)
            .map(|tile| tile.target_fps)
            .max()
    }

    pub fn get(&self, id: PreviewId) -> Option<&VideoTile> {
        self.tiles.get(&id)
    }

    pub fn get_mut(&mut self, id: PreviewId) -> Option<&mut VideoTile> {
        self.tiles.get_mut(&id)
    }

    pub fn remove(&mut self, id: PreviewId) -> Option<VideoTile> {
        let tile = self.tiles.remove(&id)?;
        tile.session.renderer.stop();
        self.cleanup_queue
            .lock()
            .push(tile.session.renderer.clone());
        Some(tile)
    }

    pub fn clear(&mut self) {
        let ids = self.tiles.keys().copied().collect::<Vec<_>>();
        for id in ids {
            let _ = self.remove(id);
        }
    }

    pub fn ids(&self) -> impl Iterator<Item = PreviewId> + '_ {
        self.tiles.keys().copied()
    }

    pub fn poll(&mut self) -> Vec<(PreviewId, VideoUpdate)> {
        let mut updates = Vec::new();
        for (id, tile) in &mut self.tiles {
            updates.extend(tile.session.poll().into_iter().map(|update| (*id, update)));
        }
        updates
    }

    /// Read the renderer's current FBO after this frame's normal tile paint.
    /// Foreground ordering guarantees that a visible video gets its newest
    /// frame before the one-shot snapshot callback runs.
    pub fn schedule_snapshot(
        &self,
        id: PreviewId,
        ctx: &egui::Context,
    ) -> Option<mpsc::Receiver<Option<VideoSnapshot>>> {
        let renderer = self.tiles.get(&id)?.session.renderer.clone();
        let (sender, receiver) = mpsc::channel();
        let callback = egui::PaintCallback {
            rect: ctx.screen_rect(),
            callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |_info, painter| {
                let _ = sender.send(renderer.snapshot(painter.gl()));
            })),
        };
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new(("libmpv_snapshot", id.0)),
        ))
        .add(callback);
        Some(receiver)
    }

    /// Queue GL-safe destruction after paint callbacks for the current frame.
    pub fn schedule_cleanup(&self, ctx: &egui::Context) {
        if self.cleanup_queue.lock().is_empty() {
            return;
        }
        let queue = self.cleanup_queue.clone();
        let callback = egui::PaintCallback {
            rect: ctx.screen_rect(),
            callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |_info, painter| {
                let retired = std::mem::take(&mut *queue.lock());
                for renderer in retired {
                    renderer.cleanup(painter.gl());
                }
            })),
        };
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("libmpv_cleanup"),
        ))
        .add(callback);
    }

    pub fn cleanup_all(&mut self, gl: &glow::Context) {
        self.clear();
        let retired = std::mem::take(&mut *self.cleanup_queue.lock());
        for renderer in retired {
            renderer.cleanup(gl);
        }
        self.api = None;
    }
}

fn source_key(source: &video::VideoThumbnailSource) -> String {
    match source {
        video::VideoThumbnailSource::LocalFile(path) => format!("file:{}", path.display()),
        video::VideoThumbnailSource::Stream { url, quality, .. } => {
            format!("stream:{url}|{quality}")
        }
    }
}

fn rgb0_to_rgba(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
}

fn bucket_time(time: f64) -> f64 {
    time.round().max(0.0)
}

fn prefetch_times(center: f64, duration: Option<f64>) -> Vec<f64> {
    let center = center.round() as i64;
    SEEK_PREVIEW_PREFETCH_OFFSETS
        .iter()
        .filter_map(|offset| {
            let time = center.checked_add(*offset)?;
            if time < 0 {
                return None;
            }
            let time = time as f64;
            if duration.is_some_and(|duration| time > duration.max(0.0)) {
                return None;
            }
            Some(time)
        })
        .collect()
}

fn next_preview_time(
    requested: f64,
    duration: Option<f64>,
    cache: &SeekPreviewCache,
    prefetch: bool,
) -> Option<f64> {
    let requested = bucket_time(requested);
    if !cache.covered(requested) {
        return Some(requested);
    }
    if !prefetch {
        return None;
    }
    prefetch_times(requested, duration)
        .into_iter()
        .find(|time| !cache.covered(*time))
}

fn frame_matches_hover(frame_time: f64, hover_time: f64) -> bool {
    (frame_time - hover_time).abs() <= SEEK_PREVIEW_MATCH_SECS
}

#[derive(Default)]
struct SeekPreviewCache {
    frames: VecDeque<(i64, video::VideoThumbnail)>,
    misses: HashSet<i64>,
}

impl SeekPreviewCache {
    fn insert(&mut self, thumbnail: video::VideoThumbnail) {
        let key = thumbnail.time.round() as i64;
        self.misses.remove(&key);
        self.frames.retain(|(existing, _)| *existing != key);
        self.frames.push_back((key, thumbnail));
        while self.frames.len() > SEEK_PREVIEW_CACHE_LEN {
            self.frames.pop_front();
        }
    }

    fn insert_grab(&mut self, requested: f64, thumbnail: video::VideoThumbnail) {
        if !frame_matches_hover(thumbnail.time, requested) {
            self.misses.insert(requested.round() as i64);
        }
        self.insert(thumbnail);
    }

    fn get(&self, time: f64) -> Option<&video::VideoThumbnail> {
        let key = time.round() as i64;
        self.frames
            .iter()
            .rev()
            .find(|(existing, _)| *existing == key)
            .map(|(_, thumbnail)| thumbnail)
            .filter(|thumbnail| frame_matches_hover(thumbnail.time, time))
    }

    fn covered(&self, time: f64) -> bool {
        self.get(time).is_some() || self.misses.contains(&(time.round() as i64))
    }
}

enum SeekPreviewCommand {
    Grab(f64),
}

enum SeekPreviewResult {
    Ready,
    Frame(video::VideoThumbnail),
    Error(String),
}

enum GrabWait {
    Ready,
    Newer(f64),
    Shutdown,
}

struct ThumbnailCore {
    api: Arc<MpvApi>,
    handle: *mut MpvHandle,
    render_context: *mut MpvRenderContext,
    destroyed: bool,
}

unsafe impl Send for ThumbnailCore {}

impl ThumbnailCore {
    fn new(api: Arc<MpvApi>, network_source: bool) -> Result<Self, String> {
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err("libmpv could not create a timeline preview core".to_owned());
        }
        let mut core = Self {
            api,
            handle,
            render_context: ptr::null_mut(),
            destroyed: false,
        };
        for (name, value) in [
            ("config", "no"),
            ("terminal", "no"),
            ("osc", "no"),
            ("osd-level", "0"),
            ("input-default-bindings", "no"),
            ("load-scripts", "no"),
            ("keep-open", "yes"),
            ("idle", "yes"),
            ("pause", "yes"),
            ("vo", "libmpv"),
            ("ao", "null"),
            ("audio", "no"),
            ("aid", "no"),
            ("sid", "no"),
            ("sub", "no"),
            ("sub-auto", "no"),
            ("audio-file-auto", "no"),
            ("ytdl", "no"),
            ("hr-seek", "yes"),
            ("hr-seek-framedrop", "yes"),
            // Pin a single copy-mode decoder so the first grab does not probe
            // every safe backend. mpv falls back to software if this fails.
            ("hwdec", "d3d11va-copy"),
        ] {
            if let Err(error) = core.set_option(name, value) {
                core.destroy();
                return Err(error);
            }
        }
        if network_source {
            for (name, value) in [
                ("cache", "yes"),
                ("cache-secs", "8"),
                ("demuxer-readahead-secs", "2"),
                ("cache-pause", "no"),
            ] {
                if let Err(error) = core.set_option(name, value) {
                    core.destroy();
                    return Err(error);
                }
            }
        } else {
            for (name, value) in [("cache", "no"), ("demuxer-readahead-secs", "0.2")] {
                if let Err(error) = core.set_option(name, value) {
                    core.destroy();
                    return Err(error);
                }
            }
        }
        let result = unsafe { (core.api.initialize)(core.handle) };
        if result < 0 {
            let error = core.api.error(result);
            core.destroy();
            return Err(format!(
                "Could not initialize the timeline preview core: {error}"
            ));
        }
        if let Err(error) = core.ensure_sw_render_context() {
            core.destroy();
            return Err(error);
        }
        Ok(core)
    }

    fn set_option(&self, name: &str, value: &str) -> Result<(), String> {
        let name = cstring(name)?;
        let value = cstring(value)?;
        let result =
            unsafe { (self.api.set_option_string)(self.handle, name.as_ptr(), value.as_ptr()) };
        self.check(result, "set a timeline preview option")
    }

    fn set_property(&self, name: &str, value: &str) -> Result<(), String> {
        let name = cstring(name)?;
        let value = cstring(value)?;
        let result =
            unsafe { (self.api.set_property_string)(self.handle, name.as_ptr(), value.as_ptr()) };
        self.check(result, "change a timeline preview property")
    }

    fn command(&self, arguments: &[String]) -> Result<(), String> {
        let strings = arguments
            .iter()
            .map(|argument| cstring(argument))
            .collect::<Result<Vec<_>, _>>()?;
        let mut pointers = strings
            .iter()
            .map(|argument| argument.as_ptr())
            .collect::<Vec<_>>();
        pointers.push(ptr::null());
        let result = unsafe { (self.api.command)(self.handle, pointers.as_ptr()) };
        self.check(result, "run a timeline preview command")
    }

    fn check(&self, result: c_int, action: &str) -> Result<(), String> {
        if result < 0 {
            Err(format!("Could not {action}: {}", self.api.error(result)))
        } else {
            Ok(())
        }
    }

    fn ensure_sw_render_context(&mut self) -> Result<(), String> {
        if !self.render_context.is_null() {
            return Ok(());
        }
        let api_type = b"sw\0";
        let mut params = [
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_API_TYPE,
                data: api_type.as_ptr() as *mut c_void,
            },
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];
        let result = unsafe {
            (self.api.render_context_create)(
                &mut self.render_context,
                self.handle,
                params.as_mut_ptr(),
            )
        };
        self.check(result, "create the software timeline renderer")
    }

    fn load_source(&self, source: OsString, start: f64) -> Result<(), String> {
        if start > 0.05 {
            self.set_property("start", &format!("{start:.3}"))?;
        }
        let source = source.to_string_lossy().into_owned();
        self.command(&["loadfile".to_owned(), source, "replace".to_owned()])
    }

    fn seek_exact(&self, time: f64) -> Result<(), String> {
        self.command(&[
            "seek".to_owned(),
            time.max(0.0).to_string(),
            "absolute+exact".to_owned(),
        ])
    }

    fn property_f64(&self, name: &str) -> Option<f64> {
        let name = CString::new(name).ok()?;
        let mut value = 0.0_f64;
        let result = unsafe {
            (self.api.get_property)(
                self.handle,
                name.as_ptr(),
                MPV_FORMAT_DOUBLE,
                (&mut value as *mut f64).cast(),
            )
        };
        (result >= 0 && value.is_finite()).then_some(value)
    }

    fn wait_until_loaded(&mut self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .as_secs_f64();
            if remaining <= 0.0 {
                return Err("Timeline preview timed out".to_owned());
            }
            let event = unsafe { (self.api.wait_event)(self.handle, remaining.min(0.05)) };
            let event_id = if event.is_null() {
                MPV_EVENT_NONE
            } else {
                unsafe { (*event).event_id }
            };
            match event_id {
                MPV_EVENT_SHUTDOWN => {
                    return Err("The timeline preview core shut down unexpectedly".to_owned());
                }
                MPV_EVENT_END_FILE => {
                    return Err("The timeline preview file could not be opened".to_owned());
                }
                MPV_EVENT_FILE_LOADED | MPV_EVENT_PLAYBACK_RESTART => return Ok(()),
                _ => {}
            }
            if !self.render_context.is_null()
                && unsafe { (self.api.render_context_update)(self.render_context) }
                    & MPV_RENDER_UPDATE_FRAME
                    != 0
            {
                return Ok(());
            }
        }
    }

    fn wait_for_frame(
        &mut self,
        time: f64,
        timeout: Duration,
        commands: &Receiver<SeekPreviewCommand>,
    ) -> Result<GrabWait, String> {
        let mut seek_completed = false;
        self.wait_for(
            timeout,
            commands,
            true,
            |event_id, update| {
                if event_id == MPV_EVENT_SEEK {
                    return false;
                }
                if event_id == MPV_EVENT_PLAYBACK_RESTART || event_id == MPV_EVENT_FILE_LOADED {
                    seek_completed = true;
                }
                seek_completed
                    && (event_id == MPV_EVENT_PLAYBACK_RESTART
                        || event_id == MPV_EVENT_FILE_LOADED
                        || update & MPV_RENDER_UPDATE_FRAME != 0)
            },
            Some(time),
            false,
        )
    }

    fn time_matches(&self, time: f64) -> bool {
        self.property_f64("time-pos")
            .is_some_and(|actual| frame_matches_hover(actual, time))
    }

    fn wait_for(
        &mut self,
        timeout: Duration,
        commands: &Receiver<SeekPreviewCommand>,
        abort_on_grab: bool,
        mut ready: impl FnMut(c_int, u64) -> bool,
        target_time: Option<f64>,
        fail_on_timeout: bool,
    ) -> Result<GrabWait, String> {
        let deadline = Instant::now() + timeout;
        loop {
            match peek_grab(commands) {
                Some(GrabWait::Newer(time)) if abort_on_grab => return Ok(GrabWait::Newer(time)),
                Some(GrabWait::Newer(_)) => {}
                Some(GrabWait::Shutdown) => return Ok(GrabWait::Shutdown),
                Some(GrabWait::Ready) | None => {}
            }
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .as_secs_f64();
            if remaining <= 0.0 {
                return if fail_on_timeout {
                    Err("Timeline preview timed out".to_owned())
                } else {
                    Ok(GrabWait::Ready)
                };
            }
            let event = unsafe { (self.api.wait_event)(self.handle, remaining.min(0.05)) };
            let event_id = if event.is_null() {
                MPV_EVENT_NONE
            } else {
                unsafe { (*event).event_id }
            };
            match event_id {
                MPV_EVENT_SHUTDOWN => {
                    return Err("The timeline preview core shut down unexpectedly".to_owned());
                }
                MPV_EVENT_END_FILE if fail_on_timeout => {
                    return Err("The timeline preview file could not be opened".to_owned());
                }
                _ => {}
            }
            let update = if self.render_context.is_null() {
                0
            } else {
                unsafe { (self.api.render_context_update)(self.render_context) }
            };
            if ready(event_id, update) && target_time.is_none_or(|time| self.time_matches(time)) {
                return Ok(GrabWait::Ready);
            }
        }
    }

    fn render_frame(&mut self, time: f64) -> Result<video::VideoThumbnail, String> {
        let mut size = [SEEK_PREVIEW_WIDTH, SEEK_PREVIEW_HEIGHT];
        let format = b"rgb0\0";
        let mut stride: usize = (SEEK_PREVIEW_WIDTH as usize) * 4;
        let mut pixels = vec![0_u8; stride * SEEK_PREVIEW_HEIGHT as usize];
        let mut block_for_target_time: c_int = 1;
        let mut params = [
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_SW_SIZE,
                data: size.as_mut_ptr().cast(),
            },
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_SW_FORMAT,
                data: format.as_ptr() as *mut c_void,
            },
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_SW_STRIDE,
                data: (&mut stride as *mut usize).cast(),
            },
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_SW_POINTER,
                data: pixels.as_mut_ptr().cast(),
            },
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME,
                data: (&mut block_for_target_time as *mut c_int).cast(),
            },
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];
        let result =
            unsafe { (self.api.render_context_render)(self.render_context, params.as_mut_ptr()) };
        self.check(result, "render the timeline preview")?;
        rgb0_to_rgba(&mut pixels);
        let actual_time = self
            .property_f64("time-pos")
            .filter(|actual| actual.is_finite() && *actual >= 0.0)
            .unwrap_or(time);
        Ok(video::VideoThumbnail {
            time: actual_time,
            width: SEEK_PREVIEW_WIDTH as u32,
            height: SEEK_PREVIEW_HEIGHT as u32,
            rgba: pixels,
        })
    }

    fn grab(
        &mut self,
        mut time: f64,
        commands: &Receiver<SeekPreviewCommand>,
    ) -> Result<Option<video::VideoThumbnail>, String> {
        loop {
            self.seek_exact(time)?;
            match self.wait_for_frame(time, Duration::from_secs(2), commands)? {
                GrabWait::Newer(newer) => {
                    time = newer;
                    continue;
                }
                GrabWait::Shutdown => return Ok(None),
                GrabWait::Ready => {}
            }
            match peek_grab(commands) {
                Some(GrabWait::Newer(newer)) => {
                    time = newer;
                    continue;
                }
                Some(GrabWait::Shutdown) => return Ok(None),
                Some(GrabWait::Ready) | None => {}
            }
            return Ok(Some(self.render_frame(time)?));
        }
    }

    fn destroy(&mut self) {
        if self.destroyed {
            return;
        }
        if !self.render_context.is_null() {
            unsafe { (self.api.render_context_free)(self.render_context) };
            self.render_context = ptr::null_mut();
        }
        if !self.handle.is_null() {
            unsafe { (self.api.terminate_destroy)(self.handle) };
            self.handle = ptr::null_mut();
        }
        self.destroyed = true;
    }
}

impl Drop for ThumbnailCore {
    fn drop(&mut self) {
        self.destroy();
    }
}

fn peek_grab(commands: &Receiver<SeekPreviewCommand>) -> Option<GrabWait> {
    match commands.try_recv() {
        Ok(SeekPreviewCommand::Grab(time)) => {
            let mut latest = time;
            loop {
                match commands.try_recv() {
                    Ok(SeekPreviewCommand::Grab(time)) => latest = time,
                    Err(TryRecvError::Empty) => return Some(GrabWait::Newer(latest)),
                    Err(TryRecvError::Disconnected) => return Some(GrabWait::Shutdown),
                }
            }
        }
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => Some(GrabWait::Shutdown),
    }
}

fn coalesce_command(
    first: SeekPreviewCommand,
    commands: &Receiver<SeekPreviewCommand>,
) -> Result<SeekPreviewCommand, GrabWait> {
    let mut command = first;
    loop {
        match commands.try_recv() {
            Ok(SeekPreviewCommand::Grab(time)) => command = SeekPreviewCommand::Grab(time),
            Err(TryRecvError::Empty) => return Ok(command),
            Err(TryRecvError::Disconnected) => return Err(GrabWait::Shutdown),
        }
    }
}

fn resolve_thumbnail_source(source: &video::VideoThumbnailSource) -> Result<OsString, String> {
    match source {
        video::VideoThumbnailSource::LocalFile(path) => Ok(path.as_os_str().to_owned()),
        video::VideoThumbnailSource::Stream {
            streamlink_path,
            url,
            quality,
        } => video::resolve_stream_url(streamlink_path, url, quality),
    }
}

fn run_seek_preview_worker(
    api: Arc<MpvApi>,
    source: video::VideoThumbnailSource,
    start: f64,
    commands: Receiver<SeekPreviewCommand>,
    results: Sender<SeekPreviewResult>,
) {
    let network = matches!(source, video::VideoThumbnailSource::Stream { .. });
    let mut core = match ThumbnailCore::new(api, network) {
        Ok(core) => core,
        Err(error) => {
            let _ = results.send(SeekPreviewResult::Error(error));
            return;
        }
    };
    let opened = (|| {
        let path = resolve_thumbnail_source(&source)?;
        core.load_source(path, start)?;
        core.wait_until_loaded(if network {
            Duration::from_secs(30)
        } else {
            Duration::from_secs(12)
        })?;
        Ok(())
    })();
    match opened {
        Ok(()) => {
            if results.send(SeekPreviewResult::Ready).is_err() {
                return;
            }
        }
        Err(error) => {
            let _ = results.send(SeekPreviewResult::Error(error));
            return;
        }
    }

    loop {
        let command = match commands.recv_timeout(Duration::from_millis(50)) {
            Ok(command) => match coalesce_command(command, &commands) {
                Ok(command) => command,
                Err(GrabWait::Shutdown) => break,
                Err(_) => continue,
            },
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let SeekPreviewCommand::Grab(time) = command;
        match core.grab(time, &commands) {
            Ok(Some(frame)) => {
                if results.send(SeekPreviewResult::Frame(frame)).is_err() {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                if results.send(SeekPreviewResult::Error(error)).is_err() {
                    break;
                }
            }
        }
    }
}

struct SeekPreviewSession {
    commands: Sender<SeekPreviewCommand>,
    results: Receiver<SeekPreviewResult>,
    source_key: String,
    cache: SeekPreviewCache,
    ready: bool,
    requested_time: f64,
    sent_time: Option<f64>,
    duration: Option<f64>,
    last_hover: Instant,
    due: Instant,
}

impl SeekPreviewSession {
    fn spawn(
        api: Arc<MpvApi>,
        source: video::VideoThumbnailSource,
        time: f64,
        duration: Option<f64>,
    ) -> Result<Self, String> {
        let (command_sender, command_receiver) = mpsc::channel();
        let (result_sender, result_receiver) = mpsc::channel();
        let source_key = source_key(&source);
        thread::Builder::new()
            .name("pluriview-seek-preview".to_owned())
            .spawn(move || {
                run_seek_preview_worker(api, source, time, command_receiver, result_sender);
            })
            .map_err(|error| format!("Could not start the timeline preview worker: {error}"))?;
        Ok(Self {
            commands: command_sender,
            results: result_receiver,
            source_key,
            cache: SeekPreviewCache::default(),
            ready: false,
            requested_time: time,
            sent_time: None,
            duration,
            last_hover: Instant::now(),
            due: Instant::now() + SEEK_PREVIEW_COLD_DEBOUNCE,
        })
    }

    fn request(&mut self, time: f64, duration: Option<f64>) {
        self.last_hover = Instant::now();
        self.duration = duration;
        if (self.requested_time - time).abs() <= 0.01 {
            return;
        }
        self.requested_time = time;
        let debounce = if self.ready {
            SEEK_PREVIEW_WARM_DEBOUNCE
        } else {
            SEEK_PREVIEW_COLD_DEBOUNCE
        };
        self.due = Instant::now() + debounce;
    }

    fn hovering(&self) -> bool {
        self.last_hover.elapsed() <= SEEK_PREVIEW_PREFETCH_GRACE
    }

    fn next_work(&self) -> Option<f64> {
        next_preview_time(
            self.requested_time,
            self.duration,
            &self.cache,
            self.hovering(),
        )
    }

    fn pump_request(&mut self) {
        if !self.ready || Instant::now() < self.due {
            return;
        }
        let Some(time) = self.next_work() else {
            return;
        };
        if self
            .sent_time
            .is_some_and(|sent| (sent - time).abs() <= 0.01)
        {
            return;
        }
        if self.commands.send(SeekPreviewCommand::Grab(time)).is_ok() {
            self.sent_time = Some(time);
        }
    }
}

/// Persistent, in-process libmpv cores used for seek-bar hover previews.
#[derive(Default)]
pub struct SeekPreviewManager {
    api: Option<Arc<MpvApi>>,
    sessions: HashMap<PreviewId, SeekPreviewSession>,
}

impl SeekPreviewManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cached_frame(&self, id: PreviewId, time: f64) -> Option<video::VideoThumbnail> {
        self.sessions
            .get(&id)
            .and_then(|session| session.cache.get(time))
            .cloned()
            .filter(|thumbnail| frame_matches_hover(thumbnail.time, time))
    }

    pub fn request(
        &mut self,
        id: PreviewId,
        mpv_path: &Path,
        source: video::VideoThumbnailSource,
        time: f64,
        duration: Option<f64>,
    ) -> Result<(), String> {
        let time = bucket_time(time);
        let key = source_key(&source);
        if self
            .sessions
            .get(&id)
            .is_some_and(|session| session.source_key != key)
        {
            self.sessions.remove(&id);
        }
        if let Some(session) = self.sessions.get_mut(&id) {
            session.request(time, duration);
            session.pump_request();
            return Ok(());
        }
        if self.api.is_none() {
            self.api = Some(MpvApi::load(mpv_path)?);
        }
        let api = self.api.as_ref().expect("libmpv was loaded").clone();
        self.sessions
            .insert(id, SeekPreviewSession::spawn(api, source, time, duration)?);
        Ok(())
    }

    pub fn poll(&mut self) -> Vec<(PreviewId, video::VideoThumbnail)> {
        let mut frames = Vec::new();
        let ids: Vec<_> = self.sessions.keys().copied().collect();
        for id in ids {
            let Some(session) = self.sessions.get_mut(&id) else {
                continue;
            };
            loop {
                match session.results.try_recv() {
                    Ok(SeekPreviewResult::Ready) => {
                        session.ready = true;
                        session.due = Instant::now();
                    }
                    Ok(SeekPreviewResult::Frame(thumbnail)) => {
                        let requested = session.sent_time.unwrap_or(session.requested_time);
                        let matches_hover =
                            frame_matches_hover(thumbnail.time, session.requested_time);
                        session.cache.insert_grab(requested, thumbnail.clone());
                        session.sent_time = None;
                        if matches_hover {
                            frames.push((id, thumbnail));
                        }
                    }
                    Ok(SeekPreviewResult::Error(error)) => {
                        log::debug!("Timeline preview failed: {error}");
                        session.sent_time = None;
                        session.due = Instant::now() + Duration::from_secs(2);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.sessions.remove(&id);
                        break;
                    }
                }
            }
            if let Some(session) = self.sessions.get_mut(&id) {
                session.pump_request();
            }
        }
        frames
    }

    pub fn needs_repaint(&self) -> bool {
        self.sessions.values().any(|session| {
            !session.ready || Instant::now() < session.due || session.next_work().is_some()
        })
    }

    pub fn remove(&mut self, id: PreviewId) {
        self.sessions.remove(&id);
    }

    pub fn retain(&mut self, mut keep: impl FnMut(PreviewId) -> bool) {
        self.sessions.retain(|id, _| keep(*id));
    }

    pub fn clear(&mut self) {
        self.sessions.clear();
    }
}

fn parse_track_selection(value: Option<String>) -> TrackSelection {
    match value.as_deref() {
        None | Some("no") => TrackSelection::Disabled,
        Some(value) => value
            .parse::<i64>()
            .map(TrackSelection::Id)
            .unwrap_or_else(|_| TrackSelection::Other(value.to_owned())),
    }
}

fn cstring(value: &str) -> Result<CString, String> {
    CString::new(value).map_err(|_| "A video value contained an embedded null byte".to_owned())
}

#[link(name = "opengl32")]
extern "system" {
    fn wglGetProcAddress(name: *const c_char) -> *mut c_void;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleA(name: *const c_char) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
}

unsafe extern "C" fn get_gl_proc_address(
    _context: *mut c_void,
    name: *const c_char,
) -> *mut c_void {
    let address = wglGetProcAddress(name);
    let invalid =
        address.is_null() || matches!(address as usize, 1 | 2 | 3) || address as isize == -1;
    if !invalid {
        return address;
    }
    let module = GetModuleHandleA(b"opengl32.dll\0".as_ptr().cast());
    if module.is_null() {
        ptr::null_mut()
    } else {
        GetProcAddress(module, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_rows_are_flipped_from_opengl_order() {
        let mut pixels = vec![
            1, 2, 3, 4, 5, 6, 7, 8, // bottom row
            9, 10, 11, 12, 13, 14, 15, 16, // top row
        ];

        flip_rgba_rows(&mut pixels, 2, 2);

        assert_eq!(
            pixels,
            vec![9, 10, 11, 12, 13, 14, 15, 16, 1, 2, 3, 4, 5, 6, 7, 8]
        );
    }

    #[test]
    fn rgb0_pixels_gain_opaque_alpha() {
        let mut pixels = vec![10, 20, 30, 0, 40, 50, 60, 7];
        rgb0_to_rgba(&mut pixels);
        assert_eq!(pixels, vec![10, 20, 30, 255, 40, 50, 60, 255]);
    }

    #[test]
    fn video_blit_destination_keeps_full_size_beyond_each_screen_edge() {
        let screen_height = 200;
        assert_eq!(
            video_blit_destination(
                egui::Rect::from_min_max(egui::pos2(-80.0, 20.0), egui::pos2(240.0, 200.0)),
                1.0,
                screen_height,
            ),
            BlitDestination {
                left: -80,
                bottom: 0,
                right: 240,
                top: 180,
            }
        );
        assert_eq!(
            video_blit_destination(
                egui::Rect::from_min_max(egui::pos2(100.0, -50.0), egui::pos2(420.0, 130.0)),
                1.0,
                screen_height,
            ),
            BlitDestination {
                left: 100,
                bottom: 70,
                right: 420,
                top: 250,
            }
        );
        assert_eq!(
            video_blit_destination(
                egui::Rect::from_min_max(egui::pos2(100.0, 100.0), egui::pos2(420.0, 260.0)),
                1.0,
                screen_height,
            ),
            BlitDestination {
                left: 100,
                bottom: -60,
                right: 420,
                top: 100,
            }
        );
    }

    #[test]
    fn seek_preview_cache_keeps_the_most_recent_frames() {
        let mut cache = SeekPreviewCache::default();
        for index in 0..SEEK_PREVIEW_CACHE_LEN + 5 {
            cache.insert(video::VideoThumbnail {
                time: index as f64,
                width: 1,
                height: 1,
                rgba: vec![index as u8, 0, 0, 255],
            });
        }
        assert!(cache.get(0.0).is_none());
        assert_eq!(cache.get(5.4).unwrap().rgba[0], 5);
        assert_eq!(cache.get(24.0).unwrap().rgba[0], 24);
        cache.insert(video::VideoThumbnail {
            time: 24.2,
            width: 1,
            height: 1,
            rgba: vec![99, 0, 0, 255],
        });
        assert_eq!(cache.get(24.0).unwrap().rgba[0], 99);
    }

    #[test]
    fn seek_preview_prefetch_neighbors_nearer_seconds_first() {
        assert_eq!(prefetch_times(0.4, Some(90.0)), vec![1.0, 2.0]);
        assert_eq!(
            prefetch_times(12.0, Some(90.0)),
            vec![11.0, 13.0, 10.0, 14.0]
        );
        assert_eq!(prefetch_times(89.6, Some(90.0)), vec![89.0, 88.0]);
    }

    #[test]
    fn seek_preview_work_prefers_the_hovered_time_then_neighbors() {
        let mut cache = SeekPreviewCache::default();
        assert_eq!(
            next_preview_time(12.0, Some(90.0), &cache, true),
            Some(12.0)
        );
        cache.insert(video::VideoThumbnail {
            time: 12.0,
            width: 1,
            height: 1,
            rgba: vec![1, 0, 0, 255],
        });
        assert_eq!(next_preview_time(12.0, Some(90.0), &cache, false), None);
        assert_eq!(
            next_preview_time(12.0, Some(90.0), &cache, true),
            Some(11.0)
        );
        cache.insert(video::VideoThumbnail {
            time: 11.0,
            width: 1,
            height: 1,
            rgba: vec![2, 0, 0, 255],
        });
        assert_eq!(
            next_preview_time(12.0, Some(90.0), &cache, true),
            Some(13.0)
        );
    }

    #[test]
    fn seek_preview_frame_must_be_within_half_a_second() {
        assert!(frame_matches_hover(12.0, 12.4));
        assert!(frame_matches_hover(12.5, 12.0));
        assert!(!frame_matches_hover(8.0, 12.0));
        assert!(!frame_matches_hover(14.0, 12.0));
    }

    #[test]
    fn seek_preview_cache_does_not_serve_a_keyframe_from_the_wrong_second() {
        let mut cache = SeekPreviewCache::default();
        cache.insert_grab(
            12.0,
            video::VideoThumbnail {
                time: 8.2,
                width: 1,
                height: 1,
                rgba: vec![8, 0, 0, 255],
            },
        );
        assert!(cache.get(12.0).is_none());
        assert!(cache.covered(12.0));
        assert_eq!(cache.get(8.0).unwrap().rgba[0], 8);
        assert_eq!(
            next_preview_time(12.0, Some(90.0), &cache, true),
            Some(11.0)
        );
    }

    #[test]
    fn bundled_runtime_initializes_and_accepts_immediate_controls() {
        let api = MpvApi::load(Path::new("mpv.exe")).expect("load bundled libmpv");
        let mut core =
            MpvCore::new(api, true, true, false).expect("initialize network-tuned libmpv");
        core.set_property("pause", "no").expect("resume");
        core.set_property("pause", "yes").expect("pause");
        core.set_property("volume", "37").expect("set volume");
        core.destroy_without_render_context();
    }

    #[test]
    #[ignore = "requires PLURIVIEW_TEST_VIDEO and a desktop OpenGL context"]
    fn direct_renderer_pause_then_resume_advances_a_real_video() {
        use windows::Win32::{
            Foundation::HWND,
            Graphics::{
                Gdi::{GetDC, ReleaseDC},
                OpenGL::{
                    wglCreateContext, wglDeleteContext, wglMakeCurrent, ChoosePixelFormat,
                    SetPixelFormat, PFD_DOUBLEBUFFER, PFD_DRAW_TO_WINDOW, PFD_MAIN_PLANE,
                    PFD_SUPPORT_OPENGL, PFD_TYPE_RGBA, PIXELFORMATDESCRIPTOR,
                },
            },
        };

        let media = PathBuf::from(
            std::env::var_os("PLURIVIEW_TEST_VIDEO").expect("set PLURIVIEW_TEST_VIDEO"),
        );
        assert!(
            media.is_file(),
            "test video does not exist: {}",
            media.display()
        );

        let host = crate::video::VideoHost::new(None, 320, 180).expect("create WGL test window");
        let hwnd = HWND(host.hwnd() as *mut c_void);
        let dc = unsafe { GetDC(hwnd) };
        assert!(!dc.is_invalid(), "get window DC");
        let descriptor = PIXELFORMATDESCRIPTOR {
            nSize: std::mem::size_of::<PIXELFORMATDESCRIPTOR>() as u16,
            nVersion: 1,
            dwFlags: PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER,
            iPixelType: PFD_TYPE_RGBA,
            cColorBits: 32,
            cDepthBits: 24,
            cStencilBits: 8,
            iLayerType: PFD_MAIN_PLANE.0 as u8,
            ..Default::default()
        };
        let format = unsafe { ChoosePixelFormat(dc, &descriptor) };
        assert!(format > 0, "choose pixel format");
        unsafe { SetPixelFormat(dc, format, &descriptor) }.expect("set pixel format");
        let context = unsafe { wglCreateContext(dc) }.expect("create WGL context");
        unsafe { wglMakeCurrent(dc, context) }.expect("make WGL context current");
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                let name = CString::new(name).expect("OpenGL symbol name");
                get_gl_proc_address(ptr::null_mut(), name.as_ptr()).cast_const()
            })
        };

        let api = MpvApi::load(Path::new("mpv.exe")).expect("load bundled libmpv");
        let wallpaper = std::env::var_os("PLURIVIEW_TEST_WALLPAPER").is_some();
        let renderer = VideoRenderer::new(api, false, false, wallpaper).expect("create renderer");
        let launch = VideoLaunch {
            mpv_path: PathBuf::from("mpv.exe"),
            source: video::VideoSource::LocalFile(media.clone()),
            start_paused: false,
            wallpaper,
        };
        let mut session = VideoSession::new(renderer.clone(), &launch).expect("create session");
        // Keep the diagnostic silent even when the supplied video has audio;
        // libmpv still initializes and advances the audio pipeline.
        session.set_volume(0.0).expect("mute diagnostic playback");
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 180.0));
        let info = || egui::PaintCallbackInfo {
            viewport: rect,
            clip_rect: rect,
            pixels_per_point: 1.0,
            screen_size_px: [320, 180],
        };
        let mut paint_samples = Vec::new();
        let mut poll_samples = Vec::new();
        macro_rules! timed {
            ($samples:expr, $operation:expr) => {{
                let started = Instant::now();
                let result = $operation;
                $samples.push(started.elapsed());
                result
            }};
        }

        let start_deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < start_deadline {
            timed!(paint_samples, renderer.paint(info(), &gl));
            let _ = timed!(poll_samples, session.poll());
            if session.state.time_pos.is_some_and(|time| time > 0.2) {
                break;
            }
            thread::sleep(Duration::from_millis(16));
        }
        let before_pause = session.state.time_pos.expect("video did not start");
        let snapshot = renderer.snapshot(&gl).expect("rendered video snapshot");
        assert!(
            snapshot
                .rgba
                .chunks_exact(4)
                .any(|pixel| pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0),
            "libmpv rendered only black pixels"
        );
        let mut presented = vec![0_u8; 320 * 180 * 4];
        unsafe {
            gl.read_pixels(
                0,
                0,
                320,
                180,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(&mut presented),
            );
        }
        assert!(
            presented
                .chunks_exact(4)
                .any(|pixel| pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0),
            "video frame was not copied to the destination framebuffer"
        );
        let partially_offscreen = egui::PaintCallbackInfo {
            viewport: egui::Rect::from_min_size(egui::pos2(-80.0, 0.0), egui::vec2(320.0, 180.0)),
            clip_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(240.0, 180.0)),
            pixels_per_point: 1.0,
            screen_size_px: [320, 180],
        };
        renderer.paint(partially_offscreen, &gl);
        assert_eq!(
            unsafe { gl.get_error() },
            glow::NO_ERROR,
            "partially offscreen video paint produced an OpenGL error"
        );
        if std::env::var_os("PLURIVIEW_TEST_REQUIRE_HWDEC").is_some() {
            let hwdec = renderer
                .core
                .lock()
                .property("hwdec-current")
                .unwrap_or_default();
            assert!(
                !hwdec.is_empty() && hwdec != "no",
                "hardware decoding was not activated"
            );
        }
        if std::env::var_os("PLURIVIEW_TEST_REQUIRE_AUDIO").is_some() {
            assert!(
                session
                    .state
                    .track_list
                    .iter()
                    .any(|track| track.kind == "audio"),
                "the diagnostic media has no audio track"
            );
            assert!(
                !matches!(session.state.audio_track, TrackSelection::Disabled),
                "libmpv did not select the audio track"
            );
        }
        session.set_paused(true).expect("pause video");
        for _ in 0..20 {
            timed!(paint_samples, renderer.paint(info(), &gl));
            let _ = timed!(poll_samples, session.poll());
            thread::sleep(Duration::from_millis(16));
        }
        let paused_at = session.state.time_pos.expect("paused position");
        assert!(
            (paused_at - before_pause).abs() < 0.12,
            "pause was delayed: {before_pause:.3} -> {paused_at:.3}"
        );

        session.play().expect("resume video");
        let resume_deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < resume_deadline {
            timed!(paint_samples, renderer.paint(info(), &gl));
            let _ = timed!(poll_samples, session.poll());
            if session
                .state
                .time_pos
                .is_some_and(|time| time > paused_at + 0.2)
            {
                break;
            }
            thread::sleep(Duration::from_millis(16));
        }
        assert!(
            session
                .state
                .time_pos
                .is_some_and(|time| time > paused_at + 0.2),
            "video did not advance after Play"
        );

        // Folder playlists use this exact path: replace the media in the
        // existing renderer/session and immediately continue playback.
        session
            .load_local_file(&media)
            .expect("replace playlist video");
        let replacement_deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < replacement_deadline {
            timed!(paint_samples, renderer.paint(info(), &gl));
            let _ = timed!(poll_samples, session.poll());
            if session.state.time_pos.is_some_and(|time| time > 0.15) {
                break;
            }
            thread::sleep(Duration::from_millis(16));
        }
        assert!(
            session.state.time_pos.is_some_and(|time| time > 0.15),
            "replacement video did not begin in the existing libmpv session"
        );

        if std::env::var_os("PLURIVIEW_TEST_PROFILE").is_some() {
            let summarize = |samples: &[Duration]| {
                let total = samples.iter().copied().sum::<Duration>();
                let average = total.as_secs_f64() * 1_000.0 / samples.len().max(1) as f64;
                let max = samples.iter().copied().max().unwrap_or_default();
                (samples.len(), average, max.as_secs_f64() * 1_000.0)
            };
            let (paint_count, paint_average, paint_max) = summarize(&paint_samples);
            let (poll_count, poll_average, poll_max) = summarize(&poll_samples);
            eprintln!(
                "libmpv profile: paint n={paint_count} avg={paint_average:.3}ms max={paint_max:.3}ms; poll n={poll_count} avg={poll_average:.3}ms max={poll_max:.3}ms"
            );
        }

        renderer.cleanup(&gl);
        drop(gl);
        unsafe {
            wglMakeCurrent(None, None).expect("release WGL context");
            wglDeleteContext(context).expect("delete WGL context");
            let _ = ReleaseDC(hwnd, dc);
        }
        drop(host);
    }
}
