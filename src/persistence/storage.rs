use super::workspace::is_valid_workspace_id;
use super::{AppConfig, SavedLayout, WorkspaceIndex};
use std::fs;
use std::path::PathBuf;

/// File storage for layouts and config
pub struct Storage {
    /// Data directory path
    data_dir: PathBuf,
}

impl Storage {
    /// Create a new storage instance
    pub fn new() -> Option<Self> {
        // Development builds live under target/, which Cargo may erase at any
        // time. Keep test workspaces and imported media in the repository's
        // own ignored data folder instead.
        #[cfg(debug_assertions)]
        {
            let development_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("pluriview_data");
            if fs::create_dir_all(&development_dir).is_ok() {
                return Some(Self {
                    data_dir: development_dir,
                });
            }
        }

        // Try portable mode first (next to executable)
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let portable_dir = exe_dir.join("pluriview_data");

                // If portable directory exists or we can create it
                if portable_dir.exists() || fs::create_dir_all(&portable_dir).is_ok() {
                    return Some(Self {
                        data_dir: portable_dir,
                    });
                }
            }
        }

        // Fallback to standard app data directory
        directories::ProjectDirs::from("com", "pluriview", "Pluriview").map(|dirs| {
            let data_dir = dirs.data_dir().to_path_buf();
            let _ = fs::create_dir_all(&data_dir);
            Self { data_dir }
        })
    }

    /// Get auto-save path
    pub fn autosave_path(&self) -> PathBuf {
        self.data_dir.join("autosave.json")
    }

    /// App-global settings path (independent of the active workspace).
    pub fn config_path(&self) -> PathBuf {
        self.data_dir.join("config.json")
    }

    /// Save app-global settings.
    pub fn save_config(&self, config: &AppConfig) -> Result<(), std::io::Error> {
        let json = serde_json::to_string_pretty(config)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        fs::write(self.config_path(), json)
    }

    /// Load app-global settings. A first run without config.json uses defaults.
    pub fn load_config(&self) -> Result<AppConfig, Box<dyn std::error::Error>> {
        let path = self.config_path();
        if !path.exists() {
            let config = AppConfig::default();
            self.save_config(&config)?;
            return Ok(config);
        }
        let json = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&json)?)
    }

    fn workspace_index_path(&self) -> PathBuf {
        self.data_dir.join("workspaces.json")
    }

    fn workspaces_dir(&self) -> Result<PathBuf, std::io::Error> {
        let path = self.data_dir.join("workspaces");
        fs::create_dir_all(&path)?;
        Ok(path)
    }

    fn workspace_path(&self, id: &str) -> Result<PathBuf, std::io::Error> {
        if !is_valid_workspace_id(id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid workspace identifier",
            ));
        }
        Ok(self.workspaces_dir()?.join(format!("{id}.json")))
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
        let original_name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid image filename")
            })?;

        if source.parent().is_some_and(|parent| {
            parent.canonicalize().ok().as_deref() == media_dir.canonicalize().ok().as_deref()
        }) {
            return Ok(original_name.to_owned());
        }

        let original_path = std::path::Path::new(original_name);
        let stem = original_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("image");
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

    /// Load the workspace catalog. The first v0.5 launch copies the legacy
    /// autosave into the Default workspace without removing the old file.
    pub fn load_or_initialize_workspaces(
        &self,
    ) -> Result<WorkspaceIndex, Box<dyn std::error::Error>> {
        let index_path = self.workspace_index_path();
        if index_path.exists() {
            let json = fs::read_to_string(index_path)?;
            let mut index: WorkspaceIndex = serde_json::from_str(&json)?;
            index.repair();
            self.save_workspace_index(&index)?;
            return Ok(index);
        }

        let index = WorkspaceIndex::default();
        if self.autosave_path().exists() {
            let legacy_layout = self.load_autosave()?;
            self.save_workspace(&index.active_workspace_id, &legacy_layout)?;
        }
        self.save_workspace_index(&index)?;
        Ok(index)
    }

    pub fn save_workspace_index(&self, index: &WorkspaceIndex) -> Result<(), std::io::Error> {
        let json = serde_json::to_string_pretty(index)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        fs::write(self.workspace_index_path(), json)
    }

    pub fn save_workspace(&self, id: &str, layout: &SavedLayout) -> Result<(), std::io::Error> {
        let json = serde_json::to_string_pretty(layout)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        fs::write(self.workspace_path(id)?, json)
    }

    pub fn load_workspace(&self, id: &str) -> Result<SavedLayout, Box<dyn std::error::Error>> {
        let json = fs::read_to_string(self.workspace_path(id)?)?;
        Ok(serde_json::from_str(&json)?)
    }

    /// Save the active workspace and keep autosave.json as a downgrade-safe
    /// mirror for older Pluriview versions.
    pub fn save_active_workspace(
        &self,
        index: &WorkspaceIndex,
        layout: &SavedLayout,
    ) -> Result<(), std::io::Error> {
        self.save_workspace(&index.active_workspace_id, layout)?;
        self.save_workspace_index(index)?;
        self.save_autosave(layout)
    }

    pub fn delete_workspace(&self, id: &str) -> Result<(), std::io::Error> {
        let path = self.workspace_path(id)?;
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
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
    use crate::persistence::{AppConfig, SavedLayout, WorkspaceIndex};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn media_imports_never_overwrite_and_saved_paths_cannot_escape() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pluriview-storage-test-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("sample.png");
        fs::write(&source, b"test image bytes").unwrap();
        let storage = Storage {
            data_dir: root.clone(),
        };

        assert_eq!(storage.import_media(&source).unwrap(), "sample.png");
        assert_eq!(storage.import_media(&source).unwrap(), "sample_2.png");
        assert!(storage.resolve_media("sample.png").is_some());
        assert!(storage.resolve_media("../autosave.json").is_none());
        assert!(storage.resolve_media("folder/sample.png").is_none());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn app_config_is_stored_at_the_data_root() {
        let root = temp_root("app-config");
        let storage = Storage {
            data_dir: root.clone(),
        };
        let defaults = storage.load_config().unwrap();
        assert!(defaults.external_tools.mpv_path.is_none());
        assert!(storage.config_path().exists());

        let mut config = AppConfig::default();
        config.external_tools.mpv_path = Some(std::path::PathBuf::from(r"C:\Tools\mpv.exe"));

        storage.save_config(&config).unwrap();
        let restored = storage.load_config().unwrap();

        assert_eq!(storage.config_path(), root.join("config.json"));
        assert_eq!(
            restored.external_tools.mpv_path,
            config.external_tools.mpv_path
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_autosave_is_copied_into_the_default_workspace() {
        let root = temp_root("workspace-migration");
        let storage = Storage {
            data_dir: root.clone(),
        };
        let mut legacy = SavedLayout::new();
        legacy.canvas.zoom = 1.75;
        storage.save_autosave(&legacy).unwrap();

        let index = storage.load_or_initialize_workspaces().unwrap();
        let migrated = storage.load_workspace(&index.active_workspace_id).unwrap();

        assert_eq!(index.active().unwrap().name, "Default");
        assert_eq!(migrated.canvas.zoom, 1.75);
        assert!(storage.autosave_path().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_names_do_not_affect_layout_paths() {
        let root = temp_root("workspace-paths");
        let storage = Storage {
            data_dir: root.clone(),
        };
        let mut index = WorkspaceIndex::default();
        let id = index.add("../../Research: Q3".to_owned());
        storage.save_workspace(&id, &SavedLayout::new()).unwrap();

        assert!(root.join("workspaces").join(format!("{id}.json")).exists());
        assert!(storage
            .save_workspace("../outside", &SavedLayout::new())
            .is_err());
        fs::remove_dir_all(root).unwrap();
    }

    fn temp_root(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("pluriview-{label}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        root
    }
}
