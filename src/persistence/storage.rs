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

    /// Directory containing portable copies of user-imported tile media.
    pub fn media_dir(&self) -> Result<PathBuf, std::io::Error> {
        let path = self.data_dir.join("media");
        fs::create_dir_all(&path)?;
        Ok(path)
    }

    /// Copy an image into managed storage and return its relative filename.
    /// Existing files are never overwritten.
    pub fn import_media(&self, source: &std::path::Path) -> Result<String, std::io::Error> {
        let media_dir = self.media_dir()?;
        let source = source.canonicalize()?;
        let original_name = source.file_name().and_then(|name| name.to_str())
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid image filename"))?;

        if source.parent().is_some_and(|parent| {
            parent.canonicalize().ok().as_deref() == media_dir.canonicalize().ok().as_deref()
        }) {
            return Ok(original_name.to_owned());
        }

        let original_path = std::path::Path::new(original_name);
        let stem = original_path.file_stem().and_then(|value| value.to_str()).unwrap_or("image");
        let extension = original_path.extension().and_then(|value| value.to_str());
        let mut suffix = 1u32;
        let destination = loop {
            let filename = if suffix == 1 {
                original_name.to_owned()
            } else if let Some(extension) = extension {
                format!("{stem}_{suffix}.{extension}")
            } else {
                format!("{stem}_{suffix}")
            };
            let candidate = media_dir.join(&filename);
            if !candidate.exists() {
                break (filename, candidate);
            }
            suffix += 1;
        };

        fs::copy(source, &destination.1)?;
        Ok(destination.0)
    }

    /// Resolve a saved managed filename without permitting path traversal.
    pub fn resolve_media(&self, filename: &str) -> Option<PathBuf> {
        let path = std::path::Path::new(filename);
        let mut components = path.components();
        let safe = matches!(components.next(), Some(std::path::Component::Normal(_)))
            && components.next().is_none();
        safe.then(|| self.data_dir.join("media").join(path))
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

#[cfg(test)]
mod tests {
    use super::Storage;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn media_imports_never_overwrite_and_saved_paths_cannot_escape() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("pluriview-storage-test-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("sample.png");
        fs::write(&source, b"test image bytes").unwrap();
        let storage = Storage { data_dir: root.clone() };

        assert_eq!(storage.import_media(&source).unwrap(), "sample.png");
        assert_eq!(storage.import_media(&source).unwrap(), "sample_2.png");
        assert!(storage.resolve_media("sample.png").is_some());
        assert!(storage.resolve_media("../autosave.json").is_none());
        assert!(storage.resolve_media("folder/sample.png").is_none());

        fs::remove_dir_all(root).unwrap();
    }
}
