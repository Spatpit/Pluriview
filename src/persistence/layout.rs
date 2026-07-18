use crate::preview::PreviewLayout;
use serde::{Deserialize, Serialize};

/// Complete saved layout
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedLayout {
    /// Version for compatibility
    pub version: u32,

    /// Layout name
    pub name: String,

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

    /// Creation timestamp
    pub created_at: String,

    /// Last modified timestamp
    pub modified_at: String,
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
    pub fn new(name: String) -> Self {
        let now = chrono_now();
        Self {
            version: 1,
            name,
            canvas: CanvasLayout {
                pan: (0.0, 0.0),
                zoom: 1.0,
                show_grid: true,
            },
            previews: Vec::new(),
            recent_browser_urls: Vec::new(),
            monitor_device: None,
            adblock_enabled: true,
            created_at: now.clone(),
            modified_at: now,
        }
    }

    /// Update modification time
    #[allow(dead_code)]
    pub fn touch(&mut self) {
        self.modified_at = chrono_now();
    }
}

fn default_true() -> bool {
    true
}

/// Get current timestamp as string
fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();

    format!("{}", duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::SavedLayout;

    #[test]
    fn older_layouts_default_adblocking_to_enabled() {
        let layout = SavedLayout::new("test".to_owned());
        let mut value = serde_json::to_value(layout).unwrap();
        value.as_object_mut().unwrap().remove("adblock_enabled");

        let restored: SavedLayout = serde_json::from_value(value).unwrap();
        assert!(restored.adblock_enabled);
    }
}
