use image::codecs::gif::GifDecoder;
use image::{AnimationDecoder, ImageFormat, ImageReader};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Avoid allowing a pathological animation to consume unbounded memory.
const MAX_DECODED_BYTES: usize = 512 * 1024 * 1024;
const MIN_FRAME_DELAY: Duration = Duration::from_millis(10);
const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];
const VIDEO_EXTENSIONS: [&str; 15] = [
    "mp4", "mkv", "webm", "avi", "mov", "m4v", "wmv", "flv", "mpeg", "mpg", "ts", "m2ts", "3gp",
    "ogv", "mts",
];

fn extension_matches(path: &Path, candidates: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            candidates
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

/// Whether a path has one of the video extensions exposed by the native file
/// picker. MPV still performs the authoritative media validation at launch.
pub fn is_supported_image_path(path: &Path) -> bool {
    extension_matches(path, &IMAGE_EXTENSIONS)
}

pub fn is_supported_video_path(path: &Path) -> bool {
    extension_matches(path, &VIDEO_EXTENSIONS)
}

#[derive(Clone)]
pub struct MediaFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub duration: Duration,
}

pub struct MediaAsset {
    pub frames: Vec<MediaFrame>,
    pub width: u32,
    pub height: u32,
}

/// Decode a supported static image or all frames of an animated GIF.
pub fn load(path: &Path) -> Result<MediaAsset, String> {
    let reader = ImageReader::open(path)
        .map_err(|error| format!("Could not open image: {error}"))?
        .with_guessed_format()
        .map_err(|error| format!("Could not identify image format: {error}"))?;

    if reader.format() == Some(ImageFormat::Gif) {
        return load_gif(path);
    }

    let image = reader
        .decode()
        .map_err(|error| format!("Could not decode image: {error}"))?
        .into_rgba8();
    let (width, height) = image.dimensions();
    validate_frame_size(width, height, image.len())?;
    Ok(MediaAsset {
        frames: vec![MediaFrame {
            width,
            height,
            rgba: image.into_raw(),
            duration: Duration::ZERO,
        }],
        width,
        height,
    })
}

fn load_gif(path: &Path) -> Result<MediaAsset, String> {
    let file = File::open(path).map_err(|error| format!("Could not open GIF: {error}"))?;
    let decoder = GifDecoder::new(BufReader::new(file))
        .map_err(|error| format!("Could not decode GIF: {error}"))?;
    let mut frames = Vec::new();
    let mut decoded_bytes = 0usize;

    for frame in decoder.into_frames() {
        let frame = frame.map_err(|error| format!("Could not decode GIF frame: {error}"))?;
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let delay_ms = if denominator == 0 {
            MIN_FRAME_DELAY.as_millis() as u64
        } else {
            (u64::from(numerator) / u64::from(denominator)).max(1)
        };
        let buffer = frame.into_buffer();
        let (width, height) = buffer.dimensions();
        decoded_bytes = decoded_bytes
            .checked_add(buffer.len())
            .ok_or_else(|| "Animated image is too large".to_owned())?;
        validate_frame_size(width, height, decoded_bytes)?;
        frames.push(MediaFrame {
            width,
            height,
            rgba: buffer.into_raw(),
            duration: Duration::from_millis(delay_ms).max(MIN_FRAME_DELAY),
        });
    }

    let first = frames
        .first()
        .ok_or_else(|| "GIF contains no frames".to_owned())?;
    Ok(MediaAsset {
        width: first.width,
        height: first.height,
        frames,
    })
}

fn validate_frame_size(width: u32, height: u32, decoded_bytes: usize) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("Image has invalid dimensions".to_owned());
    }
    if decoded_bytes > MAX_DECODED_BYTES {
        return Err("Image is too large to load safely".to_owned());
    }
    Ok(())
}

#[cfg(windows)]
fn pick_file_with_filter(
    owner: Option<isize>,
    filter_text: &str,
    title_text: &str,
) -> Option<PathBuf> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    let mut path = vec![0u16; 32_768];
    let filter: Vec<u16> = filter_text.encode_utf16().collect();
    let title: Vec<u16> = format!("{title_text}\0").encode_utf16().collect();
    let mut dialog = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner.map_or_else(HWND::default, |hwnd| HWND(hwnd as *mut _)),
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(path.as_mut_ptr()),
        nMaxFile: path.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };

    if unsafe { GetOpenFileNameW(&mut dialog) }.as_bool() {
        let length = path
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(path.len());
        let path = PathBuf::from(String::from_utf16_lossy(&path[..length]));
        if path.is_absolute() {
            Some(path)
        } else {
            std::env::current_dir()
                .ok()
                .map(|directory| directory.join(path))
        }
    } else {
        None
    }
}

/// Show the native Windows image picker. Cancellation is not an error.
#[cfg(windows)]
pub fn pick_file(owner: Option<isize>) -> Option<PathBuf> {
    pick_file_with_filter(
        owner,
        "Images (*.png;*.jpg;*.jpeg;*.gif;*.webp;*.bmp)\0*.png;*.jpg;*.jpeg;*.gif;*.webp;*.bmp\0All files (*.*)\0*.*\0\0",
        "Add Image",
    )
}

/// Show the native Windows video picker without importing or copying the file.
#[cfg(windows)]
pub fn pick_video_file(owner: Option<isize>) -> Option<PathBuf> {
    pick_file_with_filter(
        owner,
        "Videos (*.mp4;*.mkv;*.webm;*.avi;*.mov;*.m4v;*.wmv;*.flv;*.mpeg;*.mpg;*.ts;*.m2ts;*.3gp;*.ogv)\0*.mp4;*.mkv;*.webm;*.avi;*.mov;*.m4v;*.wmv;*.flv;*.mpeg;*.mpg;*.ts;*.m2ts;*.3gp;*.ogv\0All files (*.*)\0*.*\0\0",
        "Add Video",
    )
}

/// Show a combined image/video picker for the canvas wallpaper.
#[cfg(windows)]
pub fn pick_wallpaper_file(owner: Option<isize>) -> Option<PathBuf> {
    pick_file_with_filter(
        owner,
        "Wallpaper (*.png;*.jpg;*.jpeg;*.gif;*.webp;*.bmp;*.mp4;*.mkv;*.webm;*.avi;*.mov;*.m4v;*.wmv;*.flv;*.mpeg;*.mpg;*.ts;*.m2ts;*.3gp;*.ogv)\0*.png;*.jpg;*.jpeg;*.gif;*.webp;*.bmp;*.mp4;*.mkv;*.webm;*.avi;*.mov;*.m4v;*.wmv;*.flv;*.mpeg;*.mpg;*.ts;*.m2ts;*.3gp;*.ogv\0Images (*.png;*.jpg;*.jpeg;*.gif;*.webp;*.bmp)\0*.png;*.jpg;*.jpeg;*.gif;*.webp;*.bmp\0Videos (*.mp4;*.mkv;*.webm;*.avi;*.mov;*.m4v;*.wmv;*.flv;*.mpeg;*.mpg;*.ts;*.m2ts;*.3gp;*.ogv)\0*.mp4;*.mkv;*.webm;*.avi;*.mov;*.m4v;*.wmv;*.flv;*.mpeg;*.mpg;*.ts;*.m2ts;*.3gp;*.ogv\0All files (*.*)\0*.*\0\0",
        "Set Wallpaper",
    )
}

#[cfg(not(windows))]
pub fn pick_file(_owner: Option<isize>) -> Option<PathBuf> {
    None
}

#[cfg(not(windows))]
pub fn pick_video_file(_owner: Option<isize>) -> Option<PathBuf> {
    None
}

#[cfg(not(windows))]
pub fn pick_wallpaper_file(owner: Option<isize>) -> Option<PathBuf> {
    pick_file(owner)
}

#[cfg(test)]
mod tests {
    use super::{is_supported_image_path, is_supported_video_path, load};
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::{Delay, Frame, Rgba, RgbaImage};
    use std::fs::{self, File};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn common_video_extensions_are_detected_case_insensitively() {
        assert!(is_supported_video_path(std::path::Path::new("clip.MP4")));
        assert!(is_supported_video_path(std::path::Path::new(
            "recording.mkv"
        )));
        assert!(is_supported_image_path(std::path::Path::new("poster.PNG")));
        assert!(!is_supported_video_path(std::path::Path::new("poster.png")));
        assert!(!is_supported_image_path(std::path::Path::new("clip.mp4")));
        assert!(!is_supported_video_path(std::path::Path::new(
            "extensionless"
        )));
    }

    #[test]
    fn animated_gif_keeps_frames_and_authored_timing() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pluriview-gif-test-{}-{nonce}.gif",
            std::process::id()
        ));
        let file = File::create(&path).unwrap();
        let mut encoder = GifEncoder::new(file);
        encoder.set_repeat(Repeat::Infinite).unwrap();
        encoder
            .encode_frame(Frame::from_parts(
                RgbaImage::from_pixel(2, 1, Rgba([255, 0, 0, 255])),
                0,
                0,
                Delay::from_numer_denom_ms(50, 1),
            ))
            .unwrap();
        encoder
            .encode_frame(Frame::from_parts(
                RgbaImage::from_pixel(2, 1, Rgba([0, 255, 0, 255])),
                0,
                0,
                Delay::from_numer_denom_ms(120, 1),
            ))
            .unwrap();
        drop(encoder);

        let asset = load(&path).unwrap();
        assert_eq!((asset.width, asset.height), (2, 1));
        assert_eq!(asset.frames.len(), 2);
        assert_eq!(asset.frames[0].duration, Duration::from_millis(50));
        assert_eq!(asset.frames[1].duration, Duration::from_millis(120));

        fs::remove_file(path).unwrap();
    }
}
