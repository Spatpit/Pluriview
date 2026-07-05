use std::{num::NonZeroIsize, sync::OnceLock};

use windows::{
    core::w,
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        Graphics::Gdi::ClientToScreen,
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetForegroundWindow, IsChild,
            RegisterClassW, SetForegroundWindow, SetWindowPos, ShowWindow, HWND_BOTTOM, HWND_TOP,
            SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_SHOW, SW_SHOWNOACTIVATE, WNDCLASSW, WS_CLIPCHILDREN,
            WS_EX_TOOLWINDOW, WS_POPUP,
        },
    },
};
use wry::{
    raw_window_handle::{
        HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
    },
    WebContext, WebView, WebViewBuilder,
};

const PARK_X: i32 = -30_000;
const PARK_Y: i32 = -30_000;
const WIDTH: i32 = 1280;
const HEIGHT: i32 = 720;

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

pub struct BrowserHost {
    webview: Option<WebView>,
    _context: WebContext,
    window: NativeWindow,
    active: bool,
}

impl BrowserHost {
    pub fn new(url: &str) -> Result<Self, String> {
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

        let data_dir = directories::ProjectDirs::from("com", "pluriview", "Pluriview")
            .map(|dirs| dirs.data_dir().join("webview2"));
        let mut context = WebContext::new(data_dir);
        let webview = WebViewBuilder::new_with_web_context(&mut context)
            .with_url(url)
            .with_download_started_handler(|_, _| false)
            .build_as_child(&window)
            .map_err(|error| error.to_string())?;

        Ok(Self {
            webview: Some(webview),
            _context: context,
            window,
            active: false,
        })
    }

    pub fn hwnd(&self) -> isize {
        self.window.0 .0 as isize
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn activate(&mut self, parent: HWND, rect: egui::Rect, pixels_per_point: f32) {
        let mut origin = POINT {
            x: (rect.min.x * pixels_per_point).round() as i32,
            y: (rect.min.y * pixels_per_point).round() as i32,
        };
        unsafe {
            let _ = ClientToScreen(parent, &mut origin);
        }
        let size = rect.size() * pixels_per_point;
        unsafe {
            let _ = SetWindowPos(
                self.window.0,
                HWND_TOP,
                origin.x,
                origin.y,
                size.x.round() as i32,
                size.y.round() as i32,
                SWP_SHOWWINDOW,
            );
            let _ = ShowWindow(self.window.0, SW_SHOW);
            let _ = SetForegroundWindow(self.window.0);
        }
        if let Some(webview) = self.webview.as_ref() {
            let _ = webview.focus();
        }
        self.active = true;
    }

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
        self.active = false;
    }

    pub fn parent_has_focus(&self, parent: HWND) -> bool {
        let foreground = unsafe { GetForegroundWindow() };
        foreground == parent || unsafe { IsChild(parent, foreground).as_bool() }
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
    use super::NativeWindow;
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
}
