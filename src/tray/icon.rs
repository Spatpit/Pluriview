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

/// Create the leaf icon (32x32 green leaf)
fn create_default_icon() -> Option<Icon> {
    Some(create_leaf_icon(32))
}

/// Create leaf icon at specified size - used for both tray and window icon
pub fn create_leaf_icon(size: u32) -> Icon {
    let size = size as usize;
    let mut rgba = vec![0u8; size * size * 4];

    let cx = size as f64 / 2.0;
    let cy = size as f64 / 2.0;
    let margin = size as f64 / 10.0;
    let leaf_length = size as f64 - 2.0 * margin;
    let leaf_width = leaf_length * 0.55;

    // Rotation angle (35 degrees)
    let angle = 35.0_f64.to_radians();
    let cos_a = angle.cos();
    let sin_a = angle.sin();

    // Colors
    let base_color = (107u8, 170u8, 75u8);  // Main green
    let dark_color = (70u8, 125u8, 50u8);   // Vein color

    // Draw each pixel
    for y in 0..size {
        for x in 0..size {
            let idx = (y * size + x) * 4;

            // Transform point back to unrotated space
            let px = x as f64;
            let py = y as f64;

            // Inverse rotation
            let ux = cos_a * (px - cx) + sin_a * (py - cy) + cx;
            let uy = -sin_a * (px - cx) + cos_a * (py - cy) + cy;

            // Check if point is inside leaf shape
            let t = (uy - margin) / leaf_length;

            if t >= 0.0 && t <= 1.0 {
                // Leaf width at this height
                let width_factor = (t * std::f64::consts::PI).sin().powf(0.8);
                let half_width = (leaf_width / 2.0) * width_factor;

                let dist_from_center = (ux - cx).abs();

                if dist_from_center <= half_width {
                    // Inside leaf - check if on vein
                    let on_main_vein = dist_from_center < size as f64 / 18.0 && t > 0.08 && t < 0.92;

                    // Check side veins
                    let mut on_side_vein = false;
                    for i in 1..=5 {
                        let vein_t = 0.15 + (i as f64 / 6.0) * 0.7;
                        let vein_y = margin + vein_t * leaf_length;
                        let y_dist = (uy - vein_y).abs();

                        if y_dist < size as f64 / 20.0 {
                            let vein_width_factor = (vein_t * std::f64::consts::PI).sin().powf(0.8);
                            let vein_len = (leaf_width / 2.0) * vein_width_factor * 0.75;

                            // Side veins go diagonally upward
                            let expected_x_offset = (ux - cx).abs();
                            let expected_y_offset = expected_x_offset * 0.35;
                            let actual_y = vein_y - expected_y_offset;

                            if (uy - actual_y).abs() < size as f64 / 25.0 && expected_x_offset < vein_len {
                                on_side_vein = true;
                                break;
                            }
                        }
                    }

                    let color = if on_main_vein || on_side_vein {
                        dark_color
                    } else {
                        base_color
                    };

                    rgba[idx] = color.0;     // R
                    rgba[idx + 1] = color.1; // G
                    rgba[idx + 2] = color.2; // B
                    rgba[idx + 3] = 255;     // A
                }
            }
        }
    }

    Icon::from_rgba(rgba, size as u32, size as u32).unwrap()
}

impl Default for TrayManager {
    fn default() -> Self {
        Self::new().expect("Failed to create tray manager")
    }
}
