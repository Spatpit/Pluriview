mod animation;
mod state;
mod wallpaper;

pub use state::{
    BrowserAction, BrowserNavigation, CanvasKeyboardInput, CanvasState, MediaAction,
    PlaylistAction, TabDetach, TabTransfer, TileActivityAction, VideoAction,
};
pub use wallpaper::{CanvasWallpaper, WallpaperSource, WALLPAPER_VIDEO_ID};
