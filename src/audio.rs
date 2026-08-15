//! Stream audio monitor: plays a copy of the browser tiles' audio from the
//! Pluriview process to a user-chosen output device, OBS-style.
//!
//! Why: WebView2 renders sound from its own msedgewebview2.exe subprocesses,
//! so per-app audio capture (Discord window share, OBS "Application Audio
//! Capture", Game Bar) hears nothing from browser tiles when Pluriview is
//! shared (MicrosoftEdge/WebView2Feedback#2236). The monitor makes Pluriview
//! itself render a copy, which those tools pick up — the per-process
//! loopback tap they use is device-agnostic (verified experimentally), so
//! the copy can go to a device the user doesn't listen to (a virtual cable,
//! an unconnected output) and viewers hear it while the user doesn't.
//!
//! Unlike a ducking approach, this touches nothing outside Pluriview: no
//! session volumes, no other apps, no persistence side effects. The user
//! hears the original WebView2 audio, untouched. Off by default.
//!
//! Pipeline: WASAPI process loopback of the WebView2 process tree in the
//! default device's mix format (requesting any other format makes the
//! engine channel-matrix the tap, about -13 dB for 7.1 -> stereo), fold to
//! stereo ourselves with unity front-channel coefficients, render to the
//! monitor device.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread::JoinHandle,
    time::Duration,
};

use windows::{
    core::{w, Interface, HRESULT, HSTRING},
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Foundation::{CloseHandle, HANDLE},
        Media::Audio::{
            eConsole, eRender, ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
            IActivateAudioInterfaceCompletionHandler,
            IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
            IAudioRenderClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
            AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
            AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, AUDIOCLIENT_ACTIVATION_PARAMS,
            AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, DEVICE_STATE_ACTIVE,
            PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, WAVEFORMATEX,
        },
        System::{
            Com::{
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
                STGM_READ,
            },
            Threading::{CreateEventW, WaitForSingleObject},
        },
    },
};

/// How long the render buffer is primed with silence before real samples,
/// absorbing capture-packet jitter. This is the monitor copy's latency
/// relative to the audio the user hears (irrelevant to viewers).
const PRIME_MS: u32 = 50;

/// A render endpoint the user can pick as the monitor target.
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// Active render devices, default first. Safe to call from the UI thread.
pub fn render_devices() -> Vec<AudioDevice> {
    let mut devices = Vec::new();
    let result: windows::core::Result<()> = (|| unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let default_id = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .and_then(|d| d.GetId())
            .map(|id| {
                let owned = id.to_string().unwrap_or_default();
                CoTaskMemFree(Some(id.as_ptr() as *const _));
                owned
            })
            .unwrap_or_default();
        let collection = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        for i in 0..collection.GetCount()? {
            let device = collection.Item(i)?;
            let raw_id = device.GetId()?;
            let id = raw_id.to_string().unwrap_or_default();
            CoTaskMemFree(Some(raw_id.as_ptr() as *const _));
            let name = device
                .OpenPropertyStore(STGM_READ)
                .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
                .map(|value| format!("{value}"))
                .unwrap_or_else(|_| id.clone());
            let is_default = id == default_id;
            devices.push(AudioDevice {
                id,
                name,
                is_default,
            });
        }
        Ok(())
    })();
    if let Err(error) = result {
        log::warn!("audio device enumeration failed: {error}");
    }
    devices.sort_by_key(|d| !d.is_default);
    devices
}

/// Plays a copy of one WebView2 browser process tree's audio from this
/// process to the chosen render device. Dropping it stops the monitor.
pub struct AudioMonitor {
    browser_pid: u32,
    device_id: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AudioMonitor {
    pub fn start(browser_pid: u32, device_id: String) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread_device = device_id.clone();
        let thread = std::thread::Builder::new()
            .name("audio-monitor".into())
            .spawn(move || monitor_thread(browser_pid, &thread_device, &thread_stop))
            .ok();
        Self {
            browser_pid,
            device_id,
            stop,
            thread,
        }
    }

    pub fn browser_pid(&self) -> u32 {
        self.browser_pid
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }
}

impl Drop for AudioMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn monitor_thread(browser_pid: u32, device_id: &str, stop: &AtomicBool) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let mut samples: Vec<f32> = Vec::new();
    let mut stereo: Vec<f32> = Vec::new();
    let mut logged = false;

    // Outer loop rebuilds the streams after errors (monitor device removed,
    // format changes); waits and retries rather than giving up so plugging
    // the device back in resumes the monitor.
    'monitor: while !stop.load(Ordering::Relaxed) {
        let streams = MixFormat::for_default_device().and_then(|format| {
            let capture = Capture::start(browser_pid, &format)?;
            let render = Render::start(device_id, format.sample_rate)?;
            Ok((capture, render, format))
        });
        let (capture, render, format) = match streams {
            Ok(streams) => streams,
            Err(error) => {
                if !logged {
                    log::warn!("audio monitor unavailable: {error}");
                    logged = true;
                }
                for _ in 0..20 {
                    if stop.load(Ordering::Relaxed) {
                        break 'monitor;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                continue;
            }
        };
        if !logged {
            log::info!(
                "audio monitor started: WebView2 process {browser_pid} ({} ch, {} Hz) -> device {device_id}",
                format.channels,
                format.sample_rate
            );
            logged = true;
        }

        while !stop.load(Ordering::Relaxed) {
            unsafe { WaitForSingleObject(capture.event, 100) };
            loop {
                match capture.read(&mut samples, format.channels) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(error) => {
                        log::warn!("audio monitor capture error, rebuilding: {error}");
                        continue 'monitor;
                    }
                }
                fold_to_stereo(&samples, format.channels as usize, &mut stereo);
                if let Err(error) = render.write(&stereo) {
                    log::warn!("audio monitor render error, rebuilding: {error}");
                    continue 'monitor;
                }
            }
        }
    }
}

/// Fold interleaved `channels`-channel audio to stereo with unity front
/// channels, so plain stereo content (the norm for web pages) keeps its
/// exact level even when the engine mix is 5.1/7.1.
///
/// Standard WAVEFORMATEXTENSIBLE order: FL FR FC LFE BL BR SL SR.
/// L = FL + 0.707*(FC + BL + SL), R = FR + 0.707*(FC + BR + SR); LFE dropped.
fn fold_to_stereo(input: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    if channels == 0 {
        return;
    }
    let frames = input.len() / channels;
    out.reserve(frames * 2);
    for frame in input.chunks_exact(channels) {
        let (left, right) = match channels {
            1 => (frame[0], frame[0]),
            2 => (frame[0], frame[1]),
            _ => {
                let center = if channels > 2 { frame[2] } else { 0.0 };
                let mut left = frame[0] + 0.707 * center;
                let mut right = frame[1] + 0.707 * center;
                // Back pair (BL BR), then side pair (SL SR).
                if channels > 5 {
                    left += 0.707 * frame[4];
                    right += 0.707 * frame[5];
                }
                if channels > 7 {
                    left += 0.707 * frame[6];
                    right += 0.707 * frame[7];
                }
                (left, right)
            }
        };
        out.push(left.clamp(-1.0, 1.0));
        out.push(right.clamp(-1.0, 1.0));
    }
}

/// The default render device's mix format: capturing in it passes the tap
/// through without channel-matrixing or resampling.
struct MixFormat {
    /// WAVEFORMATEX(EXTENSIBLE) blob, passed to IAudioClient::Initialize.
    blob: Vec<u8>,
    channels: u16,
    sample_rate: u32,
}

impl MixFormat {
    fn for_default_device() -> Result<Self, String> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| e.to_string())?;
            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|e| e.to_string())?;
            let client: IAudioClient = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| e.to_string())?;
            let raw = client.GetMixFormat().map_err(|e| e.to_string())?;
            // WAVEFORMATEX is packed; copy the fields out before formatting.
            let head = *raw;
            let (tag, channels, sample_rate, bits, cb_size) = (
                { head.wFormatTag },
                { head.nChannels },
                { head.nSamplesPerSec },
                { head.wBitsPerSample },
                { head.cbSize },
            );
            let size = std::mem::size_of::<WAVEFORMATEX>() + cb_size as usize;
            let blob = std::slice::from_raw_parts(raw as *const u8, size).to_vec();
            CoTaskMemFree(Some(raw as *const _));

            // The engine mixes in 32-bit float; the fold step relies on it.
            // WAVE_FORMAT_IEEE_FLOAT directly, or EXTENSIBLE whose SubFormat
            // GUID starts with 0x00000003.
            let is_float = bits == 32
                && (tag == 3
                    || (tag == 0xFFFE && blob.len() >= 28 && blob[24..28] == [3, 0, 0, 0]));
            if !is_float {
                return Err(format!("unsupported mix format (tag {tag}, {bits} bit)"));
            }
            Ok(Self {
                blob,
                channels,
                sample_rate,
            })
        }
    }

    fn as_wave_format(&self) -> *const WAVEFORMATEX {
        self.blob.as_ptr() as *const WAVEFORMATEX
    }
}

const VT_BLOB: u16 = 65;

/// Raw C-layout PROPVARIANT holding a VT_BLOB, passed by pointer to
/// ActivateAudioInterfaceAsync. Built manually because the windows crate's
/// PROPVARIANT wrapper has no blob constructor.
#[repr(C)]
struct RawPropVariant {
    vt: u16,
    r1: u16,
    r2: u16,
    r3: u16,
    blob_size: u32,
    _pad: u32,
    blob_data: *const u8,
}

#[windows::core::implement(IActivateAudioInterfaceCompletionHandler)]
struct CompletionHandler(mpsc::Sender<()>);

impl IActivateAudioInterfaceCompletionHandler_Impl for CompletionHandler_Impl {
    fn ActivateCompleted(
        &self,
        _operation: Option<&IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let _ = self.0.send(());
        Ok(())
    }
}

/// WASAPI process-loopback capture of the WebView2 process tree, at full
/// volume — nothing about the sessions is modified.
struct Capture {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: HANDLE,
}

impl Capture {
    fn start(pid: u32, format: &MixFormat) -> Result<Self, String> {
        let params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        let prop = RawPropVariant {
            vt: VT_BLOB,
            r1: 0,
            r2: 0,
            r3: 0,
            blob_size: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
            _pad: 0,
            blob_data: &params as *const _ as *const u8,
        };

        let (sender, receiver) = mpsc::channel();
        let handler: IActivateAudioInterfaceCompletionHandler = CompletionHandler(sender).into();
        let operation = unsafe {
            ActivateAudioInterfaceAsync(
                w!("VAD\\Process_Loopback"),
                &IAudioClient::IID,
                Some(&prop as *const _ as *const _),
                &handler,
            )
        }
        .map_err(|e| e.to_string())?;
        receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "activation timed out".to_owned())?;

        let mut result = HRESULT(0);
        let mut unknown: Option<windows::core::IUnknown> = None;
        unsafe { operation.GetActivateResult(&mut result, &mut unknown) }
            .map_err(|e| e.to_string())?;
        result.ok().map_err(|e| e.to_string())?;
        let client: IAudioClient = unknown
            .ok_or("no interface returned")?
            .cast()
            .map_err(|e| e.to_string())?;

        let event = unsafe {
            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                    2_000_000, // 200 ms
                    0,
                    format.as_wave_format(),
                    None,
                )
                .map_err(|e| e.to_string())?;
            let event = CreateEventW(None, false, false, None).map_err(|e| e.to_string())?;
            client.SetEventHandle(event).map_err(|e| e.to_string())?;
            client.Start().map_err(|e| e.to_string())?;
            event
        };
        let capture = unsafe { client.GetService() }.map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            capture,
            event,
        })
    }

    /// Read the next pending packet into `samples`. Returns the sample count
    /// (0 when the queue is empty).
    fn read(&self, samples: &mut Vec<f32>, channels: u16) -> Result<usize, String> {
        samples.clear();
        unsafe {
            if self
                .capture
                .GetNextPacketSize()
                .map_err(|e| e.to_string())?
                == 0
            {
                return Ok(0);
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            self.capture
                .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                .map_err(|e| e.to_string())?;
            let count = frames as usize * channels as usize;
            samples.extend_from_slice(std::slice::from_raw_parts(data as *const f32, count));
            self.capture
                .ReleaseBuffer(frames)
                .map_err(|e| e.to_string())?;
            Ok(count)
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
            let _ = CloseHandle(self.event);
        }
    }
}

/// Shared-mode stereo float render stream on the monitor device; the engine
/// converts to whatever that device actually runs.
struct Render {
    client: IAudioClient,
    render: IAudioRenderClient,
    buffer_frames: u32,
}

impl Render {
    fn start(device_id: &str, sample_rate: u32) -> Result<Self, String> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| e.to_string())?;
            let device: IMMDevice = enumerator
                .GetDevice(&HSTRING::from(device_id))
                .map_err(|e| format!("monitor device unavailable: {e}"))?;
            let client: IAudioClient = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| e.to_string())?;
            let format = WAVEFORMATEX {
                wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
                nChannels: 2,
                nSamplesPerSec: sample_rate,
                nAvgBytesPerSec: sample_rate * 8,
                nBlockAlign: 8,
                wBitsPerSample: 32,
                cbSize: 0,
            };
            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    2_000_000, // 200 ms
                    0,
                    &format,
                    None,
                )
                .map_err(|e| e.to_string())?;
            let render: IAudioRenderClient = client.GetService().map_err(|e| e.to_string())?;
            let buffer_frames = client.GetBufferSize().map_err(|e| e.to_string())?;

            // Prime with silence so capture-packet jitter doesn't underrun.
            let prime = (sample_rate * PRIME_MS / 1000).min(buffer_frames);
            let data = render.GetBuffer(prime).map_err(|e| e.to_string())?;
            std::ptr::write_bytes(data, 0, prime as usize * 8);
            render.ReleaseBuffer(prime, 0).map_err(|e| e.to_string())?;
            client.Start().map_err(|e| e.to_string())?;

            Ok(Self {
                client,
                render,
                buffer_frames,
            })
        }
    }

    /// Queue stereo samples for playback, dropping whatever exceeds the
    /// free space (capture and render share the engine clock, so overflow
    /// means the device stalled, not drift).
    fn write(&self, samples: &[f32]) -> Result<(), String> {
        let frames = samples.len() as u32 / 2;
        unsafe {
            let padding = self.client.GetCurrentPadding().map_err(|e| e.to_string())?;
            let write = frames.min(self.buffer_frames - padding);
            if write == 0 {
                return Ok(());
            }
            let data = self.render.GetBuffer(write).map_err(|e| e.to_string())?;
            std::ptr::copy_nonoverlapping(samples.as_ptr(), data as *mut f32, write as usize * 2);
            self.render
                .ReleaseBuffer(write, 0)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

impl Drop for Render {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fold_to_stereo, render_devices, AudioMonitor, Capture, MixFormat};
    use std::time::{Duration, Instant};
    use windows::core::HSTRING;
    use windows::Win32::{
        Media::Audio::{
            eConsole, eRender, Endpoints::IAudioEndpointVolume, IMMDevice, IMMDeviceEnumerator,
            MMDeviceEnumerator,
        },
        System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED},
    };

    #[test]
    fn fold_passes_stereo_through() {
        let mut out = Vec::new();
        fold_to_stereo(&[0.1, -0.2, 0.3, -0.4], 2, &mut out);
        assert_eq!(out, vec![0.1, -0.2, 0.3, -0.4]);
    }

    #[test]
    fn fold_keeps_front_channels_at_unity_for_surround() {
        // One 7.1 frame with content only in FL/FR (typical web audio).
        let frame = [0.5, -0.5, 0.0, 0.9, 0.0, 0.0, 0.0, 0.0]; // LFE ignored
        let mut out = Vec::new();
        fold_to_stereo(&frame, 8, &mut out);
        assert_eq!(out, vec![0.5, -0.5]);
    }

    #[test]
    fn fold_mixes_center_and_surrounds() {
        let frame = [0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0]; // FC + BL
        let mut out = Vec::new();
        fold_to_stereo(&frame, 8, &mut out);
        assert!((out[0] - 1.0).abs() < 1e-3); // 0.707 + 0.707 clamped to 1.0
        assert!((out[1] - 0.707).abs() < 1e-3); // center only
    }

    #[test]
    fn fold_upmixes_mono() {
        let mut out = Vec::new();
        fold_to_stereo(&[0.25], 1, &mut out);
        assert_eq!(out, vec![0.25, 0.25]);
    }

    /// Mutes a device endpoint so audio tests are inaudible; the process
    /// loopback tap sits before endpoint volume/mute, so captures are
    /// unaffected. Restores the previous state on drop.
    struct MuteGuard {
        endpoint: IAudioEndpointVolume,
        was_muted: bool,
    }

    impl MuteGuard {
        fn engage(device: &IMMDevice) -> windows::core::Result<Self> {
            unsafe {
                let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
                let was_muted = endpoint.GetMute()?.as_bool();
                endpoint.SetMute(true, std::ptr::null())?;
                Ok(Self {
                    endpoint,
                    was_muted,
                })
            }
        }
    }

    impl Drop for MuteGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = self.endpoint.SetMute(self.was_muted, std::ptr::null());
            }
        }
    }

    fn device_by_id(id: &str) -> IMMDevice {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            enumerator.GetDevice(&HSTRING::from(id)).unwrap()
        }
    }

    fn default_device() -> IMMDevice {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .unwrap()
        }
    }

    fn rms_of(capture: &Capture, channels: u16, secs: f32) -> f64 {
        let mut samples = Vec::new();
        let mut sum_sq = 0f64;
        let mut count = 0u64;
        let deadline = Instant::now() + Duration::from_secs_f32(secs);
        while Instant::now() < deadline {
            match capture.read(&mut samples, channels) {
                Ok(0) => std::thread::sleep(Duration::from_millis(5)),
                Ok(n) => {
                    sum_sq += samples
                        .iter()
                        .map(|s| (*s as f64) * (*s as f64))
                        .sum::<f64>();
                    count += n as u64;
                }
                Err(error) => panic!("capture failed: {error}"),
            }
        }
        (sum_sq / count.max(1) as f64).sqrt()
    }

    /// Full-pipeline check: a tone child stands in for the WebView2 tree;
    /// the monitor must make THIS process (like pluriview.exe on a stream)
    /// emit its audio on the chosen non-default device. Both the default
    /// device and the monitor device are endpoint-muted throughout, so the
    /// test is silent, and no session volumes are touched by design.
    #[test]
    #[ignore = "needs a second render device; run with -- --ignored"]
    fn monitor_replays_child_audio_from_this_process() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let Some(monitor_device) = render_devices().into_iter().find(|d| !d.is_default) else {
            eprintln!("skipped: no non-default render device available");
            return;
        };
        let _mute_default = MuteGuard::engage(&default_device()).expect("mute default");
        let _mute_monitor =
            MuteGuard::engage(&device_by_id(&monitor_device.id)).expect("mute monitor");

        let wav = std::env::temp_dir().join("pluriview_monitor_test_tone.wav");
        write_tone_wav(&wav);
        let mut child = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "(New-Object Media.SoundPlayer '{}').PlayLooping(); Start-Sleep 60",
                    wav.display()
                ),
            ])
            .spawn()
            .expect("spawn tone player");
        let child_pid = child.id();
        std::thread::sleep(Duration::from_secs(2));

        let monitor = AudioMonitor::start(child_pid, monitor_device.id);
        std::thread::sleep(Duration::from_secs(2));

        let format = MixFormat::for_default_device().expect("mix format");

        // What the child actually plays, per the (untouched) tap.
        let child_capture = Capture::start(child_pid, &format).expect("child capture");
        let child_rms = rms_of(&child_capture, format.channels, 2.0);
        drop(child_capture);
        assert!(
            child_rms > 0.01,
            "tone child should be audible to the tap, got RMS {child_rms:.5}"
        );

        // What a per-app capture of Pluriview hears: our own process tree
        // (the child is a subprocess of the test runner, not of us — but the
        // monitor's render stream is ours).
        let self_capture = Capture::start(std::process::id(), &format).expect("self capture");
        let self_rms = rms_of(&self_capture, format.channels, 2.0);
        drop(self_capture);

        let ratio = self_rms / child_rms;
        assert!(
            (0.2..=5.0).contains(&ratio),
            "monitor should replay at a comparable level: child {child_rms:.5}, self {self_rms:.5}, ratio {ratio:.3}"
        );

        drop(monitor);
        let _ = child.kill();
        let _ = child.wait();
    }

    fn write_tone_wav(path: &std::path::Path) {
        use std::io::Write as _;
        let rate = 44_100u32;
        let n = rate * 60;
        let data_len = n * 2;
        let mut buf = Vec::with_capacity(44 + data_len as usize);
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&(36 + data_len).to_le_bytes());
        buf.extend_from_slice(b"WAVEfmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&rate.to_le_bytes());
        buf.extend_from_slice(&(rate * 2).to_le_bytes());
        buf.extend_from_slice(&2u16.to_le_bytes());
        buf.extend_from_slice(&16u16.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..n {
            let t = i as f32 / rate as f32;
            let s = (0.3 * (t * 440.0 * std::f32::consts::TAU).sin() * i16::MAX as f32) as i16;
            buf.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::File::create(path)
            .unwrap()
            .write_all(&buf)
            .unwrap();
    }
}
