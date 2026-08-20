mod coordinator;
mod downsample;
mod spout;

pub use coordinator::CaptureCoordinator;
pub use downsample::{capture_lod_factor, window_capture_target};
