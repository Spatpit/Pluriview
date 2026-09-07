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
mod ui_theme;
#[cfg(windows)]
mod video;
mod window_picker;

use app::PluriviewApp;
use eframe::egui;
use persistence::{Storage, WindowLayout, DEFAULT_WINDOW_SIZE};

fn main() -> eframe::Result<()> {
    #[cfg(windows)]
    configure_runtime_library_directory();

    env_logger::init();

    // Share the Canvas P branding with the tray and executable resource.
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

/// Make packaged runtimes available before eframe asks Windows to load EGL.
/// Release builds keep every DLL under `lib/`; the development fallback keeps
/// `cargo run` working with the prepared runtimes under `vendor/`.
#[cfg(windows)]
fn configure_runtime_library_directory() {
    use std::os::windows::ffi::OsStrExt;
    use windows::{core::PCWSTR, Win32::System::LibraryLoader::SetDllDirectoryW};

    let executable = std::env::current_exe().ok();
    let Some(directory) = runtime_library_candidates(executable.as_deref())
        .into_iter()
        .find(|directory| {
            directory.join("libEGL.dll").is_file() && directory.join("libGLESv2.dll").is_file()
        })
    else {
        return;
    };

    let wide_path: Vec<u16> = directory
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    if let Err(error) = unsafe { SetDllDirectoryW(PCWSTR(wide_path.as_ptr())) } {
        eprintln!(
            "Could not configure the runtime library folder {}: {error}",
            directory.display()
        );
    }
}

#[cfg(windows)]
fn runtime_library_candidates(executable: Option<&std::path::Path>) -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();
    if let Some(directory) = executable.and_then(std::path::Path::parent) {
        candidates.push(directory.join("lib"));

        let is_build_profile = directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, "debug" | "release"));
        let target_directory = directory.parent();
        let is_target_directory = target_directory
            .and_then(std::path::Path::file_name)
            .and_then(|name| name.to_str())
            == Some("target");
        if is_build_profile && is_target_directory {
            if let Some(workspace) = target_directory.and_then(std::path::Path::parent) {
                candidates.push(workspace.join("vendor"));
            }
        }
    }
    candidates
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

/// Create a high-resolution window icon for the taskbar and Alt+Tab.
fn create_window_icon() -> egui::IconData {
    let size = 256;
    egui::IconData {
        rgba: crate::tray::create_app_icon_rgba(size),
        width: size,
        height: size,
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::runtime_library_candidates;
    use std::path::Path;

    #[test]
    fn packaged_lib_folder_precedes_development_fallbacks() {
        let candidates = runtime_library_candidates(Some(Path::new(
            r"C:\Program Files\Pluriview\pluriview.exe",
        )));

        assert_eq!(
            candidates,
            vec![Path::new(r"C:\Program Files\Pluriview\lib").to_path_buf()]
        );
    }

    #[test]
    fn source_build_uses_workspace_vendor_as_fallback() {
        let candidates = runtime_library_candidates(Some(Path::new(
            r"S:\src\pluriview\target\debug\pluriview.exe",
        )));

        assert_eq!(
            candidates,
            vec![
                Path::new(r"S:\src\pluriview\target\debug\lib").to_path_buf(),
                Path::new(r"S:\src\pluriview\vendor").to_path_buf(),
            ]
        );
    }
}
