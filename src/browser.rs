use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs,
    io::{self, Cursor, Read},
    num::NonZeroIsize,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc::{self, Receiver, TryRecvError},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use webview2_com::{
    take_pwstr, BrowserExtensionEnableCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2BrowserExtension, ICoreWebView2Profile7, ICoreWebView2_13, ICoreWebView2_3,
        ICoreWebView2_8,
    },
    ProfileAddBrowserExtensionCompletedHandler, ProfileGetBrowserExtensionsCompletedHandler,
    TrySuspendCompletedHandler,
};
use webview2_core::{Interface, HSTRING as WebViewHString, PWSTR as WebViewPWSTR};
use windows::{
    core::w,
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        Graphics::Gdi::ClientToScreen,
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE, VK_NUMPAD0},
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, GetForegroundWindow, IsChild,
                RegisterClassW, SetForegroundWindow, SetWindowPos, ShowWindow, HWND_BOTTOM,
                HWND_TOP, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, WNDCLASSW,
                WS_CLIPCHILDREN, WS_EX_TOOLWINDOW, WS_POPUP,
            },
        },
    },
};
use wry::{
    dpi::{PhysicalPosition, PhysicalSize},
    raw_window_handle::{
        HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
    },
    NewWindowResponse, Rect, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows,
    WebViewExtWindows,
};

use crate::preview::PreviewId;

const PARK_X: i32 = -30_000;
const PARK_Y: i32 = -30_000;
/// Backing viewport of a parked browser, in physical pixels. This is what
/// Windows Graphics Capture sees, so it stays fixed regardless of tile size.
const WIDTH: i32 = 1280;
const HEIGHT: i32 = 720;

/// WebView2 rejects zoom factors outside roughly this range.
const MIN_ZOOM: f64 = 0.25;
const MAX_ZOOM: f64 = 4.0;
/// Parked browsers use a stable supersampled backing. It is deliberately
/// independent of canvas zoom so navigation never resizes the webpage.
const MIN_CAPTURE_WIDTH: i32 = 2560;
const MAX_CAPTURE_WIDTH: i32 = 3840;
const MAX_CAPTURE_HEIGHT: i32 = 2160;
/// Give WebView2's compositor time to apply a new viewport and zoom before
/// the native host replaces the frozen captured preview onscreen.
const INTERACTION_PREP_DELAY: Duration = Duration::from_millis(80);

const UBOL_VERSION: &str = "2026.714.1952";
const UBOL_ARCHIVE: &[u8] =
    include_bytes!("../assets/third_party/ubol/uBOLite_2026.714.1952.edge.zip");
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

fn is_sensitive_query_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    matches!(
        key.as_str(),
        "code" | "state" | "key" | "sig" | "session" | "sessionid"
    ) || [
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

/// Schemes a page may navigate to while staying inside the tile. `about:` is
/// needed because sites navigate iframes/blank targets through it.
fn is_allowed_navigation(url: &str) -> bool {
    url::Url::parse(url)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https" | "about" | "blob" | "data"))
}

/// True while the virtual key is held. Once the WebView has focus, egui never
/// sees keyboard input, so keys that must work in interaction mode are polled.
fn key_down(vk: i32) -> bool {
    (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0
}

/// True while the Escape key is held. Used to exit interaction mode.
pub fn escape_pressed() -> bool {
    key_down(VK_ESCAPE.0 as i32)
}

/// True while the numpad `digit` key is held. Requires NumLock on: with it off
/// Windows reports the navigation keys (End, Down, ...) instead, which we must
/// not steal.
pub fn numpad_digit_down(digit: u8) -> bool {
    key_down(VK_NUMPAD0.0 as i32 + i32::from(digit))
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
    /// A page requested a new window; we navigate the same tile instead.
    pending_navigation: Option<String>,
    dirty: bool,
}

/// Title/URL changes since the last poll, for the app to mirror onto the tile.
#[derive(Default)]
pub struct BrowserUpdate {
    pub title: Option<String>,
    pub url: Option<String>,
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

pub fn capture_size_for_tile(width: i32, height: i32) -> (i32, i32) {
    let width = width.max(1) as f64;
    let height = height.max(1) as f64;
    let upscale = (MIN_CAPTURE_WIDTH as f64 / width).max(1.0);
    let cap = (MAX_CAPTURE_WIDTH as f64 / width).min(MAX_CAPTURE_HEIGHT as f64 / height);
    let scale = upscale.min(cap).max(f64::MIN_POSITIVE);
    (
        (width * scale).round().max(1.0) as i32,
        (height * scale).round().max(1.0) as i32,
    )
}

pub struct BrowserHost {
    webview: Option<WebView>,
    window: NativeWindow,
    active: bool,
    muted: bool,
    /// Last host/page geometry applied while active (physical px), to skip
    /// redundant positioning calls while glued to the tile.
    last_geometry: Option<BrowserGeometry>,
    /// While set, the host remains offscreen at its interactive size until
    /// WebView2 has had time to render the resized/zoomed page.
    reveal_at: Option<Instant>,
    /// Backing resolution used while parked and captured. Kept in sync with
    /// the tile's physical display size so preview and live modes are 1:1.
    capture_size: (i32, i32),
    /// When the live address was last read from WebView2 (see [`Self::poll`]).
    last_url_check: Option<Instant>,
    shared: Arc<Mutex<SharedState>>,
    /// Desired sleeping state. WebView2 suspension itself is best-effort, but
    /// the controller remains hidden until the user explicitly resumes it.
    suspended: bool,
    /// Ad-block activation may request navigation while a tile sleeps. Keep
    /// it deferred because WebView2 navigation would otherwise auto-resume it.
    after_resume: Option<AfterResume>,
}

enum AfterResume {
    Load(String),
    Reload,
}

impl BrowserHost {
    pub fn new(context: &mut WebContext, url: &str) -> Result<Self, String> {
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
        let window = NativeWindow(hwnd);
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }

        let shared = Arc::new(Mutex::new(SharedState {
            current_url: url.to_owned(),
            ..Default::default()
        }));

        let nav_shared = shared.clone();
        let title_shared = shared.clone();
        let new_window_shared = shared.clone();

        let webview = WebViewBuilder::new_with_web_context(context)
            // All tiles share one WebView2 environment/profile, so this must
            // be identical on every builder. Runtime 120+ supports the flag;
            // uBOL itself requires Chromium/WebView2 122+.
            .with_browser_extensions_enabled(true)
            // Wry enables autoplay by default, which passes WebView2 the
            // `no-user-gesture-required` Chromium policy. Do not opt into
            // that policy: restored media pages (notably YouTube watch URLs)
            // should wait for the user to press Play after app startup.
            .with_autoplay(false)
            .with_initialization_script(MEDIA_AUTOPLAY_GUARD)
            .with_url(url)
            .with_bounds(parked_bounds())
            .with_download_started_handler(|_, _| false)
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
            .with_document_title_changed_handler(move |title| {
                let mut state = title_shared.lock();
                state.current_title = Some(title);
                state.dirty = true;
            })
            .with_new_window_req_handler(move |url, _features| {
                if is_web_url(&url) {
                    new_window_shared.lock().pending_navigation = Some(url);
                }
                NewWindowResponse::Deny
            })
            .build_as_child(&window)
            .map_err(|error| error.to_string())?;

        Ok(Self {
            webview: Some(webview),
            window,
            active: false,
            muted: false,
            last_geometry: None,
            reveal_at: None,
            capture_size: (WIDTH, HEIGHT),
            last_url_check: None,
            shared,
            suspended: false,
            after_resume: None,
        })
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

    pub fn is_muted(&self) -> bool {
        self.muted
    }

    /// Match an inactive browser's capture backing to its displayed physical
    /// size. The zoom changes with the backing width, preserving the same CSS
    /// viewport while eliminating bitmap upscaling in the canvas preview.
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
        if let Some(webview) = self.webview.as_ref() {
            let _ = webview.set_bounds(Rect {
                position: PhysicalPosition::new(0, 0).into(),
                size: PhysicalSize::new(size.0, size.1).into(),
            });
            let _ = webview.zoom((size.0 as f64 / WIDTH as f64).clamp(MIN_ZOOM, MAX_ZOOM));
        }
    }

    pub fn current_url(&self) -> String {
        self.shared.lock().current_url.clone()
    }

    /// The page's address read straight from WebView2, or None while the host
    /// has no WebView or sits on a non-web page.
    pub fn live_url(&self) -> Option<String> {
        let url = self.webview.as_ref()?.url().ok()?;
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

    /// Drain pending title/URL changes and apply queued same-tile navigation
    /// from blocked new-window requests. Call once per frame.
    pub fn poll(&mut self) -> BrowserUpdate {
        if self.suspended {
            return BrowserUpdate::default();
        }
        self.refresh_live_url();
        let (update, navigate) = {
            let mut state = self.shared.lock();
            let navigate = state.pending_navigation.take();
            if !state.dirty && navigate.is_none() {
                return BrowserUpdate::default();
            }
            state.dirty = false;
            (
                BrowserUpdate {
                    title: state.current_title.clone(),
                    url: Some(state.current_url.clone()),
                },
                navigate,
            )
        };
        if let Some(url) = navigate {
            self.load(&url);
        }
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
        if !take_focus
            && self.active
            && self.reveal_at.is_none()
            && self.last_geometry == Some(geometry)
        {
            return;
        }

        let resize_webview = |webview: &WebView| {
            let _ = webview.set_bounds(Rect {
                position: PhysicalPosition::new(geometry.page_x, geometry.page_y).into(),
                size: PhysicalSize::new(geometry.page_width, geometry.page_height).into(),
            });
            // Keep the page's apparent scale identical to the captured
            // texture: layout width stays WIDTH/dpi CSS px in both modes.
            let _ =
                webview.zoom((geometry.page_width as f64 / WIDTH as f64).clamp(MIN_ZOOM, MAX_ZOOM));
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
            if let Some(webview) = self.webview.as_ref() {
                resize_webview(webview);
            }
            self.active = true;
            self.last_geometry = Some(geometry);
            self.reveal_at = Some(Instant::now() + INTERACTION_PREP_DELAY);
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
                if let Some(webview) = self.webview.as_ref() {
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
                    HWND_TOP,
                    geometry.host_x,
                    geometry.host_y,
                    geometry.host_width,
                    geometry.host_height,
                    SWP_SHOWWINDOW,
                );
                let _ = SetForegroundWindow(self.window.0);
            }
            if let Some(webview) = self.webview.as_ref() {
                let _ = webview.focus();
            }
            self.reveal_at = None;
            self.last_geometry = Some(geometry);
            return;
        }

        unsafe {
            let _ = SetWindowPos(
                self.window.0,
                HWND_TOP,
                geometry.host_x,
                geometry.host_y,
                geometry.host_width,
                geometry.host_height,
                SWP_SHOWWINDOW | SWP_NOACTIVATE,
            );
        }
        // While already active, resize after moving so a panning or zooming
        // tile stays glued to its new position.
        if let Some(webview) = self.webview.as_ref() {
            resize_webview(webview);
        }
        self.active = true;
        self.last_geometry = Some(geometry);
    }

    /// Move the host back offscreen at capture resolution. Audio keeps
    /// playing and Windows Graphics Capture keeps rendering it.
    pub fn park(&mut self) {
        let (width, height) = self.capture_size;
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
        if let Some(webview) = self.webview.as_ref() {
            let _ = webview.set_bounds(Rect {
                position: PhysicalPosition::new(0, 0).into(),
                size: PhysicalSize::new(width, height).into(),
            });
            let _ = webview.zoom((width as f64 / WIDTH as f64).clamp(MIN_ZOOM, MAX_ZOOM));
        }
        self.active = false;
        self.last_geometry = None;
        self.reveal_at = None;
    }

    /// Hide the controller and ask WebView2 to put this page to sleep. The
    /// callback is intentionally non-blocking because WebView2 dispatches it
    /// through the UI thread's message pump.
    pub fn suspend(&mut self) -> Result<(), String> {
        if self.suspended {
            return Ok(());
        }
        self.park();
        let webview = self.webview.as_ref().ok_or("browser is closed")?;
        let controller = webview.controller();
        unsafe {
            controller
                .SetIsVisible(false)
                .map_err(|error| error.to_string())?;
        }
        self.suspended = true;
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
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    /// Wake a manually frozen page before its capture session is restarted.
    pub fn resume(&mut self) -> Result<(), String> {
        if !self.suspended {
            return Ok(());
        }
        let webview = self.webview.as_ref().ok_or("browser is closed")?;
        let controller = webview.controller();
        let resume_result = unsafe { controller.CoreWebView2() }
            .map_err(|error| error.to_string())
            .and_then(|core| {
                let core3: ICoreWebView2_3 = core.cast().map_err(|error| error.to_string())?;
                unsafe { core3.Resume().map_err(|error| error.to_string()) }
            });
        unsafe {
            controller
                .SetIsVisible(true)
                .map_err(|error| error.to_string())?;
        }
        self.suspended = false;
        if let Some(action) = self.after_resume.take() {
            match action {
                AfterResume::Load(url) => self.load(&url),
                AfterResume::Reload => self.reload(),
            }
        }
        self.park();
        if let Err(error) = resume_result {
            log::warn!("WebView2 resume API was unavailable; restored visibility instead: {error}");
        }
        Ok(())
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
        if let Some(webview) = self.webview.as_ref() {
            let _ = webview.load_url(url);
        }
        let mut state = self.shared.lock();
        state.current_url = url.to_owned();
        state.dirty = true;
    }

    /// Record the address a tile held on `about:blank` will navigate to once
    /// filtering is live, so its label and saved layout show the real target.
    fn set_deferred_url(&self, url: &str) {
        let mut state = self.shared.lock();
        state.current_url = url.to_owned();
        state.dirty = true;
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
        if let Some(webview) = self.webview.as_ref() {
            let _ = webview.reload();
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

    pub fn set_muted(&mut self, muted: bool) -> Result<(), String> {
        self.with_core(|core| {
            let core8: ICoreWebView2_8 = core.cast().map_err(|e| e.to_string())?;
            unsafe { core8.SetIsMuted(muted) }.map_err(|e| e.to_string())
        })?;
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
        let webview = self.webview.as_ref().ok_or("browser is closed")?;
        let core = unsafe { webview.controller().CoreWebView2() }.map_err(|e| e.to_string())?;
        f(&core)
    }
}

impl Drop for BrowserHost {
    fn drop(&mut self) {
        self.webview.take();
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
}

impl BrowserManager {
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
        }
    }

    /// Create a host + WebView for `url` and register it under `id`.
    /// Returns the host HWND for capture.
    pub fn create(&mut self, id: PreviewId, url: &str) -> Result<isize, String> {
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
        let host = BrowserHost::new(&mut self.context, initial_url)?;

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
    /// navigate. Callers should keep repainting so the wait actually ends.
    pub fn adblock_settling(&self) -> bool {
        self.adblock_settle_until
            .is_some_and(|until| Instant::now() < until)
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
            let action = deferred
                .get(id)
                .cloned()
                .map(AfterResume::Load)
                .unwrap_or(AfterResume::Reload);
            if host.suspended {
                host.after_resume = Some(action);
            } else {
                match action {
                    AfterResume::Load(url) => host.load(&url),
                    AfterResume::Reload => host.reload(),
                }
            }
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

    /// Drop hosts whose previews no longer exist.
    pub fn retain(&mut self, keep: impl Fn(PreviewId) -> bool) {
        self.hosts.retain(|id, _| keep(*id));
        self.deferred_loads.retain(|id, _| keep(*id));
    }

    pub fn clear(&mut self) {
        self.hosts.clear();
        self.deferred_loads.clear();
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

    if installed_ubol_matches_embedded_with_progress(extension_dir, progress, 0.03, 0.38) {
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
    let extract_result = extract_ubol_archive_with_progress(&staging, progress, 0.4, 0.74);
    if let Err(error) = extract_result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if !installed_ubol_matches_embedded_with_progress(&staging, progress, 0.75, 0.98) {
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
    if !extension_dir.is_dir() || !installed_ubol_manifest_matches_embedded(extension_dir) {
        return false;
    }
    let Some(marker_path) = ubol_verification_marker_path(extension_dir) else {
        return false;
    };
    fs::read_to_string(marker_path)
        .is_ok_and(|fingerprint| fingerprint.trim() == UBOL_ARCHIVE_FINGERPRINT)
}

fn installed_ubol_manifest_matches_embedded(extension_dir: &Path) -> bool {
    let Ok(installed_manifest) = fs::read(extension_dir.join("manifest.json")) else {
        return false;
    };
    let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(UBOL_ARCHIVE)) else {
        return false;
    };
    let Ok(mut embedded_manifest) = archive.by_name("manifest.json") else {
        return false;
    };
    let mut expected = Vec::with_capacity(embedded_manifest.size() as usize);
    embedded_manifest.read_to_end(&mut expected).is_ok() && installed_manifest == expected
}

fn write_ubol_verification_marker(extension_dir: &Path) {
    let Some(marker_path) = ubol_verification_marker_path(extension_dir) else {
        log::warn!("Could not determine the uBlock Origin Lite verification marker path");
        return;
    };
    if let Err(error) = fs::write(marker_path, UBOL_ARCHIVE_FINGERPRINT) {
        // The verified extension is still safe to use. A missing marker only
        // means the next launch will repeat the full verification.
        log::warn!("Could not cache uBlock Origin Lite verification: {error}");
    }
}

fn extract_ubol_archive_with_progress(
    destination: &Path,
    progress: &AtomicU32,
    start: f32,
    end: f32,
) -> Result<(), String> {
    let reader = Cursor::new(UBOL_ARCHIVE);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|error| format!("Open embedded uBlock Origin Lite package: {error}"))?;

    let entry_count = archive.len().max(1);
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("Read extension archive entry: {error}"))?;
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| format!("Unsafe path in extension archive: {}", entry.name()))?;
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
fn installed_ubol_matches_embedded(extension_dir: &Path) -> bool {
    let progress = AtomicU32::new(0);
    installed_ubol_matches_embedded_with_progress(extension_dir, &progress, 0.0, 1.0)
}

fn installed_ubol_matches_embedded_with_progress(
    extension_dir: &Path,
    progress: &AtomicU32,
    start: f32,
    end: f32,
) -> bool {
    let reader = Cursor::new(UBOL_ARCHIVE);
    let Ok(mut archive) = zip::ZipArchive::new(reader) else {
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
        browser_geometry, capture_size_for_tile, extract_ubol_archive_with_progress,
        installed_ubol_has_current_marker, installed_ubol_matches_embedded, is_allowed_navigation,
        normalize_url, parked_bounds, profile_install_marker_path, read_profile_install_marker,
        scrub_url_for_storage, ubol_verification_marker_path, write_profile_install_marker,
        write_ubol_verification_marker, NativeWindow, PREPARATION_PROGRESS_SCALE, UBOL_ARCHIVE,
        UBOL_VERSION,
    };
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
        assert_eq!(capture_size_for_tile(640, 360), (2560, 1440));
        assert_eq!(capture_size_for_tile(1280, 720), (2560, 1440));
        assert_eq!(capture_size_for_tile(5000, 2813), (3839, 2160));
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
    fn embedded_ubol_package_matches_pinned_version() {
        let mut archive = zip::ZipArchive::new(Cursor::new(UBOL_ARCHIVE)).unwrap();
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
        let progress = AtomicU32::new(0);
        extract_ubol_archive_with_progress(&destination, &progress, 0.0, 1.0).unwrap();
        assert_eq!(progress.load(Ordering::Relaxed), PREPARATION_PROGRESS_SCALE);
        assert!(installed_ubol_matches_embedded(&destination));
        assert!(!installed_ubol_has_current_marker(&destination));

        write_ubol_verification_marker(&destination);
        assert!(installed_ubol_has_current_marker(&destination));

        std::fs::write(destination.join("manifest.json"), b"{}").unwrap();
        assert!(!installed_ubol_has_current_marker(&destination));
        assert!(!installed_ubol_matches_embedded(&destination));
        std::fs::remove_dir_all(&destination).unwrap();
        std::fs::remove_file(marker).unwrap();
    }
}
