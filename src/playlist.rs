use eframe::egui::TextureHandle;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use crate::media;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderPlaylistLayout {
    pub folder: PathBuf,
    #[serde(default)]
    pub selected: Option<PathBuf>,
    #[serde(default = "default_true")]
    pub autoplay: bool,
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default)]
    pub repeat: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbnailState {
    Idle,
    Loading,
    Failed,
}

#[derive(Clone)]
pub struct PlaylistEntry {
    pub path: PathBuf,
    pub name: String,
    pub thumbnail: Option<TextureHandle>,
    pub thumbnail_state: ThumbnailState,
}

#[derive(Clone)]
pub struct FolderPlaylist {
    pub folder: PathBuf,
    pub entries: Vec<PlaylistEntry>,
    pub selected: Option<PathBuf>,
    pub autoplay: bool,
    pub shuffle: bool,
    pub repeat: bool,
    pub scroll_offset: f32,
    pub error: Option<String>,
}

impl FolderPlaylist {
    pub fn scan(folder: PathBuf, selected: Option<PathBuf>) -> Result<Self, String> {
        if !folder.is_dir() {
            return Err(format!("Video folder is unavailable: {}", folder.display()));
        }

        let mut paths = std::fs::read_dir(&folder)
            .map_err(|error| format!("Could not read video folder: {error}"))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && media::is_supported_video_path(path))
            .collect::<Vec<_>>();
        paths.sort_by(|left, right| natural_path_cmp(left, right));

        if paths.is_empty() {
            return Err(format!(
                "No supported videos were found directly inside {}",
                folder.display()
            ));
        }

        let selected = selected
            .filter(|candidate| paths.iter().any(|path| path == candidate))
            .or_else(|| paths.first().cloned());
        let entries = paths
            .into_iter()
            .map(|path| PlaylistEntry {
                name: path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Video")
                    .to_owned(),
                path,
                thumbnail: None,
                thumbnail_state: ThumbnailState::Idle,
            })
            .collect();

        Ok(Self {
            folder,
            entries,
            selected,
            autoplay: true,
            shuffle: false,
            repeat: false,
            scroll_offset: 0.0,
            error: None,
        })
    }

    pub fn from_layout(layout: &FolderPlaylistLayout) -> Result<Self, String> {
        let mut playlist = match Self::scan(layout.folder.clone(), layout.selected.clone()) {
            Ok(playlist) => playlist,
            Err(error) => Self {
                folder: layout.folder.clone(),
                entries: Vec::new(),
                selected: layout.selected.clone(),
                autoplay: layout.autoplay,
                shuffle: layout.shuffle,
                repeat: layout.repeat,
                scroll_offset: 0.0,
                error: Some(error),
            },
        };
        playlist.autoplay = layout.autoplay;
        playlist.shuffle = layout.shuffle;
        playlist.repeat = layout.repeat;
        Ok(playlist)
    }

    pub fn layout(&self) -> FolderPlaylistLayout {
        FolderPlaylistLayout {
            folder: self.folder.clone(),
            selected: self.selected.clone(),
            autoplay: self.autoplay,
            shuffle: self.shuffle,
            repeat: self.repeat,
        }
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.entries.iter().any(|entry| entry.path == path)
    }

    pub fn adjacent_path(&self, direction: i32) -> Option<PathBuf> {
        if self.entries.is_empty() {
            return None;
        }
        if self.shuffle && direction > 0 && self.entries.len() > 1 {
            let current = self
                .selected
                .as_ref()
                .and_then(|path| self.entries.iter().position(|entry| &entry.path == path))
                .unwrap_or(0);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos() as usize;
            let mut next = nanos % self.entries.len();
            if next == current {
                next = (next + 1) % self.entries.len();
            }
            return Some(self.entries[next].path.clone());
        }

        let current = self
            .selected
            .as_ref()
            .and_then(|path| self.entries.iter().position(|entry| &entry.path == path));
        let next = match (current, direction.cmp(&0)) {
            (Some(index), Ordering::Greater) if index + 1 < self.entries.len() => index + 1,
            (Some(_), Ordering::Greater) if self.repeat => 0,
            (Some(index), Ordering::Less) if index > 0 => index - 1,
            (Some(_), Ordering::Less) if self.repeat => self.entries.len() - 1,
            (None, _) => 0,
            _ => return None,
        };
        Some(self.entries[next].path.clone())
    }
}

fn natural_path_cmp(left: &Path, right: &Path) -> Ordering {
    let left = left
        .file_name()
        .unwrap_or(left.as_os_str())
        .to_string_lossy();
    let right = right
        .file_name()
        .unwrap_or(right.as_os_str())
        .to_string_lossy();
    natural_cmp(&left, &right)
}

fn natural_cmp(left: &str, right: &str) -> Ordering {
    let mut left = left.chars().peekable();
    let mut right = right.chars().peekable();
    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(a), Some(b)) if a.is_ascii_digit() && b.is_ascii_digit() => {
                let mut a_number = String::new();
                let mut b_number = String::new();
                while left.peek().is_some_and(char::is_ascii_digit) {
                    a_number.push(left.next().unwrap());
                }
                while right.peek().is_some_and(char::is_ascii_digit) {
                    b_number.push(right.next().unwrap());
                }
                let a_trimmed = a_number.trim_start_matches('0');
                let b_trimmed = b_number.trim_start_matches('0');
                let ordering = a_trimmed
                    .len()
                    .cmp(&b_trimmed.len())
                    .then_with(|| a_trimmed.cmp(b_trimmed))
                    .then_with(|| a_number.len().cmp(&b_number.len()));
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (Some(a), Some(b)) => {
                left.next();
                right.next();
                let ordering = a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase());
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{natural_cmp, FolderPlaylist, FolderPlaylistLayout};
    use std::cmp::Ordering;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn natural_sort_orders_numbered_videos_for_people() {
        assert_eq!(natural_cmp("video 2.mp4", "video 10.mp4"), Ordering::Less);
        assert_eq!(
            natural_cmp("Chapter 02.mkv", "chapter 3.mkv"),
            Ordering::Less
        );
    }

    #[test]
    fn folder_scan_is_top_level_and_ignores_non_video_files() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("pluriview-playlist-{suffix}"));
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("video 10.mp4"), b"").unwrap();
        fs::write(root.join("video 2.mkv"), b"").unwrap();
        fs::write(root.join("notes.txt"), b"").unwrap();
        fs::write(root.join("nested").join("hidden.mp4"), b"").unwrap();

        let playlist = FolderPlaylist::scan(root.clone(), None).unwrap();
        let names = playlist
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["video 2.mkv", "video 10.mp4"]);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unavailable_saved_folder_restores_as_a_visible_error_tile() {
        let missing = std::env::temp_dir().join("pluriview-folder-that-does-not-exist");
        let restored = FolderPlaylist::from_layout(&FolderPlaylistLayout {
            folder: missing,
            selected: None,
            autoplay: true,
            shuffle: false,
            repeat: false,
        })
        .unwrap();

        assert!(restored.entries.is_empty());
        assert!(restored.error.is_some());
    }
}
