use serde::{Deserialize, Serialize};

pub const DEFAULT_WORKSPACE_ID: &str = "workspace-1";
pub const DEFAULT_WORKSPACE_NAME: &str = "Default";

/// Lightweight catalog entry. Workspace contents live in a separate layout file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub id: String,
    pub name: String,
}

/// Catalog of named workspaces and the workspace currently shown by the app.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceIndex {
    pub version: u32,
    pub active_workspace_id: String,
    pub workspaces: Vec<WorkspaceSummary>,
    #[serde(default = "default_next_workspace_id")]
    next_workspace_id: u64,
}

impl Default for WorkspaceIndex {
    fn default() -> Self {
        Self {
            version: 1,
            active_workspace_id: DEFAULT_WORKSPACE_ID.to_owned(),
            workspaces: vec![WorkspaceSummary {
                id: DEFAULT_WORKSPACE_ID.to_owned(),
                name: DEFAULT_WORKSPACE_NAME.to_owned(),
            }],
            next_workspace_id: default_next_workspace_id(),
        }
    }
}

impl WorkspaceIndex {
    pub fn active(&self) -> Option<&WorkspaceSummary> {
        self.workspaces
            .iter()
            .find(|workspace| workspace.id == self.active_workspace_id)
    }

    pub fn add(&mut self, name: String) -> String {
        let id = loop {
            let id = format!("workspace-{}", self.next_workspace_id);
            self.next_workspace_id += 1;
            if self.workspaces.iter().all(|workspace| workspace.id != id) {
                break id;
            }
        };
        self.workspaces.push(WorkspaceSummary {
            id: id.clone(),
            name,
        });
        id
    }

    pub fn rename(&mut self, id: &str, name: String) -> bool {
        let Some(workspace) = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == id)
        else {
            return false;
        };
        workspace.name = name;
        true
    }

    pub fn remove(&mut self, id: &str) -> bool {
        if self.workspaces.len() <= 1 {
            return false;
        }
        let original_len = self.workspaces.len();
        self.workspaces.retain(|workspace| workspace.id != id);
        if self.workspaces.len() == original_len {
            return false;
        }
        if self.active_workspace_id == id {
            self.active_workspace_id = self.workspaces[0].id.clone();
        }
        true
    }

    pub fn repair(&mut self) {
        let mut seen_ids = Vec::new();
        self.workspaces.retain(|workspace| {
            is_valid_workspace_id(&workspace.id)
                && !workspace.name.trim().is_empty()
                && if seen_ids.contains(&workspace.id) {
                    false
                } else {
                    seen_ids.push(workspace.id.clone());
                    true
                }
        });
        if self.workspaces.is_empty() {
            *self = Self::default();
            return;
        }
        if self.active().is_none() {
            self.active_workspace_id = self.workspaces[0].id.clone();
        }
        let largest_id = self
            .workspaces
            .iter()
            .filter_map(|workspace| workspace.id.strip_prefix("workspace-"))
            .filter_map(|suffix| suffix.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        self.next_workspace_id = self.next_workspace_id.max(largest_id + 1);
    }
}

pub fn is_valid_workspace_id(id: &str) -> bool {
    id.strip_prefix("workspace-").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn default_next_workspace_id() -> u64 {
    2
}

#[cfg(test)]
mod tests {
    use super::{is_valid_workspace_id, WorkspaceIndex};

    #[test]
    fn workspace_ids_are_stable_and_path_safe() {
        let mut index = WorkspaceIndex::default();
        let second = index.add("Research".to_owned());
        let third = index.add("Streaming / OBS".to_owned());

        assert_eq!(second, "workspace-2");
        assert_eq!(third, "workspace-3");
        assert!(is_valid_workspace_id(&second));
        assert!(!is_valid_workspace_id("../autosave"));
        assert!(!is_valid_workspace_id("workspace-not-a-number"));
    }

    #[test]
    fn deleting_the_active_workspace_selects_a_survivor() {
        let mut index = WorkspaceIndex::default();
        let second = index.add("Research".to_owned());
        index.active_workspace_id = second.clone();

        assert!(index.remove(&second));
        assert_eq!(index.active_workspace_id, "workspace-1");
        assert!(!index.remove("workspace-1"));
    }
}
