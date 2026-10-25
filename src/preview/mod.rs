mod manager;
// `preview::preview` holds the `Preview` tile model itself.
#[allow(clippy::module_inception)]
mod preview;
mod video_preferences;

pub use video_preferences::{VideoPlaybackPreferences, VideoTrackPreference};

pub use manager::{PreviewManager, RemovedPreviewInfo};
pub use preview::{
    compact_title, is_usable_media_title, video_tile_title, BrowserTileStatus, FpsPreset, Preview,
    PreviewId, PreviewLayout, TileGroup, VideoPlaybackState, VideoSource, VideoTileStatus,
    VideoTrack, ViewportAnchor, ViewportPin, WindowHandle,
};
