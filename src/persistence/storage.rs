use std::path::PathBuf;
use std::fs;
use super::SavedLayout;

/// File storage for layouts and config
pub struct Storage {
    /// Data directory path
    data_dir: PathBuf,
}

impl Storage {
    /// Create a new storage instance
    pub fn new() -> Option<Self> {
        // Try portable mode first (next to executable)
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let portable_dir = exe_dir.join("pluriview_data");

                // If portable directory exists or we can create it
                if portable_dir.exists() || fs::create_dir_all(&portable_dir).is_ok() {
                    return Some(Self { data_dir: portable_dir });
                }
            }
        }

        // Fallback to standard app data directory
        directories::ProjectDirs::from("com", "pluriview", "Pluriview")
            .map(|dirs| {
                let data_dir = dirs.data_dir().to_path_buf();
                let _ = fs::create_dir_all(&data_dir);
                Self { data_dir }
            })
    }

    /// Get auto-save path
    pub fn autosave_path(&self) -> PathBuf {
        self.data_dir.join("autosave.json")
    }

    /// Save autosave
    pub fn save_autosave(&self, layout: &SavedLayout) -> Result<(), std::io::Error> {
        let json = serde_json::to_string_pretty(layout)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(self.autosave_path(), json)
    }

    /// Load autosave
    pub fn load_autosave(&self) -> Result<SavedLayout, Box<dyn std::error::Error>> {
        let json = fs::read_to_string(self.autosave_path())?;
        let layout: SavedLayout = serde_json::from_str(&json)?;
        Ok(layout)
    }
}

impl Default for Storage {
    fn default() -> Self {
        Self::new().expect("Failed to initialize storage")
    }
}
