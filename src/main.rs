#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
#[cfg(windows)]
mod audio;
#[cfg(windows)]
mod browser;
mod canvas;
mod capture;
mod external_tools;
mod hotkeys;
#[cfg(windows)]
mod libmpv;
mod media;
mod overlay;
mod persistence;
mod playlist;
mod preview;
mod privacy;
mod spout;
mod tray;
#[cfg(windows)]
mod video;
mod window_picker;

use app::PluriviewApp;
use eframe::egui;
use persistence::{Storage, WindowLayout, DEFAULT_WINDOW_SIZE};

fn main() -> eframe::Result<()> {
    env_logger::init();

    // Create the window icon (leaf)
    let icon = create_window_icon();

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([DEFAULT_WINDOW_SIZE.0, DEFAULT_WINDOW_SIZE.1])
        .with_min_inner_size([800.0, 600.0])
        .with_title("Pluriview")
        .with_icon(icon)
        // We draw our own title bar (see app.rs) so it can match the
        // app's dark theme instead of the OS chrome.
        .with_decorations(false);

    // Reopen where the window was left. The app itself loads this layout
    // again for its tiles; the geometry has to be read here because the
    // viewport is built before the app exists.
    //
    // The saved size and position are the *restored* ones even when the
    // window was closed maximized, so the window is always built at them and
    // the app re-maximizes on its first frame. Building it maximized here
    // would not survive anyway: winit applies the position afterwards, which
    // restores the window.
    if let Some(window) = saved_window_geometry() {
        viewport = viewport.with_inner_size([window.size.0, window.size.1]);
        if let Some(position) = window.position.filter(is_on_screen) {
            viewport = viewport.with_position([position.0, position.1]);
        }
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Pluriview",
        options,
        Box::new(|cc| Ok(Box::new(PluriviewApp::new(cc)))),
    )
}

/// Geometry the main window was last closed at, if a layout has one.
fn saved_window_geometry() -> Option<WindowLayout> {
    Storage::new()?.load_autosave().ok()?.window
}

/// Whether a saved top-left corner still lands somewhere the user can reach.
/// Without this, a window saved on a monitor that is now unplugged would
/// reopen off-screen — and with the OS decorations off there is no title bar
/// to drag it back with.
#[cfg(windows)]
fn is_on_screen(position: &(f32, f32)) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    // Logical points, compared against physical pixels: on a scaled display
    // the logical rect is the smaller of the two, so this only ever errs
    // toward rejecting a position and opening centered instead.
    let (x, y) = *position;
    let left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) } as f32;
    let top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) } as f32;
    let right = left + unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) } as f32;
    let bottom = top + unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) } as f32;

    // Leave room for a grabbable strip of the window rather than accepting a
    // corner that sits one pixel inside the desktop.
    const VISIBLE_MARGIN: f32 = 80.0;
    x >= left && y >= top && x <= right - VISIBLE_MARGIN && y <= bottom - VISIBLE_MARGIN
}

#[cfg(not(windows))]
fn is_on_screen(_position: &(f32, f32)) -> bool {
    true
}

/// Create the window icon (green leaf) for title bar and taskbar
fn create_window_icon() -> egui::IconData {
    let size = 32;
    egui::IconData {
        rgba: crate::tray::create_leaf_rgba(size),
        width: size,
        height: size,
    }
}
