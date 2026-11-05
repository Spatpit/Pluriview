use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs,
    io::{self, Cursor, Read},
    num::NonZeroIsize,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver, TryRecvError},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};

use eframe::egui;
use parking_lot::Mutex;
use webview2_com::{
    take_pwstr, BrowserExtensionEnableCompletedHandler, ClearBrowsingDataCompletedHandler,
    ContextMenuRequestedEventHandler, CustomItemSelectedEventHandler, FaviconChangedEventHandler,
    GetFaviconCompletedHandler, HistoryChangedEventHandler,
    IsDocumentPlayingAudioChangedEventHandler,
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2BrowserExtension, ICoreWebView2ContextMenuItemCollection,
        ICoreWebView2Environment9, ICoreWebView2PermissionRequestedEventArgs3,
        ICoreWebView2Profile2, ICoreWebView2Profile7, ICoreWebView2_11, ICoreWebView2_13,
        ICoreWebView2_15, ICoreWebView2_3, ICoreWebView2_8,
        COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND, COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG,
        COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_AUTOPLAY,
        COREWEBVIEW2_PERMISSION_STATE_DENY, COREWEBVIEW2_PROCESS_FAILED_KIND,
        COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE,
    },
    NewBrowserVersionAvailableEventHandler, PermissionRequestedEventHandler,
    ProcessFailedEventHandler, ProfileAddBrowserExtensionCompletedHandler,
    ProfileGetBrowserExtensionsCompletedHandler, TrySuspendCompletedHandler,
};
use webview2_core::{Interface, HSTRING as WebViewHString, PWSTR as WebViewPWSTR};
use windows::{
    core::w,
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, POINT, WAIT_OBJECT_0, WPARAM},
        Graphics::Gdi::ClientToScreen,
        System::{
            LibraryLoader::GetModuleHandleW,
            Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
        },
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetForegroundWindow, GetWindowLongW,
            IsChild, RegisterClassW, SetForegroundWindow, SetWindowPos, ShowWindow, GWL_EXSTYLE,
            HWND_BOTTOM, HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOZORDER,
            SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, WNDCLASSW, WS_CLIPCHILDREN, WS_EX_TOOLWINDOW,
            WS_EX_TOPMOST, WS_POPUP,
        },
    },
};
use wry::{
    dpi::{PhysicalPosition, PhysicalSize},
    raw_window_handle::{
        HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
    },
    NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder,
    WebViewBuilderExtWindows, WebViewExtWindows,
};

use crate::preview::{BrowserTab, Favicon, PreviewId};

const PARK_X: i32 = -30_000;
const PARK_Y: i32 = -30_000;
/// Backing viewport of a parked browser, in physical pixels. This is what
/// Windows Graphics Capture sees, so it stays fixed regardless of tile size.
const WIDTH: i32 = 1280;
const HEIGHT: i32 = 720;

/// WebView2 rejects zoom factors outside roughly this range.
const MIN_ZOOM: f64 = 0.25;
const MAX_ZOOM: f64 = 4.0;
/// Parked browsers use a 2× supersampled backing from the tile's physical
/// size, not canvas zoom, so navigation never resizes the webpage. Zoom
/// (`capture_width / WIDTH`) keeps the CSS viewport at WIDTH pixels.
const CAPTURE_SCALE: f64 = 2.0;
const MAX_CAPTURE_WIDTH: i32 = 3840;
const MAX_CAPTURE_HEIGHT: i32 = 2160;
/// Give WebView2's compositor time to apply a new viewport and zoom before
/// the native host replaces the frozen captured preview onscreen.
const INTERACTION_PREP_DELAY: Duration = Duration::from_millis(80);
/// The same settling time after a host parks: its capture stays paused, so
/// the tile keeps showing the last interactive frame instead of the frames
/// WebView2 renders while it grows back to capture resolution.
const PARK_SETTLE_DELAY: Duration = Duration::from_millis(150);
/// A background tab that has not been shown or played sound for this long
/// unloads its page to free memory. It reloads when shown, like a browser's
/// memory saver.
const TAB_SLEEP_AFTER: Duration = Duration::from_secs(10 * 60);
/// Site icons are drawn at most this many pixels across.
const FAVICON_SIZE: u32 = 64;
/// Larger downloads are not icons; ignore them.
const MAX_FAVICON_BYTES: usize = 512 * 1024;
/// WebView2's own context menu item after which "Open Link in New Tile" goes.
const OPEN_LINK_IN_NEW_WINDOW_ITEM: &str = "openLinkInNewWindow";

/// wry's default WebView2 arguments also disable `msSmartScreenProtection`,
/// which turns off Microsoft Defender SmartScreen's phishing and malware site
/// warnings. Browser tiles open arbitrary websites, so keep only wry's other
/// two (Edge's text-selection mini menu and PDF mini menu). All tiles share
/// one WebView2 environment, so every builder must pass this same string.
const BROWSER_ARGUMENTS: &str = "--disable-features=msWebOOUI,msPdfOOUI";

/// Address bar searches go here, as in a new browser profile.
const SEARCH_URL: &str = "https://www.google.com/search";

/// Page zoom limits offered per tile, before the tile-size zoom is applied.
pub const MIN_PAGE_ZOOM: f32 = 0.5;
pub const MAX_PAGE_ZOOM: f32 = 3.0;
/// The zoom steps browsers use, so - and + feel familiar.
const PAGE_ZOOM_STEPS: [f32; 13] = [
    0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
];

/// The next page zoom step above (`zoom_in`) or below the current zoom.
pub fn step_page_zoom(current: f32, zoom_in: bool) -> f32 {
    const EPSILON: f32 = 0.005;
    if zoom_in {
        PAGE_ZOOM_STEPS
            .into_iter()
            .find(|step| *step > current + EPSILON)
            .unwrap_or(MAX_PAGE_ZOOM)
    } else {
        PAGE_ZOOM_STEPS
            .into_iter()
            .rev()
            .find(|step| *step < current - EPSILON)
            .unwrap_or(MIN_PAGE_ZOOM)
    }
}

/// Scroll the innermost scrollable element under a normalized page point,
/// falling back to the page itself, as a mouse wheel over that point would.
/// Arguments: horizontal and vertical position (0..1) and CSS pixels.
const SCROLL_PAGE_SCRIPT: &str = r#"
((u, v, dy) => {
    let element = document.elementFromPoint(u * innerWidth, v * innerHeight);
    while (element && element !== document.body && element !== document.documentElement) {
        const overflow = getComputedStyle(element).overflowY;
        if (/(auto|scroll|overlay)/.test(overflow) && element.scrollHeight > element.clientHeight + 1) {
            const before = element.scrollTop;
            element.scrollBy({ top: dy, behavior: "instant" });
            if (element.scrollTop !== before) return;
        }
        element = element.parentElement;
    }
    window.scrollBy({ top: dy, behavior: "instant" });
})"#;

/// The largest visible video's picture as normalized viewport coordinates
/// `[left, top, right, bottom]`, excluding the letterbox bars a video element
/// adds by default, or null when no video covers at least 1% of the page.
const FIND_VIDEO_SCRIPT: &str = r#"
(() => {
    const width = innerWidth, height = innerHeight;
    let best = null, bestArea = 0;
    for (const video of document.querySelectorAll("video")) {
        const box = video.getBoundingClientRect();
        if (box.width < 2 || box.height < 2) continue;
        let left = box.left, top = box.top, w = box.width, h = box.height;
        const fit = getComputedStyle(video).objectFit;
        if (video.videoWidth && video.videoHeight && fit !== "fill" && fit !== "cover") {
            const scale = Math.min(w / video.videoWidth, h / video.videoHeight);
            const pictureWidth = video.videoWidth * scale, pictureHeight = video.videoHeight * scale;
            left += (w - pictureWidth) / 2;
            top += (h - pictureHeight) / 2;
            w = pictureWidth;
            h = pictureHeight;
        }
        const l = Math.max(0, left), t = Math.max(0, top);
        const r = Math.min(width, left + w), b = Math.min(height, top + h);
        const area = Math.max(0, r - l) * Math.max(0, b - t);
        if (area > bestArea) {
            bestArea = area;
            best = [l / width, t / height, r / width, b / height];
        }
    }
    return bestArea >= width * height * 0.01 ? best : null;
})()
"#;

/// A page that crashes again this soon after being reloaded is left alone
/// rather than reloaded in a loop; the tile menu's Reload still works.
const CRASH_RELOAD_COOLDOWN: Duration = Duration::from_secs(60);
/// Upper bound on waiting for the old WebView2 browser process to exit
/// before restarted tiles create a new one.
const ENGINE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

const UBOL_VERSION: &str = "2026.714.1952";
/// Shipped in the `lib` folder beside the executable instead of embedded,
/// which keeps ~10 MB out of pluriview.exe. The build still pins its SHA-256,
/// and only a package matching that fingerprint is ever extracted.
const UBOL_ARCHIVE_NAME: &str = "uBOLite_2026.714.1952.edge.zip";
const UBOL_ARCHIVE_FINGERPRINT: &str = env!("PLURIVIEW_UBOL_SHA256");
const PREPARATION_PROGRESS_SCALE: u32 = 10_000;

/// WebView2's autoplay policy can still allow a previously used site through
/// its media-engagement heuristics. Install a document-start guard as well so
/// restored pages cannot call `play()` until this tile receives real input.
/// The capture-phase listeners unlock before the site's own Play handler runs.
const MEDIA_AUTOPLAY_GUARD: &str = r#"
(() => {
    let unlocked = false;
    const inputEvents = ["pointerdown", "keydown", "touchstart"];
    const unlock = (event) => {
        if (!event.isTrusted) return;
        unlocked = true;
        for (const eventName of inputEvents) {
            window.removeEventListener(eventName, unlock, true);
        }
    };

    for (const eventName of inputEvents) {
        window.addEventListener(eventName, unlock, true);
    }

    const nativePlay = HTMLMediaElement.prototype.play;
    HTMLMediaElement.prototype.play = function (...args) {
        if (!unlocked) {
            this.pause();
            return Promise.reject(new DOMException(
                "Media playback requires user interaction",
                "NotAllowedError"
            ));
        }
        return Reflect.apply(nativePlay, this, args);
    };

    // Also catch declarative <video autoplay> playback, which does not need
    // to call the JavaScript play() method overridden above.
    document.addEventListener("play", (event) => {
        if (!unlocked && event.target instanceof HTMLMediaElement) {
            event.target.pause();
        }
    }, true);
})();
"#;

/// Installing or re-enabling an extension makes Chromium drop the request
/// rules and content scripts it had registered; uBOL's service worker rebuilds
/// both asynchronously on its next start. Pages loaded inside that window are
/// unfiltered, and a single-page app like YouTube never reloads on its own, so
/// a tile that opens there keeps playing ads for the rest of the session.
/// Browser tiles therefore stay on `about:blank` until the window closes.
const ADBLOCK_SETTLE: Duration = Duration::from_secs(8);

struct ExtensionPreparationTask {
    progress: Arc<AtomicU32>,
    receiver: Receiver<Result<PathBuf, String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExtensionPreparationStatus {
    Idle,
    Preparing(f32),
    Ready,
    Failed(String),
}

/// How often a tile re-reads its address from WebView2. Frequent enough that
/// a layout saved right after browsing is current, cheap enough to run for
/// every tile on every frame budget.
const URL_POLL_INTERVAL: Duration = Duration::from_millis(750);

fn parked_bounds() -> Rect {
    Rect {
        position: PhysicalPosition::new(0, 0).into(),
        size: PhysicalSize::new(WIDTH, HEIGHT).into(),
    }
}

pub fn normalize_url(input: &str) -> Result<String, &'static str> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Enter a website URL");
    }

    match url::Url::parse(input) {
        Ok(url) if matches!(url.scheme(), "http" | "https") => {
            validate_web_url_credentials(&url)?;
            Ok(input.to_owned())
        }
        Ok(_) => Err("Only HTTP and HTTPS websites are supported"),
        Err(url::ParseError::RelativeUrlWithoutBase) => {
            let candidate = format!("https://{input}");
            url::Url::parse(&candidate)
                .map_err(|_| "Enter a valid website URL")
                .and_then(|url| {
                    validate_web_url_credentials(&url)?;
                    Ok(candidate)
                })
        }
        Err(_) => Err("Enter a valid website URL"),
    }
}

fn validate_web_url_credentials(url: &url::Url) -> Result<(), &'static str> {
    if !url.username().is_empty() || url.password().is_some() {
        Err("URLs containing usernames or passwords are not supported")
    } else {
        Ok(())
    }
}

fn is_web_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

/// Remove values that should not be written to the plaintext layout file.
/// The in-memory WebView keeps its complete URL; only persisted history is
/// scrubbed.
pub fn scrub_url_for_storage(input: &str) -> Option<String> {
    let mut url = url::Url::parse(input).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_fragment(None);

    let retained: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| !is_sensitive_query_key(key))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if url.query().is_some() {
        url.query_pairs_mut().clear().extend_pairs(retained);
        if url.query() == Some("") {
            url.set_query(None);
        }
    }
    Some(url.into())
}

/// Where an address bar entry goes: a web address as typed, or a web search
/// for anything that does not look like one (spaces, or a single word).
pub fn address_bar_target(input: &str) -> Result<String, &'static str> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Enter an address or search");
    }
    let has_scheme = input.contains("://");
    let looks_like_host = input.contains('.') || input.contains(':') || input == "localhost";
    if has_scheme || (!input.contains(char::is_whitespace) && looks_like_host) {
        if let Ok(url) = normalize_url(input) {
            return Ok(url);
        }
        if has_scheme {
            return normalize_url(input);
        }
    }
    url::Url::parse_with_params(SEARCH_URL, [("q", input)])
        .map(String::from)
        .map_err(|_| "Enter an address or search")
}

/// Whether `url` is an address bar search rather than a typed address.
pub fn is_search_url(url: &str) -> bool {
    url.strip_prefix(SEARCH_URL)
        .is_some_and(|rest| rest.starts_with('?'))
}

/// Scrub every tab for saving, dropping tabs without a web address (such as
/// a page still on `about:blank`). Returns the tabs and the index of the
/// active one among those kept.
pub fn scrub_tabs_for_storage(tabs: &[BrowserTab], active: usize) -> (Vec<BrowserTab>, usize) {
    let mut kept = Vec::with_capacity(tabs.len());
    let mut kept_active = 0;
    for (index, tab) in tabs.iter().enumerate() {
        let Some(url) = scrub_url_for_storage(&tab.url) else {
            continue;
        };
        if index <= active {
            kept_active = kept.len();
        }
        kept.push(BrowserTab {
            url,
            title: tab.title.clone(),
            ..BrowserTab::default()
        });
    }
    (kept, kept_active)
}

/// Query names that carry a password, a one-time code or a share secret,
/// matched exactly (lowercase). Saved browser and stream addresses both drop
/// them. `rlkey` and `resourcekey` are the secrets in Dropbox and Google
/// Drive share links.
pub(crate) const PRIVATE_QUERY_KEYS: [&str; 10] = [
    "pwd",
    "otp",
    "jwt",
    "ticket",
    "auth",
    "authkey",
    "rlkey",
    "resourcekey",
    "session",
    "sessionid",
];

fn is_sensitive_query_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    matches!(key.as_str(), "code" | "state" | "key" | "sig")
        || PRIVATE_QUERY_KEYS.contains(&key.as_str())
        || [
            "token",
            "secret",
            "password",
            "passwd",
            "authorization",
            "credential",
            "signature",
            "api_key",
            "apikey",
        ]
        .iter()
        .any(|marker| key.contains(marker))
}

/// Ordinary new-tab links keep navigating inside the tile. A sized popup
/// (`window.open` with dimensions, as sign-in providers use) opens as its own
/// WebView2 window while the user is interacting with the tile.
fn opens_as_popup(interactive: bool, sized: bool) -> bool {
    interactive && sized
}

/// Parked tiles are offscreen, so a site's camera, microphone, location,
/// notification or clipboard prompt would be invisible and could be answered
/// for every tile through the shared profile. Deny those requests while
/// parked; an interactive tile is visible, so WebView2 asks the user as usual.
fn deny_permission(kind: COREWEBVIEW2_PERMISSION_KIND, interactive: bool) -> bool {
    !interactive && kind != COREWEBVIEW2_PERMISSION_KIND_AUTOPLAY
}

fn register_permission_policy(
    webview: &WebView,
    interactive: Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), String> {
    let core = unsafe { webview.controller().CoreWebView2() }.map_err(|e| e.to_string())?;
    let handler = PermissionRequestedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
        unsafe { args.PermissionKind(&mut kind)? };
        if deny_permission(kind, interactive.load(std::sync::atomic::Ordering::Relaxed)) {
            // Do not remember the refusal: the same site may ask again once
            // the user is interacting with the tile.
            if let Ok(args) = args.cast::<ICoreWebView2PermissionRequestedEventArgs3>() {
                let _ = unsafe { args.SetSavesInProfile(false) };
            }
            unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)? };
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { core.add_PermissionRequested(&handler, &mut token) }.map_err(|e| e.to_string())
}

/// Schemes a page may navigate to while staying inside the tile. `about:` is
/// needed because sites navigate iframes/blank targets through it.
fn is_allowed_navigation(url: &str) -> bool {
    url::Url::parse(url)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https" | "about" | "blob" | "data"))
}

#[derive(Clone, Copy)]
struct NativeWindow(HWND);

impl NativeWindow {
    #[cfg(test)]
    fn from_hwnd(hwnd: isize) -> Result<Self, HandleError> {
        NonZeroIsize::new(hwnd)
            .map(|_| Self(HWND(hwnd as *mut _)))
            .ok_or(HandleError::Unavailable)
    }
}

impl HasWindowHandle for NativeWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.0 .0 as isize).ok_or(HandleError::Unavailable)?;
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(hwnd));
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

/// State written by wry callbacks (some run off the UI thread) and drained by
/// the UI thread once per frame via [`BrowserHost::poll`].
#[derive(Default)]
struct SharedState {
    current_url: String,
    current_title: Option<String>,
    /// A page requested a new window; the tile opens it as a tab, or in
    /// place when it shows no tabs.
    pending_navigation: Option<String>,
    /// The page is loading, between WebView2's start and finish events.
    loading: bool,
    /// A download was refused since the last poll.
    download_blocked: bool,
    /// The page's renderer process exited or stopped responding.
    page_crashed: bool,
    /// Whether the page is producing sound, reported by WebView2.
    playing_audio: bool,
    /// A finished Fit to Video lookup (see [`FIND_VIDEO_SCRIPT`]).
    video_rect: Option<Option<(f32, f32, f32, f32)>>,
    /// The page's icon changed: its address and PNG bytes (empty when the
    /// page has none).
    favicon: Option<(String, Vec<u8>)>,
    /// Links chosen with "Open Link in New Tile".
    new_tile_requests: Vec<String>,
    dirty: bool,
}

/// Report the page's icon whenever it changes. Needs WebView2 Runtime 108+;
/// older runtimes keep the letter badges.
fn register_favicon_handler(
    webview: &WebView,
    shared: Arc<Mutex<SharedState>>,
) -> Result<(), String> {
    let core = unsafe { webview.controller().CoreWebView2() }.map_err(|e| e.to_string())?;
    let core15: ICoreWebView2_15 = core.cast().map_err(|e| e.to_string())?;
    let handler = FaviconChangedEventHandler::create(Box::new(move |sender, _| {
        let Some(sender) = sender else {
            return Ok(());
        };
        let sender: ICoreWebView2_15 = sender.cast()?;
        let mut uri = WebViewPWSTR::null();
        unsafe { sender.FaviconUri(&mut uri)? };
        let uri = take_pwstr(uri);
        if uri.is_empty() {
            let mut state = shared.lock();
            state.favicon = Some((uri, Vec::new()));
            state.dirty = true;
            return Ok(());
        }
        let shared = shared.clone();
        let completed = GetFaviconCompletedHandler::create(Box::new(move |result, stream| {
            result?;
            let png = stream.as_ref().map(read_stream).unwrap_or_default();
            let mut state = shared.lock();
            state.favicon = Some((uri, png));
            state.dirty = true;
            Ok(())
        }));
        unsafe { sender.GetFavicon(COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG, &completed) }
    }));
    let mut token = 0i64;
    unsafe { core15.add_FaviconChanged(&handler, &mut token) }.map_err(|e| e.to_string())
}

fn read_stream(stream: &webview2_windows::Win32::System::Com::IStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let mut read = 0u32;
        let result = unsafe {
            stream.Read(
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                Some(&mut read),
            )
        };
        if result.is_err() || read == 0 {
            return bytes;
        }
        bytes.extend_from_slice(&chunk[..read as usize]);
        if bytes.len() > MAX_FAVICON_BYTES {
            return Vec::new();
        }
    }
}

/// Decode a site icon for drawing, scaled down to [`FAVICON_SIZE`].
fn decode_favicon(uri: String, png: &[u8]) -> Option<Favicon> {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?;
    let image = if image.width() > FAVICON_SIZE || image.height() > FAVICON_SIZE {
        image.thumbnail(FAVICON_SIZE, FAVICON_SIZE)
    } else {
        image
    };
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(Favicon {
        uri,
        image: Arc::new(egui::ColorImage::from_rgba_unmultiplied(
            size,
            rgba.as_raw(),
        )),
    })
}

/// Add "Open Link in New Tile" to the page's right-click menu on links.
/// Needs WebView2 Runtime 98+; older runtimes keep the standard menu.
fn register_context_menu_handler(
    webview: &WebView,
    shared: Arc<Mutex<SharedState>>,
) -> Result<(), String> {
    let core = unsafe { webview.controller().CoreWebView2() }.map_err(|e| e.to_string())?;
    let core11: ICoreWebView2_11 = core.cast().map_err(|e| e.to_string())?;
    let environment: ICoreWebView2Environment9 =
        webview.environment().cast().map_err(|e| e.to_string())?;
    let handler = ContextMenuRequestedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let target = unsafe { args.ContextMenuTarget()? };
        let mut has_link = Default::default();
        unsafe { target.HasLinkUri(&mut has_link)? };
        if !has_link.as_bool() {
            return Ok(());
        }
        let mut link = WebViewPWSTR::null();
        unsafe { target.LinkUri(&mut link)? };
        let link = take_pwstr(link);
        if !is_web_url(&link) {
            return Ok(());
        }
        let item = unsafe {
            environment.CreateContextMenuItem(
                &WebViewHString::from("Open Link in New Tile"),
                None::<&webview2_windows::Win32::System::Com::IStream>,
                COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND,
            )?
        };
        let shared = shared.clone();
        let selected = CustomItemSelectedEventHandler::create(Box::new(move |_, _| {
            let mut state = shared.lock();
            state.new_tile_requests.push(link.clone());
            state.dirty = true;
            Ok(())
        }));
        let mut token = 0i64;
        unsafe { item.add_CustomItemSelected(&selected, &mut token)? };
        let items = unsafe { args.MenuItems()? };
        let index = menu_index_after(&items, OPEN_LINK_IN_NEW_WINDOW_ITEM);
        unsafe { items.InsertValueAtIndex(index, &item) }
    }));
    let mut token = 0i64;
    unsafe { core11.add_ContextMenuRequested(&handler, &mut token) }.map_err(|e| e.to_string())
}

/// The position just after the menu item called `name`, or the top.
fn menu_index_after(items: &ICoreWebView2ContextMenuItemCollection, name: &str) -> u32 {
    let mut count = 0u32;
    if unsafe { items.Count(&mut count) }.is_err() {
        return 0;
    }
    (0..count)
        .find(|&index| {
            unsafe { items.GetValueAtIndex(index) }
                .and_then(|item| {
                    let mut item_name = WebViewPWSTR::null();
                    unsafe { item.Name(&mut item_name)? };
                    Ok(take_pwstr(item_name))
                })
                .is_ok_and(|item_name| item_name == name)
        })
        .map_or(0, |index| index + 1)
}

fn parse_video_rect(json: &str) -> Option<(f32, f32, f32, f32)> {
    let [left, top, right, bottom] = serde_json::from_str::<Option<[f32; 4]>>(json).ok()??;
    let valid = [left, top, right, bottom]
        .iter()
        .all(|value| (0.0..=1.0).contains(value))
        && right - left > 0.01
        && bottom - top > 0.01;
    valid.then_some((left, top, right, bottom))
}

fn register_audio_handler(
    webview: &WebView,
    shared: Arc<Mutex<SharedState>>,
) -> Result<(), String> {
    let core = unsafe { webview.controller().CoreWebView2() }.map_err(|e| e.to_string())?;
    let core8: ICoreWebView2_8 = core.cast().map_err(|e| e.to_string())?;
    let handler = IsDocumentPlayingAudioChangedEventHandler::create(Box::new(move |sender, _| {
        let Some(sender) = sender else {
            return Ok(());
        };
        let mut playing = Default::default();
        unsafe {
            sender
                .cast::<ICoreWebView2_8>()?
                .IsDocumentPlayingAudio(&mut playing)?
        };
        let mut state = shared.lock();
        state.playing_audio = playing.as_bool();
        state.dirty = true;
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { core8.add_IsDocumentPlayingAudioChanged(&handler, &mut token) }
        .map_err(|e| e.to_string())
}

/// Back/forward availability changes without a document load when a
/// single-page app pushes history, so listen for history changes directly.
fn register_history_handler(
    webview: &WebView,
    shared: Arc<Mutex<SharedState>>,
) -> Result<(), String> {
    let core = unsafe { webview.controller().CoreWebView2() }.map_err(|e| e.to_string())?;
    let handler = HistoryChangedEventHandler::create(Box::new(move |_, _| {
        shared.lock().dirty = true;
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { core.add_HistoryChanged(&handler, &mut token) }.map_err(|e| e.to_string())
}

/// Engine-wide WebView2 events, shared by every host's handlers.
#[derive(Clone, Default)]
struct EngineSignals {
    /// A newer WebView2 Runtime (usually a security update) was installed.
    /// Running tiles keep the old engine until every WebView is released.
    update_ready: Arc<AtomicBool>,
    /// The shared browser process exited, taking every tile's page with it.
    lost: Arc<AtomicBool>,
}

/// Unread state a page advertises in its title, as Discord, YouTube, Gmail
/// and most chat sites do ("(3) Discord", "• Discord", "Inbox (12) - ...").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnreadIndicator {
    Count(u32),
    /// Something is unread, without a number.
    Dot,
}

pub fn unread_indicator(title: &str) -> Option<UnreadIndicator> {
    let title = title.trim_start();
    if let Some(count) = title.strip_prefix('(').and_then(parenthesized_count) {
        return Some(UnreadIndicator::Count(count));
    }
    if title.starts_with('\u{2022}') || title.starts_with('\u{25CF}') {
        return Some(UnreadIndicator::Dot);
    }
    let inbox = title.find("Inbox (")?;
    parenthesized_count(&title[inbox + "Inbox (".len()..]).map(UnreadIndicator::Count)
}

/// The positive count in `12)`, `99+)` or `1,234)`.
fn parenthesized_count(text: &str) -> Option<u32> {
    let inner = &text[..text.find(')')?];
    let digits: String = inner
        .trim()
        .trim_end_matches('+')
        .chars()
        .filter(|character| *character != ',')
        .collect();
    if digits.is_empty() || !digits.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|count| *count > 0)
}

/// How a tile handled its page crashing since the last poll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageCrash {
    Reloaded,
    /// It crashed again within [`CRASH_RELOAD_COOLDOWN`] and was left as is.
    Repeated,
}

fn crash_response(last_reload: Option<Instant>, now: Instant) -> PageCrash {
    if last_reload.is_some_and(|at| now.duration_since(at) < CRASH_RELOAD_COOLDOWN) {
        PageCrash::Repeated
    } else {
        PageCrash::Reloaded
    }
}

/// Chromium restarts its GPU and utility processes by itself, and a crashed
/// iframe only replaces that frame. These are the failures a tile must act on.
fn register_process_failure_handler(
    webview: &WebView,
    shared: Arc<Mutex<SharedState>>,
    engine_lost: Arc<AtomicBool>,
) -> Result<(), String> {
    let core = unsafe { webview.controller().CoreWebView2() }.map_err(|e| e.to_string())?;
    let handler = ProcessFailedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
        unsafe { args.ProcessFailedKind(&mut kind)? };
        if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED {
            engine_lost.store(true, Ordering::Release);
        } else if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
            || kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE
        {
            let mut state = shared.lock();
            state.page_crashed = true;
            state.dirty = true;
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { core.add_ProcessFailed(&handler, &mut token) }.map_err(|e| e.to_string())
}

fn register_engine_update_handler(
    webview: &WebView,
    update_ready: Arc<AtomicBool>,
) -> Result<(), String> {
    let handler = NewBrowserVersionAvailableEventHandler::create(Box::new(move |_, _| {
        update_ready.store(true, Ordering::Release);
        Ok(())
    }));
    let mut token = 0i64;
    unsafe {
        webview
            .environment()
            .add_NewBrowserVersionAvailable(&handler, &mut token)
    }
    .map_err(|e| e.to_string())
}

/// Title/URL changes since the last poll, for the app to mirror onto the tile.
#[derive(Default)]
pub struct BrowserUpdate {
    /// Something below changed; the other fields describe the whole tile.
    pub changed: bool,
    pub title: Option<String>,
    pub url: Option<String>,
    /// The page tried to download a file, which browser tiles do not allow.
    pub download_blocked: bool,
    pub page_crash: Option<PageCrash>,
    pub playing_audio: bool,
    /// A Fit to Video request finished: the video's crop, or None if the
    /// page has no visible video.
    pub video_rect: Option<Option<(f32, f32, f32, f32)>>,
    pub tabs: Vec<BrowserTab>,
    pub active_tab: usize,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub loading: bool,
    /// Pages asked to open these addresses in a new window.
    pub new_window_requests: Vec<String>,
    /// Links chosen with "Open Link in New Tile".
    pub new_tile_requests: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct BrowserGeometry {
    host_x: i32,
    host_y: i32,
    host_width: i32,
    host_height: i32,
    page_x: i32,
    page_y: i32,
    page_width: i32,
    page_height: i32,
}

fn browser_geometry(
    page_rect: egui::Rect,
    visible_rect: egui::Rect,
    pixels_per_point: f32,
    host_screen_origin: (i32, i32),
) -> BrowserGeometry {
    let host_size = visible_rect.size() * pixels_per_point;
    let page_size = page_rect.size() * pixels_per_point;
    BrowserGeometry {
        host_x: host_screen_origin.0,
        host_y: host_screen_origin.1,
        host_width: (host_size.x.round() as i32).max(1),
        host_height: (host_size.y.round() as i32).max(1),
        page_x: ((page_rect.min.x - visible_rect.min.x) * pixels_per_point).round() as i32,
        page_y: ((page_rect.min.y - visible_rect.min.y) * pixels_per_point).round() as i32,
        page_width: (page_size.x.round() as i32).max(1),
        page_height: (page_size.y.round() as i32).max(1),
    }
}

/// Move and resize a page and set its zoom in one step. The page's CSS
/// viewport is `size / zoom`, which every call keeps the same. wry's separate
/// `set_bounds` and `zoom` let the page lay out once at the new size with the
/// old zoom: half or twice as wide, which responsive sites like YouTube
/// answer by switching layouts, visible as the page jumping in and out of
/// interaction.
fn set_page_geometry(webview: &WebView, position: (i32, i32), size: (i32, i32), zoom: f64) {
    let controller = webview.controller();
    unsafe {
        // wry hosts each controller in its own child window; size it first so
        // the new bounds are not clipped. It belongs to this thread, so the
        // resize is synchronous, unlike wry's own `set_bounds`.
        let mut container = webview2_windows::Win32::Foundation::HWND::default();
        if controller.ParentWindow(&mut container).is_ok() {
            let _ = SetWindowPos(
                HWND(container.0),
                None,
                position.0,
                position.1,
                size.0,
                size.1,
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
        }
        let bounds = webview2_windows::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: size.0,
            bottom: size.1,
        };
        let _ = controller.SetBoundsAndZoomFactor(bounds, zoom);
    }
}

/// Where the tab at `index` ends up after the tab at `from` moves to `to`.
fn moved_index(index: usize, from: usize, to: usize) -> usize {
    if index == from {
        to
    } else if from < index && index <= to {
        index - 1
    } else if to <= index && index < from {
        index + 1
    } else {
        index
    }
}

pub fn capture_size_for_tile(width: i32, height: i32) -> (i32, i32) {
    let width = width.max(1) as f64;
    let height = height.max(1) as f64;
    // WebView2 zoom is capture_width / WIDTH, clamped to MIN_ZOOM. Stay at or
    // above that width so the page still layouts at WIDTH CSS pixels.
    let min_width = WIDTH as f64 * MIN_ZOOM;
    let desired = CAPTURE_SCALE.max(min_width / width);
    let cap = (MAX_CAPTURE_WIDTH as f64 / width).min(MAX_CAPTURE_HEIGHT as f64 / height);
    let scale = desired.min(cap).max(f64::MIN_POSITIVE);
    (
        (width * scale).round().max(1.0) as i32,
        (height * scale).round().max(1.0) as i32,
    )
}

/// One loaded page of a browser tile and the handlers reporting its state.
struct TabView {
    webview: WebView,
    shared: Arc<Mutex<SharedState>>,
    /// Whether the tile showing this page is interactive, for its popup and
    /// permission handlers. Per page, so a page can move to another tile.
    interactive: Arc<std::sync::atomic::AtomicBool>,
    /// When the live address was last read from WebView2 (see [`BrowserHost::poll`]).
    last_url_check: Option<Instant>,
    /// Ad-block activation may request navigation while a tile sleeps. Keep
    /// it deferred because WebView2 navigation would otherwise auto-resume it.
    after_resume: Option<AfterResume>,
    /// When this page last reloaded itself after it crashed.
    last_crash_reload: Option<Instant>,
    /// A repeated crash was already reported for the current cooldown.
    crash_reported: bool,
}

impl TabView {
    fn current_url(&self) -> String {
        self.shared.lock().current_url.clone()
    }

    fn live_url(&self) -> Option<String> {
        let url = self.webview.url().ok()?;
        url.starts_with("http").then_some(url)
    }

    /// WebView2 reports navigation only for real document loads, so a
    /// single-page app (YouTube, Reddit, ...) would leave the tile stuck on
    /// the address it opened with. Re-read the live address on a timer so
    /// tiles — and the layouts saved from them — follow in-page navigation.
    fn refresh_live_url(&mut self) {
        let due = self
            .last_url_check
            .is_none_or(|at| at.elapsed() >= URL_POLL_INTERVAL);
        if !due {
            return;
        }
        self.last_url_check = Some(Instant::now());
        let Some(url) = self.live_url() else {
            return;
        };
        let mut state = self.shared.lock();
        if state.current_url != url {
            state.current_url = url;
            state.dirty = true;
        }
    }

    fn load(&self, url: &str) {
        let _ = self.webview.load_url(url);
        let mut state = self.shared.lock();
        state.current_url = url.to_owned();
        state.dirty = true;
    }

    fn reload(&self) {
        let _ = self.webview.reload();
    }

    fn run(&self, action: AfterResume) {
        match action {
            AfterResume::Load(url) => self.load(&url),
            AfterResume::Reload => self.reload(),
        }
    }

    /// Reload a crashed page, unless it already crashed shortly after the
    /// last reload. Returns what to tell the user, if anything.
    fn recover_crashed_page(&mut self) -> Option<PageCrash> {
        let now = Instant::now();
        match crash_response(self.last_crash_reload, now) {
            PageCrash::Reloaded => {
                self.last_crash_reload = Some(now);
                self.crash_reported = false;
                self.reload();
                Some(PageCrash::Reloaded)
            }
            PageCrash::Repeated if self.crash_reported => None,
            PageCrash::Repeated => {
                self.crash_reported = true;
                Some(PageCrash::Repeated)
            }
        }
    }

    fn with_core<T>(
        &self,
        f: impl FnOnce(
            &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        let core =
            unsafe { self.webview.controller().CoreWebView2() }.map_err(|e| e.to_string())?;
        f(&core)
    }

    fn set_muted(&self, muted: bool) -> Result<(), String> {
        self.with_core(|core| {
            let core8: ICoreWebView2_8 = core.cast().map_err(|e| e.to_string())?;
            unsafe { core8.SetIsMuted(muted) }.map_err(|e| e.to_string())
        })
    }

    /// Whether the page can go back and forward in its history.
    fn history_state(&self) -> (bool, bool) {
        self.with_core(|core| {
            let mut back = Default::default();
            let mut forward = Default::default();
            unsafe {
                core.CanGoBack(&mut back).map_err(|e| e.to_string())?;
                core.CanGoForward(&mut forward).map_err(|e| e.to_string())?;
            }
            Ok((back.as_bool(), forward.as_bool()))
        })
        .unwrap_or((false, false))
    }

    /// Hide the page and ask WebView2 to put it to sleep. The callback is
    /// intentionally non-blocking because WebView2 dispatches it through the
    /// UI thread's message pump.
    fn suspend(&self) -> Result<(), String> {
        let controller = self.webview.controller();
        unsafe {
            controller
                .SetIsVisible(false)
                .map_err(|error| error.to_string())?;
        }
        let core = unsafe { controller.CoreWebView2() }.map_err(|error| error.to_string())?;
        let core3: ICoreWebView2_3 = core.cast().map_err(|error| error.to_string())?;
        let callback = TrySuspendCompletedHandler::create(Box::new(|result, successful| {
            if let Err(error) = result {
                log::warn!("WebView2 tile suspension failed: {error}");
            } else if !successful {
                log::warn!("WebView2 declined to suspend a frozen tile");
            }
            Ok(())
        }));
        unsafe {
            core3
                .TrySuspend(&callback)
                .map_err(|error| error.to_string())
        }
    }

    fn resume(&mut self, visible: bool) -> Result<(), String> {
        let controller = self.webview.controller();
        let resume_result = unsafe { controller.CoreWebView2() }
            .map_err(|error| error.to_string())
            .and_then(|core| {
                let core3: ICoreWebView2_3 = core.cast().map_err(|error| error.to_string())?;
                unsafe { core3.Resume().map_err(|error| error.to_string()) }
            });
        unsafe {
            controller
                .SetIsVisible(visible)
                .map_err(|error| error.to_string())?;
        }
        if let Some(action) = self.after_resume.take() {
            self.run(action);
        }
        if let Err(error) = resume_result {
            log::warn!("WebView2 resume API was unavailable; restored visibility instead: {error}");
        }
        Ok(())
    }
}

/// A tab of a browser tile. Tabs restored from a layout keep only their
/// address and title until they are first shown, so reopening a workspace
/// starts one page per tile rather than every saved tab at once.
struct HostTab {
    view: Option<TabView>,
    url: String,
    title: String,
    playing_audio: bool,
    favicon: Option<Favicon>,
    /// When this loaded tab was last hidden or heard; None while shown.
    hidden_since: Option<Instant>,
}

impl HostTab {
    fn loaded(view: TabView, url: &str, shown: bool) -> Self {
        Self {
            view: Some(view),
            url: url.to_owned(),
            title: String::new(),
            playing_audio: false,
            favicon: None,
            hidden_since: (!shown).then(Instant::now),
        }
    }

    fn unloaded(tab: &BrowserTab) -> Self {
        Self {
            view: None,
            url: tab.url.clone(),
            title: tab.title.clone(),
            playing_audio: false,
            favicon: tab.favicon.clone(),
            hidden_since: None,
        }
    }

    fn snapshot(&self) -> BrowserTab {
        BrowserTab {
            url: self.url.clone(),
            title: self.title.clone(),
            playing_audio: self.playing_audio,
            favicon: self.favicon.clone(),
            sleeping: self.view.is_none(),
        }
    }

    /// Whether a background tab has been idle long enough to sleep.
    fn ready_to_sleep(&mut self, now: Instant) -> bool {
        if self.view.is_none() {
            return false;
        }
        if self.playing_audio {
            self.hidden_since = Some(now);
            return false;
        }
        self.hidden_since
            .is_some_and(|since| now.duration_since(since) >= TAB_SLEEP_AFTER)
    }
}

pub struct BrowserHost {
    /// Every page of the tile. Each loaded tab is its own WebView inside the
    /// host window; only the active one is visible, so the tile's capture of
    /// the host window always shows it. Never empty.
    tabs: Vec<HostTab>,
    active_tab: usize,
    /// Tabs were opened, closed or switched since the last poll.
    tabs_dirty: bool,
    window: NativeWindow,
    active: bool,
    muted: bool,
    /// Last host/page geometry applied while active (physical px), to skip
    /// redundant positioning calls while glued to the tile.
    last_geometry: Option<BrowserGeometry>,
    /// While set, the host remains offscreen at its interactive size until
    /// WebView2 has had time to render the resized/zoomed page.
    reveal_at: Option<Instant>,
    /// While set, the host was just parked and its capture should stay
    /// paused until WebView2 has rendered the page at capture resolution.
    capture_resume_at: Option<Instant>,
    /// Backing resolution used while parked and captured. Kept in sync with
    /// the tile's physical display size so preview and live modes are 1:1.
    capture_size: (i32, i32),
    /// Desired sleeping state. WebView2 suspension itself is best-effort, but
    /// the controller remains hidden until the user explicitly resumes it.
    suspended: bool,
    /// The tile's own page zoom, multiplied into every zoom this host sets.
    page_zoom: f64,
}

enum AfterResume {
    Load(String),
    Reload,
}

impl BrowserHost {
    fn new(context: &mut WebContext, url: &str, signals: &EngineSignals) -> Result<Self, String> {
        let mut host = Self::empty()?;
        let view = host.build_view(context, url, signals, true)?;
        host.tabs.push(HostTab::loaded(view, url, true));
        Ok(host)
    }

    /// A parked host window without tabs yet. It must get one before it is
    /// used: see [`Self::adopt_tab`].
    fn empty() -> Result<Self, String> {
        static REGISTERED: OnceLock<Result<(), String>> = OnceLock::new();
        REGISTERED.get_or_init(register_window_class).clone()?;

        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("PluriviewBrowserHost"),
                w!("Pluriview Browser"),
                WS_POPUP | WS_CLIPCHILDREN,
                PARK_X,
                PARK_Y,
                WIDTH,
                HEIGHT,
                None,
                None,
                None,
                None,
            )
            .map_err(|error| error.to_string())?
        };
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }

        Ok(Self {
            tabs: Vec::new(),
            active_tab: 0,
            tabs_dirty: true,
            window: NativeWindow(hwnd),
            active: false,
            muted: false,
            last_geometry: None,
            reveal_at: None,
            capture_resume_at: None,
            capture_size: (WIDTH, HEIGHT),
            suspended: false,
            page_zoom: 1.0,
        })
    }

    /// Create a WebView for one tab inside this host's window.
    fn build_view(
        &self,
        context: &mut WebContext,
        url: &str,
        signals: &EngineSignals,
        visible: bool,
    ) -> Result<TabView, String> {
        let shared = Arc::new(Mutex::new(SharedState {
            current_url: url.to_owned(),
            ..Default::default()
        }));

        let nav_shared = shared.clone();
        let title_shared = shared.clone();
        let new_window_shared = shared.clone();
        let download_shared = shared.clone();
        let load_shared = shared.clone();
        let interactive = Arc::new(std::sync::atomic::AtomicBool::new(self.active));
        let popup_interactive = interactive.clone();

        let webview = WebViewBuilder::new_with_web_context(context)
            // All tiles share one WebView2 environment/profile, so this must
            // be identical on every builder. Runtime 120+ supports the flag;
            // uBOL itself requires Chromium/WebView2 122+.
            .with_browser_extensions_enabled(true)
            .with_additional_browser_args(BROWSER_ARGUMENTS)
            // Preserve transparent page backgrounds for overlay widgets.
            .with_transparent(true)
            // Captured hosts start parked; focus belongs to the canvas until
            // the user explicitly enters browser interaction.
            .with_focused(false)
            // Background tabs start hidden so they never cover the tab the
            // tile is showing.
            .with_visible(visible)
            // Wry enables autoplay by default, which passes WebView2 the
            // `no-user-gesture-required` Chromium policy. Do not opt into
            // that policy: restored media pages (notably YouTube watch URLs)
            // should wait for the user to press Play after app startup.
            .with_autoplay(false)
            .with_initialization_script(MEDIA_AUTOPLAY_GUARD)
            .with_url(url)
            .with_bounds(parked_bounds())
            .with_download_started_handler(move |_, _| {
                let mut state = download_shared.lock();
                state.download_blocked = true;
                state.dirty = true;
                false
            })
            .with_navigation_handler(move |url| {
                if !is_allowed_navigation(&url) {
                    return false;
                }
                if url.starts_with("http") {
                    let mut state = nav_shared.lock();
                    state.current_url = url;
                    state.dirty = true;
                }
                true
            })
            .with_on_page_load_handler(move |event, _| {
                let mut state = load_shared.lock();
                state.loading = matches!(event, PageLoadEvent::Started);
                state.dirty = true;
            })
            .with_document_title_changed_handler(move |title| {
                let mut state = title_shared.lock();
                state.current_title = Some(title);
                state.dirty = true;
            })
            .with_new_window_req_handler(move |url, features| {
                let interactive = popup_interactive.load(std::sync::atomic::Ordering::Relaxed);
                if opens_as_popup(interactive, features.size.is_some()) && is_web_url(&url) {
                    // Sign-in flows ("Continue with Google/Discord") open a
                    // sized popup and talk back to this page through
                    // window.opener; replacing the tile's page breaks them.
                    return NewWindowResponse::Allow;
                }
                if is_web_url(&url) {
                    let mut state = new_window_shared.lock();
                    state.pending_navigation = Some(url);
                    state.dirty = true;
                }
                NewWindowResponse::Deny
            })
            .build_as_child(&self.window)
            .map_err(|error| error.to_string())?;
        if let Err(error) = register_permission_policy(&webview, interactive.clone()) {
            log::warn!("Browser permission requests use WebView2 defaults: {error}");
        }
        if let Err(error) =
            register_process_failure_handler(&webview, shared.clone(), signals.lost.clone())
        {
            log::warn!("Browser tiles cannot detect crashed pages: {error}");
        }
        if let Err(error) = register_audio_handler(&webview, shared.clone()) {
            log::warn!("Browser tiles cannot report playing audio: {error}");
        }
        if let Err(error) = register_history_handler(&webview, shared.clone()) {
            log::warn!("Browser tiles cannot follow page history: {error}");
        }
        if let Err(error) = register_favicon_handler(&webview, shared.clone()) {
            log::warn!("Browser tabs cannot show site icons: {error}");
        }
        if let Err(error) = register_context_menu_handler(&webview, shared.clone()) {
            log::warn!("Browser pages cannot offer Open Link in New Tile: {error}");
        }
        if let Err(error) = register_engine_update_handler(&webview, signals.update_ready.clone()) {
            log::warn!("Browser tiles cannot detect WebView2 updates: {error}");
        }

        let view = TabView {
            webview,
            shared,
            interactive,
            last_url_check: None,
            after_resume: None,
            last_crash_reload: None,
            crash_reported: false,
        };
        if self.muted {
            if let Err(error) = view.set_muted(true) {
                log::warn!("A new browser tab could not be muted: {error}");
            }
        }
        Ok(view)
    }

    /// Replace the single tab a new host opened with `tabs`, keeping the
    /// loaded page as the active one. The others load when first shown.
    fn restore_tabs(&mut self, tabs: &[BrowserTab], active: usize) {
        if tabs.len() < 2 || active >= tabs.len() || self.tabs.len() != 1 {
            return;
        }
        let mut loaded = self.tabs.pop().expect("a new host has one tab");
        loaded.title.clone_from(&tabs[active].title);
        self.tabs = tabs.iter().map(HostTab::unloaded).collect();
        self.tabs[active] = loaded;
        self.active_tab = active;
        self.tabs_dirty = true;
    }

    fn view(&self) -> Option<&TabView> {
        self.tabs.get(self.active_tab)?.view.as_ref()
    }

    fn webview(&self) -> Option<&WebView> {
        self.view().map(|view| &view.webview)
    }

    fn loaded_views(&self) -> impl Iterator<Item = &TabView> {
        self.tabs.iter().filter_map(|tab| tab.view.as_ref())
    }

    pub fn hwnd(&self) -> isize {
        self.window.0 .0 as isize
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn is_preparing_interaction(&self) -> bool {
        self.reveal_at.is_some()
    }

    /// True once, when a host that was just parked has settled at capture
    /// resolution and its capture can resume.
    pub fn take_settled_capture(&mut self) -> bool {
        if self
            .capture_resume_at
            .is_some_and(|resume_at| Instant::now() >= resume_at)
        {
            self.capture_resume_at = None;
            return true;
        }
        false
    }

    pub fn is_settling_capture(&self) -> bool {
        self.capture_resume_at.is_some()
    }

    pub fn is_muted(&self) -> bool {
        self.muted
    }

    /// Open `url` in a new tab after the active one, showing it if `activate`.
    fn open_tab(
        &mut self,
        context: &mut WebContext,
        url: &str,
        signals: &EngineSignals,
        activate: bool,
    ) -> Result<(), String> {
        if self.suspended {
            return Err("Unfreeze this browser tile to open a tab".to_owned());
        }
        if !is_web_url(url) {
            return Err("Only web addresses can open in a browser tile".to_owned());
        }
        let view = self.build_view(context, url, signals, false)?;
        let index = (self.active_tab + 1).min(self.tabs.len());
        self.tabs.insert(index, HostTab::loaded(view, url, false));
        self.tabs_dirty = true;
        if activate {
            self.select_tab(context, signals, index)?;
        }
        Ok(())
    }

    /// Show tab `index`, loading it first if it was restored unloaded.
    fn select_tab(
        &mut self,
        context: &mut WebContext,
        signals: &EngineSignals,
        index: usize,
    ) -> Result<(), String> {
        if index >= self.tabs.len() || self.suspended {
            return Ok(());
        }
        if self.tabs[index].view.is_none() {
            let url = self.tabs[index].url.clone();
            let view = self.build_view(context, &url, signals, false)?;
            self.tabs[index].view = Some(view);
        }
        if index != self.active_tab {
            if let Some(previous) = self.webview() {
                let _ = previous.set_visible(false);
            }
            self.tabs[self.active_tab].hidden_since = Some(Instant::now());
            self.active_tab = index;
            self.tabs_dirty = true;
        }
        self.tabs[index].hidden_since = None;
        self.show_active_view();
        Ok(())
    }

    /// Move tab `from` to position `to`, keeping the same tab shown.
    fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active_tab = moved_index(self.active_tab, from, to);
        self.tabs_dirty = true;
    }

    /// Unload background tabs that have been idle for [`TAB_SLEEP_AFTER`].
    fn sleep_idle_tabs(&mut self, now: Instant) {
        if self.suspended {
            return;
        }
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            if index == self.active_tab || !tab.ready_to_sleep(now) {
                continue;
            }
            if let Some(url) = tab.view.as_ref().and_then(TabView::live_url) {
                tab.url = url;
            }
            tab.view = None;
            tab.hidden_since = None;
            tab.playing_audio = false;
            self.tabs_dirty = true;
        }
    }

    /// Close tab `index`. The last tab stays: closing it would leave the tile
    /// without a page.
    fn close_tab(
        &mut self,
        context: &mut WebContext,
        signals: &EngineSignals,
        index: usize,
    ) -> Result<(), String> {
        if self.tabs.len() < 2 || index >= self.tabs.len() {
            return Ok(());
        }
        self.take_tab(context, signals, index).map(drop)
    }

    /// Remove tab `index` with its page still loaded, for another host to
    /// adopt. If it was shown, the tab that takes its place is shown. Taking
    /// the last tab leaves the host empty, and the caller then drops it.
    fn take_tab(
        &mut self,
        context: &mut WebContext,
        signals: &EngineSignals,
        index: usize,
    ) -> Result<HostTab, String> {
        if index >= self.tabs.len() {
            return Err("That tab is no longer open".to_owned());
        }
        let taking_active = index == self.active_tab;
        let tab = self.tabs.remove(index);
        self.tabs_dirty = true;
        if self.tabs.is_empty() {
            self.active_tab = 0;
        } else if index < self.active_tab {
            self.active_tab -= 1;
        } else if taking_active {
            // Like a browser, move to the tab that took this one's place.
            let next = index.min(self.tabs.len() - 1);
            self.active_tab = next;
            self.tabs[next].hidden_since = None;
            if self.tabs[next].view.is_none() {
                let url = self.tabs[next].url.clone();
                match self.build_view(context, &url, signals, false) {
                    Ok(view) => self.tabs[next].view = Some(view),
                    Err(error) => log::warn!("Could not load the next browser tab: {error}"),
                }
            }
            self.show_active_view();
        }
        Ok(tab)
    }

    /// Add `tab`, taken from another host with its page loaded, at `slot` and
    /// show it. On failure the tab is handed back with the reason.
    fn adopt_tab(
        &mut self,
        context: &mut WebContext,
        signals: &EngineSignals,
        mut tab: HostTab,
        slot: usize,
    ) -> Result<(), Box<(HostTab, String)>> {
        if self.suspended {
            return Err(Box::new((
                tab,
                "Unfreeze this browser tile to add tabs".to_owned(),
            )));
        }
        if let Some(view) = tab.view.as_ref() {
            // Hidden until shown at this host's size, so it never flashes at
            // the old one.
            let _ = view.webview.set_visible(false);
            if let Err(error) = view.webview.reparent(self.hwnd()) {
                return Err(Box::new((tab, error.to_string())));
            }
            // Its pop-ups and input follow the new window from here on.
            unsafe {
                let _ = view
                    .webview
                    .controller()
                    .NotifyParentWindowPositionChanged();
            }
            view.interactive
                .store(self.active, std::sync::atomic::Ordering::Relaxed);
            if let Err(error) = view.set_muted(self.muted) {
                log::warn!("A moved browser tab kept its old mute state: {error}");
            }
        }
        tab.hidden_since = None;
        let index = slot.min(self.tabs.len());
        if !self.tabs.is_empty() && index <= self.active_tab {
            self.active_tab += 1;
        }
        self.tabs.insert(index, tab);
        self.tabs_dirty = true;
        if let Err(error) = self.select_tab(context, signals, index) {
            log::warn!("Could not show a moved browser tab: {error}");
        }
        Ok(())
    }

    /// Size, zoom and reveal the active tab's WebView for the host's mode.
    fn show_active_view(&self) {
        let Some(webview) = self.webview() else {
            return;
        };
        let (x, y, width, height) = match (self.active, self.last_geometry) {
            (true, Some(geometry)) => (
                geometry.page_x,
                geometry.page_y,
                geometry.page_width,
                geometry.page_height,
            ),
            _ => (0, 0, self.capture_size.0, self.capture_size.1),
        };
        set_page_geometry(webview, (x, y), (width, height), self.zoom_for_width(width));
        let _ = webview.set_visible(true);
        if self.active && self.reveal_at.is_none() {
            let _ = webview.focus();
        }
    }

    /// Match an inactive browser's capture backing to 2× its displayed
    /// physical size. The zoom changes with the backing width, preserving the
    /// same CSS viewport while keeping the canvas preview reasonably sharp.
    pub fn sync_capture_size(&mut self, width: i32, height: i32) {
        if self.active || self.suspended {
            return;
        }
        let size = (width.max(1), height.max(1));
        if self.capture_size == size {
            return;
        }
        self.capture_size = size;
        unsafe {
            let _ = SetWindowPos(
                self.window.0,
                HWND_BOTTOM,
                PARK_X,
                PARK_Y,
                size.0,
                size.1,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
        if let Some(webview) = self.webview() {
            set_page_geometry(webview, (0, 0), size, self.zoom_for_width(size.0));
        }
        // A tile that just parked can settle at a new capture size, for
        // example after its tab strip collapsed while it was interactive.
        if self.capture_resume_at.is_some() {
            self.capture_resume_at = Some(Instant::now() + PARK_SETTLE_DELAY);
        }
    }

    /// WebView2 zoom for a page `width` physical pixels wide: the tile-size
    /// zoom that keeps the layout at WIDTH CSS pixels, times the tile's own
    /// page zoom.
    fn zoom_for_width(&self, width: i32) -> f64 {
        (width as f64 / WIDTH as f64 * self.page_zoom).clamp(MIN_ZOOM, MAX_ZOOM)
    }

    /// Apply the tile's page zoom (1.0 = the page's normal size).
    pub fn set_page_zoom(&mut self, zoom: f32) {
        let zoom = f64::from(zoom.clamp(MIN_PAGE_ZOOM, MAX_PAGE_ZOOM));
        if (self.page_zoom - zoom).abs() < 1e-4 || self.suspended {
            return;
        }
        self.page_zoom = zoom;
        let width = match (self.active, self.last_geometry) {
            (true, Some(geometry)) => geometry.page_width,
            _ => self.capture_size.0,
        };
        if let Some(webview) = self.webview() {
            let _ = webview.zoom(self.zoom_for_width(width));
        }
    }

    /// Scroll the page under a normalized point by `delta` CSS pixels
    /// (positive scrolls down), without entering interaction mode.
    pub fn scroll_page(&self, u: f32, v: f32, delta: f32) {
        if self.suspended || !u.is_finite() || !v.is_finite() || !delta.is_finite() {
            return;
        }
        if let Some(webview) = self.webview() {
            let script = format!(
                "{SCROLL_PAGE_SCRIPT}({}, {}, {});",
                u.clamp(0.0, 1.0),
                v.clamp(0.0, 1.0),
                delta
            );
            let _ = webview.evaluate_script(&script);
        }
    }

    /// Look for the page's main video. The answer arrives through
    /// [`BrowserUpdate::video_rect`].
    pub fn request_video_rect(&self) {
        if self.suspended {
            return;
        }
        let Some(view) = self.view() else {
            return;
        };
        let shared = view.shared.clone();
        let requested =
            view.webview
                .evaluate_script_with_callback(FIND_VIDEO_SCRIPT, move |json| {
                    let mut state = shared.lock();
                    state.video_rect = Some(parse_video_rect(&json));
                    state.dirty = true;
                });
        if requested.is_err() {
            let mut state = view.shared.lock();
            state.video_rect = Some(None);
            state.dirty = true;
        }
    }

    /// The active tab's address.
    pub fn current_url(&self) -> String {
        match self.view() {
            Some(view) => view.current_url(),
            None => self.tabs[self.active_tab].url.clone(),
        }
    }

    /// The active page's address read straight from WebView2, or None while
    /// the tab has no WebView or sits on a non-web page.
    pub fn live_url(&self) -> Option<String> {
        self.view()?.live_url()
    }

    /// Every tab's address for saving, read live where the page is loaded.
    pub fn tab_urls(&self) -> Vec<String> {
        self.tabs
            .iter()
            .map(|tab| {
                tab.view
                    .as_ref()
                    .and_then(TabView::live_url)
                    .unwrap_or_else(|| tab.url.clone())
            })
            .collect()
    }

    /// Drain pending title/URL changes from every tab. Pages that asked for
    /// a new window are reported in [`BrowserUpdate::new_window_requests`].
    /// Call once per frame.
    pub fn poll(&mut self) -> BrowserUpdate {
        if self.suspended {
            return BrowserUpdate::default();
        }
        let mut update = BrowserUpdate::default();
        let mut changed = std::mem::take(&mut self.tabs_dirty);
        let active_tab = self.active_tab;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let HostTab {
                view,
                url,
                title,
                playing_audio,
                favicon,
                ..
            } = tab;
            let Some(view) = view.as_mut() else {
                continue;
            };
            view.refresh_live_url();
            let (navigate, crashed, video_rect, icon) = {
                let mut state = view.shared.lock();
                if !state.dirty {
                    continue;
                }
                state.dirty = false;
                url.clone_from(&state.current_url);
                if let Some(page_title) = &state.current_title {
                    title.clone_from(page_title);
                }
                *playing_audio = state.playing_audio;
                update.download_blocked |= std::mem::take(&mut state.download_blocked);
                update
                    .new_tile_requests
                    .append(&mut state.new_tile_requests);
                (
                    state.pending_navigation.take(),
                    std::mem::take(&mut state.page_crashed),
                    state.video_rect.take(),
                    state.favicon.take(),
                )
            };
            changed = true;
            if let Some((uri, png)) = icon {
                *favicon = decode_favicon(uri, &png);
            }
            if let Some(url) = navigate {
                update.new_window_requests.push(url);
            }
            if crashed {
                update.page_crash = view.recover_crashed_page().or(update.page_crash);
            }
            if index == active_tab {
                update.video_rect = video_rect;
            }
        }
        if !changed {
            return update;
        }

        let active = &self.tabs[self.active_tab];
        update.changed = true;
        update.title = Some(active.title.clone()).filter(|title| !title.is_empty());
        update.url = Some(active.url.clone());
        update.playing_audio = self.tabs.iter().any(|tab| tab.playing_audio);
        if let Some(view) = active.view.as_ref() {
            update.loading = view.shared.lock().loading;
            (update.can_go_back, update.can_go_forward) = view.history_state();
        }
        update.tabs = self.tabs.iter().map(HostTab::snapshot).collect();
        update.active_tab = self.active_tab;
        update
    }

    /// Position the host over the visible part of `page_rect` inside
    /// `visible_rect` (egui points, client coordinates of `parent`). The
    /// WebView retains the full page size and is offset inside the clipped
    /// host, so viewport clipping never changes the page's layout or scale.
    pub fn place(
        &mut self,
        parent: HWND,
        page_rect: egui::Rect,
        visible_rect: egui::Rect,
        pixels_per_point: f32,
        take_focus: bool,
    ) {
        if self.suspended {
            return;
        }
        let mut host_origin = POINT {
            x: (visible_rect.min.x * pixels_per_point).round() as i32,
            y: (visible_rect.min.y * pixels_per_point).round() as i32,
        };
        unsafe {
            let _ = ClientToScreen(parent, &mut host_origin);
        }
        let geometry = browser_geometry(
            page_rect,
            visible_rect,
            pixels_per_point,
            (host_origin.x, host_origin.y),
        );
        // Interactive hosts must share the canvas's z-order band, otherwise
        // an always-on-top canvas covers the page that is receiving input.
        let parent_topmost =
            unsafe { GetWindowLongW(parent, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST.0 != 0;
        let host_topmost =
            unsafe { GetWindowLongW(self.window.0, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST.0 != 0;
        let insert_after = if parent_topmost {
            HWND_TOPMOST
        } else if host_topmost {
            HWND_NOTOPMOST
        } else {
            HWND_TOP
        };
        if !take_focus
            && self.active
            && self.reveal_at.is_none()
            && self.last_geometry == Some(geometry)
            && parent_topmost == host_topmost
        {
            return;
        }

        // Keep the page's apparent scale identical to the captured texture:
        // layout width stays WIDTH/dpi CSS px in both modes.
        let zoom = self.zoom_for_width(geometry.page_width);
        let resize_webview = |webview: &WebView| {
            set_page_geometry(
                webview,
                (geometry.page_x, geometry.page_y),
                (geometry.page_width, geometry.page_height),
                zoom,
            );
        };

        if take_focus {
            // Resize both the offscreen host and its child, then leave them
            // parked long enough for WebView2's asynchronous compositor to
            // settle. The app freezes the captured tile during this window.
            unsafe {
                let _ = SetWindowPos(
                    self.window.0,
                    HWND_BOTTOM,
                    PARK_X,
                    PARK_Y,
                    geometry.host_width,
                    geometry.host_height,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
            if let Some(webview) = self.webview() {
                resize_webview(webview);
            }
            self.set_active(true);
            self.last_geometry = Some(geometry);
            self.reveal_at = Some(Instant::now() + INTERACTION_PREP_DELAY);
            // Re-entered before a previous park settled: the app resumes the
            // capture after the reveal instead.
            self.capture_resume_at = None;
            return;
        }

        if let Some(reveal_at) = self.reveal_at {
            // If layout changed while preparing (for example, the context
            // menu closed), settle once more at the final tile dimensions.
            if self.last_geometry != Some(geometry) {
                unsafe {
                    let _ = SetWindowPos(
                        self.window.0,
                        HWND_BOTTOM,
                        PARK_X,
                        PARK_Y,
                        geometry.host_width,
                        geometry.host_height,
                        SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                }
                if let Some(webview) = self.webview() {
                    resize_webview(webview);
                }
                self.last_geometry = Some(geometry);
                self.reveal_at = Some(Instant::now() + INTERACTION_PREP_DELAY);
                return;
            }
            if Instant::now() < reveal_at {
                return;
            }

            unsafe {
                let _ = SetWindowPos(
                    self.window.0,
                    insert_after,
                    geometry.host_x,
                    geometry.host_y,
                    geometry.host_width,
                    geometry.host_height,
                    SWP_SHOWWINDOW,
                );
                let _ = SetForegroundWindow(self.window.0);
            }
            if let Some(webview) = self.webview() {
                let _ = webview.focus();
            }
            self.reveal_at = None;
            self.last_geometry = Some(geometry);
            return;
        }

        unsafe {
            let _ = SetWindowPos(
                self.window.0,
                insert_after,
                geometry.host_x,
                geometry.host_y,
                geometry.host_width,
                geometry.host_height,
                SWP_SHOWWINDOW | SWP_NOACTIVATE,
            );
        }
        // While already active, resize after moving so a panning or zooming
        // tile stays glued to its new position.
        if let Some(webview) = self.webview() {
            resize_webview(webview);
        }
        self.set_active(true);
        self.last_geometry = Some(geometry);
    }

    fn set_active(&mut self, active: bool) {
        self.active = active;
        for view in self.loaded_views() {
            view.interactive
                .store(active, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Give keyboard focus back to an interactive page after the user
    /// clicked the tile's toolbar or tabs, which briefly focused the canvas.
    pub fn refocus(&self) {
        if !self.active || self.reveal_at.is_some() {
            return;
        }
        unsafe {
            let _ = SetForegroundWindow(self.window.0);
        }
        if let Some(webview) = self.webview() {
            let _ = webview.focus();
        }
    }

    /// Move the host back offscreen at capture resolution. Audio keeps
    /// playing and Windows Graphics Capture keeps rendering it.
    pub fn park(&mut self) {
        let size = self.capture_size;
        let (width, height) = size;
        unsafe {
            let _ = SetWindowPos(
                self.window.0,
                HWND_BOTTOM,
                PARK_X,
                PARK_Y,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
        if let Some(webview) = self.webview() {
            set_page_geometry(webview, (0, 0), size, self.zoom_for_width(width));
        }
        if self.active {
            self.capture_resume_at = Some(Instant::now() + PARK_SETTLE_DELAY);
        }
        self.set_active(false);
        self.last_geometry = None;
        self.reveal_at = None;
    }

    /// Hide every tab and ask WebView2 to put their pages to sleep.
    pub fn suspend(&mut self) -> Result<(), String> {
        if self.suspended {
            return Ok(());
        }
        self.park();
        if self.view().is_none() {
            return Err("browser is closed".to_owned());
        }
        self.suspended = true;
        let mut result = Ok(());
        for view in self.loaded_views() {
            if let Err(error) = view.suspend() {
                result = Err(error);
            }
        }
        result
    }

    /// Wake a manually frozen tile before its capture session is restarted.
    pub fn resume(&mut self) -> Result<(), String> {
        if !self.suspended {
            return Ok(());
        }
        if self.view().is_none() {
            return Err("browser is closed".to_owned());
        }
        let active_tab = self.active_tab;
        let mut result = Ok(());
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            if let Some(view) = tab.view.as_mut() {
                if let Err(error) = view.resume(index == active_tab) {
                    result = Err(error);
                }
            }
        }
        self.suspended = false;
        self.park();
        result
    }

    /// True when keyboard focus belongs to this host (or one of the WebView's
    /// child windows). Used to park on focus loss.
    pub fn owns_foreground(&self) -> bool {
        let foreground = unsafe { GetForegroundWindow() };
        foreground == self.window.0 || unsafe { IsChild(self.window.0, foreground).as_bool() }
    }

    pub fn load(&self, url: &str) {
        if !is_web_url(url) {
            log::warn!("Blocked non-web browser navigation");
            return;
        }
        if let Some(view) = self.view() {
            view.load(url);
        }
    }

    /// Record the address a tile held on `about:blank` will navigate to once
    /// filtering is live, so its label and saved layout show the real target.
    fn set_deferred_url(&self, url: &str) {
        if let Some(view) = self.view() {
            let mut state = view.shared.lock();
            state.current_url = url.to_owned();
            state.dirty = true;
        }
    }

    /// Run `action` on the active tab and reload the other loaded tabs, now
    /// or when the tile wakes up.
    fn reload_all(&mut self, active_action: Option<AfterResume>) {
        let active_tab = self.active_tab;
        let suspended = self.suspended;
        let mut active_action = active_action;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let Some(view) = tab.view.as_mut() else {
                continue;
            };
            let action = if index == active_tab {
                active_action.take().unwrap_or(AfterResume::Reload)
            } else {
                AfterResume::Reload
            };
            if suspended {
                view.after_resume.get_or_insert(action);
            } else {
                view.run(action);
            }
        }
    }

    pub fn go_back(&self) {
        self.with_core(|core| unsafe { core.GoBack().map_err(|e| e.to_string()) })
            .ok();
    }

    pub fn go_forward(&self) {
        self.with_core(|core| unsafe { core.GoForward().map_err(|e| e.to_string()) })
            .ok();
    }

    pub fn reload(&self) {
        if let Some(view) = self.view() {
            view.reload();
        }
    }

    /// PID of the WebView2 browser process (shared by every host on the
    /// same profile). Roots the stream audio monitor's loopback capture.
    pub fn browser_process_id(&self) -> Option<u32> {
        self.with_core(|core| {
            let mut pid = 0u32;
            unsafe { core.BrowserProcessId(&mut pid) }.map_err(|e| e.to_string())?;
            Ok(pid)
        })
        .ok()
        .filter(|pid| *pid != 0)
    }

    /// Mute or unmute every tab of the tile.
    pub fn set_muted(&mut self, muted: bool) -> Result<(), String> {
        self.view().ok_or("browser is closed")?.set_muted(muted)?;
        for view in self.loaded_views() {
            let _ = view.set_muted(muted);
        }
        self.muted = muted;
        Ok(())
    }

    /// The extension WebView2 already keeps in the shared profile under `id`,
    /// if it is still there. Extensions survive across sessions, so reusing
    /// one avoids the reinstall that would clear uBOL's registrations.
    fn installed_browser_extension(
        &self,
        id: &str,
    ) -> Result<Option<ICoreWebView2BrowserExtension>, String> {
        self.with_core(|core| {
            let profile = extension_profile(core)?;

            let listed = Rc::new(RefCell::new(None));
            let completed_result = listed.clone();
            ProfileGetBrowserExtensionsCompletedHandler::wait_for_async_operation(
                Box::new(move |handler| unsafe {
                    profile
                        .GetBrowserExtensions(&handler)
                        .map_err(webview2_com::Error::WindowsError)
                }),
                Box::new(move |result, extensions| {
                    result?;
                    *completed_result.borrow_mut() = extensions;
                    Ok(())
                }),
            )
            .map_err(|error| format!("Installed extensions could not be listed: {error}"))?;

            let Some(extensions) = listed.borrow_mut().take() else {
                return Ok(None);
            };
            let mut count = 0u32;
            unsafe { extensions.Count(&mut count) }.map_err(|e| e.to_string())?;
            for index in 0..count {
                let extension =
                    unsafe { extensions.GetValueAtIndex(index) }.map_err(|e| e.to_string())?;
                if extension_id(&extension)? == id {
                    return Ok(Some(extension));
                }
            }
            Ok(None)
        })
    }

    fn install_browser_extension(
        &self,
        extension_dir: &Path,
    ) -> Result<ICoreWebView2BrowserExtension, String> {
        self.with_core(|core| {
            let profile = extension_profile(core)?;

            let installed = Rc::new(RefCell::new(None));
            let completed_result = installed.clone();
            let path = WebViewHString::from(extension_dir);
            ProfileAddBrowserExtensionCompletedHandler::wait_for_async_operation(
                Box::new(move |handler| unsafe {
                    profile
                        .AddBrowserExtension(&path, &handler)
                        .map_err(webview2_com::Error::WindowsError)
                }),
                Box::new(move |result, extension| {
                    result?;
                    *completed_result.borrow_mut() = extension;
                    Ok(())
                }),
            )
            .map_err(|error| format!("uBlock Origin Lite could not be installed: {error}"))?;

            let extension = installed
                .borrow_mut()
                .take()
                .ok_or_else(|| "WebView2 did not return the installed extension".to_owned())?;
            Ok(extension)
        })
    }

    fn with_core<T>(
        &self,
        f: impl FnOnce(
            &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        self.view().ok_or("browser is closed")?.with_core(f)
    }
}

impl Drop for BrowserHost {
    fn drop(&mut self) {
        self.tabs.clear();
        unsafe {
            let _ = DestroyWindow(self.window.0);
        }
    }
}

/// Owns every browser tile's host window plus the single WebView2 profile
/// they share (cookies/logins live in one Pluriview-specific data folder).
/// Lives on the UI thread; wry WebViews are not Send/Sync.
pub struct BrowserManager {
    context: WebContext,
    hosts: HashMap<PreviewId, BrowserHost>,
    extension_dir: Option<PathBuf>,
    prepared_extension: Option<PathBuf>,
    extension_preparation: Option<ExtensionPreparationTask>,
    extension_initialized: bool,
    extension: Option<ICoreWebView2BrowserExtension>,
    adblock_enabled: bool,
    adblock_error: Option<String>,
    /// When uBOL should be filtering again after an install or an enable.
    /// See [`ADBLOCK_SETTLE`].
    adblock_settle_until: Option<Instant>,
    /// Tiles parked on `about:blank` until that moment, and where they go.
    deferred_loads: HashMap<PreviewId, String>,
    signals: EngineSignals,
    /// Set after [`Self::shut_down_engine`] until the old browser process
    /// has exited, so new tiles start on a fresh (possibly updated) engine.
    engine_shutdown: Option<EngineShutdown>,
    /// Result slot of a Clear Browser Data request still in progress.
    clearing_data: Option<ClearDataSlot>,
}

/// Filled by WebView2's completion callback for Clear Browser Data.
type ClearDataSlot = Arc<Mutex<Option<Result<(), String>>>>;

/// The old WebView2 browser process, while it shuts down.
struct EngineShutdown {
    process: Option<HANDLE>,
    deadline: Instant,
}

impl EngineShutdown {
    fn finished(&self) -> bool {
        Instant::now() >= self.deadline
            || self
                .process
                .is_none_or(|process| unsafe { WaitForSingleObject(process, 0) } == WAIT_OBJECT_0)
    }
}

impl Drop for EngineShutdown {
    fn drop(&mut self) {
        if let Some(process) = self.process.take() {
            unsafe {
                let _ = CloseHandle(process);
            }
        }
    }
}

impl BrowserManager {
    #[cfg(pluriview_performance)]
    pub(crate) fn set_performance_profile(&mut self, directory: &std::path::Path) {
        assert!(self.hosts.is_empty());
        self.context = WebContext::new(Some(directory.join("webview2")));
        self.extension_dir = Some(directory.join("extensions").join("ubol"));
    }

    pub fn new() -> Self {
        let data_dir = directories::ProjectDirs::from("com", "pluriview", "Pluriview")
            .map(|dirs| dirs.data_dir().to_owned());
        let webview_data_dir = data_dir.as_ref().map(|dir| dir.join("webview2"));
        Self {
            context: WebContext::new(webview_data_dir),
            hosts: HashMap::new(),
            extension_dir: data_dir.map(|dir| dir.join("extensions").join("ubol")),
            prepared_extension: None,
            extension_preparation: None,
            extension_initialized: false,
            extension: None,
            adblock_enabled: true,
            adblock_error: None,
            adblock_settle_until: None,
            deferred_loads: HashMap::new(),
            signals: EngineSignals::default(),
            engine_shutdown: None,
            clearing_data: None,
        }
    }

    /// Create a host for `tabs`, loading the `active` one, and register it
    /// under `id`. Returns the host HWND for capture.
    pub fn create(
        &mut self,
        id: PreviewId,
        tabs: &[BrowserTab],
        active: usize,
    ) -> Result<isize, String> {
        let url = tabs.get(active).map(|tab| tab.url.as_str()).unwrap_or("");
        // Hold the first real navigation on about:blank until uBOL has been
        // installed and enabled, avoiding an initial burst of unfiltered
        // requests. Extension failure is non-fatal: browser tiles still work.
        let initialize_extension = self.adblock_enabled
            && !self.extension_initialized
            && self.prepared_extension.is_some();
        if self.adblock_enabled
            && !self.extension_initialized
            && self.prepared_extension.is_none()
            && self.adblock_error.is_none()
        {
            return Err("The ad blocker is still being prepared".to_owned());
        }
        let hold_navigation = initialize_extension || self.adblock_settling();
        let initial_url = if hold_navigation { "about:blank" } else { url };
        let mut host = BrowserHost::new(&mut self.context, initial_url, &self.signals)?;
        host.restore_tabs(tabs, active);

        if initialize_extension {
            self.extension_initialized = true;
            let extension_dir = self
                .prepared_extension
                .clone()
                .expect("prepared extension is present for initialization");
            let activated = activate_adblock(&host, &extension_dir, self.adblock_enabled);
            self.apply_activation(activated);
        }

        if hold_navigation {
            if self.adblock_settling() {
                host.set_deferred_url(url);
                self.deferred_loads.insert(id, url.to_owned());
            } else {
                host.load(url);
            }
        }

        let hwnd = host.hwnd();
        self.hosts.insert(id, host);
        Ok(hwnd)
    }

    /// Start filesystem-heavy extension preparation away from the UI thread.
    pub fn start_extension_preparation(&mut self) {
        if !self.adblock_enabled
            || self.extension_initialized
            || self.prepared_extension.is_some()
            || self.extension_preparation.is_some()
        {
            return;
        }
        let Some(extension_dir) = self.extension_dir.clone() else {
            self.adblock_error =
                Some("Pluriview could not determine its application data directory".to_owned());
            return;
        };

        self.adblock_error = None;
        let progress = Arc::new(AtomicU32::new(0));
        let worker_progress = progress.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result =
                prepare_ubol_with_progress(&extension_dir, &worker_progress).map(|_| extension_dir);
            let _ = sender.send(result);
        });
        self.extension_preparation = Some(ExtensionPreparationTask { progress, receiver });
    }

    /// Poll the worker without blocking the UI thread.
    pub fn poll_extension_preparation(&mut self) -> ExtensionPreparationStatus {
        let completed =
            self.extension_preparation
                .as_ref()
                .and_then(|task| match task.receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(TryRecvError::Disconnected) => Some(Err(
                        "The ad blocker preparation worker stopped unexpectedly".to_owned(),
                    )),
                    Err(TryRecvError::Empty) => None,
                });

        if let Some(result) = completed {
            self.extension_preparation = None;
            match result {
                Ok(extension_dir) => {
                    self.prepared_extension = Some(extension_dir);
                    self.adblock_error = None;
                }
                Err(error) => {
                    log::error!("Ad blocker unavailable: {error}");
                    self.adblock_error = Some(error);
                }
            }
        }

        if let Some(task) = &self.extension_preparation {
            let progress =
                task.progress.load(Ordering::Relaxed) as f32 / PREPARATION_PROGRESS_SCALE as f32;
            ExtensionPreparationStatus::Preparing(progress.clamp(0.0, 1.0))
        } else if self.prepared_extension.is_some() || self.extension_initialized {
            ExtensionPreparationStatus::Ready
        } else if let Some(error) = &self.adblock_error {
            ExtensionPreparationStatus::Failed(error.clone())
        } else {
            ExtensionPreparationStatus::Idle
        }
    }

    /// Browser hosts can be created immediately when filtering is off, the
    /// extension is ready, or preparation failed and we are falling back to
    /// an unfiltered browser.
    pub fn can_create_browser(&self) -> bool {
        !self.adblock_enabled
            || self.extension_initialized
            || self.prepared_extension.is_some()
            || self.adblock_error.is_some()
    }

    /// If browsers were created while filtering was off, install the prepared
    /// extension into their shared profile once the worker finishes.
    pub fn initialize_prepared_extension_for_existing_host(&mut self) {
        if !self.adblock_enabled
            || self.extension_initialized
            || self.prepared_extension.is_none()
            || self.hosts.is_empty()
        {
            return;
        }
        let extension_dir = self.prepared_extension.clone().unwrap();
        let activated = {
            let host = self.hosts.values().next().unwrap();
            activate_adblock(host, &extension_dir, self.adblock_enabled)
        };
        self.extension_initialized = true;
        self.apply_activation(activated);
    }

    fn apply_activation(&mut self, activated: Result<Activation, String>) {
        match activated {
            Ok(activation) => {
                self.extension = Some(activation.extension);
                self.adblock_error = None;
                if activation.filtering_interrupted {
                    self.begin_adblock_settle();
                }
            }
            Err(error) => {
                log::error!("Ad blocker unavailable: {error}");
                self.adblock_error = Some(error);
            }
        }
    }

    fn begin_adblock_settle(&mut self) {
        self.adblock_settle_until = Some(Instant::now() + ADBLOCK_SETTLE);
    }

    /// True while uBOL is rebuilding its rules and browser tiles must not
    /// navigate. See [`Self::adblock_settle_remaining`] for ending the wait.
    pub fn adblock_settling(&self) -> bool {
        self.adblock_settle_until
            .is_some_and(|until| Instant::now() < until)
    }

    /// How long until [`Self::poll_adblock_settle`] can release held tiles.
    /// Callers repaint then, so the wait actually ends.
    pub fn adblock_settle_remaining(&self) -> Option<Duration> {
        self.adblock_settle_until
            .map(|until| until.saturating_duration_since(Instant::now()))
    }

    /// Send held tiles to their address once filtering is live again, and
    /// reload the ones that were already showing an unfiltered page. Call
    /// once per frame.
    pub fn poll_adblock_settle(&mut self) {
        let Some(until) = self.adblock_settle_until else {
            return;
        };
        if Instant::now() < until {
            return;
        }
        self.adblock_settle_until = None;
        let deferred = std::mem::take(&mut self.deferred_loads);
        for (id, host) in &mut self.hosts {
            host.reload_all(deferred.get(id).cloned().map(AfterResume::Load));
        }
    }

    pub fn adblock_enabled(&self) -> bool {
        self.adblock_enabled
    }

    pub fn adblock_status_text(&self) -> String {
        if !self.adblock_enabled {
            return "Off — browser tiles load without the extension".to_owned();
        }
        if let Some(task) = &self.extension_preparation {
            let percent = task.progress.load(Ordering::Relaxed) / 100;
            return format!("Preparing uBlock Origin Lite… {percent}%");
        }
        if let Some(error) = &self.adblock_error {
            return format!("Unavailable: {error}");
        }
        if self.adblock_settling() {
            return "Starting uBlock Origin Lite — tiles open once it filters".to_owned();
        }
        if !self.extension_initialized {
            if self.prepared_extension.is_some() {
                return "uBlock Origin Lite is ready".to_owned();
            }
            return "uBlock Origin Lite prepares with the first browser tile".to_owned();
        }
        if self.extension.is_some() {
            if self.adblock_enabled {
                format!("uBlock Origin Lite {UBOL_VERSION} is active")
            } else {
                format!("uBlock Origin Lite {UBOL_VERSION} is disabled")
            }
        } else {
            "uBlock Origin Lite is unavailable".to_owned()
        }
    }

    /// Update the profile-wide uBOL state. Before the first browser tile this
    /// only records the preference; initialization applies it before loading.
    pub fn set_adblock_enabled(&mut self, enabled: bool) -> Result<(), String> {
        if let Some(extension) = self.extension.clone() {
            if extension_is_enabled(&extension)? != enabled {
                set_extension_enabled(&extension, enabled)?;
                // Pages loaded while uBOL was off (or while it restarts) stay
                // unfiltered until they load again.
                if enabled {
                    self.begin_adblock_settle();
                }
            }
        } else if self.extension_initialized && enabled {
            return Err(self
                .adblock_error
                .clone()
                .unwrap_or_else(|| "uBlock Origin Lite is unavailable".to_owned()));
        }
        self.adblock_enabled = enabled;
        Ok(())
    }

    pub fn contains(&self, id: PreviewId) -> bool {
        self.hosts.contains_key(&id)
    }

    /// PID of the shared WebView2 browser process, once any host can report it.
    pub fn browser_process_id(&self) -> Option<u32> {
        self.hosts
            .values()
            .find_map(BrowserHost::browser_process_id)
    }

    pub fn get(&self, id: PreviewId) -> Option<&BrowserHost> {
        self.hosts.get(&id)
    }

    pub fn get_mut(&mut self, id: PreviewId) -> Option<&mut BrowserHost> {
        self.hosts.get_mut(&id)
    }

    /// Open `url` in a new tab of tile `id`, showing it if `activate`.
    pub fn open_tab(&mut self, id: PreviewId, url: &str, activate: bool) -> Result<(), String> {
        let host = self
            .hosts
            .get_mut(&id)
            .ok_or("This browser tile is still starting")?;
        host.open_tab(&mut self.context, url, &self.signals, activate)
    }

    pub fn select_tab(&mut self, id: PreviewId, index: usize) -> Result<(), String> {
        match self.hosts.get_mut(&id) {
            Some(host) => host.select_tab(&mut self.context, &self.signals, index),
            None => Ok(()),
        }
    }

    pub fn close_tab(&mut self, id: PreviewId, index: usize) -> Result<(), String> {
        match self.hosts.get_mut(&id) {
            Some(host) => host.close_tab(&mut self.context, &self.signals, index),
            None => Ok(()),
        }
    }

    pub fn move_tab(&mut self, id: PreviewId, from: usize, to: usize) {
        if let Some(host) = self.hosts.get_mut(&id) {
            host.move_tab(from, to);
        }
    }

    /// Move tab `index` of tile `from` to position `slot` of tile `to` with
    /// its page still loaded, and show it there. Returns true when it was
    /// `from`'s only tab: that host is then gone, and the caller removes the
    /// tile.
    pub fn transfer_tab(
        &mut self,
        from: PreviewId,
        index: usize,
        to: PreviewId,
        slot: usize,
    ) -> Result<bool, String> {
        if from == to {
            return Err("The tab is already in this tile".to_owned());
        }
        let Self {
            hosts,
            context,
            signals,
            ..
        } = self;
        let target = hosts
            .get(&to)
            .ok_or("That browser tile is still starting")?;
        if target.suspended {
            return Err("Unfreeze that browser tile to add tabs to it".to_owned());
        }
        let source = hosts
            .get_mut(&from)
            .ok_or("This browser tile is still starting")?;
        if source.suspended {
            return Err("Unfreeze this browser tile to move its tabs".to_owned());
        }
        let tab = source.take_tab(context, signals, index)?;
        let target = hosts.get_mut(&to).expect("checked above");
        if let Err(rejected) = target.adopt_tab(context, signals, tab, slot) {
            let (tab, error) = *rejected;
            Self::give_back(hosts, context, signals, from, tab, index);
            return Err(error);
        }
        Ok(self.drop_if_empty(from))
    }

    /// Open tab `index` of tile `from` as the only tab of a new tile `id`,
    /// with its page still loaded. Returns the new host's HWND for capture.
    /// `from` never loses its last tab this way.
    pub fn create_from_tab(
        &mut self,
        id: PreviewId,
        from: PreviewId,
        index: usize,
    ) -> Result<isize, String> {
        let Self {
            hosts,
            context,
            signals,
            ..
        } = self;
        let source = hosts
            .get(&from)
            .ok_or("This browser tile is still starting")?;
        if source.tabs.len() < 2 {
            return Err("A tile keeps its last tab".to_owned());
        }
        if source.suspended {
            return Err("Unfreeze this browser tile to move its tabs".to_owned());
        }
        let (muted, page_zoom) = (source.muted, source.page_zoom);
        let mut host = BrowserHost::empty()?;
        host.muted = muted;
        host.page_zoom = page_zoom;
        let source = hosts.get_mut(&from).expect("checked above");
        let tab = source.take_tab(context, signals, index)?;
        if let Err(rejected) = host.adopt_tab(context, signals, tab, 0) {
            let (tab, error) = *rejected;
            Self::give_back(hosts, context, signals, from, tab, index);
            return Err(error);
        }
        let hwnd = host.hwnd();
        hosts.insert(id, host);
        Ok(hwnd)
    }

    /// Return a tab that could not move to the tile it came from.
    fn give_back(
        hosts: &mut HashMap<PreviewId, BrowserHost>,
        context: &mut WebContext,
        signals: &EngineSignals,
        from: PreviewId,
        tab: HostTab,
        index: usize,
    ) {
        if let Some(source) = hosts.get_mut(&from) {
            if let Err(rejected) = source.adopt_tab(context, signals, tab, index) {
                let error = &rejected.1;
                log::warn!("A browser tab could not return to its tile: {error}");
            }
        }
    }

    /// Drop `id`'s host if it has no tabs left. Returns whether it did.
    fn drop_if_empty(&mut self, id: PreviewId) -> bool {
        if self.hosts.get(&id).is_some_and(|host| host.tabs.is_empty()) {
            self.hosts.remove(&id);
            self.deferred_loads.remove(&id);
            return true;
        }
        false
    }

    /// Put idle background tabs of every tile to sleep.
    pub fn sleep_idle_tabs(&mut self) {
        let now = Instant::now();
        for host in self.hosts.values_mut() {
            host.sleep_idle_tabs(now);
        }
    }

    pub fn ids(&self) -> impl Iterator<Item = PreviewId> + '_ {
        self.hosts.keys().copied()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&PreviewId, &mut BrowserHost)> {
        self.hosts.iter_mut()
    }

    /// The interactive host, if any. At most one is active at a time.
    pub fn active_id(&self) -> Option<PreviewId> {
        self.hosts
            .iter()
            .find(|(_, host)| host.is_active())
            .map(|(id, _)| *id)
    }

    pub fn park_all(&mut self) {
        for host in self.hosts.values_mut() {
            if host.is_active() {
                host.park();
            }
        }
    }

    /// Tiles whose hosts finished settling after parking, so their capture
    /// can resume.
    pub fn take_settled_captures(&mut self) -> Vec<PreviewId> {
        self.hosts
            .iter_mut()
            .filter_map(|(id, host)| host.take_settled_capture().then_some(*id))
            .collect()
    }

    pub fn any_settling_capture(&self) -> bool {
        self.hosts.values().any(BrowserHost::is_settling_capture)
    }

    /// Drop hosts whose previews no longer exist.
    pub fn retain(&mut self, keep: impl Fn(PreviewId) -> bool) {
        self.hosts.retain(|id, _| keep(*id));
        self.deferred_loads.retain(|id, _| keep(*id));
    }

    pub fn clear(&mut self) {
        self.hosts.clear();
        self.deferred_loads.clear();
    }

    /// A newer WebView2 Runtime is installed and running tiles still use the
    /// old one. Restarting every browser tile applies it.
    pub fn engine_update_ready(&self) -> bool {
        self.signals.update_ready.load(Ordering::Acquire)
    }

    /// True once after the shared browser process exits unexpectedly.
    pub fn take_engine_lost(&self) -> bool {
        self.signals.lost.swap(false, Ordering::AcqRel)
    }

    /// Close every host and release the extension so WebView2 can shut its
    /// browser process down. Returns each closed tile and the address it was
    /// showing; the caller recreates them once [`Self::engine_restarting`]
    /// turns false.
    pub fn shut_down_engine(&mut self) -> Vec<(PreviewId, String)> {
        let process = self
            .browser_process_id()
            .and_then(|pid| unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }.ok());
        let tiles = self
            .hosts
            .iter()
            .map(|(id, host)| (*id, host.current_url()))
            .collect();
        self.hosts.clear();
        self.deferred_loads.clear();
        self.extension = None;
        self.extension_initialized = false;
        self.adblock_settle_until = None;
        self.clearing_data = None;
        self.engine_shutdown = Some(EngineShutdown {
            process,
            deadline: Instant::now() + ENGINE_SHUTDOWN_TIMEOUT,
        });
        tiles
    }

    /// True while the previous browser process is still exiting. New hosts
    /// must not be created until this returns false. Call once per frame.
    pub fn engine_restarting(&mut self) -> bool {
        let Some(shutdown) = &self.engine_shutdown else {
            return false;
        };
        if !shutdown.finished() {
            return true;
        }
        self.engine_shutdown = None;
        // The next environment loads the newest installed runtime, and any
        // failure reported while the old engine closed is no longer relevant.
        self.signals.update_ready.store(false, Ordering::Release);
        self.signals.lost.store(false, Ordering::Release);
        false
    }

    /// Erase cookies, logins, site storage, cache, history and saved form
    /// data from the shared profile. Installed extensions stay. Finishes
    /// asynchronously; see [`Self::poll_clear_browsing_data`].
    pub fn clear_browsing_data(&mut self) -> Result<(), String> {
        if self.clearing_data.is_some() {
            return Ok(());
        }
        let host = self
            .hosts
            .values()
            .next()
            .ok_or("Open a browser tile first")?;
        let slot = Arc::new(Mutex::new(None));
        let completed = slot.clone();
        host.with_core(|core| {
            let profile: ICoreWebView2Profile2 = unsafe {
                core.cast::<ICoreWebView2_13>()
                    .and_then(|core13| core13.Profile())
                    .and_then(|profile| profile.cast())
            }
            .map_err(|error| format!("This WebView2 Runtime cannot clear browser data: {error}"))?;
            let handler = ClearBrowsingDataCompletedHandler::create(Box::new(move |result| {
                *completed.lock() = Some(result.map_err(|error| error.to_string()));
                Ok(())
            }));
            unsafe { profile.ClearBrowsingDataAll(&handler) }.map_err(|e| e.to_string())
        })?;
        self.clearing_data = Some(slot);
        Ok(())
    }

    pub fn clearing_browsing_data(&self) -> bool {
        self.clearing_data.is_some()
    }

    /// The finished Clear Browser Data result, once. On success every tile
    /// reloads so pages drop the signed-in state they still hold in memory.
    pub fn poll_clear_browsing_data(&mut self) -> Option<Result<(), String>> {
        let result = self.clearing_data.as_ref()?.lock().take()?;
        self.clearing_data = None;
        if result.is_ok() {
            for host in self.hosts.values_mut() {
                host.reload_all(None);
            }
        }
        Some(result)
    }
}

/// Result of putting uBOL into the state the user asked for.
struct Activation {
    extension: ICoreWebView2BrowserExtension,
    /// Chromium dropped uBOL's registrations to get here, so nothing is
    /// filtered until its service worker has rebuilt them.
    filtering_interrupted: bool,
}

/// Make the shared profile's uBOL match `enabled`, installing it only when the
/// profile does not already carry this exact package.
fn activate_adblock(
    host: &BrowserHost,
    extension_dir: &Path,
    enabled: bool,
) -> Result<Activation, String> {
    let installed = match read_profile_install_marker(extension_dir) {
        Some(id) => host.installed_browser_extension(&id)?,
        None => None,
    };

    let mut filtering_interrupted = false;
    let extension = match installed {
        Some(extension) => extension,
        None => {
            let extension = host.install_browser_extension(extension_dir)?;
            write_profile_install_marker(extension_dir, &extension_id(&extension)?);
            filtering_interrupted = true;
            extension
        }
    };

    // Enabling an already-enabled extension is not free: it restarts uBOL and
    // clears what it had registered, so only touch the state that is wrong.
    if extension_is_enabled(&extension)? != enabled {
        set_extension_enabled(&extension, enabled)?;
        filtering_interrupted |= enabled;
    }

    Ok(Activation {
        extension,
        filtering_interrupted,
    })
}

fn extension_profile(
    core: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
) -> Result<ICoreWebView2Profile7, String> {
    unsafe {
        core.cast::<ICoreWebView2_13>()
            .and_then(|core13| core13.Profile())
            .and_then(|profile| profile.cast())
    }
    .map_err(|error| format!("This WebView2 Runtime cannot install browser extensions: {error}"))
}

fn extension_id(extension: &ICoreWebView2BrowserExtension) -> Result<String, String> {
    let mut id = WebViewPWSTR::null();
    unsafe { extension.Id(&mut id) }.map_err(|error| error.to_string())?;
    Ok(take_pwstr(id))
}

fn extension_is_enabled(extension: &ICoreWebView2BrowserExtension) -> Result<bool, String> {
    let mut enabled = Default::default();
    unsafe { extension.IsEnabled(&mut enabled) }.map_err(|error| error.to_string())?;
    Ok(enabled.as_bool())
}

/// Records which package the shared WebView2 profile was given, and the id it
/// got, so later launches can find that extension instead of reinstalling it.
fn profile_install_marker_path(extension_dir: &Path) -> Option<PathBuf> {
    let directory_name = extension_dir.file_name()?.to_string_lossy();
    Some(
        extension_dir
            .parent()?
            .join(format!(".{directory_name}.profile-install")),
    )
}

/// The installed extension's id, when the profile holds this exact package.
fn read_profile_install_marker(extension_dir: &Path) -> Option<String> {
    let contents = fs::read_to_string(profile_install_marker_path(extension_dir)?).ok()?;
    let (fingerprint, id) = contents.trim().split_once('\n')?;
    (fingerprint.trim() == UBOL_ARCHIVE_FINGERPRINT).then(|| id.trim().to_owned())
}

fn write_profile_install_marker(extension_dir: &Path, id: &str) {
    let Some(marker_path) = profile_install_marker_path(extension_dir) else {
        log::warn!("Could not determine the uBlock Origin Lite install marker path");
        return;
    };
    if let Err(error) = fs::write(marker_path, format!("{UBOL_ARCHIVE_FINGERPRINT}\n{id}")) {
        // Filtering still works; the next launch just reinstalls the
        // extension and waits for it to settle again.
        log::warn!("Could not record the uBlock Origin Lite installation: {error}");
    }
}

fn set_extension_enabled(
    extension: &ICoreWebView2BrowserExtension,
    enabled: bool,
) -> Result<(), String> {
    let extension = extension.clone();
    BrowserExtensionEnableCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            extension
                .Enable(enabled, &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(|result| result),
    )
    .map_err(|error| format!("uBlock Origin Lite could not be toggled: {error}"))
}

fn set_preparation_progress(progress: &AtomicU32, value: f32) {
    progress.store(
        (value.clamp(0.0, 1.0) * PREPARATION_PROGRESS_SCALE as f32) as u32,
        Ordering::Relaxed,
    );
}

fn prepare_ubol_with_progress(extension_dir: &Path, progress: &AtomicU32) -> Result<(), String> {
    set_preparation_progress(progress, 0.02);
    if installed_ubol_has_current_marker(extension_dir) {
        set_preparation_progress(progress, 1.0);
        return Ok(());
    }

    let archive = load_ubol_archive()?;
    if installed_ubol_matches_package_with_progress(extension_dir, &archive, progress, 0.03, 0.38) {
        write_ubol_verification_marker(extension_dir);
        set_preparation_progress(progress, 1.0);
        return Ok(());
    }

    let parent = extension_dir
        .parent()
        .ok_or("Invalid uBlock Origin Lite installation path")?;
    fs::create_dir_all(parent).map_err(|error| format!("Create extension directory: {error}"))?;

    let staging = parent.join("ubol-staging");
    if staging.exists() {
        fs::remove_dir_all(&staging)
            .map_err(|error| format!("Remove stale extension staging directory: {error}"))?;
    }
    fs::create_dir(&staging)
        .map_err(|error| format!("Create extension staging directory: {error}"))?;

    set_preparation_progress(progress, 0.4);
    let extract_result =
        extract_ubol_archive_with_progress(&staging, &archive, progress, 0.4, 0.74);
    if let Err(error) = extract_result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if !installed_ubol_matches_package_with_progress(&staging, &archive, progress, 0.75, 0.98) {
        let _ = fs::remove_dir_all(&staging);
        return Err("Extracted uBlock Origin Lite package failed its integrity check".to_owned());
    }

    if extension_dir.exists() {
        fs::remove_dir_all(extension_dir)
            .map_err(|error| format!("Replace old extension version: {error}"))?;
    }
    fs::rename(&staging, extension_dir)
        .map_err(|error| format!("Activate uBlock Origin Lite files: {error}"))?;
    write_ubol_verification_marker(extension_dir);
    set_preparation_progress(progress, 1.0);
    Ok(())
}

fn ubol_verification_marker_path(extension_dir: &Path) -> Option<PathBuf> {
    let directory_name = extension_dir.file_name()?.to_string_lossy();
    Some(
        extension_dir
            .parent()?
            .join(format!(".{directory_name}.verified-sha256")),
    )
}

fn installed_ubol_has_current_marker(extension_dir: &Path) -> bool {
    if !extension_dir.is_dir() || !installed_ubol_manifest_is_pinned_version(extension_dir) {
        return false;
    }
    let Some(marker_path) = ubol_verification_marker_path(extension_dir) else {
        return false;
    };
    let Some(expected) = installed_ubol_marker(extension_dir) else {
        return false;
    };
    fs::read_to_string(marker_path).is_ok_and(|marker| marker.trim() == expected)
}

/// The verified archive fingerprint plus a digest of every installed file's
/// path, size and modification time. Startup skips re-hashing file contents
/// only while nothing in the extension folder has been added, removed or
/// rewritten since the last full verification. WebView2 itself writes
/// generated rule indexes under `_metadata`, which is therefore excluded.
fn installed_ubol_marker(extension_dir: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let mut files = Vec::new();
    let mut pending = vec![extension_dir.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).ok()? {
            let entry = entry.ok()?;
            let file_type = entry.file_type().ok()?;
            if file_type.is_dir() {
                if entry.path() != extension_dir.join("_metadata") {
                    pending.push(entry.path());
                }
            } else if file_type.is_file() {
                let metadata = entry.metadata().ok()?;
                let modified = metadata
                    .modified()
                    .ok()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()?
                    .as_nanos();
                let relative = entry.path().strip_prefix(extension_dir).ok()?.to_owned();
                files.push((relative, metadata.len(), modified));
            } else {
                return None;
            }
        }
    }
    files.sort();
    let mut digest = Sha256::new();
    for (path, length, modified) in files {
        digest.update(path.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(length.to_le_bytes());
        digest.update(modified.to_le_bytes());
    }
    let digest: String = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Some(format!("{UBOL_ARCHIVE_FINGERPRINT} {digest}"))
}

/// Cheap launch check that needs no package read; the verification marker
/// already ties the installed files to the pinned archive fingerprint.
fn installed_ubol_manifest_is_pinned_version(extension_dir: &Path) -> bool {
    let Ok(manifest) = fs::read(extension_dir.join("manifest.json")) else {
        return false;
    };
    serde_json::from_slice::<serde_json::Value>(&manifest)
        .is_ok_and(|manifest| manifest["version"] == UBOL_VERSION)
}

/// Where the pinned package may live: the `lib` folder beside the
/// executable, or the source tree's copy when running a cargo build.
fn ubol_archive_candidates(executable: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let Some(directory) = executable.and_then(Path::parent) else {
        return candidates;
    };
    candidates.push(directory.join("lib").join(UBOL_ARCHIVE_NAME));

    let is_build_profile = directory
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, "debug" | "release"));
    let target_directory = directory.parent();
    let is_target_directory = target_directory
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        == Some("target");
    if is_build_profile && is_target_directory {
        if let Some(workspace) = target_directory.and_then(Path::parent) {
            candidates.push(
                workspace
                    .join("assets")
                    .join("third_party")
                    .join("ubol")
                    .join(UBOL_ARCHIVE_NAME),
            );
        }
    }
    candidates
}

fn load_ubol_archive() -> Result<Vec<u8>, String> {
    let executable = std::env::current_exe().ok();
    let path = ubol_archive_candidates(executable.as_deref())
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| format!("{UBOL_ARCHIVE_NAME} is missing from the lib folder"))?;
    let archive = fs::read(&path).map_err(|error| format!("Read {UBOL_ARCHIVE_NAME}: {error}"))?;
    verify_ubol_archive(archive)
}

/// Only the exact package this build was pinned to is ever extracted.
fn verify_ubol_archive(archive: Vec<u8>) -> Result<Vec<u8>, String> {
    use sha2::{Digest, Sha256};
    let digest: String = Sha256::digest(&archive)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if digest == UBOL_ARCHIVE_FINGERPRINT {
        Ok(archive)
    } else {
        Err(format!(
            "{UBOL_ARCHIVE_NAME} in the lib folder doesn't match this version of Pluriview"
        ))
    }
}

fn write_ubol_verification_marker(extension_dir: &Path) {
    let Some(marker_path) = ubol_verification_marker_path(extension_dir) else {
        log::warn!("Could not determine the uBlock Origin Lite verification marker path");
        return;
    };
    let Some(marker) = installed_ubol_marker(extension_dir) else {
        log::warn!("Could not describe the verified uBlock Origin Lite files");
        return;
    };
    if let Err(error) = fs::write(marker_path, marker) {
        // The verified extension is still safe to use. A missing marker only
        // means the next launch will repeat the full verification.
        log::warn!("Could not cache uBlock Origin Lite verification: {error}");
    }
}

fn extract_ubol_archive_with_progress(
    destination: &Path,
    archive: &[u8],
    progress: &AtomicU32,
    start: f32,
    end: f32,
) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(archive))
        .map_err(|error| format!("Open uBlock Origin Lite package: {error}"))?;

    let entry_count = archive.len().max(1);
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("Read extension archive entry: {error}"))?;
        let relative = entry.enclosed_name().ok_or_else(|| {
            format!(
                "Unsafe path in extension archive: {}",
                entry.name_raw().escape_ascii()
            )
        })?;
        let output = destination.join(relative);

        if entry.is_dir() {
            fs::create_dir_all(&output)
                .map_err(|error| format!("Create extension subdirectory: {error}"))?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Create extension subdirectory: {error}"))?;
        }
        let mut file =
            fs::File::create(&output).map_err(|error| format!("Create extension file: {error}"))?;
        io::copy(&mut entry, &mut file)
            .map_err(|error| format!("Extract extension file: {error}"))?;
        let completed = (index + 1) as f32 / entry_count as f32;
        set_preparation_progress(progress, start + (end - start) * completed);
    }
    set_preparation_progress(progress, end);
    Ok(())
}

#[cfg(test)]
fn installed_ubol_matches_package(extension_dir: &Path, archive: &[u8]) -> bool {
    let progress = AtomicU32::new(0);
    installed_ubol_matches_package_with_progress(extension_dir, archive, &progress, 0.0, 1.0)
}

fn installed_ubol_matches_package_with_progress(
    extension_dir: &Path,
    archive: &[u8],
    progress: &AtomicU32,
    start: f32,
    end: f32,
) -> bool {
    let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(archive)) else {
        return false;
    };
    let mut expected_files = HashSet::new();
    let entry_count = archive.len().max(1);
    for index in 0..archive.len() {
        let Ok(mut entry) = archive.by_index(index) else {
            return false;
        };
        let Some(relative) = entry.enclosed_name() else {
            return false;
        };
        if entry.is_dir() {
            continue;
        }
        expected_files.insert(relative.to_path_buf());
        let Ok(installed) = fs::read(extension_dir.join(relative)) else {
            return false;
        };
        let mut embedded = Vec::with_capacity(entry.size() as usize);
        if entry.read_to_end(&mut embedded).is_err() || installed != embedded {
            return false;
        }
        let completed = (index + 1) as f32 / entry_count as f32;
        set_preparation_progress(progress, start + (end - start) * completed);
    }

    let mut pending = vec![extension_dir.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(directory) else {
            return false;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                return false;
            };
            if file_type.is_symlink() {
                return false;
            }
            // WebView2 writes generated rule indexes here; they are not part
            // of the package and must not force a reinstall.
            if file_type.is_dir() && path == extension_dir.join("_metadata") {
                continue;
            }
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file() {
                let Ok(relative) = path.strip_prefix(extension_dir) else {
                    return false;
                };
                if !expected_files.contains(relative) {
                    return false;
                }
            } else {
                return false;
            }
        }
    }
    set_preparation_progress(progress, end);
    true
}

fn register_window_class() -> Result<(), String> {
    let class = WNDCLASSW {
        lpfnWndProc: Some(browser_window_proc),
        hInstance: unsafe { GetModuleHandleW(None) }
            .map_err(|error| error.to_string())?
            .into(),
        lpszClassName: w!("PluriviewBrowserHost"),
        ..Default::default()
    };
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        Err(windows::core::Error::from_win32().to_string())
    } else {
        Ok(())
    }
}

unsafe extern "system" fn browser_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, message, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::{
        browser_geometry, capture_size_for_tile, decode_favicon,
        extract_ubol_archive_with_progress, installed_ubol_has_current_marker,
        installed_ubol_matches_package, is_allowed_navigation, moved_index, normalize_url,
        parked_bounds, profile_install_marker_path, read_profile_install_marker,
        scrub_url_for_storage, ubol_archive_candidates, ubol_verification_marker_path,
        verify_ubol_archive, write_profile_install_marker, write_ubol_verification_marker,
        NativeWindow, FAVICON_SIZE, PREPARATION_PROGRESS_SCALE, UBOL_ARCHIVE_NAME, UBOL_VERSION,
    };

    /// Run this thread's WebView2 callbacks until `done`.
    fn pump_until(mut done: impl FnMut() -> bool) {
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !done() {
            assert!(
                std::time::Instant::now() < deadline,
                "WebView2 did not answer in time"
            );
            unsafe {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// `script`'s result as JSON.
    fn evaluate(webview: &wry::WebView, script: &str) -> String {
        let result = std::sync::Arc::new(parking_lot::Mutex::new(None));
        let slot = result.clone();
        webview
            .evaluate_script_with_callback(script, move |value| *slot.lock() = Some(value))
            .unwrap();
        pump_until(|| result.lock().is_some());
        let value = result.lock().take();
        value.unwrap()
    }

    #[test]
    #[ignore = "starts WebView2 offscreen with a throwaway profile"]
    fn a_tab_moved_to_another_tile_keeps_its_live_page() {
        use wry::WebViewExtWindows;
        let profile =
            std::env::temp_dir().join(format!("pluriview-tab-move-test-{}", std::process::id()));
        let signals = super::EngineSignals::default();
        {
            let mut context = wry::WebContext::new(Some(profile.clone()));
            let mut source =
                super::BrowserHost::new(&mut context, "about:blank", &signals).unwrap();
            let second = source
                .build_view(&mut context, "about:blank", &signals, false)
                .unwrap();
            source
                .tabs
                .push(super::HostTab::loaded(second, "about:blank", false));
            let page = &source.tabs[0].view.as_ref().unwrap().webview;
            pump_until(|| evaluate(page, "document.readyState") == "\"complete\"");
            assert_eq!(evaluate(page, "window.pluriviewMarker = 42"), "42");

            let tab = source.take_tab(&mut context, &signals, 0).unwrap();
            assert_eq!(source.tabs.len(), 1);
            assert_eq!(source.active_tab, 0);
            let mut target = super::BrowserHost::empty().unwrap();
            assert!(target.adopt_tab(&mut context, &signals, tab, 0).is_ok());

            // The same document, not a reload: the marker is still there.
            let moved = &target.tabs[0].view.as_ref().unwrap().webview;
            assert_eq!(evaluate(moved, "String(window.pluriviewMarker)"), "\"42\"");
            // And it now lives in the other tile's window.
            let mut container = webview2_windows::Win32::Foundation::HWND::default();
            unsafe { moved.controller().ParentWindow(&mut container).unwrap() };
            let parent = unsafe {
                windows::Win32::UI::WindowsAndMessaging::GetParent(super::HWND(container.0))
            }
            .unwrap();
            assert_eq!(parent, target.window.0);
        }
        let _ = std::fs::remove_dir_all(profile);
    }

    #[test]
    fn moving_a_tab_keeps_track_of_every_other_tab() {
        // Tabs a b c d; move b (1) to the end (3): a c d b.
        assert_eq!(moved_index(1, 1, 3), 3);
        assert_eq!(moved_index(2, 1, 3), 1);
        assert_eq!(moved_index(3, 1, 3), 2);
        assert_eq!(moved_index(0, 1, 3), 0);
        // Move d (3) to the front (0): d a b c.
        assert_eq!(moved_index(3, 3, 0), 0);
        assert_eq!(moved_index(0, 3, 0), 1);
        assert_eq!(moved_index(2, 3, 0), 3);
    }

    #[test]
    fn site_icons_decode_and_shrink_to_badge_size() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(128, 128, image::Rgba([200, 40, 40, 255]))
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let favicon = decode_favicon("https://example.com/favicon.ico".to_owned(), &png).unwrap();
        assert_eq!(favicon.image.size, [FAVICON_SIZE as usize; 2]);
        assert!(decode_favicon(String::new(), b"not a png").is_none());
    }
    use eframe::egui;
    use std::io::{Cursor, Read};
    use std::sync::atomic::{AtomicU32, Ordering};
    use wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[test]
    fn native_window_exposes_its_hwnd() {
        let window = NativeWindow::from_hwnd(0x1234).unwrap();
        let handle = window.window_handle().unwrap();

        match handle.as_raw() {
            RawWindowHandle::Win32(handle) => assert_eq!(handle.hwnd.get(), 0x1234),
            other => panic!("expected Win32 handle, got {other:?}"),
        }
    }

    #[test]
    fn parked_bounds_match_capture_resolution() {
        let bounds = parked_bounds();

        assert_eq!(bounds.position.to_physical::<i32>(1.0).x, 0);
        assert_eq!(bounds.position.to_physical::<i32>(1.0).y, 0);
        assert_eq!(bounds.size.to_physical::<i32>(1.0).width, 1280);
        assert_eq!(bounds.size.to_physical::<i32>(1.0).height, 720);
    }

    #[test]
    fn oversized_page_is_clipped_to_canvas_without_changing_page_size() {
        let page = egui::Rect::from_min_max(egui::pos2(-100.0, -50.0), egui::pos2(1500.0, 900.0));
        let visible = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(800.0, 600.0));

        let geometry = browser_geometry(page, visible, 1.0, (200, 300));

        assert_eq!((geometry.host_x, geometry.host_y), (200, 300));
        assert_eq!((geometry.host_width, geometry.host_height), (800, 600));
        assert_eq!((geometry.page_x, geometry.page_y), (-100, -50));
        assert_eq!((geometry.page_width, geometry.page_height), (1600, 950));
    }

    #[test]
    fn browser_capture_is_supersampled_without_following_canvas_zoom() {
        assert_eq!(capture_size_for_tile(640, 360), (1280, 720));
        assert_eq!(capture_size_for_tile(1280, 720), (2560, 1440));
        assert_eq!(capture_size_for_tile(5000, 2813), (3839, 2160));
    }

    #[test]
    fn tiny_browser_tiles_keep_the_css_viewport_zoom_floor() {
        assert_eq!(capture_size_for_tile(100, 56), (320, 179));
    }

    #[test]
    fn address_bar_opens_addresses_and_searches_everything_else() {
        use super::address_bar_target;
        assert_eq!(
            address_bar_target("twitch.tv/example").unwrap(),
            "https://twitch.tv/example"
        );
        assert_eq!(
            address_bar_target("http://localhost:8080").unwrap(),
            "http://localhost:8080"
        );
        assert_eq!(
            address_bar_target("lofi radio").unwrap(),
            "https://www.google.com/search?q=lofi+radio"
        );
        assert_eq!(
            address_bar_target("weather").unwrap(),
            "https://www.google.com/search?q=weather"
        );
        assert!(address_bar_target("file:///C:/secret.txt").is_err());
        assert!(address_bar_target("  ").is_err());
    }

    #[test]
    fn saved_tabs_are_scrubbed_and_keep_their_active_tab() {
        use super::scrub_tabs_for_storage;
        use crate::preview::BrowserTab;
        let tabs = [
            BrowserTab::new("about:blank"),
            BrowserTab::new("https://example.com/?token=abc#top"),
            BrowserTab::new("https://youtube.com/watch?v=1"),
        ];
        let (kept, active) = scrub_tabs_for_storage(&tabs, 2);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].url, "https://example.com/");
        assert_eq!(active, 1);
        let (_, active) = scrub_tabs_for_storage(&tabs, 0);
        assert_eq!(active, 0);
    }

    #[test]
    fn normalize_url_adds_https() {
        assert_eq!(
            normalize_url(" twitch.tv/example ").unwrap(),
            "https://twitch.tv/example"
        );
    }

    #[test]
    fn normalize_url_keeps_http_urls() {
        assert_eq!(
            normalize_url("http://kick.com/example").unwrap(),
            "http://kick.com/example"
        );
        assert_eq!(
            normalize_url("https://youtube.com").unwrap(),
            "https://youtube.com"
        );
    }

    #[test]
    fn normalize_url_rejects_non_web_schemes() {
        assert!(normalize_url("file:///secret").is_err());
        assert!(normalize_url("javascript:alert(1)").is_err());
        assert!(normalize_url("https://user:password@example.com").is_err());
    }

    #[test]
    fn only_sized_popups_from_interactive_tiles_open_windows() {
        assert!(super::opens_as_popup(true, true));
        assert!(!super::opens_as_popup(true, false));
        assert!(!super::opens_as_popup(false, true));
        assert!(!super::opens_as_popup(false, false));
    }

    #[test]
    fn page_titles_report_unread_counts() {
        use super::{unread_indicator, UnreadIndicator::*};
        assert_eq!(unread_indicator("(3) Discord | #general"), Some(Count(3)));
        assert_eq!(unread_indicator("(99+) YouTube"), Some(Count(99)));
        assert_eq!(
            unread_indicator("Inbox (1,204) - me@example.com - Gmail"),
            Some(Count(1204))
        );
        assert_eq!(unread_indicator("\u{2022} Discord | #general"), Some(Dot));
        assert_eq!(unread_indicator("Discord | #general"), None);
        assert_eq!(unread_indicator("(0) Feed"), None);
        assert_eq!(unread_indicator("(Live) Concert"), None);
        assert_eq!(unread_indicator("Song (2024 Remaster) - YouTube"), None);
    }

    #[test]
    fn page_zoom_steps_like_a_browser() {
        use super::{step_page_zoom, MAX_PAGE_ZOOM, MIN_PAGE_ZOOM};
        assert_eq!(step_page_zoom(1.0, true), 1.1);
        assert_eq!(step_page_zoom(1.0, false), 0.9);
        assert_eq!(step_page_zoom(1.17, true), 1.25);
        assert_eq!(step_page_zoom(1.17, false), 1.1);
        assert_eq!(step_page_zoom(MAX_PAGE_ZOOM, true), MAX_PAGE_ZOOM);
        assert_eq!(step_page_zoom(MIN_PAGE_ZOOM, false), MIN_PAGE_ZOOM);
    }

    #[test]
    fn video_rect_results_are_validated() {
        use super::parse_video_rect;
        assert_eq!(
            parse_video_rect("[0.1,0.2,0.9,0.8]"),
            Some((0.1, 0.2, 0.9, 0.8))
        );
        assert_eq!(parse_video_rect("null"), None);
        assert_eq!(parse_video_rect("[0.5,0.5,0.5,0.9]"), None);
        assert_eq!(parse_video_rect("[-0.1,0,1,1]"), None);
        assert_eq!(parse_video_rect("oops"), None);
    }

    #[test]
    fn browser_arguments_keep_smartscreen_enabled() {
        assert!(!super::BROWSER_ARGUMENTS.contains("SmartScreen"));
        // Keep the mini menus wry removes by default turned off.
        assert!(super::BROWSER_ARGUMENTS.contains("msWebOOUI"));
        assert!(super::BROWSER_ARGUMENTS.contains("msPdfOOUI"));
    }

    #[test]
    fn crashed_pages_reload_unless_they_crash_again_soon() {
        use super::{crash_response, PageCrash, CRASH_RELOAD_COOLDOWN};
        use std::time::{Duration, Instant};
        let now = Instant::now() + CRASH_RELOAD_COOLDOWN * 2;
        assert_eq!(crash_response(None, now), PageCrash::Reloaded);
        assert_eq!(
            crash_response(Some(now - Duration::from_secs(5)), now),
            PageCrash::Repeated
        );
        assert_eq!(
            crash_response(Some(now - CRASH_RELOAD_COOLDOWN), now),
            PageCrash::Reloaded
        );
    }

    #[test]
    fn parked_tiles_cannot_request_device_or_data_permissions() {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_PERMISSION_KIND_AUTOPLAY, COREWEBVIEW2_PERMISSION_KIND_CAMERA,
            COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ, COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION,
            COREWEBVIEW2_PERMISSION_KIND_MICROPHONE, COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS,
        };
        for kind in [
            COREWEBVIEW2_PERMISSION_KIND_CAMERA,
            COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
            COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION,
            COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS,
            COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ,
        ] {
            assert!(super::deny_permission(kind, false));
            // Interactive tiles are visible, so WebView2 asks the user.
            assert!(!super::deny_permission(kind, true));
        }
        assert!(!super::deny_permission(
            COREWEBVIEW2_PERMISSION_KIND_AUTOPLAY,
            false
        ));
    }

    #[test]
    fn navigation_filter_blocks_local_schemes() {
        assert!(is_allowed_navigation("https://youtube.com/watch"));
        assert!(is_allowed_navigation("about:blank"));
        assert!(!is_allowed_navigation("file:///C:/secret.txt"));
        assert!(!is_allowed_navigation("ms-settings:display"));
        assert!(!is_allowed_navigation("http-custom:payload"));
    }

    #[test]
    fn persisted_urls_drop_secrets_and_fragments() {
        assert_eq!(
            scrub_url_for_storage(
                "https://user:pass@example.com/watch?v=42&access_token=secret&X-Amz-Signature=signed#private"
            )
            .unwrap(),
            "https://example.com/watch?v=42"
        );
    }

    #[test]
    fn persisted_urls_drop_share_secrets_and_one_time_codes() {
        for key in super::PRIVATE_QUERY_KEYS {
            let url = format!(
                "https://example.com/page?id=7&{}=secret",
                key.to_uppercase()
            );
            assert_eq!(
                scrub_url_for_storage(&url).unwrap(),
                "https://example.com/page?id=7",
                "{key}"
            );
        }
        assert_eq!(
            scrub_url_for_storage(
                "https://www.dropbox.com/scl/fi/abc/notes.txt?rlkey=private&dl=0"
            )
            .unwrap(),
            "https://www.dropbox.com/scl/fi/abc/notes.txt?dl=0"
        );
        assert_eq!(
            scrub_url_for_storage(
                "https://drive.google.com/file/d/abc/view?usp=sharing&resourcekey=0-private"
            )
            .unwrap(),
            "https://drive.google.com/file/d/abc/view?usp=sharing"
        );
        // Ordinary names that merely contain these words are kept.
        assert_eq!(
            scrub_url_for_storage("https://example.com/?author=me&tickets=2").unwrap(),
            "https://example.com/?author=me&tickets=2"
        );
    }

    fn source_ubol_archive() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("third_party")
            .join("ubol")
            .join(UBOL_ARCHIVE_NAME);
        verify_ubol_archive(std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn packaged_ubol_matches_pinned_version() {
        let archive = source_ubol_archive();
        let mut archive = zip::ZipArchive::new(Cursor::new(archive)).unwrap();
        let mut manifest = String::new();
        archive
            .by_name("manifest.json")
            .unwrap()
            .read_to_string(&mut manifest)
            .unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();

        assert_eq!(manifest["manifest_version"], 3);
        assert_eq!(manifest["version"], UBOL_VERSION);
    }

    #[test]
    fn profile_install_marker_only_matches_the_embedded_package() {
        let extension_dir = std::env::temp_dir().join(format!(
            "pluriview-ubol-install-marker-test-{}",
            std::process::id()
        ));
        let marker = profile_install_marker_path(&extension_dir).unwrap();
        let _ = std::fs::remove_file(&marker);
        assert_eq!(read_profile_install_marker(&extension_dir), None);

        write_profile_install_marker(&extension_dir, "abcdefghijklmnopqrstuvwxyzabcdef");
        assert_eq!(
            read_profile_install_marker(&extension_dir).as_deref(),
            Some("abcdefghijklmnopqrstuvwxyzabcdef")
        );

        // A different embedded package must force a reinstall.
        std::fs::write(&marker, "0000\nabcdefghijklmnopqrstuvwxyzabcdef").unwrap();
        assert_eq!(read_profile_install_marker(&extension_dir), None);
        std::fs::remove_file(marker).unwrap();
    }

    #[test]
    fn installed_extension_integrity_detects_tampering() {
        let destination = std::env::temp_dir().join(format!(
            "pluriview-ubol-integrity-test-{}",
            std::process::id()
        ));
        let marker = ubol_verification_marker_path(&destination).unwrap();
        let _ = std::fs::remove_dir_all(&destination);
        let _ = std::fs::remove_file(&marker);
        std::fs::create_dir(&destination).unwrap();
        let archive = source_ubol_archive();
        let progress = AtomicU32::new(0);
        extract_ubol_archive_with_progress(&destination, &archive, &progress, 0.0, 1.0).unwrap();
        assert_eq!(progress.load(Ordering::Relaxed), PREPARATION_PROGRESS_SCALE);
        assert!(installed_ubol_matches_package(&destination, &archive));
        assert!(!installed_ubol_has_current_marker(&destination));

        write_ubol_verification_marker(&destination);
        assert!(installed_ubol_has_current_marker(&destination));

        // Rule indexes WebView2 generates at runtime are not tampering.
        let generated = destination
            .join("_metadata")
            .join("generated_indexed_rulesets");
        std::fs::create_dir_all(&generated).unwrap();
        std::fs::write(generated.join("_ruleset1"), b"index").unwrap();
        assert!(installed_ubol_has_current_marker(&destination));
        assert!(installed_ubol_matches_package(&destination, &archive));

        // Rewriting any package file, not just the manifest, is detected.
        let script = std::fs::read_dir(&destination)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.is_file() && path.file_name().unwrap() != "manifest.json")
            .unwrap();
        let mut original = std::fs::read(&script).unwrap();
        original.extend_from_slice(b"// tampered");
        std::fs::write(&script, original).unwrap();
        assert!(!installed_ubol_has_current_marker(&destination));
        assert!(!installed_ubol_matches_package(&destination, &archive));

        std::fs::write(destination.join("manifest.json"), b"{}").unwrap();
        assert!(!installed_ubol_has_current_marker(&destination));
        assert!(!installed_ubol_matches_package(&destination, &archive));
        std::fs::remove_dir_all(&destination).unwrap();
        std::fs::remove_file(marker).unwrap();
    }

    #[test]
    fn ubol_package_with_wrong_fingerprint_is_rejected() {
        let mut archive = source_ubol_archive();
        archive.push(0);
        assert!(verify_ubol_archive(archive).is_err());
    }

    #[test]
    fn ubol_package_is_looked_up_in_lib_then_source_tree() {
        let workspace = std::path::Path::new(r"C:\work\pluriview");
        let built = workspace.join(r"target\release\pluriview.exe");
        assert_eq!(
            ubol_archive_candidates(Some(&built)),
            vec![
                workspace
                    .join(r"target\release\lib")
                    .join(UBOL_ARCHIVE_NAME),
                workspace
                    .join(r"assets\third_party\ubol")
                    .join(UBOL_ARCHIVE_NAME),
            ]
        );
        let installed = std::path::Path::new(r"D:\Apps\Pluriview\pluriview.exe");
        assert_eq!(
            ubol_archive_candidates(Some(installed)),
            vec![std::path::Path::new(r"D:\Apps\Pluriview\lib").join(UBOL_ARCHIVE_NAME)]
        );
    }
}
