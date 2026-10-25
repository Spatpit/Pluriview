//! Tile recordings: H.264 + AAC in MP4 through Windows' own Media Foundation
//! sink writer, so no codec ships with Pluriview. The UI thread only hands
//! over frames it already holds; conversion, audio capture, encoding and file
//! writing all run on one worker per recording.

use crate::screenshot::CapturedPixels;
use eframe::egui;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// Frames per second a recording keeps at most. Unchanged sources simply
/// hold their last frame, which MP4 timestamps express without duplicates.
pub const MAX_FPS: u32 = 30;
/// Largest recorded picture; bigger sources are scaled down to fit.
const MAX_SIZE: (u32, u32) = (3840, 2160);
/// Several tiles recorded together fit inside 1080p, which keeps putting
/// them together cheap enough to do 30 times a second.
pub const MAX_COMBINED_SIZE: (u32, u32) = (1920, 1080);

/// One tile's picture within a recorded frame.
pub struct Layer {
    pub pixels: Arc<CapturedPixels>,
    pub crop_uv: Option<(f32, f32, f32, f32)>,
    /// Where the tile sits in the video, as fractions of the frame. `None`
    /// fills the frame (a single tile), keeping its shape between bars.
    pub place: Option<egui::Rect>,
}

/// Layers are drawn bottom to top, each hiding what it covers.
pub struct Frame {
    pub layers: Vec<Layer>,
    pub at: Instant,
}

enum Command {
    Frame(Frame),
    Pause(Instant),
    Resume(Instant),
    Stop(Instant),
}

/// A running recording. Dropping it stops and finishes the file.
pub struct Recorder {
    commands: mpsc::SyncSender<Command>,
    worker: Option<std::thread::JoinHandle<()>>,
    finished: Option<mpsc::Receiver<Result<PathBuf, String>>>,
}

impl Recorder {
    /// Start recording into `path`. `audio_pids` are the process trees whose
    /// sound is mixed into the recording. `size` fixes the video's size;
    /// without it the first frame's single tile decides.
    pub fn start(
        path: PathBuf,
        audio_pids: Vec<u32>,
        size: Option<(u32, u32)>,
        started: Instant,
    ) -> Self {
        // Small queue: when encoding falls behind, the UI drops frames
        // instead of holding full-size copies.
        let (commands, receiver) = mpsc::sync_channel(4);
        let (done, finished) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("pluriview-recording".to_owned())
            .spawn(move || {
                let result = run(&path, &audio_pids, size, started, &receiver);
                if result.is_err() {
                    let _ = std::fs::remove_file(&path);
                }
                let _ = done.send(result.map(|()| path));
            })
            .ok();
        Self {
            commands,
            worker,
            finished: Some(finished),
        }
    }

    /// Queue a frame; returns false when the encoder is still busy.
    pub fn push(&self, frame: Frame) -> bool {
        self.commands.try_send(Command::Frame(frame)).is_ok()
    }

    pub fn pause(&self, at: Instant) {
        let _ = self.commands.send(Command::Pause(at));
    }

    pub fn resume(&self, at: Instant) {
        let _ = self.commands.send(Command::Resume(at));
    }

    /// Ask the worker to finish the file. The result arrives on the returned
    /// receiver once the MP4 is complete.
    pub fn stop(mut self) -> mpsc::Receiver<Result<PathBuf, String>> {
        let _ = self.commands.send(Command::Stop(Instant::now()));
        // The worker finishes on its own; never block the UI thread on it.
        self.worker = None;
        self.finished.take().unwrap_or_else(|| mpsc::channel().1)
    }

    /// A worker that failed early (codec missing, disk full) reports here
    /// while the recording still looks active.
    pub fn failure(&mut self) -> Option<String> {
        match self.finished.as_ref()?.try_recv() {
            Ok(Err(error)) => Some(error),
            Ok(Ok(_)) | Err(mpsc::TryRecvError::Disconnected) => {
                Some("The recording stopped unexpectedly.".to_owned())
            }
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }
}

impl Drop for Recorder {
    /// Closing Pluriview mid-recording still leaves a playable file.
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = self.commands.send(Command::Stop(Instant::now()));
            let _ = worker.join();
        }
    }
}

/// Recording clock: wall time since the start minus every paused stretch.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    started: Instant,
    paused_total: Duration,
    paused_since: Option<Instant>,
}

impl Clock {
    pub fn new(started: Instant) -> Self {
        Self {
            started,
            paused_total: Duration::ZERO,
            paused_since: None,
        }
    }

    pub fn paused(&self) -> bool {
        self.paused_since.is_some()
    }

    pub fn pause(&mut self, at: Instant) {
        self.paused_since.get_or_insert(at);
    }

    pub fn resume(&mut self, at: Instant) {
        if let Some(since) = self.paused_since.take() {
            self.paused_total += at.saturating_duration_since(since);
        }
    }

    /// Recorded time at `at`; frozen while paused.
    pub fn media_time(&self, at: Instant) -> Duration {
        let at = self.paused_since.map_or(at, |since| since.min(at));
        at.saturating_duration_since(self.started)
            .saturating_sub(self.paused_total)
    }
}

/// Even output size for a source of `width`×`height`, fitted inside
/// `MAX_SIZE` without changing its shape. H.264 needs even dimensions.
fn output_size(width: u32, height: u32) -> (u32, u32) {
    fitted_size(width as f32, height as f32, MAX_SIZE)
}

/// Even size with the shape of `width`×`height`, no larger than it or
/// `limit`.
pub fn fitted_size(width: f32, height: f32, limit: (u32, u32)) -> (u32, u32) {
    let (width, height) = (width.max(1.0) as f64, height.max(1.0) as f64);
    let scale = (limit.0 as f64 / width)
        .min(limit.1 as f64 / height)
        .min(1.0);
    let even = |value: f64| ((value * scale) as u32 & !1).max(2);
    (even(width), even(height))
}

/// Source pixels as RGBA bytes plus whether they are premultiplied.
fn rgba_bytes(pixels: &CapturedPixels) -> (&[u8], u32, u32, bool) {
    match pixels {
        CapturedPixels::Premultiplied(image) => {
            // Color32 is a #[repr(C)] [u8; 4] in RGBA order.
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    image.pixels.as_ptr().cast::<u8>(),
                    image.pixels.len() * 4,
                )
            };
            (bytes, image.size[0] as u32, image.size[1] as u32, true)
        }
        CapturedPixels::Straight(image) => (&image.rgba, image.width, image.height, false),
    }
}

/// Draw the frame's layers into a top-down BGRX picture of
/// `width`×`height`, over black. A single tile that changed shape
/// mid-recording is fitted inside with bars rather than stretched.
fn draw_frame(frame: &Frame, width: u32, height: u32, out: &mut [u8]) {
    // One tile filling the whole picture is drawn straight over black,
    // without clearing the buffer first.
    let covers_all = |layer: &Layer| layer.place.is_none() && !has_bars(layer, width, height);
    let over_black = frame.layers.len() == 1 && covers_all(&frame.layers[0]);
    if !over_black {
        for pixel in out.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[0, 0, 0, 255]);
        }
    }
    for layer in &frame.layers {
        draw_layer(layer, width, height, over_black, out);
    }
}

fn crop_size(layer: &Layer) -> (u32, u32, u32, u32) {
    let (width, height) = layer.pixels.size();
    crate::screenshot::crop_pixel_bounds(width, height, layer.crop_uv)
}

/// Pixel rectangle `(left, top, right, bottom)` the layer covers. A tile
/// moved partly outside a combined recording reaches past the picture and
/// is clipped when drawn.
fn layer_target(layer: &Layer, width: u32, height: u32) -> (i64, i64, i64, i64) {
    match layer.place {
        Some(place) => {
            let edge = |value: f32, size: u32| (value * size as f32).round() as i64;
            (
                edge(place.min.x, width),
                edge(place.min.y, height),
                edge(place.max.x, width),
                edge(place.max.y, height),
            )
        }
        None => {
            let (left, top, right, bottom) = crop_size(layer);
            let (crop_width, crop_height) = ((right - left) as f32, (bottom - top) as f32);
            let scale = (width as f32 / crop_width).min(height as f32 / crop_height);
            let draw_width = ((crop_width * scale).round() as u32).clamp(1, width);
            let draw_height = ((crop_height * scale).round() as u32).clamp(1, height);
            let (x, y) = ((width - draw_width) / 2, (height - draw_height) / 2);
            (
                i64::from(x),
                i64::from(y),
                i64::from(x + draw_width),
                i64::from(y + draw_height),
            )
        }
    }
}

fn has_bars(layer: &Layer, width: u32, height: u32) -> bool {
    layer_target(layer, width, height) != (0, 0, i64::from(width), i64::from(height))
}

fn draw_layer(layer: &Layer, width: u32, height: u32, over_black: bool, out: &mut [u8]) {
    let (source, source_width, _, premultiplied) = rgba_bytes(&layer.pixels);
    let (left, top, right, bottom) = crop_size(layer);
    let (crop_width, crop_height) = (right - left, bottom - top);
    let (x0, y0, x1, y1) = layer_target(layer, width, height);
    let (draw_width, draw_height) = (x1 - x0, y1 - y0);
    // Only the part inside the picture is drawn.
    let (clip_x0, clip_x1) = (x0.max(0), x1.min(i64::from(width)));
    let (clip_y0, clip_y1) = (y0.max(0), y1.min(i64::from(height)));
    if draw_width <= 0 || draw_height <= 0 || clip_x0 >= clip_x1 || clip_y0 >= clip_y1 {
        return;
    }
    let source_at = |offset: i64, crop: u32, draw: i64| (offset * i64::from(crop) / draw) as u32;
    let columns: Vec<usize> = (clip_x0..clip_x1)
        .map(|x| (left + source_at(x - x0, crop_width, draw_width)) as usize * 4)
        .collect();
    let row_bytes = width as usize * 4;
    for y in clip_y0..clip_y1 {
        let source_y = top + source_at(y - y0, crop_height, draw_height);
        let source_row = &source[source_y as usize * source_width as usize * 4..];
        let start = y as usize * row_bytes + clip_x0 as usize * 4;
        let target = &mut out[start..start + columns.len() * 4];
        for (pixel, &column) in target.chunks_exact_mut(4).zip(&columns) {
            let [r, g, b, a] = [
                source_row[column],
                source_row[column + 1],
                source_row[column + 2],
                source_row[column + 3],
            ];
            if a == 255 {
                pixel.copy_from_slice(&[b, g, r, 255]);
                continue;
            }
            // Transparent areas show what is below: another tile or black.
            let scale = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
            let (r, g, b) = if premultiplied {
                (r, g, b)
            } else {
                (scale(r), scale(g), scale(b))
            };
            let keep = if over_black { 0 } else { 255 - u16::from(a) };
            for (channel, value) in pixel[..3].iter_mut().zip([b, g, r]) {
                let below = ((u16::from(*channel) * keep + 127) / 255) as u8;
                *channel = value.saturating_add(below);
            }
            pixel[3] = 255;
        }
    }
}

/// Length stored in an MP4's movie header, found by walking box headers
/// so only a few bytes are read even for long recordings.
pub fn mp4_duration(path: &std::path::Path) -> Option<Duration> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let (mut start, mut end) = (0, file.metadata().ok()?.len());
    for wanted in [b"moov", b"mvhd"] {
        let mut at = start;
        let mut found = None;
        while at + 8 <= end {
            file.seek(SeekFrom::Start(at)).ok()?;
            let mut header = [0u8; 16];
            file.read_exact(&mut header[..8]).ok()?;
            let mut size = u64::from(u32::from_be_bytes(header[..4].try_into().ok()?));
            let mut header_len = 8;
            if size == 1 {
                file.read_exact(&mut header[8..]).ok()?;
                size = u64::from_be_bytes(header[8..].try_into().ok()?);
                header_len = 16;
            } else if size == 0 {
                size = end - at;
            }
            if size < header_len {
                return None;
            }
            if &header[4..8] == wanted {
                found = Some((at + header_len, at + size));
                break;
            }
            at += size;
        }
        (start, end) = found?;
    }
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut body = [0u8; 32];
    file.read_exact(&mut body).ok()?;
    let word = |at: usize| {
        u64::from(u32::from_be_bytes([
            body[at],
            body[at + 1],
            body[at + 2],
            body[at + 3],
        ]))
    };
    let (timescale, duration) = if body[0] == 1 {
        (word(20), u64::from_be_bytes(body[24..32].try_into().ok()?))
    } else {
        (word(12), word(16))
    };
    (timescale > 0).then(|| Duration::from_secs_f64(duration as f64 / timescale as f64))
}

/// First picture of a video file, scaled to fit `max_side`, for the
/// Screenshots panel. Decoded by Windows, like recordings are encoded.
#[cfg(windows)]
pub fn video_thumbnail(path: &std::path::Path, max_side: u32) -> Option<eframe::egui::ColorImage> {
    mf::thumbnail(path, max_side)
}

#[cfg(not(windows))]
pub fn video_thumbnail(
    _path: &std::path::Path,
    _max_side: u32,
) -> Option<eframe::egui::ColorImage> {
    None
}

#[cfg(not(windows))]
fn run(
    _path: &std::path::Path,
    _audio_pids: &[u32],
    _size: Option<(u32, u32)>,
    _started: Instant,
    _commands: &mpsc::Receiver<Command>,
) -> Result<(), String> {
    Err("Recording is only available on Windows.".to_owned())
}

#[cfg(windows)]
fn run(
    path: &std::path::Path,
    audio_pids: &[u32],
    size: Option<(u32, u32)>,
    started: Instant,
    commands: &mpsc::Receiver<Command>,
) -> Result<(), String> {
    mf::run(path, audio_pids, size, started, commands)
}

#[cfg(windows)]
mod mf {
    use super::*;
    use crate::audio::TileAudioCapture;
    use crate::remote_view::video::Runtime;
    use std::ffi::c_void;
    use std::rc::Rc;
    use windows::core::{GUID, HRESULT, HSTRING, PCWSTR};
    use windows::Win32::Media::MediaFoundation::*;
    use windows_core::Interface;

    const AUDIO_RATE: u32 = 48_000;
    /// AAC-LC frames: 1024 stereo samples per written audio sample.
    const AUDIO_CHUNK: usize = 1024;
    const HNS_PER_SECOND: i64 = 10_000_000;

    type CreateAttributes = unsafe extern "system" fn(*mut *mut c_void, u32) -> HRESULT;
    type CreateSinkWriter =
        unsafe extern "system" fn(PCWSTR, *mut c_void, *mut c_void, *mut *mut c_void) -> HRESULT;

    fn hns(time: Duration) -> i64 {
        (time.as_nanos() / 100) as i64
    }

    struct Writer {
        writer: IMFSinkWriter,
        video: u32,
        audio: Option<u32>,
        width: u32,
        height: u32,
        last_video: i64,
        // Declared last so COM objects drop before MFShutdown.
        runtime: Rc<Runtime>,
        _library: libloading::Library,
    }

    impl Writer {
        unsafe fn open(
            runtime: &Rc<Runtime>,
            path: &std::path::Path,
            width: u32,
            height: u32,
            audio: bool,
        ) -> Result<Self, String> {
            // Loaded on demand: Windows N without the Media Feature Pack lacks
            // it, and LOAD_LIBRARY_SEARCH_SYSTEM32 ignores app-folder copies.
            let library: libloading::Library =
                libloading::os::windows::Library::load_with_flags("mfreadwrite.dll", 0x800)
                    .map_err(|_| unavailable())?
                    .into();
            let create_writer = *library
                .get::<CreateSinkWriter>(b"MFCreateSinkWriterFromURL\0")
                .map_err(|_| unavailable())?;
            let mfplat: libloading::Library =
                libloading::os::windows::Library::load_with_flags("mfplat.dll", 0x800)
                    .map_err(|_| unavailable())?
                    .into();
            let create_attributes = *mfplat
                .get::<CreateAttributes>(b"MFCreateAttributes\0")
                .map_err(|_| unavailable())?;

            // Prefer the GPU encoder; some drivers refuse system-memory
            // input, so retry once with Windows' software encoder.
            let mut last_error = String::new();
            for hardware in [true, false] {
                match Self::create(
                    runtime,
                    path,
                    width,
                    height,
                    audio,
                    hardware,
                    create_attributes,
                    create_writer,
                ) {
                    Ok((writer, video, audio)) => {
                        return Ok(Self {
                            writer,
                            video,
                            audio,
                            width,
                            height,
                            last_video: -1,
                            runtime: runtime.clone(),
                            _library: library,
                        })
                    }
                    Err(error) => last_error = error.message(),
                }
            }
            Err(format!(
                "Windows couldn't start the video encoder: {last_error}"
            ))
        }

        #[allow(clippy::too_many_arguments)]
        unsafe fn create(
            runtime: &Rc<Runtime>,
            path: &std::path::Path,
            width: u32,
            height: u32,
            audio: bool,
            hardware: bool,
            create_attributes: CreateAttributes,
            create_writer: CreateSinkWriter,
        ) -> windows::core::Result<(IMFSinkWriter, u32, Option<u32>)> {
            let mut raw = std::ptr::null_mut();
            create_attributes(&mut raw, 2).ok()?;
            let attributes = IMFAttributes::from_raw(raw);
            attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, hardware as u32)?;
            attributes.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
            let url = HSTRING::from(path.as_os_str());
            let mut raw = std::ptr::null_mut();
            create_writer(
                PCWSTR(url.as_ptr()),
                std::ptr::null_mut(),
                attributes.as_raw(),
                &mut raw,
            )
            .ok()?;
            let writer = IMFSinkWriter::from_raw(raw);

            let video_type = |subtype: GUID| -> windows::core::Result<IMFMediaType> {
                let ty = runtime.media_type()?;
                ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                ty.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
                ty.SetUINT64(
                    &MF_MT_FRAME_SIZE,
                    (u64::from(width) << 32) | u64::from(height),
                )?;
                ty.SetUINT64(&MF_MT_FRAME_RATE, (u64::from(MAX_FPS) << 32) | 1)?;
                ty.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
                ty.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
                Ok(ty)
            };
            let output = video_type(MFVideoFormat_H264)?;
            // About 0.1 bit per pixel per frame: ~6 Mbps at 1080p30.
            let bitrate = (f64::from(width) * f64::from(height) * f64::from(MAX_FPS) * 0.1)
                .clamp(2_000_000.0, 40_000_000.0) as u32;
            output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
            output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
            let video = writer.AddStream(&output)?;
            let input = video_type(MFVideoFormat_RGB32)?;
            // Positive stride: rows are top-down, as drawn.
            input.SetUINT32(&MF_MT_DEFAULT_STRIDE, width * 4)?;
            writer.SetInputMediaType(video, &input, None)?;

            let audio = if audio {
                let audio_type = |subtype: GUID| -> windows::core::Result<IMFMediaType> {
                    let ty = runtime.media_type()?;
                    ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                    ty.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
                    ty.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
                    ty.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE)?;
                    ty.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                    Ok(ty)
                };
                let output = audio_type(MFAudioFormat_AAC)?;
                // 24000 bytes/s = 192 kbps, the AAC encoder's highest rate.
                output.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000)?;
                let stream = writer.AddStream(&output)?;
                let input = audio_type(MFAudioFormat_PCM)?;
                input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
                input.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AUDIO_RATE * 4)?;
                writer.SetInputMediaType(stream, &input, None)?;
                Some(stream)
            } else {
                None
            };
            writer.BeginWriting()?;
            Ok((writer, video, audio))
        }

        unsafe fn write(
            &mut self,
            stream: u32,
            time: i64,
            duration: i64,
            length: u32,
            fill: impl FnOnce(&mut [u8]),
        ) -> windows::core::Result<()> {
            let buffer = self.runtime.buffer(length)?;
            let mut data = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None)?;
            fill(std::slice::from_raw_parts_mut(data, length as usize));
            buffer.Unlock()?;
            buffer.SetCurrentLength(length)?;
            let sample = self.runtime.sample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(time)?;
            sample.SetSampleDuration(duration)?;
            self.writer.WriteSample(stream, &sample)
        }

        unsafe fn video_frame(&mut self, time: i64, frame: &Frame) -> windows::core::Result<()> {
            // Sample times must keep increasing even if frames arrive close.
            let time = time.max(self.last_video + 1);
            self.last_video = time;
            let (width, height) = (self.width, self.height);
            self.write(
                self.video,
                time,
                HNS_PER_SECOND / i64::from(MAX_FPS),
                width * height * 4,
                |out| draw_frame(frame, width, height, out),
            )
        }

        unsafe fn audio_chunk(&mut self, time: i64, samples: &[i16]) -> windows::core::Result<()> {
            let Some(stream) = self.audio else {
                return Ok(());
            };
            let frames = (samples.len() / 2) as i64;
            self.write(
                stream,
                time,
                frames * HNS_PER_SECOND / i64::from(AUDIO_RATE),
                (samples.len() * 2) as u32,
                |out| {
                    for (target, sample) in out.chunks_exact_mut(2).zip(samples) {
                        target.copy_from_slice(&sample.to_le_bytes());
                    }
                },
            )
        }
    }

    fn unavailable() -> String {
        "Recording needs Windows' media components (install the Media Feature Pack on Windows N)."
            .to_owned()
    }

    /// One recorded tile's sound, waiting to be mixed.
    struct AudioSource {
        capture: TileAudioCapture,
        /// Interleaved stereo samples.
        queue: std::collections::VecDeque<f32>,
    }

    /// Stereo audio mixed from every recorded source and kept in step with
    /// the recording clock: a source that played nothing adds silence, so
    /// sound never drifts away from the picture.
    struct AudioTrack {
        sources: Vec<AudioSource>,
        scratch: Vec<f32>,
        mixed: Vec<f32>,
        samples: Vec<i16>,
        /// Stereo frames handed to the writer so far.
        written: i64,
    }

    impl AudioTrack {
        fn start(pids: &[u32]) -> Option<Self> {
            let deadline = Instant::now() + Duration::from_secs(3);
            let sources: Vec<AudioSource> = pids
                .iter()
                .filter_map(|&pid| {
                    match TileAudioCapture::start(pid, &|| Instant::now() > deadline) {
                        Ok(capture) => Some(AudioSource {
                            capture,
                            queue: Default::default(),
                        }),
                        Err(error) => {
                            log::warn!("Recording one source without sound: {error}");
                            None
                        }
                    }
                })
                .collect();
            (!sources.is_empty()).then(|| Self {
                sources,
                scratch: Vec::new(),
                mixed: Vec::new(),
                samples: Vec::new(),
                written: 0,
            })
        }

        /// Read what each source played; keep it only while not paused.
        fn read(&mut self, keep: bool) {
            for source in &mut self.sources {
                while let Ok(true) = source.capture.read(&mut self.scratch) {
                    if keep {
                        source.queue.extend(self.scratch.iter().copied());
                    }
                }
            }
        }

        /// Write mixed AAC chunks up to `media_time`, less ~100 ms of
        /// capture latency until the end, dropping any backlog beyond it.
        unsafe fn flush(&mut self, writer: &mut Writer, media_time: Duration, finish: bool) {
            let due = (media_time.as_nanos() * u128::from(AUDIO_RATE) / 1_000_000_000) as i64;
            let slack = i64::from(AUDIO_RATE) / 10;
            let target = if finish { due } else { due - slack };
            for source in &mut self.sources {
                let ahead = self.written + (source.queue.len() / 2) as i64 - (due + slack);
                if ahead > 0 {
                    let extra = (ahead as usize * 2).min(source.queue.len());
                    source.queue.drain(..extra);
                }
            }
            while self.written + AUDIO_CHUNK as i64 <= target || (finish && self.written < target) {
                let frames = (AUDIO_CHUNK as i64).min(target - self.written) as usize;
                self.mixed.clear();
                self.mixed.resize(frames * 2, 0.0);
                for source in &mut self.sources {
                    let take = (frames * 2).min(source.queue.len());
                    for (mixed, sample) in self.mixed.iter_mut().zip(source.queue.drain(..take)) {
                        *mixed += sample;
                    }
                }
                self.samples.clear();
                self.samples.extend(
                    self.mixed
                        .iter()
                        .map(|sample| (sample.clamp(-1.0, 1.0) * 32767.0) as i16),
                );
                let time = self.written * HNS_PER_SECOND / i64::from(AUDIO_RATE);
                if let Err(error) = writer.audio_chunk(time, &self.samples) {
                    log::warn!("Recording audio write failed: {error}");
                }
                self.written += frames as i64;
            }
        }
    }

    pub(super) fn thumbnail(
        path: &std::path::Path,
        max_side: u32,
    ) -> Option<eframe::egui::ColorImage> {
        type CreateReader =
            unsafe extern "system" fn(PCWSTR, *mut c_void, *mut *mut c_void) -> HRESULT;
        unsafe {
            let runtime = Runtime::new().ok()?;
            let library: libloading::Library =
                libloading::os::windows::Library::load_with_flags("mfreadwrite.dll", 0x800)
                    .ok()?
                    .into();
            let create_reader = *library
                .get::<CreateReader>(b"MFCreateSourceReaderFromURL\0")
                .ok()?;
            let mfplat: libloading::Library =
                libloading::os::windows::Library::load_with_flags("mfplat.dll", 0x800)
                    .ok()?
                    .into();
            let create_attributes = *mfplat
                .get::<CreateAttributes>(b"MFCreateAttributes\0")
                .ok()?;
            let mut raw = std::ptr::null_mut();
            create_attributes(&mut raw, 1).ok().ok()?;
            let attributes = IMFAttributes::from_raw(raw);
            // Let Windows convert the decoded picture to RGB for us.
            attributes
                .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
                .ok()?;
            let url = HSTRING::from(path.as_os_str());
            let mut raw = std::ptr::null_mut();
            create_reader(PCWSTR(url.as_ptr()), attributes.as_raw(), &mut raw)
                .ok()
                .ok()?;
            let reader = IMFSourceReader::from_raw(raw);
            let video = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .ok()?;
            reader.SetStreamSelection(video, true).ok()?;
            let wanted = runtime.media_type().ok()?;
            wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).ok()?;
            wanted.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).ok()?;
            reader.SetCurrentMediaType(video, None, &wanted).ok()?;
            let current = reader.GetCurrentMediaType(video).ok()?;
            let size = current.GetUINT64(&MF_MT_FRAME_SIZE).ok()?;
            let (width, height) = ((size >> 32) as usize, (size & 0xffff_ffff) as usize);
            let stride = current
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .map_or(width as isize * 4, |stride| stride as i32 as isize);
            let mut sample = None;
            for _ in 0..8 {
                let mut flags = 0;
                reader
                    .ReadSample(video, 0, None, Some(&mut flags), None, Some(&mut sample))
                    .ok()?;
                if sample.is_some() || flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
            }
            let buffer = sample?.ConvertToContiguousBuffer().ok()?;
            let mut data = std::ptr::null_mut();
            let mut length = 0;
            buffer.Lock(&mut data, None, Some(&mut length)).ok()?;
            let image = (width > 0
                && height > 0
                && stride.unsigned_abs() >= width * 4
                && length as usize >= stride.unsigned_abs() * height)
                .then(|| {
                    let bytes = std::slice::from_raw_parts(data, length as usize);
                    let scale = (max_side as f32 / width.max(height) as f32).min(1.0);
                    let out_width = ((width as f32 * scale) as usize).max(1);
                    let out_height = ((height as f32 * scale) as usize).max(1);
                    // A negative stride means the rows are stored bottom-up.
                    let row_start = |y: usize| {
                        if stride < 0 {
                            (height - 1 - y) * stride.unsigned_abs()
                        } else {
                            y * stride as usize
                        }
                    };
                    let pixels = (0..out_height)
                        .flat_map(|y| {
                            let row = row_start(y * height / out_height);
                            (0..out_width).map(move |x| {
                                let at = row + x * width / out_width * 4;
                                eframe::egui::Color32::from_rgb(
                                    bytes[at + 2],
                                    bytes[at + 1],
                                    bytes[at],
                                )
                            })
                        })
                        .collect();
                    eframe::egui::ColorImage {
                        size: [out_width, out_height],
                        pixels,
                    }
                });
            let _ = buffer.Unlock();
            image
        }
    }

    pub(super) fn run(
        path: &std::path::Path,
        audio_pids: &[u32],
        size: Option<(u32, u32)>,
        started: Instant,
        commands: &mpsc::Receiver<Command>,
    ) -> Result<(), String> {
        let runtime = Runtime::new().map_err(|_| unavailable())?;
        let mut audio = AudioTrack::start(audio_pids);
        let mut clock = Clock::new(started);
        let mut writer: Option<Writer> = None;
        let mut last_frame: Option<Frame> = None;
        let result = loop {
            let command = match commands.recv_timeout(Duration::from_millis(15)) {
                Ok(command) => Some(command),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                // The UI side vanished without Stop: finish what we have.
                Err(mpsc::RecvTimeoutError::Disconnected) => Some(Command::Stop(Instant::now())),
            };
            if let Some(track) = audio.as_mut() {
                track.read(writer.is_some() && !clock.paused());
            }
            match command {
                Some(Command::Frame(frame)) if !clock.paused() => {
                    if writer.is_none() {
                        let (width, height) = size.unwrap_or_else(|| {
                            frame.layers.first().map_or((2, 2), |layer| {
                                let (left, top, right, bottom) = crop_size(layer);
                                output_size(right - left, bottom - top)
                            })
                        });
                        match unsafe {
                            Writer::open(&runtime, path, width, height, audio.is_some())
                        } {
                            Ok(opened) => writer = Some(opened),
                            Err(error) => break Err(error),
                        }
                    }
                    if let Some(writer) = writer.as_mut() {
                        let time = hns(clock.media_time(frame.at));
                        if let Err(error) = unsafe { writer.video_frame(time, &frame) } {
                            break Err(format!("Writing the recording failed: {error}"));
                        }
                    }
                    last_frame = Some(frame);
                }
                Some(Command::Frame(_)) => {}
                Some(Command::Pause(at)) => clock.pause(at),
                Some(Command::Resume(at)) => clock.resume(at),
                Some(Command::Stop(at)) => {
                    let Some(writer) = writer.as_mut() else {
                        break Err("The tile didn't show anything to record.".to_owned());
                    };
                    let end = clock.media_time(at);
                    // Hold the final picture until the moment Stop was pressed.
                    if let Some(frame) = &last_frame {
                        let _ = unsafe { writer.video_frame(hns(end), frame) };
                    }
                    if let Some(track) = audio.as_mut() {
                        unsafe { track.flush(writer, end, true) };
                    }
                    break unsafe { writer.writer.Finalize() }
                        .map_err(|error| format!("Finishing the recording failed: {error}"));
                }
                None => {}
            }
            if let (Some(track), Some(writer)) = (audio.as_mut(), writer.as_mut()) {
                if !clock.paused() {
                    unsafe { track.flush(writer, clock.media_time(Instant::now()), false) };
                }
            }
        };
        // Capture and sink-writer COM objects go before the runtime.
        drop(audio);
        drop(writer);
        drop(runtime);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screenshot::ScreenshotImage;

    #[test]
    fn paused_time_is_left_out_of_the_recording() {
        let start = Instant::now();
        let mut clock = Clock::new(start);
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        assert_eq!(clock.media_time(at(2)), Duration::from_secs(2));
        clock.pause(at(2));
        assert_eq!(clock.media_time(at(9)), Duration::from_secs(2));
        clock.resume(at(10));
        assert_eq!(clock.media_time(at(13)), Duration::from_secs(5));
        clock.pause(at(13));
        clock.pause(at(14));
        clock.resume(at(15));
        assert_eq!(clock.media_time(at(16)), Duration::from_secs(6));
    }

    #[test]
    fn output_is_even_and_fits_4k() {
        assert_eq!(output_size(1921, 1081), (1920, 1080));
        assert_eq!(output_size(7680, 4320), (3840, 2160));
        assert_eq!(output_size(1, 1), (2, 2));
    }

    #[test]
    fn frames_are_cropped_to_bgr_and_letterboxed_when_reshaped() {
        // 2x1 source: red | half-transparent white (straight alpha).
        let image =
            ScreenshotImage::from_straight_rgba(2, 1, vec![255, 0, 0, 255, 255, 255, 255, 128])
                .unwrap();
        let frame = |crop_uv| Frame {
            layers: vec![Layer {
                pixels: Arc::new(CapturedPixels::Straight(image.clone())),
                crop_uv,
                place: None,
            }],
            at: Instant::now(),
        };
        let mut out = vec![9; 4];
        draw_frame(&frame(Some((0.0, 0.0, 0.5, 1.0))), 1, 1, &mut out);
        assert_eq!(out, [0, 0, 255, 255]);
        draw_frame(&frame(Some((0.5, 0.0, 1.0, 1.0))), 1, 1, &mut out);
        assert_eq!(out, [128, 128, 128, 255]);

        // A wide source drawn into a square keeps its shape between bars.
        let mut out = vec![9; 2 * 2 * 4];
        draw_frame(&frame(None), 2, 2, &mut out);
        assert_eq!(&out[..8], &[0, 0, 255, 255, 128, 128, 128, 255]);
        assert_eq!(&out[8..], &[0, 0, 0, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn combined_tiles_sit_where_placed_and_cover_lower_ones() {
        let solid = |rgba: [u8; 4]| {
            Arc::new(CapturedPixels::Straight(
                ScreenshotImage::from_straight_rgba(1, 1, rgba.to_vec()).unwrap(),
            ))
        };
        let layer = |pixels, min: (f32, f32), max: (f32, f32)| Layer {
            pixels,
            crop_uv: None,
            place: Some(egui::Rect::from_min_max(min.into(), max.into())),
        };
        let frame = Frame {
            layers: vec![
                // Red on the left half, then half-transparent white over
                // the middle two pixels.
                layer(solid([255, 0, 0, 255]), (0.0, 0.0), (0.5, 1.0)),
                layer(solid([255, 255, 255, 128]), (0.25, 0.0), (0.75, 1.0)),
            ],
            at: Instant::now(),
        };
        let mut out = vec![9; 4 * 4];
        draw_frame(&frame, 4, 1, &mut out);
        assert_eq!(&out[..4], &[0, 0, 255, 255]);
        assert_eq!(&out[4..8], &[128, 128, 255, 255]);
        assert_eq!(&out[8..12], &[128, 128, 128, 255]);
        // Nothing placed here: black.
        assert_eq!(&out[12..], &[0, 0, 0, 255]);
    }

    #[test]
    fn tiles_moved_past_the_edge_are_clipped_not_squeezed() {
        // Red | blue source, placed half outside on the left.
        let pixels = Arc::new(CapturedPixels::Straight(
            ScreenshotImage::from_straight_rgba(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255])
                .unwrap(),
        ));
        let frame = Frame {
            layers: vec![Layer {
                pixels,
                crop_uv: None,
                place: Some(egui::Rect::from_min_max(
                    egui::pos2(-0.5, 0.0),
                    egui::pos2(0.5, 1.0),
                )),
            }],
            at: Instant::now(),
        };
        let mut out = vec![9; 2 * 4];
        draw_frame(&frame, 2, 1, &mut out);
        // Only the blue half is still inside.
        assert_eq!(out, [255, 0, 0, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn combined_size_keeps_the_shape_inside_1080p() {
        assert_eq!(fitted_size(3000.0, 1000.0, MAX_COMBINED_SIZE), (1920, 640));
        assert_eq!(fitted_size(801.0, 601.0, MAX_COMBINED_SIZE), (800, 600));
    }

    /// Encodes through the real Windows encoder; run with `--ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn records_an_mp4_without_the_paused_stretch() {
        let path = std::env::temp_dir().join(format!(
            "pluriview-recording-test-{}.mp4",
            std::process::id()
        ));
        let start = Instant::now();
        // Our own silent process: exercises the AAC track and its padding.
        let recorder = Recorder::start(path.clone(), vec![std::process::id()], None, start);
        let frame = |shade: u8, at: Instant| Frame {
            layers: vec![Layer {
                pixels: Arc::new(CapturedPixels::Straight(
                    ScreenshotImage::from_straight_rgba(
                        641,
                        361,
                        [shade, 64, 255 - shade, 255].repeat(641 * 361),
                    )
                    .unwrap(),
                )),
                crop_uv: None,
                place: None,
            }],
            at,
        };
        let tick = Duration::from_millis(1000 / u64::from(MAX_FPS));
        let send = |from: u32, to: u32| {
            for index in from..to {
                let at = start + tick * index;
                while Instant::now() < at {
                    std::thread::sleep(Duration::from_millis(2));
                }
                while !recorder.push(frame(index as u8, at)) {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        };
        send(0, 30);
        recorder.pause(start + tick * 30);
        std::thread::sleep(Duration::from_millis(800));
        recorder.resume(start + tick * 30 + Duration::from_millis(800));
        send(54, 84);
        let result = recorder.stop().recv().unwrap();
        let path = result.unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let seconds = mp4_duration(&path).unwrap().as_secs_f64();
        let thumbnail = video_thumbnail(&path, 160).expect("first frame decodes");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(thumbnail.size, [160, 90]);
        // The first frame is shade 0: no red, full blue.
        let pixel = thumbnail.pixels[90 / 2 * 160 + 80];
        assert!(pixel.r() < 40 && pixel.b() > 200, "{pixel:?}");
        assert_eq!(&bytes[4..8], b"ftyp");
        assert!(bytes.windows(4).any(|w| w == b"mp4a"), "no audio track");
        assert!((1.8..2.4).contains(&seconds), "recorded {seconds} s");
    }

    /// Two tiles side by side through the real encoder; run with `--ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn records_tiles_together_side_by_side() {
        let path = std::env::temp_dir().join(format!(
            "pluriview-combined-recording-test-{}.mp4",
            std::process::id()
        ));
        let start = Instant::now();
        let recorder = Recorder::start(path.clone(), Vec::new(), Some((640, 360)), start);
        let solid = |rgba: [u8; 4]| {
            Arc::new(CapturedPixels::Straight(
                ScreenshotImage::from_straight_rgba(32, 32, rgba.repeat(32 * 32)).unwrap(),
            ))
        };
        let (red, blue) = (solid([255, 0, 0, 255]), solid([0, 0, 255, 255]));
        let tick = Duration::from_millis(1000 / u64::from(MAX_FPS));
        for index in 0..15 {
            let layer = |pixels: &Arc<CapturedPixels>, left: f32| Layer {
                pixels: pixels.clone(),
                crop_uv: None,
                place: Some(egui::Rect::from_min_max(
                    egui::pos2(left, 0.0),
                    egui::pos2(left + 0.5, 1.0),
                )),
            };
            let frame = || Frame {
                layers: vec![layer(&red, 0.0), layer(&blue, 0.5)],
                at: start + tick * index,
            };
            while !recorder.push(frame()) {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        let path = recorder.stop().recv().unwrap().unwrap();
        let thumbnail = video_thumbnail(&path, 160).expect("first frame decodes");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(thumbnail.size, [160, 90]);
        let left = thumbnail.pixels[45 * 160 + 20];
        let right = thumbnail.pixels[45 * 160 + 140];
        assert!(left.r() > 200 && left.b() < 60, "{left:?}");
        assert!(right.b() > 200 && right.r() < 60, "{right:?}");
    }
}
