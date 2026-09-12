use crate::window_controls::WindowAction;
use std::sync::{mpsc, OnceLock};
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
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
const MENU_ALWAYS_ON_TOP: &str = "always_on_top";
const MENU_CLICK_THROUGH: &str = "click_through";

/// Global storage for the main window HWND (needed for static closure)
static MAIN_WINDOW_HWND: OnceLock<isize> = OnceLock::new();

/// Manages the system tray icon and menu
pub struct TrayManager {
    /// The tray icon (must be kept alive)
    _tray_icon: TrayIcon,
    always_on_top: CheckMenuItem,
    click_through: CheckMenuItem,
    actions: mpsc::Receiver<WindowAction>,
}

impl TrayManager {
    /// Set the main window HWND (call this after window is created)
    pub fn set_window_hwnd(hwnd: isize) {
        let _ = MAIN_WINDOW_HWND.set(hwnd);
    }

    /// Create a new tray manager with icon and menu
    pub fn new(ctx: eframe::egui::Context, always_on_top: bool) -> Option<Self> {
        let (sender, actions) = mpsc::channel();
        // Keep Show/Hide/Quit native so they work while painting is stopped.
        // Other actions wake the app and share its window/browser state.
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = match event.id.0.as_str() {
                MENU_ALWAYS_ON_TOP => Some(WindowAction::ToggleAlwaysOnTop),
                MENU_CLICK_THROUGH => Some(WindowAction::ToggleClickThrough),
                MENU_SHOW => Some(WindowAction::Show),
                _ => None,
            };
            if let Some(action) = action {
                let _ = sender.send(action);
                // Wake even an idle or click-through canvas. Window state and
                // browser parking are changed together on the app thread.
                ctx.request_repaint();
            }
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
        let always_on_top = CheckMenuItem::with_id(
            MENU_ALWAYS_ON_TOP,
            "Always on Top",
            true,
            always_on_top,
            None,
        );
        let click_through =
            CheckMenuItem::with_id(MENU_CLICK_THROUGH, "Click Through", true, false, None);

        // Build the menu
        let menu = Menu::with_items(&[
            &show_item,
            &hide_item,
            &PredefinedMenuItem::separator(),
            &always_on_top,
            &click_through,
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
            always_on_top,
            click_through,
            actions,
        })
    }

    pub(crate) fn next_action(&self) -> Option<WindowAction> {
        self.actions.try_recv().ok()
    }

    pub(crate) fn sync_window_controls(&self, always_on_top: bool, click_through: bool) {
        self.always_on_top.set_checked(always_on_top);
        self.click_through.set_checked(click_through);
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
