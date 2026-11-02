//! Using a captured window from its tile.
//!
//! A window tile only shows a capture, so to use the app in place Pluriview
//! moves the real window exactly over the tile and puts it back afterwards.
//! Only the window's position changes, never its size, so the app does not
//! rearrange its layout. Moves are asynchronous: a busy or hung app must
//! never stall the canvas.

use eframe::egui::{Pos2, Rect, Vec2};

/// Height of the grab bar shown above a window in use, in points.
const GRAB_BAR_HEIGHT: f32 = 28.0;

/// A tile within this fraction of the window's real size counts as showing
/// it at that size, so the window lines up with the tile's corner.
const NATIVE_SCALE_TOLERANCE: f32 = 0.015;

/// Physical-pixel rectangle in screen coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PixelRect {
    pub fn width(self) -> i32 {
        self.right - self.left
    }

    pub fn height(self) -> i32 {
        self.bottom - self.top
    }

    pub fn origin(self) -> (i32, i32) {
        (self.left, self.top)
    }

    pub fn size(self) -> (i32, i32) {
        (self.width(), self.height())
    }
}

/// A top-level window's outer rectangle, which `SetWindowPos` positions, and
/// its visible frame, which Windows Graphics Capture records. They differ by
/// the invisible resize borders of Windows 10 and 11.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowFrame {
    pub outer: PixelRect,
    pub visible: PixelRect,
}

impl WindowFrame {
    /// Outer origin that puts the visible frame's top-left at `visible_origin`.
    pub fn outer_origin_for(self, visible_origin: (i32, i32)) -> (i32, i32) {
        (
            visible_origin.0 + self.outer.left - self.visible.left,
            visible_origin.1 + self.outer.top - self.visible.top,
        )
    }
}

fn crop_parts(crop: Option<(f32, f32, f32, f32)>) -> (Vec2, Vec2) {
    let (min_u, min_v, max_u, max_v) = crop.unwrap_or((0.0, 0.0, 1.0, 1.0));
    (
        Vec2::new(min_u, min_v),
        Vec2::new((max_u - min_u).max(0.001), (max_v - min_v).max(0.001)),
    )
}

/// Canvas zoom that shows a tile `tile_width` canvas units wide at the real
/// size of a window whose visible frame is `visible_width` pixels.
pub fn native_zoom(
    tile_width: f32,
    crop: Option<(f32, f32, f32, f32)>,
    visible_width: i32,
    pixels_per_point: f32,
) -> f32 {
    let (_, crop_size) = crop_parts(crop);
    crop_size.x * visible_width as f32 / pixels_per_point.max(0.01) / tile_width.max(1.0)
}

/// The whole window's rectangle for a tile showing `crop` of it at `tile`.
/// A cropped tile's window reaches past the tile on the cropped sides.
pub fn window_rect_for_tile(tile: Rect, crop: Option<(f32, f32, f32, f32)>) -> Rect {
    let (crop_min, crop_size) = crop_parts(crop);
    let size = tile.size() / crop_size;
    Rect::from_min_size(tile.min - crop_min * size, size)
}

/// Where the window's visible frame of `visible_size` pixels goes so the
/// part the tile shows covers the tile. `tile` is in egui points relative to
/// the client area at `client_origin` (pixels). A tile showing the window at
/// its real size lines up corner to corner; any other (a pinned tile, or a
/// zoom Pluriview could not reach) is covered centered.
pub fn visible_origin_for_tile(
    tile: Rect,
    crop: Option<(f32, f32, f32, f32)>,
    visible_size: (i32, i32),
    pixels_per_point: f32,
    client_origin: (i32, i32),
) -> (i32, i32) {
    let (crop_min, crop_size) = crop_parts(crop);
    let visible = Vec2::new(visible_size.0 as f32, visible_size.1 as f32);
    let shown = crop_size * visible;
    let tile_pixels = tile.size() * pixels_per_point;
    let native = (tile_pixels.x / shown.x.max(1.0) - 1.0).abs() <= NATIVE_SCALE_TOLERANCE;
    let origin = if native {
        tile.min.to_vec2() * pixels_per_point - crop_min * visible
    } else {
        tile.center().to_vec2() * pixels_per_point - (crop_min + crop_size / 2.0) * visible
    };
    (
        client_origin.0 + origin.x.round() as i32,
        client_origin.1 + origin.y.round() as i32,
    )
}

/// The visible frame's rectangle in egui points for a frame at `origin`.
pub fn visible_rect_in_points(
    origin: (i32, i32),
    visible_size: (i32, i32),
    pixels_per_point: f32,
    client_origin: (i32, i32),
) -> Rect {
    let ppp = pixels_per_point.max(0.01);
    Rect::from_min_size(
        Pos2::new(
            (origin.0 - client_origin.0) as f32 / ppp,
            (origin.1 - client_origin.1) as f32 / ppp,
        ),
        Vec2::new(visible_size.0 as f32 / ppp, visible_size.1 as f32 / ppp),
    )
}

/// A window moved across monitors is rescaled by the ratio of their scales,
/// whether the app redraws for the new scale or Windows stretches it.
pub fn size_after_dpi_change(size: (i32, i32), from_dpi: u32, to_dpi: u32) -> (i32, i32) {
    if from_dpi == 0 || to_dpi == 0 || from_dpi == to_dpi {
        return size;
    }
    let ratio = to_dpi as f32 / from_dpi as f32;
    (
        (size.0 as f32 * ratio).round() as i32,
        (size.1 as f32 * ratio).round() as i32,
    )
}

/// Pan that shows `subject` (canvas units) at `new_zoom`: its center stays
/// where it is on screen when possible, then it slides just far enough to
/// fit inside `canvas_rect` with room for the grab bar above it. A subject
/// larger than the canvas is centered.
///
/// `screen` is the usable part of the monitor in the same points. The real
/// window lands where the subject does and may hang past Pluriview's edges,
/// so it then slides just far enough to be fully on that screen, or, when
/// larger than it, to keep its top-left corner (title bar, menus) there.
pub fn interaction_pan(
    canvas_rect: Rect,
    pan: Vec2,
    zoom: f32,
    subject: Rect,
    new_zoom: f32,
    margin: f32,
    screen: Option<Rect>,
) -> Vec2 {
    let screen_center = (subject.center().to_vec2() + pan) * zoom + canvas_rect.min.to_vec2();
    let mut new_pan =
        (screen_center - canvas_rect.min.to_vec2()) / new_zoom - subject.center().to_vec2();
    let shown = Rect::from_center_size(screen_center.to_pos2(), subject.size() * new_zoom);
    let room = Rect::from_min_max(
        canvas_rect.min + Vec2::new(margin, margin + GRAB_BAR_HEIGHT),
        canvas_rect.max - Vec2::splat(margin),
    );
    let shift = |low: f32, high: f32, room_low: f32, room_high: f32| {
        if high - low > room_high - room_low {
            (room_low + room_high) / 2.0 - (low + high) / 2.0
        } else if low < room_low {
            room_low - low
        } else if high > room_high {
            room_high - high
        } else {
            0.0
        }
    };
    let mut offset = Vec2::new(
        shift(shown.min.x, shown.max.x, room.min.x, room.max.x),
        shift(shown.min.y, shown.max.y, room.min.y, room.max.y),
    );
    if let Some(screen) = screen.filter(|screen| screen.is_positive()) {
        let shown = shown.translate(offset);
        let onto_screen = |low: f32, high: f32, screen_low: f32, screen_high: f32| {
            if high - low > screen_high - screen_low || low < screen_low {
                screen_low - low
            } else if high > screen_high {
                screen_high - high
            } else {
                0.0
            }
        };
        offset += Vec2::new(
            onto_screen(shown.min.x, shown.max.x, screen.min.x, screen.max.x),
            onto_screen(shown.min.y, shown.max.y, screen.min.y, screen.max.y),
        );
    }
    new_pan += offset / new_zoom;
    new_pan
}

/// Where the grab bar goes for a window covering `window` (points): on top,
/// below when the top has no room, kept inside the canvas.
pub fn grab_bar_rect(window: Rect, canvas_rect: Rect) -> Rect {
    let width = window.width().clamp(180.0, canvas_rect.width().max(180.0));
    let left = window.left().clamp(
        canvas_rect.left(),
        (canvas_rect.right() - width).max(canvas_rect.left()),
    );
    let top = if window.top() - GRAB_BAR_HEIGHT >= canvas_rect.top() {
        window.top() - GRAB_BAR_HEIGHT
    } else if window.bottom() + GRAB_BAR_HEIGHT <= canvas_rect.bottom() {
        window.bottom()
    } else {
        canvas_rect.top()
    };
    Rect::from_min_size(Pos2::new(left, top), Vec2::new(width, GRAB_BAR_HEIGHT))
}

#[cfg(windows)]
pub use native::*;

#[cfg(windows)]
mod native {
    use super::{PixelRect, WindowFrame};
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::Graphics::Dwm::{
        DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
    };
    use windows::Win32::Graphics::Gdi::{
        ClientToScreen, GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindow, GetWindowLongW, GetWindowPlacement, GetWindowRect,
        GetWindowThreadProcessId, IsHungAppWindow, IsIconic, IsWindow, IsWindowVisible, IsZoomed,
        SetForegroundWindow, SetWindowPlacement, SetWindowPos, ShowWindow, GWL_EXSTYLE,
        GW_HWNDNEXT, GW_HWNDPREV, GW_OWNER, HWND_NOTOPMOST, HWND_TOP, SWP_ASYNCWINDOWPOS,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_SHOWMINNOACTIVE,
        SW_SHOWNOACTIVATE, WINDOWPLACEMENT, WS_EX_TOPMOST,
    };

    fn hwnd(handle: isize) -> HWND {
        HWND(handle as *mut _)
    }

    fn pixel_rect(rect: RECT) -> PixelRect {
        PixelRect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }

    /// The window's outer rectangle and visible frame, in physical pixels.
    pub fn read_frame(handle: isize) -> Option<WindowFrame> {
        let mut outer = RECT::default();
        unsafe { GetWindowRect(hwnd(handle), &mut outer) }.ok()?;
        let mut visible = RECT::default();
        let visible = unsafe {
            DwmGetWindowAttribute(
                hwnd(handle),
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&mut visible as *mut RECT).cast(),
                std::mem::size_of::<RECT>() as u32,
            )
        }
        .ok()
        .map(|_| visible)
        .filter(|rect| rect.right > rect.left && rect.bottom > rect.top)
        .unwrap_or(outer);
        Some(WindowFrame {
            outer: pixel_rect(outer),
            visible: pixel_rect(visible),
        })
    }

    pub fn is_alive(handle: isize) -> bool {
        exists(handle) && unsafe { IsWindowVisible(hwnd(handle)).as_bool() }
    }

    /// The window has not been closed, though it may be hidden.
    pub fn exists(handle: isize) -> bool {
        unsafe { IsWindow(hwnd(handle)).as_bool() }
    }

    pub fn is_minimized(handle: isize) -> bool {
        unsafe { IsIconic(hwnd(handle)).as_bool() }
    }

    pub fn is_maximized(handle: isize) -> bool {
        unsafe { IsZoomed(hwnd(handle)).as_bool() }
    }

    /// Not responding for several seconds; moving it would do nothing.
    pub fn is_hung(handle: isize) -> bool {
        unsafe { IsHungAppWindow(hwnd(handle)).as_bool() }
    }

    /// Hidden by Windows, such as a window on another virtual desktop.
    pub fn is_cloaked(handle: isize) -> bool {
        let mut cloaked = 0u32;
        unsafe {
            DwmGetWindowAttribute(
                hwnd(handle),
                DWMWA_CLOAKED,
                (&mut cloaked as *mut u32).cast(),
                std::mem::size_of::<u32>() as u32,
            )
        }
        .is_ok()
            && cloaked != 0
    }

    fn is_topmost(handle: HWND) -> bool {
        let style = unsafe { GetWindowLongW(handle, GWL_EXSTYLE) } as u32;
        style & WS_EX_TOPMOST.0 != 0
    }

    /// Effective scale of the monitor showing most of the window.
    pub fn monitor_dpi(handle: isize) -> u32 {
        let monitor = unsafe { MonitorFromWindow(hwnd(handle), MONITOR_DEFAULTTONEAREST) };
        let (mut x, mut y) = (0u32, 0u32);
        match unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) } {
            Ok(()) => x,
            Err(_) => 96,
        }
    }

    /// The usable part (without the taskbar) of the monitor showing most of
    /// the window, in physical pixels.
    pub fn work_area(handle: isize) -> Option<PixelRect> {
        let monitor = unsafe { MonitorFromWindow(hwnd(handle), MONITOR_DEFAULTTONEAREST) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        unsafe { GetMonitorInfoW(monitor, &mut info) }
            .as_bool()
            .then(|| pixel_rect(info.rcWork))
            .filter(|rect| rect.width() > 0 && rect.height() > 0)
    }

    /// Screen position of `handle`'s client area, in physical pixels.
    pub fn client_origin(handle: isize) -> (i32, i32) {
        let mut point = POINT::default();
        unsafe {
            let _ = ClientToScreen(hwnd(handle), &mut point);
        }
        (point.x, point.y)
    }

    pub fn process_id(handle: isize) -> u32 {
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(hwnd(handle), Some(&mut pid));
        }
        pid
    }

    pub fn foreground() -> isize {
        unsafe { GetForegroundWindow() }.0 as isize
    }

    /// The foreground window is the app itself: its window, a menu or dialog
    /// it owns, or another of its windows.
    pub fn foreground_belongs_to(foreground: isize, handle: isize, pid: u32) -> bool {
        if foreground == 0 {
            return false;
        }
        if pid != 0 && process_id(foreground) == pid {
            return true;
        }
        is_or_is_owned_by(hwnd(foreground), handle)
    }

    /// `window` is `handle` or one of its menus or dialogs, following owner
    /// links, which also cover windows owned without the popup style that
    /// GetAncestor's root owner skips.
    fn is_or_is_owned_by(mut window: HWND, handle: isize) -> bool {
        for _ in 0..32 {
            if window.0 as isize == handle {
                return true;
            }
            match unsafe { GetWindow(window, GW_OWNER) } {
                Ok(owner) if !owner.0.is_null() => window = owner,
                _ => return false,
            }
        }
        false
    }

    /// The windows covering `handle`, nearest first, for putting it back
    /// under them later. Only windows that can cover it count: shown, not
    /// minimized, on this desktop and not always on top. Its own menus and
    /// dialogs, and windows of `skip_pids`, are left out.
    pub fn windows_above(handle: isize, skip_pids: &[u32]) -> Vec<isize> {
        // The nearest few are plenty: the first that still exists is used.
        const KEEP: usize = 8;
        let mut found = Vec::new();
        let mut window = hwnd(handle);
        // Bounded in case the z-order changes while it is walked.
        for _ in 0..4096 {
            match unsafe { GetWindow(window, GW_HWNDPREV) } {
                Ok(previous) if !previous.0.is_null() => window = previous,
                _ => break,
            }
            if can_cover(window)
                && !is_or_is_owned_by(window, handle)
                && !skip_pids.contains(&process_id(window.0 as isize))
            {
                found.push(window.0 as isize);
                if found.len() == KEEP {
                    break;
                }
            }
        }
        found
    }

    /// Not always on top either: a window put under one of those would
    /// become always on top too.
    fn can_cover(window: HWND) -> bool {
        let handle = window.0 as isize;
        is_alive(handle) && !is_minimized(handle) && !is_cloaked(handle) && !is_topmost(window)
    }

    /// Put `handle` back directly under the nearest of `covering` that can
    /// still cover it, without activating anything. False if none can.
    pub fn place_below(handle: isize, covering: &[isize]) -> bool {
        let Some(&above) = covering.iter().find(|&&window| can_cover(hwnd(window))) else {
            return false;
        };
        // Asynchronous like `move_window`, so a busy app cannot block us.
        unsafe {
            SetWindowPos(
                hwnd(handle),
                hwnd(above),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            )
        }
        .is_ok()
    }

    /// Whether `handle` sits somewhere below `above` in the z-order.
    pub fn is_below(handle: isize, above: isize) -> bool {
        let mut next = hwnd(above);
        // Bounded in case the z-order changes while it is walked.
        for _ in 0..4096 {
            match unsafe { GetWindow(next, GW_HWNDNEXT) } {
                Ok(window) if !window.0.is_null() => {
                    if window.0 as isize == handle {
                        return true;
                    }
                    next = window;
                }
                _ => return false,
            }
        }
        false
    }

    /// Ask the window to move its outer top-left to `origin`. The app moves
    /// itself when it next handles messages, so a busy app cannot block us.
    pub fn move_window(handle: isize, origin: (i32, i32)) -> bool {
        unsafe {
            SetWindowPos(
                hwnd(handle),
                HWND::default(),
                origin.0,
                origin.1,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            )
        }
        .is_ok()
    }

    /// Bring the window above Pluriview and give it the keyboard. Works while
    /// Pluriview is the foreground app.
    pub fn raise_and_focus(handle: isize) -> bool {
        unsafe { SetForegroundWindow(hwnd(handle)).as_bool() }
    }

    /// Put Pluriview's window directly above `handle` without activating it,
    /// staying below anything the user brought in front of both.
    pub fn place_above(own: isize, handle: isize) {
        let above = unsafe { GetWindow(hwnd(handle), GW_HWNDPREV) }
            .ok()
            .filter(|window| !window.0.is_null() && window.0 as isize != own);
        // Following a topmost window would make Pluriview topmost too.
        let insert_after = match above {
            Some(window) if !is_topmost(window) => window,
            _ => HWND_TOP,
        };
        unsafe {
            let _ = SetWindowPos(
                hwnd(own),
                insert_after,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// Drop Pluriview's own always-on-top level right away, so the window in
    /// use can come in front of it.
    pub fn drop_topmost(own: isize) {
        unsafe {
            let _ = SetWindowPos(
                hwnd(own),
                HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// How a minimized window was laid out, to minimize it again unchanged.
    #[derive(Clone, Copy)]
    pub struct SavedPlacement(WINDOWPLACEMENT);

    pub fn placement(handle: isize) -> Option<SavedPlacement> {
        let mut placement = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        unsafe { GetWindowPlacement(hwnd(handle), &mut placement) }.ok()?;
        Some(SavedPlacement(placement))
    }

    /// Show a minimized window where it was, without activating it.
    pub fn show_without_activating(handle: isize) {
        unsafe {
            let _ = ShowWindow(hwnd(handle), SW_SHOWNOACTIVATE);
        }
    }

    /// Minimize the window back into `saved`'s layout without activating
    /// anything.
    pub fn minimize_into(handle: isize, saved: SavedPlacement) {
        let mut placement = saved.0;
        placement.showCmd = SW_SHOWMINNOACTIVE.0 as u32;
        unsafe {
            let _ = SetWindowPlacement(hwnd(handle), &placement);
        }
    }

    /// Move the place the window is restored to by `offset` and minimize it
    /// there, or leave it minimized: for a window minimized over its tile.
    pub fn shift_restored_position(handle: isize, offset: (i32, i32)) {
        let Some(SavedPlacement(mut placement)) = placement(handle) else {
            return;
        };
        let normal = &mut placement.rcNormalPosition;
        normal.left += offset.0;
        normal.right += offset.0;
        normal.top += offset.1;
        normal.bottom += offset.1;
        placement.showCmd = SW_SHOWMINNOACTIVE.0 as u32;
        placement.flags.0 = 0;
        unsafe {
            let _ = SetWindowPlacement(hwnd(handle), &placement);
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::test_windows::{pump_messages, TestWindow};
    use super::*;

    fn frame(outer: (i32, i32, i32, i32), visible: (i32, i32, i32, i32)) -> WindowFrame {
        let rect = |(left, top, right, bottom)| PixelRect {
            left,
            top,
            right,
            bottom,
        };
        WindowFrame {
            outer: rect(outer),
            visible: rect(visible),
        }
    }

    #[test]
    fn invisible_borders_are_kept_when_positioning_the_visible_frame() {
        // Windows 10/11 frames are 7 px wider than they look on three sides.
        let window = frame((93, 50, 1107, 857), (100, 50, 1100, 850));
        assert_eq!(window.outer_origin_for((400, 300)), (393, 300));
    }

    #[test]
    fn native_zoom_shows_the_window_pixel_for_pixel() {
        // A 1000 px window on a 150% display is 666.7 points wide.
        let zoom = native_zoom(500.0, None, 1000, 1.5);
        assert!((zoom * 500.0 * 1.5 - 1000.0).abs() < 0.01);
        // A tile cropped to half the width shows half as many pixels.
        let cropped = native_zoom(500.0, Some((0.25, 0.0, 0.75, 1.0)), 1000, 1.0);
        assert!((cropped - 1.0).abs() < 0.001);
    }

    #[test]
    fn a_native_size_tile_lines_up_corner_to_corner() {
        let tile = Rect::from_min_size(Pos2::new(100.0, 80.0), Vec2::new(500.0, 400.0));
        let origin = visible_origin_for_tile(tile, None, (1000, 800), 2.0, (10, 20));
        assert_eq!(origin, (210, 180));
    }

    #[test]
    fn a_cropped_native_tile_puts_the_shown_part_over_the_tile() {
        // The tile shows the right half of a 1000x800 window at 100%.
        let tile = Rect::from_min_size(Pos2::new(300.0, 100.0), Vec2::new(500.0, 800.0));
        let crop = Some((0.5, 0.0, 1.0, 1.0));
        assert_eq!(
            visible_origin_for_tile(tile, crop, (1000, 800), 1.0, (0, 0)),
            (-200, 100)
        );
        let whole = window_rect_for_tile(tile, crop);
        assert_eq!(
            whole,
            Rect::from_min_size(Pos2::new(-200.0, 100.0), Vec2::new(1000.0, 800.0))
        );
    }

    #[test]
    fn a_tile_at_another_scale_is_covered_centered() {
        // A pinned 250x200 tile of a 1000x800 window.
        let tile = Rect::from_min_size(Pos2::new(500.0, 400.0), Vec2::new(250.0, 200.0));
        let origin = visible_origin_for_tile(tile, None, (1000, 800), 1.0, (0, 0));
        assert_eq!(origin, (125, 100));
    }

    #[test]
    fn visible_rect_converts_back_to_points() {
        let rect = visible_rect_in_points((210, 180), (1000, 800), 2.0, (10, 20));
        assert_eq!(
            rect,
            Rect::from_min_size(Pos2::new(100.0, 80.0), Vec2::new(500.0, 400.0))
        );
    }

    #[test]
    fn moving_between_monitors_rescales_by_their_ratio() {
        assert_eq!(size_after_dpi_change((1000, 800), 96, 144), (1500, 1200));
        assert_eq!(size_after_dpi_change((1000, 800), 144, 144), (1000, 800));
        assert_eq!(size_after_dpi_change((1000, 800), 0, 144), (1000, 800));
    }

    #[test]
    fn interaction_keeps_the_tile_center_when_it_fits() {
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0));
        let tile = Rect::from_min_size(Pos2::new(700.0, 400.0), Vec2::new(200.0, 100.0));
        let pan = interaction_pan(canvas, Vec2::ZERO, 1.0, tile, 2.0, 16.0, None);
        let center = (tile.center().to_vec2() + pan) * 2.0;
        assert!((center - Vec2::new(800.0, 450.0)).length() < 0.01);
    }

    #[test]
    fn interaction_slides_the_tile_into_view_below_the_grab_bar() {
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0));
        // Near the top-left corner; at 2x it would spill past both edges.
        let tile = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(200.0, 100.0));
        let pan = interaction_pan(canvas, Vec2::ZERO, 1.0, tile, 2.0, 16.0, None);
        let min = (tile.min.to_vec2() + pan) * 2.0;
        assert!((min.x - 16.0).abs() < 0.01);
        assert!((min.y - (16.0 + GRAB_BAR_HEIGHT)).abs() < 0.01);
    }

    #[test]
    fn interaction_centers_a_window_larger_than_the_canvas() {
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let tile = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(500.0, 400.0));
        let pan = interaction_pan(canvas, Vec2::ZERO, 1.0, tile, 2.0, 16.0, None);
        let center = (tile.center().to_vec2() + pan) * 2.0;
        assert!((center.x - 400.0).abs() < 0.01);
        assert!((center.y - (300.0 + GRAB_BAR_HEIGHT / 2.0)).abs() < 0.01);
    }

    /// Pluriview's 1000 x 700 canvas sits at the left of a 1920 x 1032
    /// screen, 100 pixels down, so the screen starts at y = -100 in canvas
    /// points.
    const SCREEN: Rect = Rect {
        min: Pos2::new(0.0, -100.0),
        max: Pos2::new(1920.0, 932.0),
    };

    fn shown_at(pan: Vec2, tile: Rect, zoom: f32) -> Rect {
        Rect::from_min_size(
            ((tile.min.to_vec2() + pan) * zoom).to_pos2(),
            tile.size() * zoom,
        )
    }

    #[test]
    fn interaction_slides_a_window_larger_than_pluriview_onto_the_screen() {
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0));
        // A 1400 x 900 window: centered on the canvas, its left 200 points
        // would be past the screen's left edge.
        let tile = Rect::from_min_size(Pos2::new(100.0, 100.0), Vec2::new(700.0, 450.0));
        let centered = shown_at(
            interaction_pan(canvas, Vec2::ZERO, 1.0, tile, 2.0, 16.0, None),
            tile,
            2.0,
        );
        assert!((centered.min.x + 200.0).abs() < 0.01, "{centered:?}");

        let shown = shown_at(
            interaction_pan(canvas, Vec2::ZERO, 1.0, tile, 2.0, 16.0, Some(SCREEN)),
            tile,
            2.0,
        );
        assert!(shown.min.x.abs() < 0.01, "{shown:?}");
        // It already fit vertically, so only its left edge moved.
        assert!((shown.min.y - centered.min.y).abs() < 0.01);
        assert!(SCREEN.contains_rect(shown));
    }

    #[test]
    fn interaction_keeps_the_top_left_of_a_window_larger_than_the_screen() {
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0));
        let tile = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 600.0));
        let shown = shown_at(
            interaction_pan(canvas, Vec2::ZERO, 1.0, tile, 2.0, 16.0, Some(SCREEN)),
            tile,
            2.0,
        );
        assert!((shown.min - SCREEN.min).length() < 0.01, "{shown:?}");
    }

    #[test]
    fn interaction_leaves_a_window_that_fits_pluriview_where_it_was() {
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0));
        let tile = Rect::from_min_size(Pos2::new(700.0, 400.0), Vec2::new(200.0, 100.0));
        let screen = Rect::from_min_size(Pos2::new(-50.0, -50.0), Vec2::new(1920.0, 1080.0));
        assert_eq!(
            interaction_pan(canvas, Vec2::ZERO, 1.0, tile, 2.0, 16.0, Some(screen)),
            interaction_pan(canvas, Vec2::ZERO, 1.0, tile, 2.0, 16.0, None)
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "creates native windows; run with --ignored on a Windows desktop"]
    fn native_moves_land_exactly_and_ownership_and_stacking_are_read_correctly() {
        let main = TestWindow::new(None);
        let other = TestWindow::new(None);
        let popup = TestWindow::new(Some(&main));
        pump_messages();

        let frame = read_frame(main.handle()).unwrap();
        assert!(frame.outer.left <= frame.visible.left && frame.outer.top <= frame.visible.top);
        assert!(frame.outer.right >= frame.visible.right);
        assert!(frame.outer.bottom >= frame.visible.bottom);

        // Moving by the outer origin puts the visible frame exactly in place.
        let goal = (-29_000, -28_500);
        let outer = frame.outer_origin_for(goal);
        assert!(move_window(main.handle(), outer));
        for _ in 0..100 {
            pump_messages();
            if read_frame(main.handle()).unwrap().outer.origin() == outer {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let moved = read_frame(main.handle()).unwrap();
        assert_eq!(moved.visible.origin(), goal);
        assert_eq!(moved.visible.size(), frame.visible.size());

        // Menus and dialogs the app owns, and its other windows, are the app.
        let pid = std::process::id();
        assert_eq!(process_id(main.handle()), pid);
        assert!(foreground_belongs_to(popup.handle(), main.handle(), 0));
        assert!(!foreground_belongs_to(other.handle(), main.handle(), 0));
        assert!(foreground_belongs_to(other.handle(), main.handle(), pid));
        assert!(!foreground_belongs_to(0, main.handle(), pid));

        place_above(other.handle(), main.handle());
        assert!(is_below(main.handle(), other.handle()));
        assert!(!is_below(other.handle(), main.handle()));
        place_above(main.handle(), other.handle());
        assert!(is_below(other.handle(), main.handle()));

        assert!(is_alive(main.handle()));
        assert!(!is_minimized(main.handle()) && !is_maximized(main.handle()));
        assert!(!is_cloaked(main.handle()) && !is_hung(main.handle()));
        assert!(monitor_dpi(main.handle()) >= 96);
        let handle = popup.handle();
        drop(popup);
        assert!(!is_alive(handle));
    }

    /// A window brought to the front for use goes back under the windows that
    /// covered it, or under the next one if the nearest has closed.
    #[cfg(windows)]
    #[test]
    #[ignore = "creates native windows; run with --ignored on a Windows desktop"]
    fn a_window_goes_back_under_the_windows_that_covered_it() {
        let under = TestWindow::new(None);
        let window = TestWindow::new(None);
        let near = TestWindow::new(None);
        let far = TestWindow::new(None);
        // Top to bottom: far, near, window, under.
        place_above(window.handle(), under.handle());
        place_above(near.handle(), window.handle());
        place_above(far.handle(), near.handle());
        // Its own menu, open above it, is not something it goes under.
        let menu = TestWindow::new(Some(&window));
        pump_messages();
        assert!(is_below(window.handle(), near.handle()));

        let covering = windows_above(window.handle(), &[]);
        assert_eq!(covering[..2], [near.handle(), far.handle()]);
        assert!(!covering.contains(&menu.handle()));
        // Pluriview's own windows are left out.
        let others = windows_above(window.handle(), &[std::process::id()]);
        assert!(!others.contains(&near.handle()) && !others.contains(&far.handle()));

        // In use: in front of all of them.
        place_above(window.handle(), far.handle());
        assert!(is_below(far.handle(), window.handle()));
        assert!(place_below(window.handle(), &covering));
        pump_messages();
        assert!(is_below(window.handle(), near.handle()));
        assert!(is_below(under.handle(), window.handle()));

        place_above(window.handle(), far.handle());
        drop(near);
        assert!(place_below(window.handle(), &covering));
        pump_messages();
        assert!(is_below(window.handle(), far.handle()));
        assert!(is_below(under.handle(), window.handle()));

        // None left of these two: it stays where it is. (The rest of the
        // list are real windows on this desktop, and not used here.)
        drop(far);
        assert!(!place_below(window.handle(), &covering[..2]));
    }

    /// Placement assumes a window capture records exactly the visible frame,
    /// not the invisible resize borders around it.
    #[cfg(windows)]
    #[test]
    #[ignore = "captures a native window; run with --ignored on a Windows desktop"]
    fn window_capture_records_exactly_the_visible_frame() {
        use crate::capture::CaptureCoordinator;
        use crate::preview::PreviewManager;

        let window = TestWindow::new(None);
        pump_messages();
        let frame = read_frame(window.handle()).unwrap();
        assert_ne!(frame.outer.size(), frame.visible.size());

        let mut previews = PreviewManager::new();
        let id = previews.add_for_window(
            window.handle(),
            std::process::id(),
            "Pluriview Live Window Test".into(),
            Pos2::ZERO,
            Vec2::new(640.0, 480.0),
        );
        let mut captures = CaptureCoordinator::new();
        captures.start_capture(id, window.handle(), "Pluriview Live Window Test".into(), 30);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while previews.get(id).unwrap().source_frame_size.is_none()
            && std::time::Instant::now() < deadline
        {
            pump_messages();
            captures.process_frames(&mut previews);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        captures.stop_all();
        let (width, height) = previews
            .get(id)
            .unwrap()
            .source_frame_size
            .expect("no captured frame");
        assert_eq!((width as i32, height as i32), frame.visible.size());
    }

    /// Turning the pointer on restarts the capture, which must keep running
    /// with the tile's frame rate, output size and pause intact.
    #[cfg(windows)]
    #[test]
    #[ignore = "captures a native window; run with --ignored on a Windows desktop"]
    fn pointer_capture_restarts_the_tile_capture_with_its_settings() {
        use crate::capture::CaptureCoordinator;
        use crate::preview::PreviewManager;

        let window = TestWindow::new(None);
        pump_messages();
        let mut previews = PreviewManager::new();
        let id = previews.add_for_window(
            window.handle(),
            std::process::id(),
            "Pluriview Live Window Test".into(),
            Pos2::ZERO,
            Vec2::new(640.0, 480.0),
        );
        let mut captures = CaptureCoordinator::new();
        let frame_after = |captures: &mut CaptureCoordinator, previews: &mut PreviewManager| {
            previews.get_mut(id).unwrap().source_frame_size = None;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while previews.get(id).unwrap().source_frame_size.is_none()
                && std::time::Instant::now() < deadline
            {
                pump_messages();
                captures.process_frames(previews);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            previews.get(id).unwrap().source_frame_size.is_some()
        };
        captures.start_capture(id, window.handle(), "Pluriview Live Window Test".into(), 30);
        captures.set_target_size(id, 320, 240);
        assert!(frame_after(&mut captures, &mut previews));
        assert!(!captures.is_in_use(id));

        captures.set_in_use(id, true);
        assert!(captures.is_in_use(id));
        assert!(captures.is_live(id));
        assert!(
            frame_after(&mut captures, &mut previews),
            "no frame after restart"
        );
        let (width, height) = previews.get(id).unwrap().frame_size.unwrap();
        assert!(width <= 320 && height <= 240, "{width}x{height}");

        captures.set_in_use(id, false);
        assert!(!captures.is_in_use(id));
        assert!(frame_after(&mut captures, &mut previews));
        captures.stop_all();
    }

    /// While in use, a menu the app opens shows on its tile (Windows 11 24H2
    /// and later), and one hanging past the window's edge does not change the
    /// tile's frame size.
    #[cfg(windows)]
    #[test]
    #[ignore = "captures a native window; run with --ignored on a Windows desktop"]
    fn an_app_in_use_shows_its_menus_on_the_tile() {
        use crate::capture::CaptureCoordinator;
        use crate::preview::PreviewManager;
        use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};

        if !windows_capture::graphics_capture_api::GraphicsCaptureApi::is_secondary_windows_supported()
            .unwrap_or(false)
        {
            eprintln!("Skipped: this Windows version cannot capture an app's menus");
            return;
        }
        let window = TestWindow::painted(None, true);
        pump_messages();
        let frame = read_frame(window.handle()).unwrap();
        // A dark "menu" over the white window, hanging past its right edge.
        let menu = TestWindow::painted(Some(&window), false);
        let (menu_left, menu_top) = (frame.visible.right - 60, frame.visible.top + 200);
        unsafe {
            SetWindowPos(
                menu.0,
                windows::Win32::Foundation::HWND::default(),
                menu_left,
                menu_top,
                120,
                80,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
            .unwrap();
        }

        let mut previews = PreviewManager::new();
        let id = previews.add_for_window(
            window.handle(),
            std::process::id(),
            "Pluriview Live Window Test".into(),
            Pos2::ZERO,
            Vec2::new(640.0, 480.0),
        );
        let mut captures = CaptureCoordinator::new();
        // The menu's pixel just inside the window's right edge.
        let menu_pixel = |previews: &PreviewManager| {
            let image = previews.get(id)?.latest_capture_frame()?;
            let x = (menu_left + 30 - frame.visible.left) as usize;
            let y = (menu_top + 40 - frame.visible.top) as usize;
            Some((image.size, image.pixels[y * image.size[0] + x]))
        };
        let settle = |captures: &mut CaptureCoordinator, previews: &mut PreviewManager| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                pump_messages();
                captures.process_frames(previews);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            menu_pixel(previews).expect("no captured frame")
        };
        let size = [
            frame.visible.width() as usize,
            frame.visible.height() as usize,
        ];

        captures.start_capture(id, window.handle(), "Pluriview Live Window Test".into(), 30);
        let (shown, pixel) = settle(&mut captures, &mut previews);
        assert_eq!(shown, size);
        assert!(pixel.r() > 200, "menu captured while not in use: {pixel:?}");

        captures.set_in_use(id, true);
        let (shown, pixel) = settle(&mut captures, &mut previews);
        assert_eq!(shown, size, "the menu changed the frame size");
        assert!(pixel.r() < 50, "menu missing while in use: {pixel:?}");
        captures.stop_all();
    }

    #[test]
    fn grab_bar_sits_above_the_window_or_below_when_the_top_is_full() {
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0));
        let window = Rect::from_min_size(Pos2::new(300.0, 200.0), Vec2::new(600.0, 400.0));
        let bar = grab_bar_rect(window, canvas);
        assert_eq!(bar.bottom(), window.top());
        assert_eq!(bar.width(), window.width());

        let at_top = window.translate(Vec2::new(0.0, -200.0));
        assert_eq!(grab_bar_rect(at_top, canvas).top(), at_top.bottom());

        // Narrow windows still get a bar wide enough for its controls.
        let narrow = Rect::from_min_size(Pos2::new(1550.0, 200.0), Vec2::new(40.0, 40.0));
        let bar = grab_bar_rect(narrow, canvas);
        assert_eq!(bar.width(), 180.0);
        assert_eq!(bar.right(), canvas.right());
    }
}

/// Native windows for tests that move, stack and capture real windows.
#[cfg(all(test, windows))]
pub(crate) mod test_windows {
    /// An off-screen tool window owned by this test, never activated.
    pub(crate) struct TestWindow(pub(crate) windows::Win32::Foundation::HWND);

    impl TestWindow {
        pub(crate) fn new(owner: Option<&TestWindow>) -> Self {
            Self::create(owner, windows::core::w!("PluriviewLiveWindowTest"), false)
        }

        /// A white window, or a black pop-up like a menu its owner opened.
        pub(crate) fn painted(owner: Option<&TestWindow>, white: bool) -> Self {
            use windows::core::w;
            if white {
                Self::create(owner, w!("PluriviewLiveWindowTestWhite"), false)
            } else {
                Self::create(owner, w!("PluriviewLiveWindowTestBlack"), true)
            }
        }

        fn create(
            owner: Option<&TestWindow>,
            class_name: windows::core::PCWSTR,
            popup: bool,
        ) -> Self {
            use windows::core::w;
            use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
            use windows::Win32::Graphics::Gdi::{GetStockObject, BLACK_BRUSH, HBRUSH, WHITE_BRUSH};
            use windows::Win32::System::LibraryLoader::GetModuleHandleW;
            use windows::Win32::UI::WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, RegisterClassW, ShowWindow, SW_SHOWNOACTIVATE,
                WNDCLASSW, WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW, WS_POPUP,
            };

            unsafe extern "system" fn window_proc(
                hwnd: HWND,
                message: u32,
                wparam: WPARAM,
                lparam: LPARAM,
            ) -> LRESULT {
                unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            }

            static REGISTERED: std::sync::Once = std::sync::Once::new();
            REGISTERED.call_once(|| {
                let classes = [
                    (w!("PluriviewLiveWindowTest"), HBRUSH::default()),
                    (
                        w!("PluriviewLiveWindowTestWhite"),
                        HBRUSH(unsafe { GetStockObject(WHITE_BRUSH) }.0),
                    ),
                    (
                        w!("PluriviewLiveWindowTestBlack"),
                        HBRUSH(unsafe { GetStockObject(BLACK_BRUSH) }.0),
                    ),
                ];
                for (name, background) in classes {
                    let class = WNDCLASSW {
                        lpfnWndProc: Some(window_proc),
                        hInstance: unsafe { GetModuleHandleW(None) }.unwrap().into(),
                        lpszClassName: name,
                        hbrBackground: background,
                        ..Default::default()
                    };
                    assert_ne!(unsafe { RegisterClassW(&class) }, 0);
                }
            });
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    class_name,
                    w!("Pluriview Live Window Test"),
                    if popup { WS_POPUP } else { WS_OVERLAPPEDWINDOW },
                    -30_000,
                    -30_000,
                    640,
                    480,
                    owner.map_or_else(HWND::default, |owner| owner.0),
                    None,
                    None,
                    None,
                )
            }
            .unwrap();
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
            Self(hwnd)
        }

        pub(crate) fn handle(&self) -> isize {
            self.0 .0 as isize
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0);
            }
        }
    }

    pub(crate) fn pump_messages() {
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
        };
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}
