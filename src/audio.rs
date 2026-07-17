//! Streaming audio relay: replays browser-tile audio from the Pluriview
//! process so per-app audio capture (Discord window share, OBS "Application
//! Audio Capture", Xbox Game Bar) can hear it.
//!
//! WebView2 plays sound from its own msedgewebview2.exe subprocesses, and
//! Windows attributes those audio sessions to the WebView2 runtime instead
//! of the host app (MicrosoftEdge/WebView2Feedback#2236). Discord's window
//! share only taps the shared app's own sessions, so browser tiles were
//! silent for stream viewers.
//!
//! The relay:
//! 1. drops the WebView2 sessions' mixer volume to [`ATTENUATION`],
//! 2. captures the WebView2 process tree via WASAPI process loopback (the
//!    tap sits after the session volume but before master volume, verified
//!    experimentally: capture scales linearly with the session scalar and
//!    ignores master volume/mute),
//! 3. multiplies by 1/ATTENUATION and renders the result from this process.
//!
//! Both streams use the output device's own mix format: requesting anything
//! else makes the engine channel-matrix the tap (e.g. 7.1 -> stereo costs
//! about -13 dB), which would change the loudness users hear.
//!
//! Locally you hear the relayed copy at the original level plus a -40 dB
//! residual of the original, masked by the copy. Capture tools hear the
//! relay because Pluriview itself is now the process rendering it. Session
//! volumes are restored when the relay stops.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use windows::{
    core::{w, Interface, HRESULT},
    Win32::{
        Foundation::{CloseHandle, HANDLE},
        Media::Audio::{
            eConsole, eRender, ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
            IActivateAudioInterfaceCompletionHandler,
            IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
            IAudioRenderClient, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
            ISimpleAudioVolume, MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
            AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
            PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, WAVEFORMATEX,
        },
        System::{
            Com::{
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
            },
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
            Threading::{CreateEventW, WaitForSingleObject},
        },
    },
};

/// Mixer volume applied to the WebView2 sessions while the relay runs.
/// -40 dB: inaudible under the full-level relayed copy, but still enough
/// signal for the (post-volume) loopback tap to recover losslessly, since
/// the engine mixes in 32-bit float.
const ATTENUATION: f32 = 0.01;

/// How long the render buffer is primed with silence before real samples,
/// absorbing capture-packet jitter. This is the relay's added latency.
const PRIME_MS: u32 = 50;

/// Relays audio of one WebView2 browser process tree through this process.
/// Dropping it stops the relay and restores the sessions' mixer volumes.
pub struct AudioRelay {
    browser_pid: u32,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AudioRelay {
    pub fn start(browser_pid: u32) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name("audio-relay".into())
            .spawn(move || relay_thread(browser_pid, &thread_stop))
            .ok();
        Self {
            browser_pid,
            stop,
            thread,
        }
    }

    pub fn browser_pid(&self) -> u32 {
        self.browser_pid
    }
}

impl Drop for AudioRelay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn relay_thread(browser_pid: u32, stop: &AtomicBool) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let mut ducker = SessionDucker::new(browser_pid);
    let gain = 1.0 / ATTENUATION;
    let mut samples: Vec<f32> = Vec::new();
    let mut logged_start = false;

    // Outer loop rebuilds both streams after device changes (the render
    // stream dies with AUDCLNT_E_DEVICE_INVALIDATED, and the new device may
    // have a different mix format).
    'relay: while !stop.load(Ordering::Relaxed) {
        let streams = MixFormat::for_default_device().and_then(|format| {
            let capture = Capture::start(browser_pid, &format)?;
            let render = Render::start(&format)?;
            Ok((capture, render, format))
        });
        let (capture, render, format) = match streams {
            Ok(streams) => streams,
            Err(error) => {
                // Pre-Win10-2004, no output device, or transient failure:
                // keep tiles sounding normal and retry occasionally.
                if !logged_start {
                    log::warn!("audio relay unavailable: {error}");
                    logged_start = true;
                }
                ducker.restore();
                for _ in 0..20 {
                    if stop.load(Ordering::Relaxed) {
                        break 'relay;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                continue;
            }
        };
        if !logged_start {
            log::info!(
                "audio relay started for WebView2 process {browser_pid} ({} ch, {} Hz)",
                format.channels,
                format.sample_rate
            );
            logged_start = true;
        }

        while !stop.load(Ordering::Relaxed) {
            // Sessions appear whenever a page first plays sound and whenever
            // the WebView2 audio service restarts, so rescan periodically.
            ducker.scan_if_due();

            unsafe { WaitForSingleObject(capture.event, 100) };
            loop {
                match capture.read(&mut samples, format.channels) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(error) => {
                        log::warn!("audio relay capture error, rebuilding: {error}");
                        continue 'relay;
                    }
                }
                // A ducked source peaks around ATTENUATION; anything well
                // above that is a session that appeared since the last scan
                // and hasn't been ducked yet. Amplifying it would blast
                // clipped audio, so swap in silence and duck it now.
                let peak = samples.iter().fold(0f32, |max, s| max.max(s.abs()));
                if peak > ATTENUATION * 5.0 {
                    samples.iter_mut().for_each(|s| *s = 0.0);
                    ducker.scan_now();
                } else {
                    for sample in &mut samples {
                        *sample = (*sample * gain).clamp(-1.0, 1.0);
                    }
                }
                if let Err(error) = render.write(&samples, format.channels) {
                    log::warn!("audio relay render error, rebuilding: {error}");
                    continue 'relay;
                }
            }
        }
    }

    ducker.restore();
}

/// The default render device's mix format, so capture and render pass
/// through the engine without channel-matrixing or resampling.
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

            // The engine mixes in 32-bit float; the gain step relies on it.
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

/// WASAPI process-loopback capture of the WebView2 process tree.
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

/// Shared-mode render stream on the default output in its own mix format.
struct Render {
    client: IAudioClient,
    render: IAudioRenderClient,
    buffer_frames: u32,
}

impl Render {
    fn start(format: &MixFormat) -> Result<Self, String> {
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
            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    // The format matches the mix format, but keep the auto
                    // converter as a safety net for exotic devices.
                    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    2_000_000, // 200 ms
                    0,
                    format.as_wave_format(),
                    None,
                )
                .map_err(|e| e.to_string())?;
            let render: IAudioRenderClient = client.GetService().map_err(|e| e.to_string())?;
            let buffer_frames = client.GetBufferSize().map_err(|e| e.to_string())?;

            // Prime with silence so capture-packet jitter doesn't underrun.
            let prime = (format.sample_rate * PRIME_MS / 1000).min(buffer_frames);
            let data = render.GetBuffer(prime).map_err(|e| e.to_string())?;
            std::ptr::write_bytes(data, 0, prime as usize * format.channels as usize * 4);
            render.ReleaseBuffer(prime, 0).map_err(|e| e.to_string())?;
            client.Start().map_err(|e| e.to_string())?;

            Ok(Self {
                client,
                render,
                buffer_frames,
            })
        }
    }

    /// Queue samples for playback, dropping whatever exceeds the free space
    /// (capture and render share the engine clock, so overflow means the
    /// device stalled, not drift).
    fn write(&self, samples: &[f32], channels: u16) -> Result<(), String> {
        let frames = samples.len() as u32 / channels as u32;
        unsafe {
            let padding = self.client.GetCurrentPadding().map_err(|e| e.to_string())?;
            let write = frames.min(self.buffer_frames - padding);
            if write == 0 {
                return Ok(());
            }
            let data = self.render.GetBuffer(write).map_err(|e| e.to_string())?;
            std::ptr::copy_nonoverlapping(
                samples.as_ptr(),
                data as *mut f32,
                write as usize * channels as usize,
            );
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

/// Holds the WebView2 tree's mixer sessions at [`ATTENUATION`] and restores
/// their original volumes when the relay stops.
struct SessionDucker {
    browser_pid: u32,
    /// Keyed by session instance identifier so rescans don't re-duck.
    ducked: HashMap<String, (ISimpleAudioVolume, f32)>,
    last_scan: Option<Instant>,
}

impl SessionDucker {
    fn new(browser_pid: u32) -> Self {
        Self {
            browser_pid,
            ducked: HashMap::new(),
            last_scan: None,
        }
    }

    /// Periodic rescan. 500 ms bounds how long a brand-new (not yet ducked)
    /// quiet session can leak through amplified; the overload guard in the
    /// pump loop handles loud ones within a packet.
    fn scan_if_due(&mut self) {
        if self
            .last_scan
            .is_none_or(|at| at.elapsed() >= Duration::from_millis(500))
        {
            self.scan();
            self.last_scan = Some(Instant::now());
        }
    }

    /// Immediate rescan for a just-appeared loud session, rate-limited so a
    /// sustained overload doesn't enumerate sessions on every packet.
    fn scan_now(&mut self) {
        if self
            .last_scan
            .is_none_or(|at| at.elapsed() >= Duration::from_millis(200))
        {
            self.scan();
            self.last_scan = Some(Instant::now());
        }
    }

    fn scan(&mut self) {
        let parents = parent_map();
        let result: windows::core::Result<()> = (|| unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
            let sessions = manager.GetSessionEnumerator()?;
            for i in 0..sessions.GetCount()? {
                let control = sessions.GetSession(i)?;
                let control2: IAudioSessionControl2 = control.cast()?;
                let pid = control2.GetProcessId()?;
                if !in_tree(&parents, pid, self.browser_pid) {
                    continue;
                }
                let raw = control2.GetSessionInstanceIdentifier()?;
                let key = raw.to_string().unwrap_or_default();
                CoTaskMemFree(Some(raw.as_ptr() as *const _));
                if self.ducked.contains_key(&key) {
                    continue;
                }
                let volume: ISimpleAudioVolume = control.cast()?;
                let mut original = volume.GetMasterVolume()?;
                // A near-ATTENUATION "original" is our own leftover from a
                // crashed run (Windows persists session volumes); treat the
                // user's level as full.
                if original <= ATTENUATION * 2.0 {
                    original = 1.0;
                }
                volume.SetMasterVolume(ATTENUATION, std::ptr::null())?;
                self.ducked.insert(key, (volume, original));
            }
            Ok(())
        })();
        if let Err(error) = result {
            log::warn!("audio relay session scan failed: {error}");
        }
    }

    fn restore(&mut self) {
        for (volume, original) in self.ducked.values() {
            unsafe {
                let _ = volume.SetMasterVolume(*original, std::ptr::null());
            }
        }
        self.ducked.clear();
        self.last_scan = None;
    }
}

impl Drop for SessionDucker {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Snapshot of every process's parent, for walking session PIDs up to the
/// WebView2 browser process.
fn parent_map() -> HashMap<u32, u32> {
    let mut map = HashMap::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return map;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                map.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    map
}

fn in_tree(parents: &HashMap<u32, u32>, mut pid: u32, root: u32) -> bool {
    for _ in 0..16 {
        if pid == root {
            return true;
        }
        match parents.get(&pid) {
            Some(&parent) if parent != 0 && parent != pid => pid = parent,
            _ => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{in_tree, AudioRelay, Capture, MixFormat, ATTENUATION};
    use std::collections::HashMap;
    use std::time::{Duration, Instant};
    use windows::core::Interface as _;
    use windows::Win32::{
        Media::Audio::{
            eConsole, eRender, Endpoints::IAudioEndpointVolume, IAudioSessionControl2,
            IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
        },
        System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED},
    };

    #[test]
    fn in_tree_walks_parent_chain() {
        let parents: HashMap<u32, u32> = [(30, 20), (20, 10), (99, 98)].into();
        assert!(in_tree(&parents, 30, 10));
        assert!(in_tree(&parents, 10, 10));
        assert!(!in_tree(&parents, 99, 10));
        assert!(!in_tree(&parents, 55, 10));
    }

    #[test]
    fn in_tree_survives_cycles() {
        let parents: HashMap<u32, u32> = [(2, 3), (3, 2)].into();
        assert!(!in_tree(&parents, 2, 10));
    }

    /// Mutes the master endpoint so audio tests are inaudible; the process
    /// loopback tap sits before master mute, so captures are unaffected.
    /// Restores the previous state on drop.
    struct MasterMuteGuard {
        endpoint: IAudioEndpointVolume,
        was_muted: bool,
    }

    impl MasterMuteGuard {
        fn engage() -> windows::core::Result<Self> {
            unsafe {
                let enumerator: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
                let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
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

    impl Drop for MasterMuteGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = self.endpoint.SetMute(self.was_muted, std::ptr::null());
            }
        }
    }

    /// Renders a 440 Hz sine at `amplitude` from a spawned thread until
    /// dropped, standing in for the WebView2 audio (same process would be
    /// excluded from ducking, so the test spawns a child process instead —
    /// see the ignored test below).
    fn spawn_tone_child() -> std::process::Child {
        // A PowerShell child rendering via .NET SoundPlayer gives the relay a
        // real out-of-process source, like msedgewebview2.exe. Master output
        // stays muted the whole time, so nothing is audible.
        let wav = std::env::temp_dir().join("pluriview_relay_test_tone.wav");
        write_tone_wav(&wav);
        std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "(New-Object Media.SoundPlayer '{}').PlayLooping(); Start-Sleep 60",
                    wav.display()
                ),
            ])
            .spawn()
            .expect("spawn tone player")
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

    /// Full-pipeline check: duck + capture + re-gain + render. Uses a tone
    /// player child in place of the WebView2 tree, then confirms that THIS
    /// process (like pluriview.exe on a stream) now emits the audio at the
    /// same level. Master output is muted throughout, so the test is silent.
    #[test]
    #[ignore = "manipulates live audio sessions; run with -- --ignored"]
    fn relay_replays_child_audio_from_this_process() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let _mute = MasterMuteGuard::engage().expect("master mute");
        let mut child = spawn_tone_child();
        let child_pid = child.id();

        let relay = AudioRelay::start(child_pid);
        // Child session appears ~1-2 s in; the relay rescans every 500 ms.
        std::thread::sleep(Duration::from_secs(5));

        assert_eq!(
            session_volume(child_pid).expect("child session missing"),
            ATTENUATION,
            "relay should hold the child's mixer session at ATTENUATION"
        );

        let format = MixFormat::for_default_device().expect("mix format");

        // What the child actually plays, reconstructed from its ducked
        // session: tap is post-session-volume, so scale back up.
        let child_capture = Capture::start(child_pid, &format).expect("child capture");
        let child_rms = rms_of(&child_capture, format.channels, 2.0) / ATTENUATION as f64;
        drop(child_capture);
        assert!(
            child_rms > 0.01,
            "tone child should be audible to the tap, got RMS {child_rms:.5}"
        );

        // What a per-app capture of Pluriview hears: our own process tree.
        // The child is in it too (test spawned it), but ducked to -40 dB; a
        // matching RMS can only come from the relay's render stream.
        let self_capture = Capture::start(std::process::id(), &format).expect("self capture");
        let self_rms = rms_of(&self_capture, format.channels, 2.0);
        drop(self_capture);

        let ratio = self_rms / child_rms;
        assert!(
            (0.5..=2.0).contains(&ratio),
            "relay should replay at the original level: child {child_rms:.5}, self {self_rms:.5}, ratio {ratio:.3}"
        );

        drop(relay);
        assert_eq!(
            session_volume(child_pid).expect("child session missing after relay"),
            1.0,
            "relay should restore the mixer volume on stop"
        );
        let _ = child.kill();
    }

    fn session_volume(pid: u32) -> Option<f32> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
            let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None).ok()?;
            let sessions = manager.GetSessionEnumerator().ok()?;
            for i in 0..sessions.GetCount().ok()? {
                let control = sessions.GetSession(i).ok()?;
                let control2: IAudioSessionControl2 = control.cast().ok()?;
                if control2.GetProcessId().ok()? == pid {
                    let volume: ISimpleAudioVolume = control.cast().ok()?;
                    return volume.GetMasterVolume().ok();
                }
            }
        }
        None
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
