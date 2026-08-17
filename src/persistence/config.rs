use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const CURRENT_CONFIG_VERSION: u32 = 1;

/// App-global preferences, stored separately from workspace layouts.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct AppConfig {
    pub version: u32,
    pub external_tools: ExternalToolsConfig,
    /// When true (default), the custom title bar stays on screen. When false,
    /// it hides until the pointer is at the top of the window.
    pub always_show_title_bar: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CURRENT_CONFIG_VERSION,
            external_tools: ExternalToolsConfig::default(),
            always_show_title_bar: true,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ExternalToolsConfig {
    pub mpv_path: Option<PathBuf>,
    pub streamlink_path: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, CURRENT_CONFIG_VERSION};
    use std::path::PathBuf;

    #[test]
    fn missing_fields_load_with_current_defaults() {
        let config: AppConfig = serde_json::from_str("{}").unwrap();

        assert_eq!(config.version, CURRENT_CONFIG_VERSION);
        assert!(config.always_show_title_bar);
        assert!(config.external_tools.mpv_path.is_none());
        assert!(config.external_tools.streamlink_path.is_none());
    }

    #[test]
    fn legacy_partial_config_and_unknown_fields_are_compatible() {
        let config: AppConfig = serde_json::from_str(
            r#"{
                "external_tools": { "mpv_path": "C:\\Tools\\mpv.exe" },
                "setting_from_a_newer_build": true
            }"#,
        )
        .unwrap();

        assert_eq!(
            config.external_tools.mpv_path,
            Some(PathBuf::from(r"C:\Tools\mpv.exe"))
        );
        assert!(config.external_tools.streamlink_path.is_none());
        assert!(config.always_show_title_bar);
    }

    #[test]
    fn config_round_trip_preserves_overrides_and_version() {
        let mut config = AppConfig::default();
        config.external_tools.mpv_path = Some(PathBuf::from(r"D:\Portable\mpv.exe"));
        config.external_tools.streamlink_path = Some(PathBuf::from(r"D:\Portable\streamlink.exe"));
        config.always_show_title_bar = false;

        let json = serde_json::to_string(&config).unwrap();
        let restored: AppConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.version, CURRENT_CONFIG_VERSION);
        assert!(!restored.always_show_title_bar);
        assert_eq!(
            restored.external_tools.mpv_path,
            config.external_tools.mpv_path
        );
        assert_eq!(
            restored.external_tools.streamlink_path,
            config.external_tools.streamlink_path
        );
    }
}
