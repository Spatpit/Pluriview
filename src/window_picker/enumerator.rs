use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible, GetWindowLongW, GWL_EXSTYLE,
    WS_EX_TOOLWINDOW, WS_EX_APPWINDOW,
    GetClassNameW, GetAncestor, GA_ROOTOWNER,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

/// Information about a window
#[derive(Clone, Debug)]
pub struct WindowInfo {
    /// Window handle
    pub hwnd: isize,

    /// Window title
    pub title: String,

    /// Process ID
    pub process_id: u32,

    /// Executable name
    pub exe_name: String,

    /// Window class name (reserved for future use)
    #[allow(dead_code)]
    pub class_name: String,
}

impl WindowInfo {
    /// Get a display name (title or exe name)
    pub fn display_name(&self) -> &str {
        if self.title.is_empty() {
            &self.exe_name
        } else {
            &self.title
        }
    }
}

/// Enumerate all visible windows suitable for capture
pub fn enumerate_windows() -> Vec<WindowInfo> {
    let mut windows: Vec<WindowInfo> = Vec::new();

    unsafe {
        let _ = EnumWindows(
            Some(enum_window_callback),
            LPARAM(&mut windows as *mut _ as isize),
        );
    }

    // Sort by title
    windows.sort_by(|a, b| a.display_name().to_lowercase().cmp(&b.display_name().to_lowercase()));

    windows
}

/// Callback for EnumWindows
unsafe extern "system" fn enum_window_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let windows = &mut *(lparam.0 as *mut Vec<WindowInfo>);

    // Skip invisible windows
    if !IsWindowVisible(hwnd).as_bool() {
        return BOOL(1);
    }

    // Get extended window style
    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;

    // Skip tool windows unless they have app window style
    if (ex_style & WS_EX_TOOLWINDOW.0) != 0 && (ex_style & WS_EX_APPWINDOW.0) == 0 {
        return BOOL(1);
    }

    // Note: Removed WS_CAPTION check - it was too strict and excluded video players
    // (VLC, MPV, PotPlayer) that use borderless/custom windows

    // Skip windows that aren't root owners
    let root_owner = GetAncestor(hwnd, GA_ROOTOWNER);
    if root_owner != hwnd {
        return BOOL(1);
    }

    // Get window title
    let title_len = GetWindowTextLengthW(hwnd);
    if title_len == 0 {
        return BOOL(1); // Skip windows without titles
    }

    let mut title_buffer: Vec<u16> = vec![0; (title_len + 1) as usize];
    let actual_len = GetWindowTextW(hwnd, &mut title_buffer);
    let title = if actual_len > 0 {
        OsString::from_wide(&title_buffer[..actual_len as usize])
            .to_string_lossy()
            .to_string()
    } else {
        String::new()
    };

    // Skip empty titles
    if title.is_empty() {
        return BOOL(1);
    }

    // Get process ID
    let mut process_id: u32 = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut process_id));

    // Get executable name
    let exe_name = get_process_name(process_id);

    // Get class name
    let mut class_buffer: Vec<u16> = vec![0; 256];
    let class_len = GetClassNameW(hwnd, &mut class_buffer);
    let class_name = if class_len > 0 {
        OsString::from_wide(&class_buffer[..class_len as usize])
            .to_string_lossy()
            .to_string()
    } else {
        String::new()
    };

    // Skip certain system classes
    let skip_classes = [
        "Progman",            // Program Manager
        "WorkerW",            // Desktop background
        "Shell_TrayWnd",      // Taskbar
        "Windows.UI.Core.CoreWindow", // Some UWP overlay windows
    ];

    if skip_classes.iter().any(|&c| class_name == c) {
        return BOOL(1);
    }

    // Skip our own window
    if title.contains("Pluriview") {
        return BOOL(1);
    }

    windows.push(WindowInfo {
        hwnd: hwnd.0 as isize,
        title,
        process_id,
        exe_name,
        class_name,
    });

    BOOL(1) // Continue enumeration
}

/// Get the process name from a process ID
fn get_process_name(process_id: u32) -> String {
    unsafe {
        let handle = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) {
            Ok(h) => h,
            Err(_) => return String::from("Unknown"),
        };

        let mut buffer: Vec<u16> = vec![0; 260];
        let mut size = buffer.len() as u32;

        if QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, windows::core::PWSTR(buffer.as_mut_ptr()), &mut size).is_ok() {
            let path = OsString::from_wide(&buffer[..size as usize])
                .to_string_lossy()
                .to_string();

            // Extract just the filename
            path.rsplit('\\').next().unwrap_or(&path).to_string()
        } else {
            String::from("Unknown")
        }
    }
}
