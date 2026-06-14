//! ShareX-style region selector overlay for cropping
//!
//! Creates a transparent overlay window on top of the source window,
//! allowing users to click and drag to select a region for cropping.

use std::sync::{Arc, Mutex};
use std::thread;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM, RECT, POINT};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, CreateSolidBrush, CreatePen, SelectObject, DeleteObject,
    FillRect, Rectangle, SetBkMode, PAINTSTRUCT, PS_SOLID, TRANSPARENT,
    GetStockObject, NULL_BRUSH, InvalidateRect,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, RegisterClassW, DestroyWindow, PostQuitMessage,
    GetMessageW, TranslateMessage, DispatchMessageW, SetWindowPos, ShowWindow,
    GetWindowRect, GetClientRect, LoadCursorW, SetCursor, SetLayeredWindowAttributes,
    WNDCLASSW, MSG, HWND_TOPMOST, SWP_SHOWWINDOW, SW_SHOW,
    WS_EX_TOPMOST, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_POPUP,
    WM_PAINT, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_KEYDOWN, WM_DESTROY,
    WM_CREATE, WM_SETCURSOR, CS_HREDRAW, CS_VREDRAW, IDC_CROSS, LAYERED_WINDOW_ATTRIBUTES_FLAGS,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetCapture, ReleaseCapture, VK_ESCAPE};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;

/// Result of a region selection
#[derive(Debug, Clone, Copy)]
pub struct RegionSelection {
    /// X coordinate relative to source window client area
    pub x: i32,
    /// Y coordinate relative to source window client area
    pub y: i32,
    /// Width of selected region
    pub width: i32,
    /// Height of selected region
    pub height: i32,
}

impl RegionSelection {
    /// Convert pixel selection to UV coordinates (0.0 to 1.0)
    pub fn to_uv(&self, source_width: u32, source_height: u32) -> (f32, f32, f32, f32) {
        let min_u = self.x as f32 / source_width as f32;
        let min_v = self.y as f32 / source_height as f32;
        let max_u = (self.x + self.width) as f32 / source_width as f32;
        let max_v = (self.y + self.height) as f32 / source_height as f32;
        (
            min_u.clamp(0.0, 1.0),
            min_v.clamp(0.0, 1.0),
            max_u.clamp(0.0, 1.0),
            max_v.clamp(0.0, 1.0),
        )
    }
}

/// Shared state for the overlay window
#[allow(dead_code)]
struct OverlayState {
    /// Is the user currently dragging?
    is_dragging: bool,
    /// Start point of drag (in client coords)
    start_point: POINT,
    /// Current point of drag (in client coords)
    current_point: POINT,
    /// Final selection result (None = cancelled, Some = selected)
    result: Option<Option<RegionSelection>>,
    /// Source window rect (for coordinate conversion)
    source_rect: RECT,
}

/// Static storage for overlay state (needed for window proc callback)
static OVERLAY_STATE: Mutex<Option<Arc<Mutex<OverlayState>>>> = Mutex::new(None);

/// Region selector that creates an overlay on a source window
pub struct RegionSelector {
    /// Thread handle for the overlay window
    thread_handle: Option<thread::JoinHandle<Option<RegionSelection>>>,
    /// Shared state with the overlay thread
    state: Arc<Mutex<OverlayState>>,
}

impl RegionSelector {
    /// Show the region selector overlay on top of the specified window
    pub fn show_for_window(source_hwnd: isize) -> Option<Self> {
        // Get source window rect
        let mut source_rect = RECT::default();
        unsafe {
            if GetWindowRect(HWND(source_hwnd as *mut _), &mut source_rect).is_err() {
                return None;
            }
        }

        // Create shared state
        let state = Arc::new(Mutex::new(OverlayState {
            is_dragging: false,
            start_point: POINT::default(),
            current_point: POINT::default(),
            result: None,
            source_rect,
        }));

        // Store state globally for window proc
        {
            let mut global_state = OVERLAY_STATE.lock().unwrap();
            *global_state = Some(Arc::clone(&state));
        }

        // Spawn overlay window in a separate thread
        let state_clone = Arc::clone(&state);
        let thread_handle = thread::spawn(move || {
            run_overlay_window(source_rect, state_clone)
        });

        Some(Self {
            thread_handle: Some(thread_handle),
            state,
        })
    }

    /// Poll for the selection result (non-blocking)
    /// Returns Some(result) when selection is complete, None if still in progress
    pub fn poll_result(&mut self) -> Option<Option<RegionSelection>> {
        // Check if we have a result
        let state = self.state.lock().unwrap();
        if state.result.is_some() {
            return state.result;
        }
        drop(state);

        // Check if thread has finished
        if let Some(handle) = self.thread_handle.take() {
            if handle.is_finished() {
                match handle.join() {
                    Ok(result) => return Some(result),
                    Err(_) => return Some(None), // Thread panicked, treat as cancel
                }
            } else {
                // Put handle back
                self.thread_handle = Some(handle);
            }
        }

        None
    }
}

/// Run the overlay window message loop
fn run_overlay_window(source_rect: RECT, state: Arc<Mutex<OverlayState>>) -> Option<RegionSelection> {
    unsafe {
        // Register window class
        let class_name = w!("PluriviewRegionSelector");

        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(overlay_window_proc),
            hInstance: GetModuleHandleW(None).unwrap().into(),
            lpszClassName: class_name,
            hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
            ..Default::default()
        };

        RegisterClassW(&wc);

        // Calculate overlay size
        let width = source_rect.right - source_rect.left;
        let height = source_rect.bottom - source_rect.top;

        // Create the overlay window
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW,
            class_name,
            w!("Region Selector"),
            WS_POPUP,
            source_rect.left,
            source_rect.top,
            width,
            height,
            None,
            None,
            None,
            None,
        ).unwrap();

        // Set window transparency (semi-transparent overlay)
        let _ = SetLayeredWindowAttributes(hwnd, None, 200, LAYERED_WINDOW_ATTRIBUTES_FLAGS(2)); // LWA_ALPHA = 2

        // Position and show the window
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            source_rect.left,
            source_rect.top,
            width,
            height,
            SWP_SHOWWINDOW,
        );
        let _ = ShowWindow(hwnd, SW_SHOW);

        // Message loop
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Get result from state
        let state = state.lock().unwrap();
        state.result.flatten()
    }
}

/// Window procedure for the overlay
unsafe extern "system" fn overlay_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            // Capture mouse to ensure we get all mouse events
            SetCapture(hwnd);
            // Set crosshair cursor immediately
            if let Ok(cursor) = LoadCursorW(None, IDC_CROSS) {
                SetCursor(cursor);
            }
            LRESULT(0)
        }

        WM_SETCURSOR => {
            // Always show crosshair cursor - prevent Windows from showing loading cursor
            if let Ok(cursor) = LoadCursorW(None, IDC_CROSS) {
                SetCursor(cursor);
            }
            LRESULT(1) // Return TRUE to prevent further processing
        }

        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);

            // Get client rect
            let mut client_rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut client_rect);

            // Fill with semi-transparent dark color
            let dark_brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00404040));
            FillRect(hdc, &client_rect, dark_brush);
            let _ = DeleteObject(dark_brush);

            // Draw selection rectangle if dragging
            if let Some(state_arc) = OVERLAY_STATE.lock().unwrap().as_ref() {
                let state = state_arc.lock().unwrap();
                if state.is_dragging {
                    // Calculate selection rect
                    let sel_rect = normalize_rect(state.start_point, state.current_point);

                    // Clear the selection area (make it "transparent" by filling with lighter color)
                    let clear_brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00606060));
                    FillRect(hdc, &sel_rect, clear_brush);
                    let _ = DeleteObject(clear_brush);

                    // Draw cyan border around selection
                    let pen = CreatePen(PS_SOLID, 2, windows::Win32::Foundation::COLORREF(0x00FFFF00)); // Cyan in BGR
                    let old_pen = SelectObject(hdc, pen);
                    let null_brush = GetStockObject(NULL_BRUSH);
                    let old_brush = SelectObject(hdc, null_brush);

                    let _ = Rectangle(hdc, sel_rect.left, sel_rect.top, sel_rect.right, sel_rect.bottom);

                    SelectObject(hdc, old_pen);
                    SelectObject(hdc, old_brush);
                    let _ = DeleteObject(pen);

                    // Draw dimension text
                    let width = (sel_rect.right - sel_rect.left).abs();
                    let height = (sel_rect.bottom - sel_rect.top).abs();
                    if width > 0 && height > 0 {
                        let text = format!("{} × {}\0", width, height);
                        let text_wide: Vec<u16> = text.encode_utf16().collect();

                        SetBkMode(hdc, TRANSPARENT);
                        windows::Win32::Graphics::Gdi::SetTextColor(hdc, windows::Win32::Foundation::COLORREF(0x0000FFFF)); // Cyan

                        let text_x = sel_rect.left + 5;
                        let text_y = sel_rect.top + 5;
                        let _ = windows::Win32::Graphics::Gdi::TextOutW(hdc, text_x, text_y, &text_wide[..text_wide.len()-1]);
                    }
                }
            }

            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }

        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;

            if let Some(state_arc) = OVERLAY_STATE.lock().unwrap().as_ref() {
                let mut state = state_arc.lock().unwrap();
                state.is_dragging = true;
                state.start_point = POINT { x, y };
                state.current_point = POINT { x, y };
            }

            // Request repaint
            let _ = InvalidateRect(hwnd, None, true);
            LRESULT(0)
        }

        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;

            if let Some(state_arc) = OVERLAY_STATE.lock().unwrap().as_ref() {
                let mut state = state_arc.lock().unwrap();
                if state.is_dragging {
                    state.current_point = POINT { x, y };
                    drop(state);
                    // Request repaint
                    let _ = InvalidateRect(hwnd, None, true);
                }
            }
            LRESULT(0)
        }

        WM_LBUTTONUP => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;

            if let Some(state_arc) = OVERLAY_STATE.lock().unwrap().as_ref() {
                let mut state = state_arc.lock().unwrap();
                if state.is_dragging {
                    state.current_point = POINT { x, y };
                    state.is_dragging = false;

                    // Calculate final selection
                    let sel_rect = normalize_rect(state.start_point, state.current_point);
                    let width = sel_rect.right - sel_rect.left;
                    let height = sel_rect.bottom - sel_rect.top;

                    // Only accept if selection has some size
                    if width > 10 && height > 10 {
                        state.result = Some(Some(RegionSelection {
                            x: sel_rect.left,
                            y: sel_rect.top,
                            width,
                            height,
                        }));
                    } else {
                        // Too small, treat as cancel
                        state.result = Some(None);
                    }
                }
            }

            let _ = ReleaseCapture();
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }

        WM_KEYDOWN => {
            let vk = wparam.0 as i32;
            if vk == VK_ESCAPE.0 as i32 {
                // ESC pressed - cancel selection
                if let Some(state_arc) = OVERLAY_STATE.lock().unwrap().as_ref() {
                    let mut state = state_arc.lock().unwrap();
                    state.result = Some(None);
                }
                let _ = ReleaseCapture();
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }

        WM_DESTROY => {
            // Clean up global state
            {
                let mut global_state = OVERLAY_STATE.lock().unwrap();
                *global_state = None;
            }
            PostQuitMessage(0);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Normalize a rect so that left < right and top < bottom
fn normalize_rect(p1: POINT, p2: POINT) -> RECT {
    RECT {
        left: p1.x.min(p2.x),
        top: p1.y.min(p2.y),
        right: p1.x.max(p2.x),
        bottom: p1.y.max(p2.y),
    }
}
