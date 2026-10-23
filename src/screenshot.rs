//! Tile screenshots: crop captured frames, name them, and save PNG files on a
//! worker thread so encoding never stalls the canvas.

use eframe::egui;
use std::fs;
use std::io::{BufWriter, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};

/// Longest tile-title part of a screenshot file name, in characters.
const MAX_TITLE_CHARS: usize = 60;
/// Combined screenshots stay within 8K so a wide spread of tiles cannot
/// allocate an enormous image.
const MAX_COMBINED_SIDE: f32 = 7680.0;
const MAX_COMBINED_PIXELS: f32 = 7680.0 * 4320.0;

/// Straight (non-premultiplied) RGBA pixels ready for PNG encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenshotImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl ScreenshotImage {
    pub fn from_straight_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Option<Self> {
        let expected = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        (width > 0 && height > 0 && rgba.len() == expected).then_some(Self {
            width,
            height,
            rgba,
        })
    }

    /// Capture frames are stored premultiplied for egui; PNG wants straight alpha.
    pub fn from_color_image(image: &egui::ColorImage) -> Option<Self> {
        let [width, height] = image.size;
        let mut rgba = Vec::with_capacity(image.pixels.len() * 4);
        for pixel in &image.pixels {
            if pixel.a() == 255 {
                rgba.extend_from_slice(&[pixel.r(), pixel.g(), pixel.b(), 255]);
            } else {
                rgba.extend_from_slice(&pixel.to_srgba_unmultiplied());
            }
        }
        Self::from_straight_rgba(width as u32, height as u32, rgba)
    }

    /// Keep only the tile's visible crop, matching what the canvas shows.
    pub fn cropped(self, crop_uv: Option<(f32, f32, f32, f32)>) -> Self {
        let (left, top, right, bottom) = crop_pixel_bounds(self.width, self.height, crop_uv);
        if (left, top, right, bottom) == (0, 0, self.width, self.height) {
            return self;
        }
        let row_bytes = (right - left) as usize * 4;
        let mut rgba = Vec::with_capacity(row_bytes * (bottom - top) as usize);
        for y in top..bottom {
            let start = (y as usize * self.width as usize + left as usize) * 4;
            rgba.extend_from_slice(&self.rgba[start..start + row_bytes]);
        }
        Self {
            width: right - left,
            height: bottom - top,
            rgba,
        }
    }
}

/// Pixels handed over by a tile. Capture frames stay shared with the texture
/// upload until the save worker converts them, so the UI thread never copies
/// a full-size frame.
pub enum CapturedPixels {
    Premultiplied(Arc<egui::ColorImage>),
    Straight(ScreenshotImage),
}

impl CapturedPixels {
    pub fn size(&self) -> (u32, u32) {
        match self {
            Self::Premultiplied(image) => (image.size[0] as u32, image.size[1] as u32),
            Self::Straight(image) => (image.width, image.height),
        }
    }

    fn pixel(&self, x: u32, y: u32) -> egui::Color32 {
        match self {
            Self::Premultiplied(image) => image.pixels[y as usize * image.size[0] + x as usize],
            Self::Straight(image) => {
                let offset = (y as usize * image.width as usize + x as usize) * 4;
                let pixel = &image.rgba[offset..offset + 4];
                egui::Color32::from_rgba_unmultiplied(pixel[0], pixel[1], pixel[2], pixel[3])
            }
        }
    }

    pub fn into_image(self) -> Option<ScreenshotImage> {
        match self {
            Self::Premultiplied(image) => ScreenshotImage::from_color_image(&image),
            Self::Straight(image) => Some(image),
        }
    }

    /// Small premultiplied preview of the cropped region for the capture
    /// animation, sampled without converting the full frame.
    pub fn thumbnail(
        &self,
        crop_uv: Option<(f32, f32, f32, f32)>,
        max_side: u32,
    ) -> Option<egui::ColorImage> {
        let (width, height) = self.size();
        if width == 0 || height == 0 {
            return None;
        }
        let (left, top, right, bottom) = crop_pixel_bounds(width, height, crop_uv);
        let (crop_width, crop_height) = (right - left, bottom - top);
        let scale = (max_side.max(1) as f32 / crop_width.max(crop_height) as f32).min(1.0);
        let out_width = ((crop_width as f32 * scale).round() as u32).max(1);
        let out_height = ((crop_height as f32 * scale).round() as u32).max(1);
        let mut pixels = Vec::with_capacity((out_width * out_height) as usize);
        for y in 0..out_height {
            let source_y =
                top + (u64::from(y) * u64::from(crop_height) / u64::from(out_height)) as u32;
            for x in 0..out_width {
                let source_x =
                    left + (u64::from(x) * u64::from(crop_width) / u64::from(out_width)) as u32;
                pixels.push(self.pixel(source_x, source_y));
            }
        }
        Some(egui::ColorImage {
            size: [out_width as usize, out_height as usize],
            pixels,
        })
    }
}

/// One tile in a combined screenshot: its pixels, crop, and where it was
/// shown on screen, in points.
pub struct CombinedPart {
    pub pixels: CapturedPixels,
    pub crop_uv: Option<(f32, f32, f32, f32)>,
    pub rect: egui::Rect,
}

impl CombinedPart {
    /// The visible crop as premultiplied RGBA bytes.
    fn premultiplied_crop(&self) -> image::RgbaImage {
        let (width, height) = self.pixels.size();
        let (left, top, right, bottom) = crop_pixel_bounds(width, height, self.crop_uv);
        let mut rgba = Vec::with_capacity((right - left) as usize * (bottom - top) as usize * 4);
        for y in top..bottom {
            match &self.pixels {
                CapturedPixels::Premultiplied(image) => {
                    let row = y as usize * image.size[0];
                    for pixel in &image.pixels[row + left as usize..row + right as usize] {
                        rgba.extend_from_slice(&pixel.to_array());
                    }
                }
                CapturedPixels::Straight(image) => {
                    let row = y as usize * image.width as usize;
                    let start = (row + left as usize) * 4;
                    let end = (row + right as usize) * 4;
                    for pixel in image.rgba[start..end].chunks_exact(4) {
                        let color = egui::Color32::from_rgba_unmultiplied(
                            pixel[0], pixel[1], pixel[2], pixel[3],
                        );
                        rgba.extend_from_slice(&color.to_array());
                    }
                }
            }
        }
        image::RgbaImage::from_raw(right - left, bottom - top, rgba)
            .expect("crop bounds match the copied rows")
    }
}

/// Lay `parts` (bottom to top) out as they sat on screen, the upper ones
/// hiding what they cover, with transparency between them. The scale keeps
/// the most detailed tile at its full resolution, within the given limits.
/// Returns premultiplied RGBA.
fn compose_premultiplied(
    parts: &[CombinedPart],
    max_side: f32,
    max_pixels: f32,
) -> Option<(u32, u32, Vec<u8>)> {
    let bounds = parts
        .iter()
        .map(|part| part.rect)
        .filter(|rect| rect.is_finite() && rect.is_positive())
        .reduce(|a, b| a.union(b))?;
    let density = parts
        .iter()
        .filter(|part| part.rect.is_positive())
        .map(|part| {
            let (width, height) = part.pixels.size();
            let (left, top, right, bottom) = crop_pixel_bounds(width, height, part.crop_uv);
            ((right - left) as f32 / part.rect.width())
                .max((bottom - top) as f32 / part.rect.height())
        })
        .fold(1.0_f32, f32::max);
    let scale = density
        .min(max_side / bounds.width().max(bounds.height()))
        .min((max_pixels / bounds.area()).sqrt());
    let out_width = ((bounds.width() * scale).round() as u32).max(1);
    let out_height = ((bounds.height() * scale).round() as u32).max(1);
    let mut out = vec![0u8; out_width as usize * out_height as usize * 4];

    for part in parts {
        if !part.rect.is_finite() || !part.rect.is_positive() {
            continue;
        }
        let edge = |value: f32, origin: f32, limit: u32| {
            (((value - origin) * scale).round().max(0.0) as u32).min(limit)
        };
        let left = edge(part.rect.min.x, bounds.min.x, out_width);
        let right = edge(part.rect.max.x, bounds.min.x, out_width);
        let top = edge(part.rect.min.y, bounds.min.y, out_height);
        let bottom = edge(part.rect.max.y, bounds.min.y, out_height);
        if left >= right || top >= bottom {
            continue;
        }
        let crop = part.premultiplied_crop();
        let placed = if crop.dimensions() == (right - left, bottom - top) {
            crop
        } else {
            image::imageops::thumbnail(&crop, right - left, bottom - top)
        };
        let row_bytes = (right - left) as usize * 4;
        for (y, source) in (top..bottom).zip(placed.as_raw().chunks_exact(row_bytes)) {
            let start = (y as usize * out_width as usize + left as usize) * 4;
            for (destination, source) in out[start..start + row_bytes]
                .chunks_exact_mut(4)
                .zip(source.chunks_exact(4))
            {
                let keep = 255 - u16::from(source[3]);
                for channel in 0..4 {
                    let below = (u16::from(destination[channel]) * keep + 127) / 255;
                    destination[channel] = source[channel].saturating_add(below as u8);
                }
            }
        }
    }
    Some((out_width, out_height, out))
}

/// The combined screenshot, with straight alpha for PNG.
pub fn compose(parts: &[CombinedPart]) -> Option<ScreenshotImage> {
    let (width, height, mut rgba) =
        compose_premultiplied(parts, MAX_COMBINED_SIDE, MAX_COMBINED_PIXELS)?;
    for pixel in rgba.chunks_exact_mut(4) {
        if pixel[3] != 255 {
            let color =
                egui::Color32::from_rgba_premultiplied(pixel[0], pixel[1], pixel[2], pixel[3]);
            pixel.copy_from_slice(&color.to_srgba_unmultiplied());
        }
    }
    ScreenshotImage::from_straight_rgba(width, height, rgba)
}

/// Small preview of the combined screenshot for the capture animation,
/// built from cheap per-tile samples rather than the full frames.
pub fn combined_thumbnail(parts: &[CombinedPart], max_side: u32) -> Option<egui::ColorImage> {
    let samples: Vec<CombinedPart> = parts
        .iter()
        .filter_map(|part| {
            Some(CombinedPart {
                pixels: CapturedPixels::Premultiplied(Arc::new(
                    part.pixels.thumbnail(part.crop_uv, max_side)?,
                )),
                crop_uv: None,
                rect: part.rect,
            })
        })
        .collect();
    let (width, height, rgba) = compose_premultiplied(&samples, max_side as f32, f32::INFINITY)?;
    Some(egui::ColorImage::from_rgba_premultiplied(
        [width as usize, height as usize],
        &rgba,
    ))
}

/// Pixel rectangle `(left, top, right, bottom)` covered by a normalized crop.
/// Invalid crops fall back to the full frame; valid ones keep at least 1 px.
pub(crate) fn crop_pixel_bounds(
    width: u32,
    height: u32,
    crop_uv: Option<(f32, f32, f32, f32)>,
) -> (u32, u32, u32, u32) {
    let full = (0, 0, width, height);
    let Some((min_u, min_v, max_u, max_v)) = crop_uv else {
        return full;
    };
    let values = [min_u, min_v, max_u, max_v];
    if values.iter().any(|value| !value.is_finite()) || min_u >= max_u || min_v >= max_v {
        return full;
    }
    let edge = |value: f32, size: u32| (value.clamp(0.0, 1.0) * size as f32).round() as u32;
    let left = edge(min_u, width).min(width.saturating_sub(1));
    let top = edge(min_v, height).min(height.saturating_sub(1));
    let right = edge(max_u, width).clamp(left + 1, width);
    let bottom = edge(max_v, height).clamp(top + 1, height);
    (left, top, right, bottom)
}

/// Wall-clock time used in screenshot names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTimestamp {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl LocalTimestamp {
    #[cfg(windows)]
    pub fn now() -> Self {
        let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        Self {
            year: time.wYear,
            month: time.wMonth as u8,
            day: time.wDay as u8,
            hour: time.wHour as u8,
            minute: time.wMinute as u8,
            second: time.wSecond as u8,
        }
    }

    #[cfg(not(windows))]
    pub fn now() -> Self {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default();
        let days = (seconds / 86_400) as i64;
        let of_day = seconds % 86_400;
        // Civil-from-days (Howard Hinnant), UTC.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
        let year = (yoe + era * 400 + i64::from(month <= 2)) as u16;
        Self {
            year,
            month,
            day,
            hour: (of_day / 3_600) as u8,
            minute: (of_day / 60 % 60) as u8,
            second: (of_day % 60) as u8,
        }
    }
}

/// `"<tile title> 2026-10-18 19-04-05"`, safe as a Windows file name.
pub fn file_stem(title: &str, time: LocalTimestamp) -> String {
    let mut name = String::new();
    for character in title.chars() {
        let invalid = character.is_control() || r#"<>:"/\|?*"#.contains(character);
        let character = if invalid { ' ' } else { character };
        if character.is_whitespace() && (name.is_empty() || name.ends_with(' ')) {
            continue;
        }
        name.push(if character.is_whitespace() {
            ' '
        } else {
            character
        });
    }
    let mut name: String = name.chars().take(MAX_TITLE_CHARS).collect();
    // Windows drops trailing dots and spaces from file names.
    while name.ends_with(['.', ' ']) {
        name.pop();
    }
    if name.is_empty() {
        name.push_str("Tile");
    }
    format!(
        "{name} {:04}-{:02}-{:02} {:02}-{:02}-{:02}",
        time.year, time.month, time.day, time.hour, time.minute, time.second
    )
}

fn candidate_path(directory: &Path, stem: &str, attempt: u32) -> PathBuf {
    if attempt <= 1 {
        directory.join(format!("{stem}.png"))
    } else {
        directory.join(format!("{stem} ({attempt}).png"))
    }
}

/// Encode a PNG under a name that does not replace an existing screenshot.
pub fn save_png(directory: &Path, stem: &str, image: &ScreenshotImage) -> Result<PathBuf, String> {
    use image::ImageEncoder as _;

    fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create the screenshots folder: {error}"))?;
    for attempt in 1..=999 {
        let path = candidate_path(directory, stem, attempt);
        let file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Could not create the screenshot file: {error}")),
        };
        let result = image::codecs::png::PngEncoder::new(BufWriter::new(file)).write_image(
            &image.rgba,
            image.width,
            image.height,
            image::ExtendedColorType::Rgba8,
        );
        return match result {
            Ok(()) => Ok(path),
            Err(error) => {
                let _ = fs::remove_file(&path);
                Err(format!("Could not save the screenshot: {error}"))
            }
        };
    }
    Err("Could not find a free screenshot file name".to_owned())
}

/// Convert, crop and save on a worker thread; the receiver gets the final
/// path or an error.
pub fn spawn_save(
    directory: PathBuf,
    stem: String,
    pixels: CapturedPixels,
    crop_uv: Option<(f32, f32, f32, f32)>,
) -> mpsc::Receiver<Result<PathBuf, String>> {
    spawn_worker(move || {
        pixels
            .into_image()
            .ok_or_else(|| "The tile's frame was empty".to_owned())
            .and_then(|image| save_png(&directory, &stem, &image.cropped(crop_uv)))
    })
}

/// Compose several tiles into one screenshot and save it on a worker thread.
pub fn spawn_save_combined(
    directory: PathBuf,
    stem: String,
    parts: Vec<CombinedPart>,
) -> mpsc::Receiver<Result<PathBuf, String>> {
    spawn_worker(move || {
        compose(&parts)
            .ok_or_else(|| "The tiles had nothing to capture".to_owned())
            .and_then(|image| save_png(&directory, &stem, &image))
    })
}

fn spawn_worker(
    save: impl FnOnce() -> Result<PathBuf, String> + Send + 'static,
) -> mpsc::Receiver<Result<PathBuf, String>> {
    let (sender, receiver) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("pluriview-screenshot".to_owned())
        .spawn(move || {
            let _ = sender.send(save());
        });
    if let Err(error) = spawned {
        let (sender, failed) = mpsc::channel();
        let _ = sender.send(Err(format!(
            "Could not start saving the screenshot: {error}"
        )));
        return failed;
    }
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time() -> LocalTimestamp {
        LocalTimestamp {
            year: 2026,
            month: 10,
            day: 8,
            hour: 7,
            minute: 5,
            second: 9,
        }
    }

    fn numbered(width: u32, height: u32) -> ScreenshotImage {
        let rgba = (0..width * height)
            .flat_map(|index| [index as u8, 0, 0, 255])
            .collect();
        ScreenshotImage::from_straight_rgba(width, height, rgba).unwrap()
    }

    #[test]
    fn rejects_mismatched_pixel_buffers() {
        assert!(ScreenshotImage::from_straight_rgba(2, 2, vec![0; 15]).is_none());
        assert!(ScreenshotImage::from_straight_rgba(0, 2, Vec::new()).is_none());
    }

    #[test]
    fn crop_keeps_the_visible_region() {
        let image = numbered(4, 4).cropped(Some((0.25, 0.5, 0.75, 1.0)));
        assert_eq!((image.width, image.height), (2, 2));
        let reds: Vec<u8> = image.rgba.chunks(4).map(|pixel| pixel[0]).collect();
        assert_eq!(reds, vec![9, 10, 13, 14]);
    }

    #[test]
    fn invalid_or_missing_crop_keeps_the_full_frame() {
        assert_eq!(crop_pixel_bounds(8, 6, None), (0, 0, 8, 6));
        assert_eq!(
            crop_pixel_bounds(8, 6, Some((0.5, 0.0, 0.5, 1.0))),
            (0, 0, 8, 6)
        );
        assert_eq!(
            crop_pixel_bounds(8, 6, Some((f32::NAN, 0.0, 1.0, 1.0))),
            (0, 0, 8, 6)
        );
    }

    #[test]
    fn tiny_crops_keep_at_least_one_pixel() {
        assert_eq!(
            crop_pixel_bounds(10, 10, Some((0.999, 0.999, 1.0, 1.0))),
            (9, 9, 10, 10)
        );
    }

    #[test]
    fn premultiplied_frames_are_saved_with_straight_alpha() {
        let image = egui::ColorImage {
            size: [2, 1],
            pixels: vec![
                egui::Color32::from_rgba_premultiplied(10, 20, 30, 255),
                egui::Color32::from_rgba_unmultiplied(200, 100, 50, 128),
            ],
        };
        let image = ScreenshotImage::from_color_image(&image).unwrap();
        assert_eq!(&image.rgba[..4], &[10, 20, 30, 255]);
        let half = &image.rgba[4..];
        assert_eq!(half[3], 128);
        assert!((i16::from(half[0]) - 200).abs() <= 2);
        assert!((i16::from(half[1]) - 100).abs() <= 2);
    }

    #[test]
    fn file_names_are_windows_safe() {
        assert_eq!(
            file_stem("YouTube: \"Live\" <now>?", time()),
            "YouTube Live now 2026-10-08 07-05-09"
        );
        assert_eq!(file_stem("  ...  ", time()), "Tile 2026-10-08 07-05-09");
        assert_eq!(file_stem("Video.", time()), "Video 2026-10-08 07-05-09");
        let long = "x".repeat(200);
        let stem = file_stem(&long, time());
        assert_eq!(stem.len(), MAX_TITLE_CHARS + " 2026-10-08 07-05-09".len());
    }

    #[test]
    fn thumbnails_fit_the_requested_size_and_crop() {
        let pixels = CapturedPixels::Straight(numbered(400, 200));
        assert_eq!(pixels.thumbnail(None, 100).unwrap().size, [100, 50]);
        let cropped = pixels.thumbnail(Some((0.0, 0.0, 0.25, 1.0)), 100).unwrap();
        assert_eq!(cropped.size, [50, 100]);
        let small = CapturedPixels::Straight(numbered(20, 10));
        assert_eq!(small.thumbnail(None, 100).unwrap().size, [20, 10]);
    }

    #[test]
    fn premultiplied_capture_frames_save_through_the_worker() {
        let frame = Arc::new(egui::ColorImage::new(
            [4, 2],
            egui::Color32::from_rgb(1, 2, 3),
        ));
        let directory = std::env::temp_dir().join(format!(
            "pluriview-screenshot-worker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let receiver = spawn_save(
            directory.clone(),
            "Frame".to_owned(),
            CapturedPixels::Premultiplied(frame),
            Some((0.5, 0.0, 1.0, 1.0)),
        );
        let path = receiver.recv().unwrap().unwrap();
        let decoded = image::open(&path).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (2, 2));
        assert_eq!(&decoded.into_raw()[..4], &[1, 2, 3, 255]);
        let _ = fs::remove_dir_all(&directory);
    }

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> CapturedPixels {
        let pixels = rgba.repeat((width * height) as usize);
        CapturedPixels::Straight(
            ScreenshotImage::from_straight_rgba(width, height, pixels).unwrap(),
        )
    }

    fn part(pixels: CapturedPixels, min: (f32, f32), max: (f32, f32)) -> CombinedPart {
        CombinedPart {
            pixels,
            crop_uv: None,
            rect: egui::Rect::from_min_max(egui::pos2(min.0, min.1), egui::pos2(max.0, max.1)),
        }
    }

    fn pixel_at(image: &ScreenshotImage, x: u32, y: u32) -> [u8; 4] {
        let offset = ((y * image.width + x) * 4) as usize;
        image.rgba[offset..offset + 4].try_into().unwrap()
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    #[test]
    fn combined_tiles_keep_their_screen_layout_with_clear_gaps() {
        let image = compose(&[
            part(solid(4, 2, RED), (100.0, 50.0), (104.0, 52.0)),
            part(solid(2, 2, BLUE), (106.0, 51.0), (108.0, 53.0)),
        ])
        .unwrap();
        assert_eq!((image.width, image.height), (8, 3));
        assert_eq!(pixel_at(&image, 0, 0), RED);
        assert_eq!(pixel_at(&image, 3, 1), RED);
        assert_eq!(pixel_at(&image, 5, 1)[3], 0);
        assert_eq!(pixel_at(&image, 0, 2)[3], 0);
        assert_eq!(pixel_at(&image, 6, 1), BLUE);
        assert_eq!(pixel_at(&image, 7, 2), BLUE);
        assert_eq!(pixel_at(&image, 6, 0)[3], 0);
    }

    #[test]
    fn upper_tiles_hide_the_parts_they_cover() {
        let image = compose(&[
            part(solid(4, 4, RED), (0.0, 0.0), (4.0, 4.0)),
            part(solid(2, 2, BLUE), (2.0, 2.0), (4.0, 4.0)),
        ])
        .unwrap();
        assert_eq!(pixel_at(&image, 1, 1), RED);
        assert_eq!(pixel_at(&image, 3, 3), BLUE);
        assert_eq!(pixel_at(&image, 2, 1), RED);
    }

    #[test]
    fn transparent_tiles_blend_over_the_ones_below() {
        let image = compose(&[
            part(solid(2, 1, RED), (0.0, 0.0), (2.0, 1.0)),
            part(solid(1, 1, [0, 0, 255, 128]), (1.0, 0.0), (2.0, 1.0)),
        ])
        .unwrap();
        let blended = pixel_at(&image, 1, 0);
        assert_eq!(blended[3], 255);
        assert!(blended[0] > 0 && blended[2] > 0);
    }

    #[test]
    fn the_most_detailed_tile_keeps_its_resolution() {
        // 8x4 pixels shown in 4x2 points: two pixels per point.
        let image = compose(&[
            part(solid(8, 4, RED), (0.0, 0.0), (4.0, 2.0)),
            part(solid(1, 1, BLUE), (4.0, 0.0), (6.0, 2.0)),
        ])
        .unwrap();
        assert_eq!((image.width, image.height), (12, 4));
        assert_eq!(pixel_at(&image, 11, 3), BLUE);
    }

    #[test]
    fn combined_tiles_show_only_their_crop() {
        let mut left = part(solid(4, 2, BLUE), (0.0, 0.0), (2.0, 2.0));
        let mut rgba = BLUE.repeat(8);
        rgba[8..16].copy_from_slice(&RED.repeat(2));
        rgba[24..32].copy_from_slice(&RED.repeat(2));
        left.pixels =
            CapturedPixels::Straight(ScreenshotImage::from_straight_rgba(4, 2, rgba).unwrap());
        left.crop_uv = Some((0.5, 0.0, 1.0, 1.0));
        let image = compose(&[left]).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert!(image.rgba.chunks(4).all(|pixel| pixel == RED));
    }

    #[test]
    fn combined_screenshots_stay_within_8k() {
        let image = compose(&[
            part(solid(4, 4, RED), (0.0, 0.0), (1.0, 1.0)),
            part(solid(1, 1, BLUE), (20_000.0, 0.0), (20_001.0, 1.0)),
        ])
        .unwrap();
        assert_eq!(image.width, 7680);
        assert!(image.height <= 1);
    }

    #[test]
    fn combined_thumbnails_fit_the_requested_size() {
        let parts = [
            part(solid(400, 200, RED), (0.0, 0.0), (400.0, 200.0)),
            part(solid(400, 200, BLUE), (400.0, 0.0), (800.0, 200.0)),
        ];
        let thumbnail = combined_thumbnail(&parts, 100).unwrap();
        assert_eq!(thumbnail.size, [100, 25]);
        assert_eq!(thumbnail.pixels[0], egui::Color32::RED);
        assert_eq!(thumbnail.pixels[99], egui::Color32::BLUE);
    }

    #[test]
    fn saving_never_replaces_an_existing_screenshot() {
        let directory = std::env::temp_dir().join(format!(
            "pluriview-screenshot-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let image = numbered(3, 2);
        let first = save_png(&directory, "Shot", &image).unwrap();
        let second = save_png(&directory, "Shot", &image).unwrap();
        assert_eq!(first.file_name().unwrap(), "Shot.png");
        assert_eq!(second.file_name().unwrap(), "Shot (2).png");
        let decoded = image::open(&second).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (3, 2));
        assert_eq!(decoded.into_raw(), image.rgba);
        let _ = fs::remove_dir_all(&directory);
    }
}
