//! Tile recordings: H.264 + AAC in MP4 through Windows' own Media Foundation
//! sink writer, so no codec ships with Pluriview. The UI thread only hands
//! over frames it already holds; conversion, audio capture, encoding and file
//! writing all run on one worker per recording.

use crate::screenshot::CapturedPixels;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Frames per second a recording keeps at most. Unchanged sources simply
/// hold their last frame, which MP4 timestamps express without duplicates.
pub const MAX_FPS: u32 = 30;
/// Largest recorded picture; bigger sources are scaled down to fit.
const MAX_SIZE: (u32, u32) = (3840, 2160);

pub struct Frame {
    pub pixels: CapturedPixels,
    pub crop_uv: Option<(f32, f32, f32, f32)>,
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
    /// Start recording into `path`. `audio_pid` is the process tree whose
    /// sound is recorded alongside the picture, if any.
    pub fn start(path: PathBuf, audio_pid: Option<u32>, started: Instant) -> Self {
        // Small queue: when encoding falls behind, the UI drops frames
        // instead of holding full-size copies.
        let (commands, receiver) = mpsc::sync_channel(4);
        let (done, finished) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("pluriview-recording".to_owned())
            .spawn(move || {
                let result = run(&path, audio_pid, started, &receiver);
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
    let scale = (MAX_SIZE.0 as f64 / width.max(1) as f64)
        .min(MAX_SIZE.1 as f64 / height.max(1) as f64)
        .min(1.0);
    let even = |value: u32| ((value as f64 * scale) as u32 & !1).max(2);
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

/// Draw the cropped frame into a top-down BGRX picture of `width`×`height`,
/// over black. A source that changed shape mid-recording is fitted inside
/// with bars rather than stretched.
fn draw_frame(frame: &Frame, width: u32, height: u32, out: &mut [u8]) {
    let (source, source_width, source_height, premultiplied) = rgba_bytes(&frame.pixels);
    let (left, top, right, bottom) =
        crate::screenshot::crop_pixel_bounds(source_width, source_height, frame.crop_uv);
    let (crop_width, crop_height) = (right - left, bottom - top);
    let scale = (width as f32 / crop_width as f32).min(height as f32 / crop_height as f32);
    let draw_width = ((crop_width as f32 * scale).round() as u32).clamp(1, width);
    let draw_height = ((crop_height as f32 * scale).round() as u32).clamp(1, height);
    let (offset_x, offset_y) = ((width - draw_width) / 2, (height - draw_height) / 2);
    if (draw_width, draw_height) != (width, height) {
        out.fill(0);
    }
    let columns: Vec<usize> = (0..draw_width)
        .map(|x| {
            let source_x =
                left + (u64::from(x) * u64::from(crop_width) / u64::from(draw_width)) as u32;
            source_x as usize * 4
        })
        .collect();
    let row_bytes = width as usize * 4;
    for y in 0..draw_height {
        let source_y =
            top + (u64::from(y) * u64::from(crop_height) / u64::from(draw_height)) as u32;
        let source_row = &source[source_y as usize * source_width as usize * 4..];
        let start = (offset_y + y) as usize * row_bytes + offset_x as usize * 4;
        let target = &mut out[start..start + draw_width as usize * 4];
        for (pixel, &column) in target.chunks_exact_mut(4).zip(&columns) {
            let [r, g, b, a] = [
                source_row[column],
                source_row[column + 1],
                source_row[column + 2],
                source_row[column + 3],
            ];
            // Transparent areas record as black, like the canvas behind them.
            let (r, g, b) = if premultiplied || a == 255 {
                (r, g, b)
            } else {
                let over_black = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
                (over_black(r), over_black(g), over_black(b))
            };
            pixel.copy_from_slice(&[b, g, r, 255]);
        }
    }
}

#[cfg(not(windows))]
fn run(
    _path: &std::path::Path,
    _audio_pid: Option<u32>,
    _started: Instant,
    _commands: &mpsc::Receiver<Command>,
) -> Result<(), String> {
    Err("Recording is only available on Windows.".to_owned())
}

#[cfg(windows)]
fn run(
    path: &std::path::Path,
    audio_pid: Option<u32>,
    started: Instant,
    commands: &mpsc::Receiver<Command>,
) -> Result<(), String> {
    mf::run(path, audio_pid, started, commands)
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

    /// Stereo audio kept in step with the recording clock: silence fills
    /// stretches where the source played nothing, so sound never drifts
    /// ahead of the picture.
    struct AudioTrack {
        capture: TileAudioCapture,
        scratch: Vec<f32>,
        pending: Vec<i16>,
        /// Stereo frames handed to the writer so far.
        written: i64,
    }

    impl AudioTrack {
        fn start(pid: u32) -> Option<Self> {
            let deadline = Instant::now() + Duration::from_secs(3);
            match TileAudioCapture::start(pid, &|| Instant::now() > deadline) {
                Ok(capture) => Some(Self {
                    capture,
                    scratch: Vec::new(),
                    pending: Vec::new(),
                    written: 0,
                }),
                Err(error) => {
                    log::warn!("Recording without sound: {error}");
                    None
                }
            }
        }

        /// Read what the source played; keep it only while not paused.
        fn read(&mut self, keep: bool) {
            while let Ok(true) = self.capture.read(&mut self.scratch) {
                if keep {
                    self.pending.extend(
                        self.scratch
                            .iter()
                            .map(|sample| (sample.clamp(-1.0, 1.0) * 32767.0) as i16),
                    );
                }
            }
        }

        /// Write whole AAC chunks up to `media_time`, padding with silence
        /// when the source was quiet and trimming any backlog beyond it.
        unsafe fn flush(&mut self, writer: &mut Writer, media_time: Duration, finish: bool) {
            let due = (media_time.as_nanos() * u128::from(AUDIO_RATE) / 1_000_000_000) as i64;
            let queued = self.written + (self.pending.len() / 2) as i64;
            // Allow ~100 ms of capture latency before inserting silence.
            let slack = i64::from(AUDIO_RATE) / 10;
            if queued < due - slack || (finish && queued < due) {
                let missing = (due - queued).max(0) as usize;
                self.pending
                    .splice(0..0, std::iter::repeat_n(0, missing * 2));
            } else if queued > due + 2 * slack {
                let extra = ((queued - due - slack) as usize * 2).min(self.pending.len());
                self.pending.drain(..extra);
            }
            let chunk = AUDIO_CHUNK * 2;
            while self.pending.len() >= chunk || (finish && !self.pending.is_empty()) {
                let take = chunk.min(self.pending.len());
                let time = self.written * HNS_PER_SECOND / i64::from(AUDIO_RATE);
                if let Err(error) = writer.audio_chunk(time, &self.pending[..take]) {
                    log::warn!("Recording audio write failed: {error}");
                }
                self.written += (take / 2) as i64;
                self.pending.drain(..take);
            }
        }
    }

    pub(super) fn run(
        path: &std::path::Path,
        audio_pid: Option<u32>,
        started: Instant,
        commands: &mpsc::Receiver<Command>,
    ) -> Result<(), String> {
        let runtime = Runtime::new().map_err(|_| unavailable())?;
        let mut audio = audio_pid.and_then(AudioTrack::start);
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
                        let (width, height) = frame.pixels.size();
                        let (left, top, right, bottom) =
                            crate::screenshot::crop_pixel_bounds(width, height, frame.crop_uv);
                        let (width, height) = output_size(right - left, bottom - top);
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
            pixels: CapturedPixels::Straight(image.clone()),
            crop_uv,
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
        assert_eq!(&out[8..], &[0; 8]);
    }

    /// Seconds in an MP4's movie header.
    fn mp4_duration(bytes: &[u8]) -> f64 {
        let at = bytes.windows(4).position(|w| w == b"mvhd").unwrap() + 4;
        let field = |offset: usize, len: usize| {
            bytes[at + offset..at + offset + len]
                .iter()
                .fold(0u64, |value, byte| value << 8 | u64::from(*byte))
        };
        let (timescale, duration) = if bytes[at] == 1 {
            (field(20, 4), field(24, 8))
        } else {
            (field(12, 4), field(16, 4))
        };
        duration as f64 / timescale as f64
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
        let recorder = Recorder::start(path.clone(), Some(std::process::id()), start);
        let frame = |shade: u8, at: Instant| Frame {
            pixels: CapturedPixels::Straight(
                ScreenshotImage::from_straight_rgba(
                    641,
                    361,
                    [shade, 64, 255 - shade, 255].repeat(641 * 361),
                )
                .unwrap(),
            ),
            crop_uv: None,
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
        std::fs::remove_file(&path).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp");
        assert!(bytes.windows(4).any(|w| w == b"mp4a"), "no audio track");
        let seconds = mp4_duration(&bytes);
        assert!((1.8..2.4).contains(&seconds), "recorded {seconds} s");
    }
}
