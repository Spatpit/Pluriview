use image::codecs::gif::GifDecoder;
use image::{AnimationDecoder, ImageDecoder, ImageError, ImageFormat, ImageReader, RgbaImage};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Avoid allowing a pathological animation to consume unbounded memory.
const MAX_DECODED_BYTES: usize = 512 * 1024 * 1024;
const MIN_FRAME_DELAY: Duration = Duration::from_millis(10);
/// Direct3D 11's texture size limit, which ANGLE reports to egui as its
/// maximum texture side. egui panics on larger uploads, so decoded frames are
/// scaled to fit (long full-page screenshots exceed it).
const MAX_TEXTURE_SIDE: u32 = 16_384;

/// egui subtracts its predicted frame time from repaint delays. GIF deadlines
/// already use wall-clock time, so compensate to avoid repeatedly waking early.
#[track_caller]
pub fn request_animation_repaint(ctx: &eframe::egui::Context, remaining: Duration) {
    let predicted = ctx.input(|input| input.predicted_dt);
    ctx.request_repaint_after(animation_repaint_delay(remaining, predicted));
}

fn animation_repaint_delay(remaining: Duration, predicted: f32) -> Duration {
    remaining
        .max(Duration::from_millis(1))
        .saturating_add(Duration::try_from_secs_f32(predicted).unwrap_or_default())
}

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
    /// Premultiplied pixels, converted once while decoding. Each upload
    /// shares them, so showing a GIF frame again converts and copies nothing.
    pub image: Arc<eframe::egui::ColorImage>,
    pub duration: Duration,
}

impl MediaFrame {
    /// A frame from straight-alpha RGBA bytes.
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8], duration: Duration) -> Self {
        Self {
            width,
            height,
            image: Arc::new(eframe::egui::ColorImage::from_rgba_unmultiplied(
                [width as usize, height as usize],
                rgba,
            )),
            duration,
        }
    }
}

pub struct MediaAsset {
    pub frames: Vec<MediaFrame>,
    pub width: u32,
    pub height: u32,
}

/// Decode a supported static image or all frames of an animated GIF.
pub fn load(path: &Path) -> Result<MediaAsset, String> {
    let reader = open_image(path)?;
    if reader.format() == Some(ImageFormat::Gif) {
        return load_gif(path);
    }

    let image = decode_still(reader)?;
    let (width, height) = image.dimensions();
    Ok(MediaAsset {
        frames: vec![MediaFrame::from_rgba(
            width,
            height,
            image.as_raw(),
            Duration::ZERO,
        )],
        width,
        height,
    })
}

/// A still image's exact straight-alpha pixels, scaled to fit a texture the
/// way [`load`] does, or `None` for a GIF. Frames from [`load`] are stored
/// premultiplied, which rounds semi-transparent pixels.
pub fn load_still_rgba(path: &Path) -> Result<Option<RgbaImage>, String> {
    let reader = open_image(path)?;
    if reader.format() == Some(ImageFormat::Gif) {
        return Ok(None);
    }
    decode_still(reader).map(Some)
}

fn open_image(path: &Path) -> Result<ImageReader<BufReader<File>>, String> {
    ImageReader::open(path)
        .map_err(|error| format!("Could not open image: {error}"))?
        .with_guessed_format()
        .map_err(|error| format!("Could not identify image format: {error}"))
}

fn decode_still(reader: ImageReader<BufReader<File>>) -> Result<RgbaImage, String> {
    let image = reader
        .decode()
        .map_err(|error| format!("Could not decode image: {error}"))?
        .into_rgba8();
    validate_frame_size(image.width(), image.height(), image.len())?;
    Ok(fit_to_texture_side(image, MAX_TEXTURE_SIDE))
}

fn load_gif(path: &Path) -> Result<MediaAsset, String> {
    let file = File::open(path).map_err(|error| format!("Could not open GIF: {error}"))?;
    decode_gif(BufReader::new(file))
}

fn decode_gif<R: BufRead + Seek>(reader: R) -> Result<MediaAsset, String> {
    let mut decoder =
        GifDecoder::new(reader).map_err(|error| format!("Could not decode GIF: {error}"))?;
    // The frame iterator allocates a full logical-screen canvas before the
    // first frame, and a tiny crafted file can declare a 65535x65535 screen.
    // The decoder needs that canvas plus one composed frame within the budget,
    // so reject a screen that cannot fit before anything is allocated, then
    // let the decoder enforce the budget on its own buffers too.
    let (screen_width, screen_height) = decoder.dimensions();
    let screen_bytes = (screen_width as usize)
        .saturating_mul(screen_height as usize)
        .saturating_mul(4);
    validate_frame_size(screen_width, screen_height, screen_bytes.saturating_mul(2))?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODED_BYTES as u64);
    decoder.set_limits(limits).map_err(gif_error)?;
    let mut frames = Vec::new();
    let mut decoded_bytes = 0usize;

    for frame in decoder.into_frames() {
        let frame = frame.map_err(gif_error)?;
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let delay_ms = if denominator == 0 {
            MIN_FRAME_DELAY.as_millis() as u64
        } else {
            (u64::from(numerator) / u64::from(denominator)).max(1)
        };
        let buffer = fit_to_texture_side(frame.into_buffer(), MAX_TEXTURE_SIDE);
        let (width, height) = buffer.dimensions();
        decoded_bytes = decoded_bytes
            .checked_add(buffer.len())
            .ok_or_else(|| "Animated image is too large".to_owned())?;
        validate_frame_size(width, height, decoded_bytes)?;
        frames.push(MediaFrame::from_rgba(
            width,
            height,
            buffer.as_raw(),
            Duration::from_millis(delay_ms).max(MIN_FRAME_DELAY),
        ));
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

/// Preserve a saved source when it cannot currently be decoded.
#[cfg(test)]
pub fn load_or_placeholder(path: &Path) -> (Vec<MediaFrame>, Option<String>) {
    match load(path) {
        Ok(asset) => (asset.frames, None),
        Err(error) => (Vec::new(), Some(error)),
    }
}

fn gif_error(error: ImageError) -> String {
    match error {
        ImageError::Limits(_) => "Image is too large to load safely".to_owned(),
        error => format!("Could not decode GIF frame: {error}"),
    }
}

/// Scale an image down, keeping its shape, so neither side exceeds `max_side`.
fn fit_to_texture_side(image: RgbaImage, max_side: u32) -> RgbaImage {
    match fitted_size(image.width(), image.height(), max_side) {
        Some((width, height)) => {
            image::imageops::resize(&image, width, height, image::imageops::FilterType::Triangle)
        }
        None => image,
    }
}

/// The size an image must shrink to so neither side exceeds `max_side`, or
/// `None` when it already fits.
fn fitted_size(width: u32, height: u32, max_side: u32) -> Option<(u32, u32)> {
    let max_side = max_side.max(1);
    let longest = width.max(height);
    if longest <= max_side {
        return None;
    }
    let scale = f64::from(max_side) / f64::from(longest);
    let fit = |side: u32| ((f64::from(side) * scale).round() as u32).clamp(1, max_side);
    Some((fit(width), fit(height)))
}

/// The upload image for a decoded frame: the frame's own pixels, shared.
/// Frames already fit `MAX_TEXTURE_SIDE`; a GPU reporting a smaller limit
/// gets a scaled copy, where egui would otherwise panic during upload.
pub fn frame_upload(frame: &MediaFrame, max_texture_side: usize) -> Arc<eframe::egui::ColorImage> {
    let max_side = u32::try_from(max_texture_side).unwrap_or(u32::MAX);
    if let Some((width, height)) = fitted_size(frame.width, frame.height, max_side) {
        // Premultiplied pixels resize without dark fringes at transparent edges.
        if let Some(image) =
            RgbaImage::from_raw(frame.width, frame.height, frame.image.as_raw().to_vec())
        {
            let image = image::imageops::resize(
                &image,
                width,
                height,
                image::imageops::FilterType::Triangle,
            );
            return Arc::new(eframe::egui::ColorImage::from_rgba_premultiplied(
                [width as usize, height as usize],
                image.as_raw(),
            ));
        }
    }
    Arc::clone(&frame.image)
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
pub(crate) fn pick_file_with_filter(
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
    fn animation_repaint_preserves_wall_clock_deadline() {
        let ctx = eframe::egui::Context::default();
        for _ in 0..2 {
            let _ = ctx.run_ui(Default::default(), |_| {});
        }
        for remaining in [Duration::from_millis(3), Duration::from_millis(83)] {
            let output = ctx.run_ui(Default::default(), |ui| {
                let ctx = &ui.ctx().clone();
                super::request_animation_repaint(ctx, remaining);
            });
            assert_eq!(
                output.viewport_output[&eframe::egui::ViewportId::ROOT].repaint_delay,
                remaining
            );
        }
    }

    #[test]
    fn animation_repaint_handles_overdue_and_invalid_prediction() {
        let predicted = 1.0 / 60.0;
        assert_eq!(
            super::animation_repaint_delay(Duration::ZERO, predicted),
            Duration::from_millis(1) + Duration::from_secs_f32(predicted)
        );
        for predicted in [f32::NAN, f32::INFINITY, -1.0] {
            assert_eq!(
                super::animation_repaint_delay(Duration::from_millis(83), predicted),
                Duration::from_millis(83)
            );
        }
    }

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
    fn corrupt_image_restores_as_a_persistable_tile_and_can_be_repaired() {
        use crate::preview::{Preview, PreviewId, PreviewLayout};
        use eframe::egui::{Context, Pos2, Vec2};
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pluriview-corrupt-image-{}-{nonce}.png",
            std::process::id()
        ));
        fs::write(&path, b"incomplete PNG").unwrap();
        let (frames, error) = super::load_or_placeholder(&path);
        assert!(frames.is_empty());
        let mut preview = Preview::new(
            PreviewId(1),
            "image".to_owned(),
            Pos2::new(12.0, 34.0),
            Vec2::new(200.0, 150.0),
        );
        preview.set_media(path.clone(), frames);
        preview.set_capture_error(error.unwrap());
        preview.crop_uv = Some((0.1, 0.2, 0.9, 0.8));
        preview.manually_frozen = true;
        let before = PreviewLayout::from(&preview);
        let saved = serde_json::to_string(&before).unwrap();
        let restored: PreviewLayout = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored.media_path, Some(path.clone()));
        assert!(preview.get_texture(&Context::default()).is_none());

        RgbaImage::from_pixel(2, 2, Rgba([255, 0, 0, 255]))
            .save(&path)
            .unwrap();
        let (frames, error) = super::load_or_placeholder(&path);
        assert!(error.is_none());
        preview.set_media(path.clone(), frames);
        assert!(preview.get_texture(&Context::default()).is_some());
        assert!(preview.capture_error.is_none());
        assert_eq!(
            serde_json::to_value(PreviewLayout::from(&preview)).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        fs::remove_file(path).unwrap();
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
        let first_pixel = |frame: &super::MediaFrame| frame.image.pixels[0];
        assert_eq!(
            first_pixel(&asset.frames[0]),
            eframe::egui::Color32::from_rgb(255, 0, 0)
        );
        assert_eq!(
            first_pixel(&asset.frames[1]),
            eframe::egui::Color32::from_rgb(0, 255, 0)
        );

        fs::remove_file(path).unwrap();
    }

    /// The classic 43-byte 1x1 GIF with its logical screen patched.
    fn gif_with_screen(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = vec![
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0xff,
            0xff, 0xff, 0x00, 0x00, 0x00, 0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c,
            0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00,
            0x3b,
        ];
        bytes[6..8].copy_from_slice(&width.to_le_bytes());
        bytes[8..10].copy_from_slice(&height.to_le_bytes());
        bytes
    }

    #[test]
    fn tiny_gif_declaring_a_huge_screen_is_rejected_before_allocating_it() {
        for (width, height) in [(65_535, 65_535), (12_000, 12_000), (9_000, 9_000)] {
            let error = super::decode_gif(std::io::Cursor::new(gif_with_screen(width, height)))
                .err()
                .unwrap();
            assert_eq!(error, "Image is too large to load safely");
        }
        let asset = super::decode_gif(std::io::Cursor::new(gif_with_screen(1, 1))).unwrap();
        assert_eq!((asset.width, asset.height), (1, 1));
    }

    #[test]
    fn gif_wider_than_the_texture_limit_is_scaled_to_fit() {
        let asset = super::decode_gif(std::io::Cursor::new(gif_with_screen(20_000, 1))).unwrap();
        assert_eq!((asset.width, asset.height), (super::MAX_TEXTURE_SIDE, 1));
        assert_eq!(
            asset.frames[0].image.size,
            [super::MAX_TEXTURE_SIDE as usize, 1]
        );
    }

    #[test]
    fn images_beyond_the_texture_limit_keep_their_shape() {
        assert_eq!(
            super::fitted_size(1920, 20_000, 16_384),
            Some((1573, 16_384))
        );
        assert_eq!(super::fitted_size(20_000, 10, 16_384), Some((16_384, 8)));
        assert_eq!(super::fitted_size(100_000, 1, 16_384), Some((16_384, 1)));
        assert_eq!(super::fitted_size(16_384, 16_384, 16_384), None);
        assert_eq!(super::fitted_size(4, 2, 0), Some((1, 1)));

        let tall = RgbaImage::from_pixel(3, 40, Rgba([10, 20, 30, 255]));
        let fitted = super::fit_to_texture_side(tall, 20);
        assert_eq!(fitted.dimensions(), (2, 20));
        assert_eq!(fitted.get_pixel(1, 10), &Rgba([10, 20, 30, 255]));
    }

    #[test]
    fn uploads_fit_a_gpu_with_a_smaller_texture_limit() {
        let frame = super::MediaFrame::from_rgba(40, 10, &[255; 40 * 10 * 4], Duration::ZERO);
        assert_eq!(super::frame_upload(&frame, 16).size, [16, 4]);
        let shared = super::frame_upload(&frame, 16_384);
        assert_eq!(shared.size, [40, 10]);
        assert!(
            std::sync::Arc::ptr_eq(&shared, &frame.image),
            "a frame that fits is uploaded without a copy"
        );
    }
}
