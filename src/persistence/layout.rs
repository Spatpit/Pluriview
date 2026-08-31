use crate::preview::PreviewLayout;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

/// Screen-space canvas background that ignores pan and zoom.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WallpaperLayout {
    /// Original image or GIF path. Relative paths are retained for backwards
    /// compatibility with the former managed-media directory.
    Image { path: PathBuf },
    /// Original local video path, matching how video tiles are stored.
    Video { path: PathBuf },
}

/// Serializable canvas state
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CanvasLayout {
    pub pan: (f32, f32),
    pub zoom: f32,
    pub show_grid: bool,
    /// Optional live wallpaper drawn behind tiles in screen space.
    #[serde(default)]
    pub wallpaper: Option<WallpaperLayout>,
    /// Named camera positions within this workspace. Centers are canvas-space
    /// coordinates, independent of window and sidebar dimensions.
    #[serde(default)]
    pub views: Vec<CanvasView>,
    /// Last selected view. It remains selected when the camera moves away so
    /// the UI can indicate that the view is modified.
    #[serde(default)]
    pub active_view: Option<usize>,
}

/// A named, workspace-local camera position on the infinite canvas.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CanvasView {
    pub name: String,
    pub center: (f32, f32),
    pub zoom: f32,
}

impl Default for CanvasLayout {
    fn default() -> Self {
        Self {
            pan: (0.0, 0.0),
            zoom: 1.0,
            show_grid: true,
            wallpaper: None,
            views: Vec::new(),
            active_view: None,
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
                wallpaper: None,
                views: Vec::new(),
                active_view: None,
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
    use super::{CanvasView, SavedLayout, WindowLayout};
    use std::path::PathBuf;

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

    #[test]
    fn wallpaper_survives_a_round_trip() {
        let mut layout = SavedLayout::new();
        layout.canvas.wallpaper = Some(super::WallpaperLayout::Image {
            path: PathBuf::from("bg.gif"),
        });
        let json = serde_json::to_string(&layout).unwrap();

        let restored: SavedLayout = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored.canvas.wallpaper,
            Some(super::WallpaperLayout::Image {
                path: PathBuf::from("bg.gif"),
            })
        );
    }

    #[test]
    fn older_layouts_have_no_wallpaper() {
        let layout = SavedLayout::new();
        let mut value = serde_json::to_value(layout).unwrap();
        value
            .get_mut("canvas")
            .and_then(|canvas| canvas.as_object_mut())
            .unwrap()
            .remove("wallpaper");

        let restored: SavedLayout = serde_json::from_value(value).unwrap();
        assert!(restored.canvas.wallpaper.is_none());
    }

    #[test]
    fn canvas_views_survive_a_round_trip() {
        let mut layout = SavedLayout::new();
        layout.canvas.views.push(CanvasView {
            name: "Streams".to_owned(),
            center: (1250.0, -320.0),
            zoom: 0.75,
        });
        layout.canvas.active_view = Some(0);

        let json = serde_json::to_string(&layout).unwrap();
        let restored: SavedLayout = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.canvas.views, layout.canvas.views);
        assert_eq!(restored.canvas.active_view, Some(0));
    }

    #[test]
    fn older_layouts_have_no_canvas_views() {
        let layout = SavedLayout::new();
        let mut value = serde_json::to_value(layout).unwrap();
        value
            .get_mut("canvas")
            .and_then(|canvas| canvas.as_object_mut())
            .unwrap()
            .remove("views");

        let restored: SavedLayout = serde_json::from_value(value).unwrap();
        assert!(restored.canvas.views.is_empty());
    }

    #[test]
    fn older_layouts_have_no_active_canvas_view() {
        let layout = SavedLayout::new();
        let mut value = serde_json::to_value(layout).unwrap();
        value
            .get_mut("canvas")
            .and_then(|canvas| canvas.as_object_mut())
            .unwrap()
            .remove("active_view");

        let restored: SavedLayout = serde_json::from_value(value).unwrap();
        assert!(restored.canvas.active_view.is_none());
    }
}
