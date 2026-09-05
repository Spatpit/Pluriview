use super::workspace::is_valid_workspace_id;
use super::{AppConfig, SavedLayout, WorkspaceIndex};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;

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
        write_json_safely(&self.config_path(), config)
    }

    /// Load app-global settings. A first run without config.json uses defaults.
    pub fn load_config(&self) -> Result<AppConfig, Box<dyn std::error::Error>> {
        let path = self.config_path();
        if !path.exists() && !backup_path(&path).exists() {
            let config = AppConfig::default();
            self.save_config(&config)?;
            return Ok(config);
        }
        load_json_with_backup(&path)
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

    /// Resolve a legacy managed filename without permitting path traversal.
    /// New image tiles retain their original absolute path instead.
    pub fn resolve_media(&self, filename: &str) -> Option<PathBuf> {
        let path = std::path::Path::new(filename);
        let mut components = path.components();
        let safe = matches!(components.next(), Some(std::path::Component::Normal(_)))
            && components.next().is_none();
        safe.then(|| self.data_dir.join("media").join(path))
    }

    /// Save autosave
    pub fn save_autosave(&self, layout: &SavedLayout) -> Result<(), std::io::Error> {
        write_json_safely(&self.autosave_path(), layout)
    }

    /// Load autosave
    pub fn load_autosave(&self) -> Result<SavedLayout, Box<dyn std::error::Error>> {
        load_json_with_backup(&self.autosave_path())
    }

    /// Load the workspace catalog. The first v0.5 launch copies the legacy
    /// autosave into the Default workspace without removing the old file.
    pub fn load_or_initialize_workspaces(
        &self,
    ) -> Result<WorkspaceIndex, Box<dyn std::error::Error>> {
        let index_path = self.workspace_index_path();
        if index_path.exists() || backup_path(&index_path).exists() {
            let mut index: WorkspaceIndex = match load_json_with_backup(&index_path) {
                Ok(index) => index,
                Err(index_error) => {
                    if is_unsupported_document(index_error.as_ref()) {
                        return Err(index_error);
                    }
                    if let Some(index) = self.rebuild_workspace_index()? {
                        self.save_workspace_index(&index)?;
                        self.save_autosave_for_active_workspace(&index)?;
                        return Ok(index);
                    }
                    return Err(index_error);
                }
            };
            let before_repair = index.clone();
            index.repair();
            let recovered = self.recover_orphaned_workspaces(&mut index)?;
            if index != before_repair || recovered {
                self.save_workspace_index(&index)?;
                self.save_autosave_for_active_workspace(&index)?;
            }
            return Ok(index);
        }

        if let Some(index) = self.rebuild_workspace_index()? {
            self.save_workspace_index(&index)?;
            self.save_autosave_for_active_workspace(&index)?;
            return Ok(index);
        }

        let index = WorkspaceIndex::default();
        let layout = if self.autosave_path().exists() || backup_path(&self.autosave_path()).exists()
        {
            self.load_autosave()?
        } else {
            SavedLayout::new()
        };
        self.save_workspace(&index.active_workspace_id, &layout)?;
        self.save_workspace_index(&index)?;
        Ok(index)
    }

    pub fn save_workspace_index(&self, index: &WorkspaceIndex) -> Result<(), std::io::Error> {
        write_json_safely(&self.workspace_index_path(), index)
    }

    pub fn save_workspace(&self, id: &str, layout: &SavedLayout) -> Result<(), std::io::Error> {
        write_json_safely(&self.workspace_path(id)?, layout)
    }

    pub fn load_workspace(&self, id: &str) -> Result<SavedLayout, Box<dyn std::error::Error>> {
        load_json_with_backup(&self.workspace_path(id)?)
    }

    /// Load the immediately preceding valid save without replacing the current one.
    pub fn load_workspace_backup(
        &self,
        id: &str,
    ) -> Result<SavedLayout, Box<dyn std::error::Error>> {
        load_json(&backup_path(&self.workspace_path(id)?))
    }

    pub fn workspace_backup_exists(&self, id: &str) -> bool {
        self.workspace_path(id)
            .ok()
            .map(|path| backup_path(&path).is_file())
            .unwrap_or(false)
    }

    /// Save layout content without rewriting the workspace catalog.
    pub fn save_active_layout(
        &self,
        index: &WorkspaceIndex,
        layout: &SavedLayout,
    ) -> Result<(), std::io::Error> {
        self.save_workspace(&index.active_workspace_id, layout)?;
        self.save_autosave(layout)
    }

    /// Save the active workspace and keep autosave.json as a downgrade-safe
    /// mirror for older Pluriview versions.
    pub fn save_active_workspace(
        &self,
        index: &WorkspaceIndex,
        layout: &SavedLayout,
    ) -> Result<(), std::io::Error> {
        self.save_workspace(&index.active_workspace_id, layout)?;
        self.save_autosave(layout)?;
        // Write the catalog last: every workspace it references is already durable.
        self.save_workspace_index(index)
    }

    pub fn delete_workspace(&self, id: &str) -> Result<(), std::io::Error> {
        let path = self.workspace_path(id)?;
        for candidate in [&path, &backup_path(&path), &temporary_path(&path)] {
            match fs::remove_file(candidate) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn save_autosave_for_active_workspace(
        &self,
        index: &WorkspaceIndex,
    ) -> Result<(), std::io::Error> {
        match self.load_workspace(&index.active_workspace_id) {
            Ok(layout) => self.save_autosave(&layout),
            Err(error) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Could not load recovered active workspace: {error}"),
            )),
        }
    }

    fn rebuild_workspace_index(&self) -> Result<Option<WorkspaceIndex>, std::io::Error> {
        let recovered = self.valid_workspace_files()?;
        if recovered.is_empty() {
            for entry in fs::read_dir(self.workspaces_dir()?)? {
                if workspace_file_id(&entry?.path()).is_some() {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData,
                        "Workspace files exist, but none could be recovered. They have been preserved."));
                }
            }
            return Ok(None);
        }
        let active = recovered
            .iter()
            .filter(|workspace| workspace.preview_count > 0)
            .max_by_key(|workspace| workspace.modified)
            .or_else(|| recovered.iter().max_by_key(|workspace| workspace.modified))
            .map(|workspace| workspace.id.clone())
            .expect("non-empty recovered workspace list");
        Ok(Some(WorkspaceIndex::from_recovered(
            recovered
                .into_iter()
                .map(|workspace| workspace.id)
                .collect(),
            active,
        )))
    }

    fn recover_orphaned_workspaces(
        &self,
        index: &mut WorkspaceIndex,
    ) -> Result<bool, std::io::Error> {
        let recovered = self.valid_workspace_files()?;
        let mut newest_added = None;
        let mut added_any = false;
        for workspace in recovered {
            if index.add_recovered(workspace.id.clone()) {
                added_any = true;
                if workspace.preview_count > 0
                    && newest_added
                        .as_ref()
                        .is_none_or(|current: &RecoveredWorkspace| {
                            workspace.modified > current.modified
                        })
                {
                    newest_added = Some(workspace);
                }
            }
        }

        if added_any {
            let active_is_blank = self
                .load_workspace(&index.active_workspace_id)
                .map(|layout| layout.previews.is_empty())
                .unwrap_or(true);
            if active_is_blank {
                if let Some(workspace) = newest_added {
                    index.active_workspace_id = workspace.id;
                }
            }
            index.repair();
        }
        Ok(added_any)
    }

    fn valid_workspace_files(&self) -> Result<Vec<RecoveredWorkspace>, std::io::Error> {
        let mut recovered = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for entry in fs::read_dir(self.workspaces_dir()?)? {
            let entry = entry?;
            let path = entry.path();
            let Some(id) = workspace_file_id(&path) else {
                continue;
            };
            if !seen.insert(id.to_owned()) {
                continue;
            }
            let Ok(layout) = self.load_workspace(id) else {
                continue;
            };
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            recovered.push(RecoveredWorkspace {
                id: id.to_owned(),
                modified,
                preview_count: layout.previews.len(),
            });
        }
        recovered.sort_by_key(|workspace| workspace.id.clone());
        Ok(recovered)
    }
}

fn workspace_file_id(path: &Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    let id = name
        .strip_suffix(".json")
        .or_else(|| name.strip_suffix(".json.bak"))?;
    is_valid_workspace_id(id).then_some(id)
}

struct RecoveredWorkspace {
    id: String,
    modified: SystemTime,
    preview_count: usize,
}

trait StoredDocument: Serialize + DeserializeOwned {
    fn version(&self) -> u32;

    fn validate_version(&self) -> Result<(), std::io::Error> {
        if self.version() != 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!(
                    "Document version {} is not supported by this Pluriview version",
                    self.version()
                ),
            ));
        }
        Ok(())
    }
}

impl StoredDocument for SavedLayout {
    fn version(&self) -> u32 {
        self.version
    }
}

impl StoredDocument for WorkspaceIndex {
    fn version(&self) -> u32 {
        self.version
    }
}

impl StoredDocument for AppConfig {
    fn version(&self) -> u32 {
        self.version
    }
}

fn is_unsupported_document(error: &(dyn std::error::Error + 'static)) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() == std::io::ErrorKind::Unsupported)
}

fn load_json_with_backup<T: StoredDocument>(path: &Path) -> Result<T, Box<dyn std::error::Error>> {
    match load_json(path) {
        Ok(value) => Ok(value),
        // Opening an older backup of a newer document would allow a downgrade
        // to silently overwrite fields that this executable does not understand.
        Err(error) if is_unsupported_document(error.as_ref()) => Err(error),
        Err(primary_error) => match load_json(&backup_path(path)) {
            Ok(value) => Ok(value),
            Err(backup_error)
                if primary_error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                    && backup_error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                Err(primary_error)
            }
            Err(backup_error) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{} could not be loaded ({primary_error}); its backup also failed ({backup_error})",
                    path.display()
                ),
            )
            .into()),
        },
    }
}

fn load_json<T: StoredDocument>(path: &Path) -> Result<T, Box<dyn std::error::Error>> {
    decode_document(&fs::read(path)?)
}

fn decode_document<T: StoredDocument>(bytes: &[u8]) -> Result<T, Box<dyn std::error::Error>> {
    let json: serde_json::Value = serde_json::from_slice(bytes)?;
    // Detect a newer schema even when its fields no longer deserialize into T.
    if let Some(version) = json.get("version").and_then(serde_json::Value::as_u64) {
        if version != 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!("Document version {version} is not supported by this Pluriview version"),
            )
            .into());
        }
    }
    let document: T = serde_json::from_value(json)?;
    document.validate_version()?;
    Ok(document)
}

/// Write a complete sibling file first, then rotate the previous valid file to
/// `.bak`. A power loss at any point leaves either the primary or backup intact.
fn write_json_safely<T: StoredDocument>(path: &Path, value: &T) -> Result<(), std::io::Error> {
    value.validate_version()?;
    // Read before touching any file. A read failure must not authorize removal
    // of an unreadable primary, and only a usable document may replace .bak.
    let primary_is_valid = match fs::read(path) {
        Ok(bytes) => match decode_document::<T>(&bytes) {
            Ok(_) => true,
            Err(error) if is_unsupported_document(error.as_ref()) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    error.to_string(),
                ))
            }
            Err(_) => false,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    let json = serde_json::to_vec_pretty(value)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let temporary = temporary_path(path);
    let backup = backup_path(path);
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(&json)?;
    file.sync_all()?;
    drop(file);

    if path.exists() {
        if primary_is_valid {
            remove_file_if_exists(&backup)?;
            fs::rename(path, &backup)?;
        } else {
            // Never replace a known-good backup with an unusable primary.
            remove_file_if_exists(path)?;
        }
    }
    if let Err(error) = fs::rename(&temporary, path) {
        if !path.exists() && backup.exists() {
            let _ = fs::rename(&backup, path);
        }
        return Err(error);
    }
    Ok(())
}

fn backup_path(path: &Path) -> PathBuf {
    path_with_suffix(path, ".bak")
}

fn temporary_path(path: &Path) -> PathBuf {
    path_with_suffix(path, ".tmp")
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn remove_file_if_exists(path: &Path) -> Result<(), std::io::Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
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
    use crate::persistence::{AppConfig, SavedLayout, WorkspaceIndex, WorkspaceSaveState};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn legacy_managed_paths_cannot_escape() {
        let root = std::env::temp_dir().join("pluriview-storage-path-test");
        let storage = Storage {
            data_dir: root.clone(),
        };

        assert_eq!(
            storage.resolve_media("sample.png"),
            Some(root.join("media").join("sample.png"))
        );
        assert!(storage.resolve_media("../autosave.json").is_none());
        assert!(storage.resolve_media("folder/sample.png").is_none());
    }

    #[test]
    fn app_config_is_stored_at_the_data_root() {
        let root = temp_root("app-config");
        let storage = Storage {
            data_dir: root.clone(),
        };
        let defaults = storage.load_config().unwrap();
        assert!(defaults.external_tools.streamlink_path.is_none());
        assert!(storage.config_path().exists());

        let mut config = AppConfig::default();
        config.external_tools.streamlink_path =
            Some(std::path::PathBuf::from(r"C:\Tools\streamlink.exe"));

        storage.save_config(&config).unwrap();
        let restored = storage.load_config().unwrap();

        assert_eq!(storage.config_path(), root.join("config.json"));
        assert_eq!(
            restored.external_tools.streamlink_path,
            config.external_tools.streamlink_path
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

    #[test]
    fn safe_workspace_writes_keep_the_previous_valid_save() {
        let root = temp_root("workspace-backup");
        let storage = Storage {
            data_dir: root.clone(),
        };
        let mut first = SavedLayout::new();
        first.canvas.zoom = 1.25;
        let mut second = SavedLayout::new();
        second.canvas.zoom = 2.5;

        storage.save_workspace("workspace-1", &first).unwrap();
        storage.save_workspace("workspace-1", &second).unwrap();

        assert_eq!(
            storage
                .load_workspace_backup("workspace-1")
                .unwrap()
                .canvas
                .zoom,
            1.25
        );
        assert_eq!(
            storage.load_workspace("workspace-1").unwrap().canvas.zoom,
            2.5
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn truncated_primary_uses_and_preserves_the_valid_backup() {
        let root = temp_root("workspace-truncated-primary");
        let storage = Storage {
            data_dir: root.clone(),
        };
        let mut first = SavedLayout::new();
        first.canvas.zoom = 1.25;
        let mut second = SavedLayout::new();
        second.canvas.zoom = 2.5;
        let mut third = SavedLayout::new();
        third.canvas.zoom = 3.75;
        storage.save_workspace("workspace-1", &first).unwrap();
        storage.save_workspace("workspace-1", &second).unwrap();
        let primary = root.join("workspaces").join("workspace-1.json");
        fs::write(&primary, "{ truncated").unwrap();

        assert_eq!(
            storage.load_workspace("workspace-1").unwrap().canvas.zoom,
            1.25
        );
        storage.save_workspace("workspace-1", &third).unwrap();
        assert_eq!(
            storage
                .load_workspace_backup("workspace-1")
                .unwrap()
                .canvas
                .zoom,
            1.25
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_catalog_is_rebuilt_from_valid_workspace_files() {
        let root = temp_root("workspace-catalog-rebuild");
        let storage = Storage {
            data_dir: root.clone(),
        };
        storage
            .save_workspace("workspace-4", &SavedLayout::new())
            .unwrap();
        storage
            .save_workspace("workspace-7", &SavedLayout::new())
            .unwrap();
        fs::write(root.join("workspaces.json"), "{ truncated").unwrap();

        let index = storage.load_or_initialize_workspaces().unwrap();
        let ids = index
            .workspaces
            .iter()
            .map(|workspace| workspace.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["workspace-4", "workspace-7"]);
        assert!(
            index.active_workspace_id == "workspace-4"
                || index.active_workspace_id == "workspace-7"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn orphaned_workspace_files_are_returned_to_the_catalog() {
        let root = temp_root("workspace-orphan-recovery");
        let storage = Storage {
            data_dir: root.clone(),
        };
        let index = WorkspaceIndex::default();
        storage
            .save_workspace("workspace-1", &SavedLayout::new())
            .unwrap();
        storage.save_workspace_index(&index).unwrap();
        storage
            .save_workspace("workspace-4", &SavedLayout::new())
            .unwrap();

        let recovered = storage.load_or_initialize_workspaces().unwrap();

        assert!(recovered
            .workspaces
            .iter()
            .any(|workspace| workspace.id == "workspace-4"));
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

    #[test]
    fn first_run_creates_a_durable_empty_workspace() {
        let root = temp_root("first-workspace");
        let storage = Storage {
            data_dir: root.clone(),
        };
        let index = storage.load_or_initialize_workspaces().unwrap();
        assert!(storage
            .load_workspace(&index.active_workspace_id)
            .unwrap()
            .previews
            .is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_workspace_schema_never_replaces_a_usable_backup() {
        let root = temp_root("workspace-schema-backup");
        let storage = Storage {
            data_dir: root.clone(),
        };
        let mut first = SavedLayout::new();
        first.canvas.zoom = 1.25;
        storage.save_workspace("workspace-1", &first).unwrap();
        storage
            .save_workspace("workspace-1", &SavedLayout::new())
            .unwrap();
        let primary = storage.workspace_path("workspace-1").unwrap();
        let backup_before = fs::read(super::backup_path(&primary)).unwrap();
        for broken in ["{}", "[]", "null", r#"{"version":1,"canvas":{}}"#] {
            fs::write(&primary, broken).unwrap();
            assert_eq!(
                storage.load_workspace("workspace-1").unwrap().canvas.zoom,
                1.25
            );
            storage
                .save_workspace("workspace-1", &SavedLayout::new())
                .unwrap();
            assert_eq!(
                fs::read(super::backup_path(&primary)).unwrap(),
                backup_before
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_load_blocks_autosave_and_exit_save_until_a_successful_retry() {
        let root = temp_root("failed-workspace-load");
        let storage = Storage {
            data_dir: root.clone(),
        };
        let index = storage.load_or_initialize_workspaces().unwrap();
        let mut state = WorkspaceSaveState::default();
        let mut layout = state.load(&storage, &index.active_workspace_id).unwrap();
        layout.canvas.zoom = 1.5;
        state.save(&storage, &index, &layout).unwrap();
        let primary = storage.workspace_path(&index.active_workspace_id).unwrap();
        let backup = super::backup_path(&primary);
        let good_bytes = fs::read(&primary).unwrap();
        let autosave = fs::read(storage.autosave_path()).unwrap();
        fs::write(&primary, "{ incomplete").unwrap();
        fs::write(&backup, "{}").unwrap();

        assert!(state.load(&storage, &index.active_workspace_id).is_err());
        assert!(!state.is_ready());
        // The same save entry point is used by periodic, explicit, and exit saves.
        for _ in 0..3 {
            assert!(state.save(&storage, &index, &SavedLayout::new()).is_err());
            assert_eq!(fs::read(&primary).unwrap(), b"{ incomplete");
            assert_eq!(fs::read(&backup).unwrap(), b"{}");
            assert_eq!(fs::read(storage.autosave_path()).unwrap(), autosave);
        }
        fs::write(&primary, good_bytes).unwrap();
        let mut recovered = state.load(&storage, &index.active_workspace_id).unwrap();
        assert!(state.is_ready());
        recovered.canvas.zoom = 2.0;
        state.save(&storage, &index, &recovered).unwrap();
        assert_eq!(
            storage
                .load_workspace(&index.active_workspace_id)
                .unwrap()
                .canvas
                .zoom,
            2.0
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unchanged_workspace_does_not_rotate_backup_or_rewrite_catalog() {
        let root = temp_root("unchanged-workspace");
        let storage = Storage {
            data_dir: root.clone(),
        };
        let index = storage.load_or_initialize_workspaces().unwrap();
        let catalog = fs::read(storage.workspace_index_path()).unwrap();
        let mut state = WorkspaceSaveState::default();
        let mut layout = state.load(&storage, &index.active_workspace_id).unwrap();
        layout.canvas.zoom = 2.0;
        state.save(&storage, &index, &layout).unwrap();
        let backup =
            super::backup_path(&storage.workspace_path(&index.active_workspace_id).unwrap());
        let backup_before = fs::read(&backup).unwrap();
        state.save(&storage, &index, &layout).unwrap();
        assert_eq!(fs::read(&backup).unwrap(), backup_before);
        assert_eq!(fs::read(storage.workspace_index_path()).unwrap(), catalog);
        assert!(!super::backup_path(&storage.workspace_index_path()).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn newer_workspace_and_catalog_versions_are_preserved() {
        let root = temp_root("newer-workspace");
        let storage = Storage {
            data_dir: root.clone(),
        };
        let index = storage.load_or_initialize_workspaces().unwrap();
        let primary = storage.workspace_path(&index.active_workspace_id).unwrap();
        storage
            .save_workspace(&index.active_workspace_id, &SavedLayout::new())
            .unwrap();
        let backup_before = fs::read(super::backup_path(&primary)).unwrap();
        let mut newer = SavedLayout::new();
        newer.version = 2;
        let bytes = serde_json::to_vec(&newer).unwrap();
        fs::write(&primary, &bytes).unwrap();
        assert!(storage.load_workspace(&index.active_workspace_id).is_err());
        assert!(storage
            .save_workspace(&index.active_workspace_id, &SavedLayout::new())
            .is_err());
        assert_eq!(fs::read(&primary).unwrap(), bytes);
        assert_eq!(
            fs::read(super::backup_path(&primary)).unwrap(),
            backup_before
        );
        let mut newer_index = index.clone();
        newer_index.version = 2;
        let bytes = serde_json::to_vec(&newer_index).unwrap();
        fs::write(storage.workspace_index_path(), &bytes).unwrap();
        assert!(storage.load_or_initialize_workspaces().is_err());
        assert_eq!(fs::read(storage.workspace_index_path()).unwrap(), bytes);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn incompatible_future_schema_never_falls_back_or_gets_overwritten() {
        let root = temp_root("future-schema");
        let storage = Storage {
            data_dir: root.clone(),
        };
        let index = storage.load_or_initialize_workspaces().unwrap();
        let primary = storage.workspace_path(&index.active_workspace_id).unwrap();
        storage
            .save_workspace(&index.active_workspace_id, &SavedLayout::new())
            .unwrap();
        let backup_before = fs::read(super::backup_path(&primary)).unwrap();
        let future = br#"{"version":2,"canvas":"a new format"}"#;
        fs::write(&primary, future).unwrap();
        assert!(storage.load_workspace(&index.active_workspace_id).is_err());
        assert!(storage
            .save_workspace(&index.active_workspace_id, &SavedLayout::new())
            .is_err());
        assert_eq!(fs::read(&primary).unwrap(), future);
        assert_eq!(
            fs::read(super::backup_path(&primary)).unwrap(),
            backup_before
        );
        fs::write(storage.workspace_index_path(), future).unwrap();
        assert!(storage.load_or_initialize_workspaces().is_err());
        assert_eq!(fs::read(storage.workspace_index_path()).unwrap(), future);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_catalog_recovers_a_backup_only_workspace() {
        let root = temp_root("backup-only-workspace");
        let storage = Storage {
            data_dir: root.clone(),
        };
        let mut layout = SavedLayout::new();
        layout.canvas.zoom = 1.75;
        storage.save_workspace("workspace-7", &layout).unwrap();
        let primary = storage.workspace_path("workspace-7").unwrap();
        fs::rename(&primary, super::backup_path(&primary)).unwrap();
        let index = storage.load_or_initialize_workspaces().unwrap();
        assert_eq!(index.active_workspace_id, "workspace-7");
        assert_eq!(index.workspaces.len(), 1);
        assert_eq!(
            storage.load_workspace("workspace-7").unwrap().canvas.zoom,
            1.75
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_catalog_does_not_replace_unrecoverable_workspace_files() {
        let root = temp_root("unrecoverable-workspaces");
        let storage = Storage {
            data_dir: root.clone(),
        };
        let primary = storage.workspace_path("workspace-1").unwrap();
        fs::write(&primary, b"{ broken").unwrap();
        fs::write(super::backup_path(&primary), b"{}").unwrap();
        assert!(storage.load_or_initialize_workspaces().is_err());
        assert_eq!(fs::read(&primary).unwrap(), b"{ broken");
        assert_eq!(fs::read(super::backup_path(&primary)).unwrap(), b"{}");
        assert!(!storage.workspace_index_path().exists());
        fs::remove_dir_all(root).unwrap();
    }
}
