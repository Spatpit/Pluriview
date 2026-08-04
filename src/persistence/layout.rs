use crate::preview::PreviewLayout;
use serde::{Deserialize, Serialize};

/// Complete saved layout
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedLayout {
    /// Version for compatibility
    pub version: u32,

    /// Canvas state
    pub canvas: CanvasLayout,

    /// All previews
    pub previews: Vec<PreviewLayout>,

    /// Recently used browser URLs, newest first (shown in the Add Browser dialog)
    #[serde(default)]
    pub recent_browser_urls: Vec<String>,

    /// Stream audio monitor target as (device id, friendly name);
    /// None = monitoring off.
    #[serde(default)]
    pub monitor_device: Option<(String, String)>,

    /// Profile-wide uBlock Origin Lite state. Defaults on for layouts saved
    /// before integrated content blocking was added.
    #[serde(default = "default_true")]
    pub adblock_enabled: bool,

    /// Window picker sidebar visibility. Defaults to shown, which is what
    /// layouts saved before it was persisted opened with.
    #[serde(default = "default_true")]
    pub picker_open: bool,

    /// Where the main window was. None for layouts saved before the window
    /// remembered its geometry; those open at the default size.
    #[serde(default)]
    pub window: Option<WindowLayout>,
}

/// Size the main window opens at before it has ever been saved.
pub const DEFAULT_WINDOW_SIZE: (f32, f32) = (1280.0, 720.0);

/// Main window geometry in logical points.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct WindowLayout {
    /// Top-left corner. None until the window reports a position.
    pub position: Option<(f32, f32)>,

    /// Inner size while *not* maximized, so unmaximizing lands back here.
    pub size: (f32, f32),

    pub maximized: bool,
}

impl Default for WindowLayout {
    fn default() -> Self {
        Self {
            position: None,
            size: DEFAULT_WINDOW_SIZE,
            maximized: false,
        }
    }
}

/// Serializable canvas state
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CanvasLayout {
    pub pan: (f32, f32),
    pub zoom: f32,
    pub show_grid: bool,
}

impl Default for CanvasLayout {
    fn default() -> Self {
        Self {
            pan: (0.0, 0.0),
            zoom: 1.0,
            show_grid: true,
        }
    }
}

impl SavedLayout {
    /// Create a new layout
    pub fn new() -> Self {
        Self {
            version: 1,
            canvas: CanvasLayout {
                pan: (0.0, 0.0),
                zoom: 1.0,
                show_grid: true,
            },
            previews: Vec::new(),
            recent_browser_urls: Vec::new(),
            monitor_device: None,
            adblock_enabled: true,
            picker_open: true,
            window: None,
        }
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::{SavedLayout, WindowLayout};

    #[test]
    fn older_layouts_default_adblocking_to_enabled() {
        let layout = SavedLayout::new();
        let mut value = serde_json::to_value(layout).unwrap();
        value.as_object_mut().unwrap().remove("adblock_enabled");

        let restored: SavedLayout = serde_json::from_value(value).unwrap();
        assert!(restored.adblock_enabled);
    }

    #[test]
    fn a_hidden_picker_survives_a_round_trip() {
        let mut layout = SavedLayout::new();
        layout.picker_open = false;
        let json = serde_json::to_string(&layout).unwrap();

        let restored: SavedLayout = serde_json::from_str(&json).unwrap();
        assert!(!restored.picker_open);
    }

    #[test]
    fn window_geometry_survives_a_round_trip() {
        let mut layout = SavedLayout::new();
        layout.window = Some(WindowLayout {
            position: Some((-1920.0, 240.0)),
            size: (1600.0, 900.0),
            maximized: true,
        });
        let json = serde_json::to_string(&layout).unwrap();

        let window = serde_json::from_str::<SavedLayout>(&json)
            .unwrap()
            .window
            .unwrap();
        assert_eq!(window.position, Some((-1920.0, 240.0)));
        assert_eq!(window.size, (1600.0, 900.0));
        assert!(window.maximized);
    }

    #[test]
    fn older_layouts_have_no_window_geometry() {
        let layout = SavedLayout::new();
        let mut value = serde_json::to_value(layout).unwrap();
        value.as_object_mut().unwrap().remove("window");

        let restored: SavedLayout = serde_json::from_value(value).unwrap();
        assert!(restored.window.is_none());
    }

    #[test]
    fn older_layouts_default_to_a_visible_picker() {
        let layout = SavedLayout::new();
        let mut value = serde_json::to_value(layout).unwrap();
        value.as_object_mut().unwrap().remove("picker_open");

        let restored: SavedLayout = serde_json::from_value(value).unwrap();
        assert!(restored.picker_open);
    }
}
