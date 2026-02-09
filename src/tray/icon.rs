use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon,
};
use std::sync::OnceLock;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    ShowWindow, SetForegroundWindow,
    SW_RESTORE, SW_MINIMIZE,
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
    #[allow(dead_code)]
    tray_icon: TrayIcon,
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
                    // Use std::process::exit for immediate termination
                    // PostQuitMessage doesn't work well with eframe/winit
                    std::process::exit(0);
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
        ]).ok()?;

        // Create a simple icon (blue square with P)
        let icon = create_default_icon()?;

        // Build the tray icon
        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Pluriview - Live Window Previews")
            .with_icon(icon)
            .build()
            .ok()?;

        Some(Self { tray_icon })
    }
}

/// Create a simple default icon (32x32 blue square with P)
fn create_default_icon() -> Option<Icon> {
    let size = 32;
    let mut rgba = vec![0u8; size * size * 4];

    // Fill with a nice blue color
    for y in 0..size {
        for x in 0..size {
            let idx = (y * size + x) * 4;

            // Create a rounded square appearance
            let margin = 2;
            let is_border = x < margin || x >= size - margin || y < margin || y >= size - margin;

            if is_border {
                // Transparent border
                rgba[idx] = 0;     // R
                rgba[idx + 1] = 0; // G
                rgba[idx + 2] = 0; // B
                rgba[idx + 3] = 0; // A
            } else {
                // Blue fill with slight gradient
                let brightness = 180 + ((x + y) % 40) as u8;
                rgba[idx] = 70;           // R
                rgba[idx + 1] = 130;      // G
                rgba[idx + 2] = brightness; // B
                rgba[idx + 3] = 255;      // A (opaque)
            }
        }
    }

    // Draw a "P" in white
    let p_points = [
        // Vertical line of P
        (10, 8), (10, 9), (10, 10), (10, 11), (10, 12), (10, 13), (10, 14), (10, 15),
        (10, 16), (10, 17), (10, 18), (10, 19), (10, 20), (10, 21), (10, 22), (10, 23),
        (11, 8), (11, 22), (11, 23),
        // Top of P
        (12, 8), (13, 8), (14, 8), (15, 8), (16, 8), (17, 8), (18, 8),
        (12, 9), (13, 9), (14, 9), (15, 9), (16, 9), (17, 9), (18, 9), (19, 9),
        // Curve of P
        (19, 10), (20, 10), (20, 11), (21, 11), (21, 12), (21, 13), (21, 14),
        (20, 15), (20, 14), (19, 15), (19, 16),
        // Middle bar of P
        (12, 15), (13, 15), (14, 15), (15, 15), (16, 15), (17, 15), (18, 15),
        (12, 16), (13, 16), (14, 16), (15, 16), (16, 16), (17, 16), (18, 16),
    ];

    for (x, y) in p_points {
        if x < size && y < size {
            let idx = (y * size + x) * 4;
            rgba[idx] = 255;     // R
            rgba[idx + 1] = 255; // G
            rgba[idx + 2] = 255; // B
            rgba[idx + 3] = 255; // A
        }
    }

    Icon::from_rgba(rgba, size as u32, size as u32).ok()
}

impl Default for TrayManager {
    fn default() -> Self {
        Self::new().expect("Failed to create tray manager")
    }
}
