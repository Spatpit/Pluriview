use serde::{Deserialize, Serialize};

use super::{VideoPlaybackState, VideoTrack};

/// Explicit choices stay separate from mpv's transient, automatically selected tracks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoTrackPreference {
    #[default]
    Auto,
    Off,
    Track(i64),
}

impl VideoTrackPreference {
    pub fn mpv_value(self) -> String {
        match self {
            Self::Track(id) if id > 0 => id.to_string(),
            Self::Off => "no".to_owned(),
            _ => "auto".to_owned(),
        }
    }

    pub fn available(self, kind: &str, tracks: &[VideoTrack]) -> Self {
        match self {
            Self::Track(id)
                if !tracks
                    .iter()
                    .any(|track| track.id == id && track.kind == kind) =>
            {
                Self::Auto
            }
            preference => preference,
        }
    }
}

/// Per-tile intent only: no position, decoder status, or live metadata is saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoPlaybackPreferences {
    pub volume: f64,
    pub muted: bool,
    pub speed: f64,
    pub looping: bool,
    pub audio_track: VideoTrackPreference,
    pub subtitle_track: VideoTrackPreference,
}

impl Default for VideoPlaybackPreferences {
    fn default() -> Self {
        Self {
            volume: 100.0,
            muted: false,
            speed: 1.0,
            looping: false,
            audio_track: VideoTrackPreference::Auto,
            subtitle_track: VideoTrackPreference::Auto,
        }
    }
}

impl VideoPlaybackPreferences {
    pub fn normalized(mut self) -> Self {
        self.volume = if self.volume.is_finite() {
            self.volume.clamp(0.0, 100.0)
        } else {
            100.0
        };
        // Match libmpv's supported speed range, including hand-edited layouts.
        self.speed = if self.speed.is_finite() && self.speed > 0.0 {
            self.speed.clamp(0.01, 100.0)
        } else {
            1.0
        };
        for track in [&mut self.audio_track, &mut self.subtitle_track] {
            if matches!(track, VideoTrackPreference::Track(id) if *id <= 0) {
                *track = VideoTrackPreference::Auto;
            }
        }
        self
    }

    pub fn apply_to_state(&self, state: &mut VideoPlaybackState) {
        state.volume = self.volume;
        state.muted = self.muted;
        state.speed = self.speed;
        state.looping = self.looping;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_preferences_keep_legacy_defaults() {
        let prefs: VideoPlaybackPreferences = serde_json::from_str(r#"{"muted":true}"#).unwrap();
        assert_eq!(
            prefs,
            VideoPlaybackPreferences {
                muted: true,
                ..Default::default()
            }
        );
    }

    #[test]
    fn unavailable_track_falls_back_without_forgetting_the_choice() {
        let preferred = VideoTrackPreference::Track(2);
        let tracks = vec![VideoTrack {
            id: 2,
            kind: "audio".to_owned(),
            ..Default::default()
        }];
        assert_eq!(preferred.available("audio", &tracks), preferred);
        assert_eq!(
            preferred.available("sub", &tracks),
            VideoTrackPreference::Auto
        );
        assert_eq!(
            preferred.available("audio", &[]),
            VideoTrackPreference::Auto
        );
        assert_eq!(
            VideoTrackPreference::Off.available("sub", &[]),
            VideoTrackPreference::Off
        );
    }

    #[test]
    fn invalid_preferences_are_normalized() {
        let prefs = VideoPlaybackPreferences {
            volume: f64::NAN,
            speed: -1.0,
            audio_track: VideoTrackPreference::Track(-1),
            ..Default::default()
        }
        .normalized();
        assert_eq!(prefs, VideoPlaybackPreferences::default());
        let prefs = VideoPlaybackPreferences {
            volume: 200.0,
            speed: 1000.0,
            ..Default::default()
        }
        .normalized();
        assert_eq!(prefs.volume, 100.0);
        assert_eq!(prefs.speed, 100.0);
    }
}
