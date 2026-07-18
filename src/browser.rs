use std::{
    cell::RefCell,
    collections::HashMap,
    fs,
    io::{self, Cursor},
    num::NonZeroIsize,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, OnceLock},
};

use parking_lot::Mutex;
use webview2_com::{
    BrowserExtensionEnableCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2BrowserExtension, ICoreWebView2Profile7, ICoreWebView2_13, ICoreWebView2_8,
    },
    ProfileAddBrowserExtensionCompletedHandler,
};
use webview2_core::{Interface, HSTRING as WebViewHString};
use windows::{
    core::w,
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        Graphics::Gdi::ClientToScreen,
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE},
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

const UBOL_VERSION: &str = "2026.714.1952";
const UBOL_ARCHIVE: &[u8] =
    include_bytes!("../assets/third_party/ubol/uBOLite_2026.714.1952.edge.zip");

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
        Ok(url) if matches!(url.scheme(), "http" | "https") => Ok(input.to_owned()),
        Ok(_) => Err("Only HTTP and HTTPS websites are supported"),
        Err(url::ParseError::RelativeUrlWithoutBase) => {
            let candidate = format!("https://{input}");
            url::Url::parse(&candidate)
                .map(|_| candidate)
                .map_err(|_| "Enter a valid website URL")
        }
        Err(_) => Err("Enter a valid website URL"),
    }
}

/// Schemes a page may navigate to while staying inside the tile. `about:` is
/// needed because sites navigate iframes/blank targets through it.
fn is_allowed_navigation(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http:")
        || lower.starts_with("https:")
        || lower.starts_with("about:")
        || lower.starts_with("blob:")
        || lower.starts_with("data:")
}

/// True while the Escape key is held. Used to exit interaction mode: once the
/// WebView has focus, egui never sees keyboard input, so we poll instead.
pub fn escape_pressed() -> bool {
    (unsafe { GetAsyncKeyState(VK_ESCAPE.0 as i32) } as u16 & 0x8000) != 0
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

pub struct BrowserHost {
    webview: Option<WebView>,
    window: NativeWindow,
    active: bool,
    muted: bool,
    /// Last screen rect applied while active (physical px), to skip redundant
    /// SetWindowPos calls while glued to the tile.
    last_rect: Option<(i32, i32, i32, i32)>,
    shared: Arc<Mutex<SharedState>>,
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
                if url.starts_with("http") {
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
            last_rect: None,
            shared,
        })
    }

    pub fn hwnd(&self) -> isize {
        self.window.0 .0 as isize
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn is_muted(&self) -> bool {
        self.muted
    }

    pub fn current_url(&self) -> String {
        self.shared.lock().current_url.clone()
    }

    /// Drain pending title/URL changes and apply queued same-tile navigation
    /// from blocked new-window requests. Call once per frame.
    pub fn poll(&mut self) -> BrowserUpdate {
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

    /// Position the host window over `rect` (egui points, client coordinates
    /// of `parent`). Sizes the WebView to fill the host and matches the zoom
    /// factor so the page keeps the exact layout it had as a captured tile.
    pub fn place(
        &mut self,
        parent: HWND,
        rect: egui::Rect,
        pixels_per_point: f32,
        take_focus: bool,
    ) {
        let mut origin = POINT {
            x: (rect.min.x * pixels_per_point).round() as i32,
            y: (rect.min.y * pixels_per_point).round() as i32,
        };
        unsafe {
            let _ = ClientToScreen(parent, &mut origin);
        }
        let size = rect.size() * pixels_per_point;
        let width = (size.x.round() as i32).max(1);
        let height = (size.y.round() as i32).max(1);

        let screen_rect = (origin.x, origin.y, width, height);
        if !take_focus && self.active && self.last_rect == Some(screen_rect) {
            return;
        }

        let flags = if take_focus {
            SWP_SHOWWINDOW
        } else {
            SWP_SHOWWINDOW | SWP_NOACTIVATE
        };
        unsafe {
            let _ = SetWindowPos(
                self.window.0,
                HWND_TOP,
                origin.x,
                origin.y,
                width,
                height,
                flags,
            );
        }
        if let Some(webview) = self.webview.as_ref() {
            let _ = webview.set_bounds(Rect {
                position: PhysicalPosition::new(0, 0).into(),
                size: PhysicalSize::new(width, height).into(),
            });
            // Keep the page's apparent scale identical to the captured
            // texture: layout width stays WIDTH/dpi CSS px in both modes.
            let _ = webview.zoom((width as f64 / WIDTH as f64).clamp(MIN_ZOOM, MAX_ZOOM));
            if take_focus {
                unsafe {
                    let _ = SetForegroundWindow(self.window.0);
                }
                let _ = webview.focus();
            }
        }
        self.active = true;
        self.last_rect = Some(screen_rect);
    }

    /// Move the host back offscreen at capture resolution. Audio keeps
    /// playing and Windows Graphics Capture keeps rendering it.
    pub fn park(&mut self) {
        unsafe {
            let _ = SetWindowPos(
                self.window.0,
                HWND_BOTTOM,
                PARK_X,
                PARK_Y,
                WIDTH,
                HEIGHT,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
        if let Some(webview) = self.webview.as_ref() {
            let _ = webview.set_bounds(parked_bounds());
            let _ = webview.zoom(1.0);
        }
        self.active = false;
        self.last_rect = None;
    }

    /// True when keyboard focus belongs to this host (or one of the WebView's
    /// child windows). Used to park on focus loss.
    pub fn owns_foreground(&self) -> bool {
        let foreground = unsafe { GetForegroundWindow() };
        foreground == self.window.0 || unsafe { IsChild(self.window.0, foreground).as_bool() }
    }

    pub fn load(&self, url: &str) {
        if let Some(webview) = self.webview.as_ref() {
            let _ = webview.load_url(url);
        }
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

    fn install_browser_extension(
        &self,
        extension_dir: &Path,
    ) -> Result<ICoreWebView2BrowserExtension, String> {
        self.with_core(|core| {
            let profile: ICoreWebView2Profile7 = unsafe {
                core.cast::<ICoreWebView2_13>()
                    .and_then(|core13| core13.Profile())
                    .and_then(|profile| profile.cast())
            }
            .map_err(|error| {
                format!("This WebView2 Runtime cannot install browser extensions: {error}")
            })?;

            let installed = Rc::new(RefCell::new(None));
            let completed_result = installed.clone();
            let profile_for_call = profile.clone();
            let path = WebViewHString::from(extension_dir);
            ProfileAddBrowserExtensionCompletedHandler::wait_for_async_operation(
                Box::new(move |handler| unsafe {
                    profile_for_call
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
    extension_initialized: bool,
    extension: Option<ICoreWebView2BrowserExtension>,
    adblock_enabled: bool,
    adblock_error: Option<String>,
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
            extension_initialized: false,
            extension: None,
            adblock_enabled: true,
            adblock_error: None,
        }
    }

    /// Create a host + WebView for `url` and register it under `id`.
    /// Returns the host HWND for capture.
    pub fn create(&mut self, id: PreviewId, url: &str) -> Result<isize, String> {
        // Hold the first real navigation on about:blank until uBOL has been
        // installed and enabled, avoiding an initial burst of unfiltered
        // requests. Extension failure is non-fatal: browser tiles still work.
        let initialize_extension = !self.extension_initialized;
        // Files must be at their final version before the shared WebView2
        // environment starts; an installed unpacked extension must not be
        // mutated while that environment is using it.
        let prepared_extension = initialize_extension.then(|| {
            self.extension_dir
                .clone()
                .ok_or_else(|| {
                    "Pluriview could not determine its application data directory".to_owned()
                })
                .and_then(|extension_dir| {
                    prepare_ubol(&extension_dir)?;
                    Ok(extension_dir)
                })
        });
        let initial_url = if initialize_extension {
            "about:blank"
        } else {
            url
        };
        let host = BrowserHost::new(&mut self.context, initial_url)?;

        if initialize_extension {
            self.extension_initialized = true;
            let initialized = prepared_extension
                .expect("extension preparation is present for first initialization")
                .and_then(|extension_dir| self.initialize_adblock(&host, &extension_dir));
            if let Err(error) = initialized {
                log::error!("Ad blocker unavailable: {error}");
                self.adblock_error = Some(error);
            }
            host.load(url);
        }

        let hwnd = host.hwnd();
        self.hosts.insert(id, host);
        Ok(hwnd)
    }

    fn initialize_adblock(
        &mut self,
        host: &BrowserHost,
        extension_dir: &Path,
    ) -> Result<(), String> {
        let extension = host.install_browser_extension(extension_dir)?;
        set_extension_enabled(&extension, self.adblock_enabled)?;
        self.extension = Some(extension);
        self.adblock_error = None;
        Ok(())
    }

    pub fn adblock_enabled(&self) -> bool {
        self.adblock_enabled
    }

    pub fn adblock_status_text(&self) -> String {
        if let Some(error) = &self.adblock_error {
            return format!("Unavailable: {error}");
        }
        if !self.extension_initialized {
            return "uBlock Origin Lite loads with the first browser tile".to_owned();
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
        if let Some(extension) = &self.extension {
            set_extension_enabled(extension, enabled)?;
        } else if self.extension_initialized {
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

    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
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
    }

    pub fn clear(&mut self) {
        self.hosts.clear();
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

fn prepare_ubol(extension_dir: &Path) -> Result<(), String> {
    if installed_ubol_version(extension_dir).as_deref() == Some(UBOL_VERSION) {
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

    let extract_result = extract_ubol_archive(&staging);
    if let Err(error) = extract_result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if installed_ubol_version(&staging).as_deref() != Some(UBOL_VERSION) {
        let _ = fs::remove_dir_all(&staging);
        return Err("Embedded uBlock Origin Lite package has an unexpected manifest".to_owned());
    }

    if extension_dir.exists() {
        fs::remove_dir_all(extension_dir)
            .map_err(|error| format!("Replace old extension version: {error}"))?;
    }
    fs::rename(&staging, extension_dir)
        .map_err(|error| format!("Activate uBlock Origin Lite files: {error}"))
}

fn extract_ubol_archive(destination: &Path) -> Result<(), String> {
    let reader = Cursor::new(UBOL_ARCHIVE);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|error| format!("Open embedded uBlock Origin Lite package: {error}"))?;

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
    }
    Ok(())
}

fn installed_ubol_version(extension_dir: &Path) -> Option<String> {
    let manifest = fs::read(extension_dir.join("manifest.json")).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&manifest).ok()?;
    value.get("version")?.as_str().map(str::to_owned)
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
        is_allowed_navigation, normalize_url, parked_bounds, NativeWindow, UBOL_ARCHIVE,
        UBOL_VERSION,
    };
    use std::io::{Cursor, Read};
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
    }

    #[test]
    fn navigation_filter_blocks_local_schemes() {
        assert!(is_allowed_navigation("https://youtube.com/watch"));
        assert!(is_allowed_navigation("about:blank"));
        assert!(!is_allowed_navigation("file:///C:/secret.txt"));
        assert!(!is_allowed_navigation("ms-settings:display"));
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
}
