use serde::{Serialize, Deserialize};
use crate::preview::PreviewLayout;

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

/// Get current timestamp as string
fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();

    format!("{}", duration.as_secs())
}
