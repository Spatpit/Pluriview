use super::{SavedLayout, Storage, WorkspaceIndex};

/// A failed load must never turn the empty/remainder canvas into a new save.
/// Both explicit saves and autosave/exit pass through this guard.
pub struct WorkspaceSaveState {
    load_error: Option<String>,
    saved_snapshot: Option<Vec<u8>>,
}

impl Default for WorkspaceSaveState {
    fn default() -> Self {
        Self {
            load_error: Some("The workspace has not been loaded.".to_owned()),
            saved_snapshot: None,
        }
    }
}

impl WorkspaceSaveState {
    pub fn load(&mut self, storage: &Storage, id: &str) -> Result<SavedLayout, String> {
        match storage.load_workspace(id) {
            Ok(layout) => {
                self.remember(&layout);
                Ok(layout)
            }
            Err(error) => {
                let message = format!("Could not load workspace: {error}");
                self.load_error = Some(message.clone());
                Err(message)
            }
        }
    }

    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    pub fn is_ready(&self) -> bool {
        self.load_error.is_none()
    }

    pub fn remember(&mut self, layout: &SavedLayout) {
        self.load_error = None;
        self.saved_snapshot = serde_json::to_vec(layout).ok();
    }

    pub fn save(
        &mut self,
        storage: &Storage,
        index: &WorkspaceIndex,
        layout: &SavedLayout,
    ) -> Result<(), String> {
        if let Some(error) = &self.load_error {
            return Err(format!(
                "Saving is paused to protect this workspace. {error}"
            ));
        }
        let snapshot = serde_json::to_vec(layout)
            .map_err(|error| format!("Could not prepare workspace save: {error}"))?;
        if self.saved_snapshot.as_deref() == Some(snapshot.as_slice()) {
            return Ok(());
        }
        storage
            .save_active_layout(index, layout)
            .map_err(|error| format!("Could not save workspace: {error}"))?;
        self.saved_snapshot = Some(snapshot);
        Ok(())
    }
}
