mod coordinator;
mod downsample;
#[cfg(windows)]
mod gpu_downsample;
mod spout;

pub use coordinator::CaptureCoordinator;
pub use downsample::{
    capture_lod_factor, screenshot_capture_size, window_capture_target, MAX_CAPTURE_HEIGHT,
    MAX_CAPTURE_WIDTH,
};
