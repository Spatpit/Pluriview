//! Windows' built-in low-latency H.264 encoder. All COM objects stay on the
//! frame worker; no codec initialization or encoding runs on the UI thread.
use std::collections::VecDeque;
use std::ffi::c_void;
use std::rc::Rc;
use std::time::{Duration, Instant};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;
use windows_core::{Interface, Result, GUID, HRESULT, VARIANT};

type CreateObject = unsafe extern "system" fn(*mut *mut c_void) -> HRESULT;
type CreateBuffer = unsafe extern "system" fn(u32, *mut *mut c_void) -> HRESULT;
type Enumerate = unsafe extern "system" fn(
    GUID,
    u32,
    *const MFT_REGISTER_TYPE_INFO,
    *const MFT_REGISTER_TYPE_INFO,
    *mut *mut Option<IMFActivate>,
    *mut u32,
) -> HRESULT;

pub struct Runtime {
    _library: libloading::Library,
    shutdown: unsafe extern "system" fn() -> HRESULT,
    create_type: CreateObject,
    create_sample: CreateObject,
    create_buffer: CreateBuffer,
    enumerate: Enumerate,
}
impl Runtime {
    pub fn new() -> Result<Rc<Self>> {
        unsafe {
            // Optional on Windows N: loading dynamically also keeps a missing
            // Media Feature Pack from preventing the desktop app from starting.
            // LOAD_LIBRARY_SEARCH_SYSTEM32 excludes app/current-directory DLLs.
            let library: libloading::Library =
                libloading::os::windows::Library::load_with_flags("mfplat.dll", 0x800)
                    .map_err(|_| windows_core::Error::from_hresult(MF_E_TOPO_CODEC_NOT_FOUND))?
                    .into();
            let unavailable = |_| windows_core::Error::from_hresult(MF_E_TOPO_CODEC_NOT_FOUND);
            let startup = *library
                .get::<unsafe extern "system" fn(u32, u32) -> HRESULT>(b"MFStartup\0")
                .map_err(unavailable)?;
            let shutdown = *library.get(b"MFShutdown\0").map_err(unavailable)?;
            let create_type = *library.get(b"MFCreateMediaType\0").map_err(unavailable)?;
            let create_sample = *library.get(b"MFCreateSample\0").map_err(unavailable)?;
            let create_buffer = *library
                .get(b"MFCreateMemoryBuffer\0")
                .map_err(unavailable)?;
            let enumerate = *library.get(b"MFTEnumEx\0").map_err(unavailable)?;
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            if let Err(error) = startup(MF_VERSION, MFSTARTUP_LITE).ok() {
                CoUninitialize();
                return Err(error);
            }
            Ok(Rc::new(Self {
                _library: library,
                shutdown,
                create_type,
                create_sample,
                create_buffer,
                enumerate,
            }))
        }
    }

    pub(super) unsafe fn media_type(&self) -> Result<IMFMediaType> {
        let mut raw = std::ptr::null_mut();
        (self.create_type)(&mut raw).ok()?;
        Ok(IMFMediaType::from_raw(raw))
    }
    pub(super) unsafe fn sample(&self) -> Result<IMFSample> {
        let mut raw = std::ptr::null_mut();
        (self.create_sample)(&mut raw).ok()?;
        Ok(IMFSample::from_raw(raw))
    }
    pub(super) unsafe fn buffer(&self, length: u32) -> Result<IMFMediaBuffer> {
        let mut raw = std::ptr::null_mut();
        (self.create_buffer)(length, &mut raw).ok()?;
        Ok(IMFMediaBuffer::from_raw(raw))
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe {
            let _ = (self.shutdown)();
            CoUninitialize();
        }
    }
}

pub struct Encoder {
    transform: IMFTransform,
    width: usize,
    height: usize,
    timestamp: i64,
    header: Vec<u8>,
    yuv: Vec<u8>,
    output_sample: Option<IMFSample>,
    events: Option<IMFMediaEventGenerator>,
    input_credit: u32,
    output_credit: u32,
    pending: VecDeque<Instant>,
    emitted: bool,
    last_output: Instant,
    fps: u32,
    pub hardware: bool,
    // Declared last so COM samples/transforms drop before MFShutdown/unload.
    runtime: Rc<Runtime>,
}

pub struct Packet {
    pub bytes: Vec<u8>,
    pub first: bool,
    pub created: Instant,
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            if let Ok(shutdown) = self.transform.cast::<IMFShutdown>() {
                let _ = shutdown.Shutdown();
            }
        }
    }
}

impl Encoder {
    #[cfg(test)]
    pub fn new(runtime: &Rc<Runtime>, width: usize, height: usize) -> Result<Self> {
        Self::with_settings(runtime, width, height, 4_000_000, 30, true)
    }

    pub fn with_settings(
        runtime: &Rc<Runtime>,
        width: usize,
        height: usize,
        bitrate: u32,
        fps: u32,
        hardware: bool,
    ) -> Result<Self> {
        if hardware {
            if let Ok(encoder) = Self::enumerate(runtime, width, height, bitrate, fps, true) {
                return Ok(encoder);
            }
        }
        Self::enumerate(runtime, width, height, bitrate, fps, false)
    }

    fn enumerate(
        runtime: &Rc<Runtime>,
        width: usize,
        height: usize,
        bitrate: u32,
        fps: u32,
        hardware: bool,
    ) -> Result<Self> {
        unsafe {
            let output = MFT_REGISTER_TYPE_INFO {
                guidMajorType: MFMediaType_Video,
                guidSubtype: MFVideoFormat_H264,
            };
            let mut activations = std::ptr::null_mut();
            let mut count = 0;
            (runtime.enumerate)(
                MFT_CATEGORY_VIDEO_ENCODER,
                ((if hardware {
                    MFT_ENUM_FLAG_HARDWARE
                } else {
                    MFT_ENUM_FLAG_SYNCMFT
                }) | MFT_ENUM_FLAG_SORTANDFILTER)
                    .0 as u32,
                std::ptr::null(),
                &output,
                &mut activations,
                &mut count,
            )
            .ok()?;
            let mut candidates = Vec::new();
            if !activations.is_null() {
                for i in 0..count as usize {
                    if let Some(activation) = (*activations.add(i)).take() {
                        candidates.push(activation);
                    }
                }
                CoTaskMemFree(Some(activations.cast()));
            }
            let mut last_error = windows_core::Error::from_hresult(MF_E_TOPO_CODEC_NOT_FOUND);
            for activation in candidates {
                match activation.ActivateObject::<IMFTransform>().and_then(|t| {
                    Self::configure(runtime.clone(), t, width, height, bitrate, fps, hardware)
                }) {
                    Ok(encoder) => return Ok(encoder),
                    Err(error) => {
                        // A partially configured async MFT still owns driver resources.
                        let _ = activation.ShutdownObject();
                        last_error = error;
                    }
                }
            }
            Err(last_error)
        }
    }

    unsafe fn configure(
        runtime: Rc<Runtime>,
        transform: IMFTransform,
        width: usize,
        height: usize,
        bitrate: u32,
        fps: u32,
        hardware: bool,
    ) -> Result<Self> {
        let attributes = transform.GetAttributes()?;
        let asynchronous = attributes.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0;
        if asynchronous {
            attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
        }
        let _ = attributes.SetUINT32(&MF_LOW_LATENCY, 1);
        let events = if asynchronous {
            Some(transform.cast::<IMFMediaEventGenerator>()?)
        } else {
            None
        };
        if let Ok(codec) = transform.cast::<ICodecAPI>() {
            let _ = codec.SetValue(&CODECAPI_AVLowLatencyMode, &VARIANT::from(true));
            let _ = codec.SetValue(&CODECAPI_AVEncCommonLowLatency, &VARIANT::from(true));
            let _ = codec.SetValue(&CODECAPI_AVEncMPVDefaultBPictureCount, &VARIANT::from(0u32));
            let _ = codec.SetValue(&CODECAPI_AVEncMPVGOPSize, &VARIANT::from(fps));
            let _ = codec.SetValue(&CODECAPI_AVEncCommonRateControlMode, &VARIANT::from(0u32));
        }
        let media_type = |subtype| -> Result<IMFMediaType> {
            let ty = runtime.media_type()?;
            ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            ty.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
            ty.SetUINT64(&MF_MT_FRAME_SIZE, ((width as u64) << 32) | height as u64)?;
            ty.SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1)?;
            ty.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
            ty.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            Ok(ty)
        };
        let output = media_type(MFVideoFormat_H264)?;
        output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
        output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32)?;
        transform.SetOutputType(0, &output, 0)?;
        let input = media_type(MFVideoFormat_NV12)?;
        transform.SetInputType(0, &input, 0)?;
        transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
        transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        let output = transform.GetOutputCurrentType(0)?;
        let mut header =
            vec![0; output.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER).unwrap_or(0) as usize];
        if !header.is_empty() {
            output.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut header, None)?;
        }
        let info = transform.GetOutputStreamInfo(0)?;
        let output_sample = if info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 == 0 {
            let sample = runtime.sample()?;
            sample.AddBuffer(&runtime.buffer(info.cbSize.max((width * height * 3) as u32))?)?;
            Some(sample)
        } else {
            None
        };
        Ok(Self {
            transform,
            width,
            height,
            timestamp: 0,
            header,
            yuv: vec![0; width * height * 3 / 2],
            output_sample,
            events,
            input_credit: 0,
            output_credit: 0,
            pending: VecDeque::with_capacity(3),
            emitted: false,
            last_output: Instant::now(),
            fps,
            hardware,
            runtime,
        })
    }

    unsafe fn poll_events(&mut self) -> Result<()> {
        if let Some(events) = &self.events {
            for _ in 0..16 {
                match events.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                    Ok(event) => {
                        event.GetStatus()?.ok()?;
                        match event.GetType()? {
                            kind if kind == METransformNeedInput.0 as u32 => {
                                self.input_credit = (self.input_credit + 1).min(8)
                            }
                            kind if kind == METransformHaveOutput.0 as u32 => {
                                self.output_credit = (self.output_credit + 1).min(8)
                            }
                            _ => {}
                        }
                    }
                    Err(error) if error.code() == MF_E_NO_EVENTS_AVAILABLE => break,
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(())
    }

    pub fn encode(&mut self, rgb: &[u8]) -> Result<Option<Packet>> {
        unsafe {
            self.poll_events()?;
            // Also catch drivers that stop issuing NeedInput after their last
            // output. Active viewing keeps calling encode; pauses reset the encoder.
            if self.last_output.elapsed() > Duration::from_millis(500) {
                return Err(windows_core::Error::from_hresult(MF_E_NOTACCEPTING));
            }
            if self
                .pending
                .front()
                .is_some_and(|time| time.elapsed() > Duration::from_millis(500))
            {
                return Err(windows_core::Error::from_hresult(MF_E_NOTACCEPTING));
            }
            if self.pending.len() < 3 && (self.events.is_none() || self.input_credit > 0) {
                rgb_to_nv12(rgb, self.width, self.height, &mut self.yuv);
                // The transform may retain an input reference after ProcessOutput.
                // Give each sample its own buffer; reuse only our conversion/output
                // storage, whose lifetime is under this worker's control.
                let buffer = self.runtime.buffer(self.yuv.len() as u32)?;
                let mut destination = std::ptr::null_mut();
                buffer.Lock(&mut destination, None, None)?;
                std::ptr::copy_nonoverlapping(self.yuv.as_ptr(), destination, self.yuv.len());
                buffer.Unlock()?;
                buffer.SetCurrentLength(self.yuv.len() as u32)?;
                let sample = self.runtime.sample()?;
                sample.AddBuffer(&buffer)?;
                sample.SetSampleTime(self.timestamp)?;
                sample.SetSampleDuration(10_000_000 / self.fps as i64)?;
                self.timestamp += 10_000_000 / self.fps as i64;
                self.transform.ProcessInput(0, &sample, 0)?;
                self.pending.push_back(Instant::now());
                if self.events.is_some() {
                    self.input_credit -= 1;
                }
            }
            if self.events.is_some() {
                let deadline = Instant::now() + Duration::from_millis(12);
                while self.output_credit == 0 && Instant::now() < deadline {
                    self.poll_events()?;
                    if self.output_credit == 0 {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
                if self.output_credit == 0 {
                    return Ok(None);
                }
                self.output_credit -= 1;
            }
            let mut outputs = [MFT_OUTPUT_DATA_BUFFER {
                pSample: std::mem::ManuallyDrop::new(self.output_sample.clone()),
                ..Default::default()
            }];
            let mut status = 0;
            let result = self.transform.ProcessOutput(0, &mut outputs, &mut status);
            let sample = std::mem::ManuallyDrop::take(&mut outputs[0].pSample);
            std::mem::ManuallyDrop::drop(&mut outputs[0].pEvents);
            match result {
                Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                other => other?,
            }
            let buffer = sample
                .ok_or_else(|| windows_core::Error::from_hresult(MF_E_TRANSFORM_NEED_MORE_INPUT))?
                .ConvertToContiguousBuffer()?;
            let mut source = std::ptr::null_mut();
            let mut length = 0;
            buffer.Lock(&mut source, None, Some(&mut length))?;
            let mut packet = std::mem::take(&mut self.header);
            packet.extend_from_slice(std::slice::from_raw_parts(source, length as usize));
            buffer.Unlock()?;
            let first = !self.emitted;
            self.emitted = true;
            self.last_output = Instant::now();
            Ok(Some(Packet {
                bytes: packet,
                first,
                created: self.pending.pop_front().unwrap_or_else(Instant::now),
            }))
        }
    }
}

// Limited-range BT.601, matching the system encoder's SD/unspecified matrix.
fn rgb_to_nv12(rgb: &[u8], w: usize, h: usize, yuv: &mut [u8]) {
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let mut sums = [0i32; 3];
            for dy in 0..2 {
                for dx in 0..2 {
                    let i = (y + dy) * w + x + dx;
                    let r = rgb[i * 3] as i32;
                    let g = rgb[i * 3 + 1] as i32;
                    let b = rgb[i * 3 + 2] as i32;
                    yuv[i] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(16, 235) as u8;
                    sums[0] += r;
                    sums[1] += g;
                    sums[2] += b;
                }
            }
            let [r, g, b] = sums.map(|v| (v + 2) / 4);
            let uv = w * h + y / 2 * w + x;
            yuv[uv] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(16, 240) as u8;
            yuv[uv + 1] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(16, 240) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nv12_has_limited_range_luma_and_neutral_chroma() {
        for (rgb, luma) in [(0, 16), (255, 235)] {
            let mut yuv = [0; 6];
            rgb_to_nv12(&[rgb; 12], 2, 2, &mut yuv);
            assert_eq!(yuv, [luma, luma, luma, luma, 128, 128]);
        }
    }

    #[test]
    #[ignore = "requires the installed Windows H.264 encoder"]
    fn system_encoder_produces_bounded_annex_b_frames() {
        let runtime = Runtime::new().unwrap();
        let mut encoder = Encoder::new(&runtime, 1280, 720).unwrap();
        let mut rgb = vec![0; 1280 * 720 * 3];
        let started = std::time::Instant::now();
        let mut frames = Vec::new();
        let mut count = 0;
        for index in 0..60 {
            rgb.fill(index * 4);
            let Some(packet) = encoder.encode(&rgb).unwrap() else {
                continue;
            };
            assert_eq!(packet.first, count == 0);
            assert!(packet.created.elapsed() < Duration::from_millis(250));
            count += 1;
            let packet = packet.bytes;
            assert!(packet.starts_with(&[0, 0, 0, 1]) || packet.starts_with(&[0, 0, 1]));
            frames.extend(packet);
        }
        eprintln!(
            "Encoded 60 720p inputs in {:?}, {} bytes",
            started.elapsed(),
            frames.len()
        );
        assert!(
            count >= 55,
            "Encoder produced only {count} frames for 60 inputs"
        );
        eprintln!("Hardware encoder: {}", encoder.hardware);
        if let Some(path) = std::env::var_os("PLURIVIEW_TEST_H264") {
            std::fs::write(path, frames).unwrap();
        }
    }

    #[test]
    #[ignore = "requires the installed Windows H.264 encoders"]
    fn hardware_and_software_accept_all_adaptive_profiles() {
        let runtime = Runtime::new().unwrap();
        for hardware in [true, false] {
            for (width, height, bitrate, fps) in [
                (1280, 720, 4_000_000, 30),
                (960, 540, 1_800_000, 30),
                (640, 360, 700_000, 20),
            ] {
                let mut encoder =
                    Encoder::with_settings(&runtime, width, height, bitrate, fps, hardware)
                        .unwrap();
                if !hardware {
                    assert!(!encoder.hardware);
                }
                let mut bytes = Vec::new();
                let mut count = 0;
                for index in 0..30 {
                    if let Some(packet) = encoder
                        .encode(&vec![index * 8; width * height * 3])
                        .unwrap()
                    {
                        assert_eq!(packet.first, count == 0);
                        assert!(
                            packet.bytes.starts_with(&[0, 0, 0, 1])
                                || packet.bytes.starts_with(&[0, 0, 1])
                        );
                        count += 1;
                        bytes.extend(packet.bytes);
                    }
                }
                assert!(
                    count >= 25,
                    "Only {count} outputs at {height}p, hardware={}",
                    encoder.hardware
                );
                eprintln!("{height}p: {count} outputs, hardware={}", encoder.hardware);
                if let Some(directory) = std::env::var_os("PLURIVIEW_TEST_PROFILES") {
                    std::fs::write(
                        std::path::PathBuf::from(directory).join(format!(
                            "{height}p-{}.h264",
                            if hardware { "preferred" } else { "software" }
                        )),
                        bytes,
                    )
                    .unwrap();
                }
                encoder.pending.clear();
                encoder.last_output = Instant::now() - Duration::from_secs(1);
                assert!(
                    encoder.encode(&vec![0; width * height * 3]).is_err(),
                    "A stalled encoder must trigger fallback even without pending input"
                );
            }
        }
    }
}
