use std::sync::OnceLock;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, SetForegroundWindow, ShowWindow, SW_MINIMIZE, SW_RESTORE, WM_CLOSE,
};

/// Menu item IDs
const MENU_SHOW: &str = "show";
const MENU_HIDE: &str = "hide";
const MENU_QUIT: &str = "quit";

/// Global storage for the main window HWND (needed for static closure)
static MAIN_WINDOW_HWND: OnceLock<isize> = OnceLock::new();

/// Manages the system tray icon and menu
pub struct TrayManager {
    /// The tray icon (must be kept alive)
    _tray_icon: TrayIcon,
}

impl TrayManager {
    /// Set the main window HWND (call this after window is created)
    pub fn set_window_hwnd(hwnd: isize) {
        let _ = MAIN_WINDOW_HWND.set(hwnd);
    }

    /// Create a new tray manager with icon and menu
    pub fn new() -> Option<Self> {
        // Set up the event handler with DIRECT Win32 API calls
        // This bypasses the need for the eframe event loop to process events
        MenuEvent::set_event_handler(Some(|event: MenuEvent| {
            #[cfg(debug_assertions)]
            println!("Tray menu event: {:?}", event.id.0);
            match event.id.0.as_str() {
                MENU_SHOW => {
                    #[cfg(debug_assertions)]
                    println!("Show clicked");
                    if let Some(&hwnd) = MAIN_WINDOW_HWND.get() {
                        unsafe {
                            let _ = ShowWindow(HWND(hwnd as *mut _), SW_RESTORE);
                            let _ = SetForegroundWindow(HWND(hwnd as *mut _));
                        }
                    }
                }
                MENU_HIDE => {
                    #[cfg(debug_assertions)]
                    println!("Hide clicked");
                    if let Some(&hwnd) = MAIN_WINDOW_HWND.get() {
                        unsafe {
                            let _ = ShowWindow(HWND(hwnd as *mut _), SW_MINIMIZE);
                        }
                    }
                }
                MENU_QUIT => {
                    #[cfg(debug_assertions)]
                    println!("Quit clicked");
                    if let Some(&hwnd) = MAIN_WINDOW_HWND.get() {
                        unsafe {
                            let _ = PostMessageW(HWND(hwnd as *mut _), WM_CLOSE, None, None);
                        }
                    }
                }
                _ => {}
            }
        }));

        // Create menu items
        let show_item = MenuItem::with_id(MENU_SHOW, "Show Pluriview", true, None);
        let hide_item = MenuItem::with_id(MENU_HIDE, "Hide", true, None);
        let quit_item = MenuItem::with_id(MENU_QUIT, "Quit", true, None);

        // Build the menu
        let menu = Menu::with_items(&[
            &show_item,
            &hide_item,
            &PredefinedMenuItem::separator(),
            &quit_item,
        ])
        .ok()?;

        let size = 32;
        let icon = Icon::from_rgba(create_app_icon_rgba(size), size, size).ok()?;

        // Build the tray icon
        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Pluriview - Live Window Previews")
            .with_icon(icon)
            .build()
            .ok()?;

        Some(Self {
            _tray_icon: tray_icon,
        })
    }
}

/// Decode the embedded production icon shared with the executable resource.
pub(crate) fn create_app_icon_rgba(size: u32) -> Vec<u8> {
    let icon = image::load_from_memory_with_format(
        include_bytes!("../../assets/icon.png"),
        image::ImageFormat::Png,
    )
    .expect("valid embedded application icon")
    .to_rgba8();
    image::imageops::resize(&icon, size, size, image::imageops::FilterType::Lanczos3).into_raw()
}

impl Default for TrayManager {
    fn default() -> Self {
        Self::new().expect("Failed to create tray manager")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn executable_icon_frames_match_runtime_branding() {
        let ico = include_bytes!("../../assets/icon.ico");
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 9, 0]);
        for (index, size) in [16, 20, 24, 32, 40, 48, 64, 128, 256]
            .into_iter()
            .enumerate()
        {
            let entry = &ico[6 + index * 16..6 + (index + 1) * 16];
            let length = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
            let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
            let frame = image::load_from_memory_with_format(
                &ico[offset..offset + length],
                image::ImageFormat::Png,
            )
            .unwrap()
            .to_rgba8();
            assert_eq!(frame.dimensions(), (size, size));
            assert_eq!(frame.get_pixel(0, 0).0[3], 0);
            assert_eq!(frame.get_pixel(size / 3, size / 2).0, [207, 161, 57, 255]);
            if size == 256 {
                assert_eq!(frame.into_raw(), super::create_app_icon_rgba(size));
            }
        }
    }
}
