//! The screenshots folder as a small library: one level of named folders
//! inside Pluriview's `screenshots` data folder, each holding PNG files.
//! Every path that reaches the file system is checked to stay inside it.

use std::cmp::Ordering;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

const MAX_FOLDER_CHARS: usize = 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenshotEntry {
    pub path: PathBuf,
    /// File name without the `.png` extension.
    pub name: String,
    pub modified: SystemTime,
}

/// One section of the panel. `name` is `None` for screenshots saved
/// directly in the screenshots folder ("Unsorted").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenshotFolder {
    pub name: Option<String>,
    pub shots: Vec<ScreenshotEntry>,
}

/// Unsorted shots first, then folders by name; shots newest first.
pub fn scan(root: &Path) -> Vec<ScreenshotFolder> {
    let mut folders = vec![ScreenshotFolder {
        name: None,
        shots: Vec::new(),
    }];
    let Ok(entries) = fs::read_dir(root) else {
        return folders;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if validate_folder_name(name).as_deref() != Ok(name) {
                continue;
            }
            folders.push(ScreenshotFolder {
                name: Some(name.to_owned()),
                shots: scan_shots(&path),
            });
        } else if let Some(shot) = shot_entry(&path, kind.is_file()) {
            folders[0].shots.push(shot);
        }
    }
    sort_shots(&mut folders[0].shots);
    folders[1..].sort_by(|left, right| natural_cmp(left.name.as_deref(), right.name.as_deref()));
    folders
}

fn scan_shots(directory: &Path) -> Vec<ScreenshotEntry> {
    let mut shots: Vec<_> = fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
            shot_entry(&entry.path(), is_file)
        })
        .collect();
    sort_shots(&mut shots);
    shots
}

fn shot_entry(path: &Path, is_file: bool) -> Option<ScreenshotEntry> {
    let is_png = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
    if !is_file || !is_png {
        return None;
    }
    let name = path.file_stem()?.to_str()?.to_owned();
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    Some(ScreenshotEntry {
        path: path.to_path_buf(),
        name,
        modified,
    })
}

fn sort_shots(shots: &mut [ScreenshotEntry]) {
    shots.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| right.name.cmp(&left.name))
    });
}

fn natural_cmp(left: Option<&str>, right: Option<&str>) -> Ordering {
    let left = left.unwrap_or_default().to_lowercase();
    let right = right.unwrap_or_default().to_lowercase();
    left.cmp(&right)
}

/// A folder name that is safe on Windows and cannot leave the library.
pub fn validate_folder_name(name: &str) -> Result<String, &'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a folder name.");
    }
    if name.chars().count() > MAX_FOLDER_CHARS {
        return Err("Use a shorter folder name.");
    }
    if name
        .chars()
        .any(|character| character.is_control() || r#"<>:"/\|?*"#.contains(character))
    {
        return Err(r#"Folder names can't contain < > : " / \ | ? *"#);
    }
    if name.ends_with('.') || name == "." || name == ".." {
        return Err("Folder names can't end with a dot.");
    }
    let base = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && base.as_bytes()[3].is_ascii_digit());
    if reserved {
        return Err("Windows reserves that name. Choose another.");
    }
    Ok(name.to_owned())
}

/// Folder that holds new screenshots; a missing or unsafe name means the
/// top level.
pub fn folder_path(root: &Path, folder: Option<&str>) -> PathBuf {
    match folder.and_then(|name| validate_folder_name(name).ok()) {
        Some(name) => root.join(name),
        None => root.to_path_buf(),
    }
}

pub fn create_folder(root: &Path, name: &str) -> Result<String, String> {
    let name = validate_folder_name(name)?;
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    match fs::create_dir(root.join(&name)) {
        Ok(()) => Ok(name),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            Err("A folder with that name already exists.".to_owned())
        }
        Err(error) => Err(format!("Could not create the folder: {error}")),
    }
}

pub fn rename_folder(root: &Path, old: &str, new: &str) -> Result<String, String> {
    let old = validate_folder_name(old)?;
    let new = validate_folder_name(new)?;
    if old == new {
        return Ok(new);
    }
    let target = root.join(&new);
    // Allow changing only the letter case of an existing folder.
    if target.exists() && !old.eq_ignore_ascii_case(&new) {
        return Err("A folder with that name already exists.".to_owned());
    }
    fs::rename(root.join(&old), target)
        .map(|()| new)
        .map_err(|error| format!("Could not rename the folder: {error}"))
}

/// The library folder a screenshot lives in, or `None` if the path is not a
/// PNG directly inside the library or one of its folders.
pub fn shot_folder(root: &Path, shot: &Path) -> Option<Option<String>> {
    let relative = shot.strip_prefix(root).ok()?;
    let parts: Vec<_> = relative.components().collect();
    let is_png = shot
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
    if !is_png {
        return None;
    }
    match parts.as_slice() {
        [Component::Normal(_)] => Some(None),
        [Component::Normal(folder), Component::Normal(_)] => {
            let folder = folder.to_str()?;
            (validate_folder_name(folder).as_deref() == Ok(folder)).then(|| Some(folder.to_owned()))
        }
        _ => None,
    }
}

/// Move a screenshot into another library folder without replacing a file.
pub fn move_shot(root: &Path, shot: &Path, folder: Option<&str>) -> Result<PathBuf, String> {
    let current = shot_folder(root, shot)
        .ok_or_else(|| "That screenshot is not in the screenshots folder.".to_owned())?;
    let folder = match folder {
        Some(name) => Some(validate_folder_name(name)?),
        None => None,
    };
    if current == folder {
        return Ok(shot.to_path_buf());
    }
    let directory = folder_path(root, folder.as_deref());
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let stem = shot
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| "That screenshot has an unreadable name.".to_owned())?;
    for attempt in 1..=999 {
        let target = if attempt == 1 {
            directory.join(format!("{stem}.png"))
        } else {
            directory.join(format!("{stem} ({attempt}).png"))
        };
        if target.exists() {
            continue;
        }
        return fs::rename(shot, &target)
            .map(|()| target)
            .map_err(|error| format!("Could not move the screenshot: {error}"));
    }
    Err("Could not find a free file name in that folder.".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pluriview-library-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn touch(path: &Path) {
        fs::write(path, b"png").unwrap();
    }

    #[test]
    fn folder_names_are_windows_safe() {
        assert_eq!(validate_folder_name("  Games  ").unwrap(), "Games");
        for bad in [
            "", "   ", "a/b", r"a\b", "..", "x.", "CON", "com1", "LPT9.txt", "a?",
        ] {
            assert!(
                validate_folder_name(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
        assert!(validate_folder_name("Console").is_ok());
        assert!(validate_folder_name(&"x".repeat(61)).is_err());
    }

    #[test]
    fn unsafe_saved_folders_fall_back_to_the_top_level() {
        let root = Path::new("C:/data/screenshots");
        assert_eq!(folder_path(root, Some("..")), root);
        assert_eq!(folder_path(root, Some("a/b")), root);
        assert_eq!(folder_path(root, Some("Trips")), root.join("Trips"));
        assert_eq!(folder_path(root, None), root);
    }

    #[test]
    fn only_library_pngs_can_be_moved() {
        let root = Path::new("C:/data/screenshots");
        assert_eq!(shot_folder(root, &root.join("a.png")), Some(None));
        assert_eq!(
            shot_folder(root, &root.join("Trips").join("a.PNG")),
            Some(Some("Trips".to_owned()))
        );
        assert_eq!(shot_folder(root, &root.join("a.txt")), None);
        assert_eq!(
            shot_folder(root, &root.join("a").join("b").join("c.png")),
            None
        );
        assert_eq!(shot_folder(root, Path::new("C:/elsewhere/a.png")), None);
        assert_eq!(shot_folder(root, &root.join("..").join("a.png")), None);
    }

    #[test]
    fn scan_lists_unsorted_first_then_folders_with_pngs_only() {
        let root = temp_root("scan");
        touch(&root.join("Loose.png"));
        touch(&root.join("notes.txt"));
        fs::create_dir(root.join("beta")).unwrap();
        fs::create_dir(root.join("Alpha")).unwrap();
        touch(&root.join("Alpha").join("One.png"));
        fs::create_dir(root.join("Alpha").join("Nested")).unwrap();

        let folders = scan(&root);
        let names: Vec<_> = folders.iter().map(|folder| folder.name.clone()).collect();
        assert_eq!(
            names,
            vec![None, Some("Alpha".to_owned()), Some("beta".to_owned())]
        );
        assert_eq!(folders[0].shots.len(), 1);
        assert_eq!(folders[0].shots[0].name, "Loose");
        assert_eq!(folders[1].shots.len(), 1);
        assert!(folders[2].shots.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_library_scans_as_empty_unsorted() {
        let folders = scan(Path::new("Z:/definitely/missing/pluriview"));
        assert_eq!(folders.len(), 1);
        assert!(folders[0].shots.is_empty());
    }

    #[test]
    fn folders_can_be_created_renamed_and_receive_moves() {
        let root = temp_root("move");
        assert_eq!(create_folder(&root, " Trips ").unwrap(), "Trips");
        assert!(create_folder(&root, "Trips").is_err());
        touch(&root.join("Shot.png"));
        touch(&root.join("Trips").join("Shot.png"));

        let moved = move_shot(&root, &root.join("Shot.png"), Some("Trips")).unwrap();
        assert_eq!(moved, root.join("Trips").join("Shot (2).png"));
        assert!(!root.join("Shot.png").exists());

        assert_eq!(
            rename_folder(&root, "Trips", "Holidays").unwrap(),
            "Holidays"
        );
        assert!(root.join("Holidays").join("Shot (2).png").exists());
        let back = move_shot(&root, &root.join("Holidays").join("Shot.png"), None).unwrap();
        assert_eq!(back, root.join("Shot.png"));
        assert!(move_shot(&root, Path::new("C:/elsewhere/x.png"), None).is_err());
        let _ = fs::remove_dir_all(&root);
    }
}
