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

    /// Get the layouts directory
    #[allow(dead_code)]
    fn layouts_dir(&self) -> PathBuf {
        let dir = self.data_dir.join("layouts");
        let _ = fs::create_dir_all(&dir);
        dir
    }

    /// Save a layout
    #[allow(dead_code)]
    pub fn save_layout(&self, layout: &SavedLayout) -> Result<(), std::io::Error> {
        let path = self.layouts_dir().join(format!("{}.json", sanitize_filename(&layout.name)));
        let json = serde_json::to_string_pretty(layout)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(path, json)
    }

    /// Load a layout by name
    #[allow(dead_code)]
    pub fn load_layout(&self, name: &str) -> Result<SavedLayout, Box<dyn std::error::Error>> {
        let path = self.layouts_dir().join(format!("{}.json", sanitize_filename(name)));
        let json = fs::read_to_string(path)?;
        let layout: SavedLayout = serde_json::from_str(&json)?;
        Ok(layout)
    }

    /// List all saved layouts
    #[allow(dead_code)]
    pub fn list_layouts(&self) -> Vec<String> {
        fs::read_dir(self.layouts_dir())
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let path = e.path();
                        if path.extension()?.to_str()? == "json" {
                            path.file_stem()?.to_str().map(String::from)
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Delete a layout
    #[allow(dead_code)]
    pub fn delete_layout(&self, name: &str) -> Result<(), std::io::Error> {
        let path = self.layouts_dir().join(format!("{}.json", sanitize_filename(name)));
        fs::remove_file(path)
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

/// Sanitize a filename to be safe for the filesystem
#[allow(dead_code)]
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
