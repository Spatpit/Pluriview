//! Windows-only mpv/Streamlink runtime for video preview tiles.
//! Shared video models, Streamlink resolution, and timeline thumbnails.
//!
//! The external-process/IPC player remains here for regression coverage, but
//! production playback uses `crate::libmpv` and no longer captures an embedded
//! child window.

#![allow(dead_code)]

use std::{
    collections::{HashMap, VecDeque},
    ffi::{c_void, OsString},
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::windows::{io::AsRawHandle, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, WPARAM},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD,
                THREADENTRY32,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            },
            LibraryLoader::GetModuleHandleW,
            Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, ShowWindow,
            SW_SHOWNOACTIVATE, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
        },
    },
};

use crate::preview::PreviewId;

const PARK_X: i32 = -30_000;
const PARK_Y: i32 = -30_000;
/// Streamlink may spend several seconds resolving a provider before it starts
/// mpv, so the IPC connector must outlive normal network/plugin startup.
const IPC_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const IPC_RETRY_DELAY: Duration = Duration::from_millis(40);
const CLEANUP_GRACE: Duration = Duration::from_millis(250);
const STDERR_TAIL_LINES: usize = 80;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_SUSPENDED: u32 = 0x0000_0004;

static PIPE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static THUMBNAIL_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// A parked, non-activating top-level window into which mpv embeds its child
/// window. The HWND remains stable until this value is dropped.
pub struct VideoHost {
    hwnd: HWND,
}

impl VideoHost {
    pub fn new(owner: Option<isize>, width: i32, height: i32) -> Result<Self, String> {
        static REGISTERED: OnceLock<Result<(), String>> = OnceLock::new();
        REGISTERED.get_or_init(register_window_class).clone()?;

        let width = width.max(1);
        let height = height.max(1);
        let owner = owner.map_or_else(HWND::default, |raw| HWND(raw as *mut c_void));
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("PluriviewVideoHost"),
                w!("Pluriview Video"),
                WS_POPUP,
                PARK_X,
                PARK_Y,
                width,
                height,
                owner,
                None,
                None,
                None,
            )
            .map_err(|error| format!("Could not create the video host window: {error}"))?
        };
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        Ok(Self { hwnd })
    }

    /// Raw HWND representation expected by the existing capture layer.
    pub fn hwnd(&self) -> isize {
        self.hwnd.0 as isize
    }
}

impl Drop for VideoHost {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn register_window_class() -> Result<(), String> {
    let class = WNDCLASSW {
        lpfnWndProc: Some(video_window_proc),
        hInstance: unsafe { GetModuleHandleW(None) }
            .map_err(|error| error.to_string())?
            .into(),
        lpszClassName: w!("PluriviewVideoHost"),
        ..Default::default()
    };
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        Err(windows::core::Error::from_win32().to_string())
    } else {
        Ok(())
    }
}

unsafe extern "system" fn video_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, message, wparam, lparam)
}

#[derive(Clone, Debug)]
pub enum VideoSource {
    LocalFile(PathBuf),
    Stream {
        url: String,
        quality: String,
        streamlink_path: PathBuf,
    },
}

#[derive(Clone, Debug)]
pub struct VideoLaunch {
    pub mpv_path: PathBuf,
    pub source: VideoSource,
    pub start_paused: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoState {
    pub connected: bool,
    pub pause: bool,
    pub time_pos: Option<f64>,
    pub duration: Option<f64>,
    pub volume: f64,
    pub mute: bool,
    pub speed: f64,
    pub loop_file: LoopMode,
    pub track_list: Vec<TrackInfo>,
    pub audio_track: TrackSelection,
    pub subtitle_track: TrackSelection,
    pub media_title: Option<String>,
    pub paused_for_cache: bool,
    pub core_idle: bool,
    pub eof_reached: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LoopMode {
    #[default]
    Off,
    Infinite,
    Count(u64),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum TrackSelection {
    Id(i64),
    Other(String),
    #[default]
    Disabled,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TrackInfo {
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub lang: Option<String>,
    #[serde(default)]
    pub selected: bool,
    #[serde(default, rename = "external")]
    pub external: bool,
    #[serde(default)]
    pub codec: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VideoProperty {
    Pause,
    TimePosition,
    Duration,
    Volume,
    Mute,
    Speed,
    LoopFile,
    TrackList,
    AudioTrack,
    SubtitleTrack,
    MediaTitle,
    PausedForCache,
    CoreIdle,
    EofReached,
}

#[derive(Clone, Debug)]
pub enum VideoUpdate {
    Connected,
    Property(VideoProperty),
    Event,
    Error(String),
    Exited {
        status: ExitStatus,
        unexpected: bool,
        stderr_tail: String,
    },
}

enum WorkerEvent {
    Connected,
    Line(String),
    Error(String),
}

#[derive(Debug)]
enum VideoCommand {
    Ipc(Value),
    SetVolume(f64),
}

/// One launcher process, its mpv JSON IPC link, and its process job.
pub struct VideoSession {
    child: Child,
    job: JobObject,
    command_sender: Sender<VideoCommand>,
    event_receiver: Receiver<WorkerEvent>,
    stop: Arc<AtomicBool>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    state: VideoState,
    /// Requested pause state retained until mpv confirms it, preventing stale
    /// property events from reverting the play button after a click.
    pending_pause: Option<bool>,
    /// Requested volume kept locally until mpv echoes the same value. This
    /// prevents older property-change messages from pulling the UI slider
    /// backwards while the user is dragging it.
    pending_volume: Option<f64>,
    quit_requested: bool,
    exit_reported: bool,
}

impl VideoSession {
    fn spawn(
        executable: &Path,
        args: &[OsString],
        pipe_name: String,
        initial_paused: bool,
    ) -> Result<Self, String> {
        let job = JobObject::new()?;
        let mut command = Command::new(executable);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        let mut child = command.spawn().map_err(|error| {
            format!("Could not launch {}: {error}", executable.to_string_lossy())
        })?;

        if let Err(error) = job.assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        if let Err(error) = resume_process(child.id()) {
            let _ = job.terminate();
            let _ = child.wait();
            return Err(error);
        }

        let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)));
        if let Some(stdout) = child.stdout.take() {
            drain_stderr(stdout, stderr_tail.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            drain_stderr(stderr, stderr_tail.clone());
        }

        let (command_sender, command_receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        start_ipc_worker(pipe_name, command_receiver, event_sender, stop.clone());

        Ok(Self {
            child,
            job,
            command_sender,
            event_receiver,
            stop,
            stderr_tail,
            state: VideoState {
                pause: initial_paused,
                volume: 100.0,
                speed: 1.0,
                ..Default::default()
            },
            pending_pause: None,
            pending_volume: None,
            quit_requested: false,
            exit_reported: false,
        })
    }

    pub fn state(&self) -> &VideoState {
        &self.state
    }

    pub fn stderr_tail(&self) -> String {
        lock_tail(&self.stderr_tail)
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Drain all currently available process/IPC updates without waiting.
    pub fn poll(&mut self) -> Vec<VideoUpdate> {
        let mut updates = Vec::new();
        while let Ok(event) = self.event_receiver.try_recv() {
            match event {
                WorkerEvent::Connected => {
                    self.state.connected = true;
                    updates.push(VideoUpdate::Connected);
                }
                WorkerEvent::Line(line) => {
                    let update_start = updates.len();
                    parse_ipc_line(&line, &mut self.state, &mut updates);
                    if updates[update_start..]
                        .iter()
                        .any(|update| matches!(update, VideoUpdate::Property(VideoProperty::Pause)))
                    {
                        if let Some(requested) = self.pending_pause {
                            if self.state.pause == requested {
                                self.pending_pause = None;
                            } else {
                                self.state.pause = requested;
                            }
                        }
                    }
                    if updates[update_start..].iter().any(|update| {
                        matches!(update, VideoUpdate::Property(VideoProperty::Volume))
                    }) {
                        if let Some(requested) = self.pending_volume {
                            if (self.state.volume - requested).abs() <= 0.01 {
                                self.pending_volume = None;
                            } else {
                                self.state.volume = requested;
                            }
                        }
                    }
                }
                WorkerEvent::Error(error) => updates.push(VideoUpdate::Error(error)),
            }
        }

        if !self.exit_reported {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.exit_reported = true;
                    self.state.connected = false;
                    let unexpected = !self.quit_requested;
                    let stderr_tail = self.stderr_tail();
                    if unexpected {
                        let detail = if stderr_tail.is_empty() {
                            String::new()
                        } else {
                            format!("\n{stderr_tail}")
                        };
                        updates.push(VideoUpdate::Error(format!(
                            "The video process exited unexpectedly with {status}.{detail}"
                        )));
                    }
                    updates.push(VideoUpdate::Exited {
                        status,
                        unexpected,
                        stderr_tail,
                    });
                }
                Ok(None) => {}
                Err(error) => updates.push(VideoUpdate::Error(format!(
                    "Could not query the video process: {error}"
                ))),
            }
        }
        updates
    }

    pub fn set_paused(&mut self, paused: bool) -> Result<(), String> {
        self.set_property("pause", json!(paused))?;
        self.state.pause = paused;
        self.pending_pause = Some(paused);
        Ok(())
    }

    pub fn play(&mut self) -> Result<(), String> {
        if self.state.eof_reached {
            self.command(json!(["seek", 0.0, "absolute+exact"]))?;
        }
        // Always write pause=false. `cycle pause` is a toggle, so a stale
        // local pause flag would send a playing session back to paused.
        self.set_paused(false)
    }

    pub fn seek_absolute(&self, seconds: f64) -> Result<(), String> {
        self.command(json!(["seek", seconds.max(0.0), "absolute+exact"]))
    }

    pub fn set_volume(&mut self, volume: f64) -> Result<(), String> {
        if !volume.is_finite() {
            return Err("Volume must be a finite number".to_owned());
        }
        let volume = volume.clamp(0.0, 100.0);
        self.command_sender
            .send(VideoCommand::SetVolume(volume))
            .map_err(|_| "The mpv IPC writer is no longer running".to_owned())?;
        self.state.volume = volume;
        self.pending_volume = Some(volume);
        Ok(())
    }

    pub fn set_muted(&self, muted: bool) -> Result<(), String> {
        self.set_property("mute", json!(muted))
    }

    pub fn set_speed(&self, speed: f64) -> Result<(), String> {
        if !speed.is_finite() || speed <= 0.0 {
            return Err("Playback speed must be a positive finite number".to_owned());
        }
        self.set_property("speed", json!(speed))
    }

    pub fn set_looping(&self, enabled: bool) -> Result<(), String> {
        self.set_property("loop-file", json!(if enabled { "inf" } else { "no" }))
    }

    pub fn select_audio_track(&self, id: i64) -> Result<(), String> {
        self.set_property("aid", json!(id))
    }

    pub fn select_subtitle_track(&self, id: i64) -> Result<(), String> {
        self.set_property("sid", json!(id))
    }

    pub fn disable_subtitles(&self) -> Result<(), String> {
        self.set_property("sid", json!("no"))
    }

    fn set_property(&self, property: &str, value: Value) -> Result<(), String> {
        self.command(json!(["set_property", property, value]))
    }

    fn command(&self, command: Value) -> Result<(), String> {
        self.command_sender
            .send(VideoCommand::Ipc(json!({ "command": command })))
            .map_err(|_| "The mpv IPC writer is no longer running".to_owned())
    }

    fn stop_process(&mut self) {
        self.quit_requested = true;
        let _ = self.command(json!(["quit"]));
        let deadline = Instant::now() + CLEANUP_GRACE;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    self.exit_reported = true;
                    self.stop.store(true, Ordering::Release);
                    return;
                }
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        self.stop.store(true, Ordering::Release);
        let _ = self.job.terminate();
        let _ = self.child.kill();
        let _ = self.child.try_wait();
    }
}

impl Drop for VideoSession {
    fn drop(&mut self) {
        self.stop_process();
    }
}

pub struct VideoTile {
    _host: VideoHost,
    pub session: VideoSession,
}

/// Owns the complete runtime for each video preview.
#[derive(Default)]
pub struct VideoManager {
    tiles: HashMap<PreviewId, VideoTile>,
}

impl VideoManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn launch(
        &mut self,
        id: PreviewId,
        owner: Option<isize>,
        size: (i32, i32),
        launch: VideoLaunch,
    ) -> Result<isize, String> {
        if self.tiles.contains_key(&id) {
            return Err(format!("A video session already exists for {id:?}"));
        }
        let host = VideoHost::new(owner, size.0, size.1)?;
        let hwnd = host.hwnd();
        let pipe_name = unique_pipe_name();
        let initial_paused = launch.start_paused;
        let (executable, args) = match launch.source {
            VideoSource::LocalFile(path) => (
                launch.mpv_path,
                build_mpv_args(hwnd, &pipe_name, launch.start_paused, Some(&path)),
            ),
            VideoSource::Stream {
                url,
                quality,
                streamlink_path,
            } => {
                let mpv_args = build_mpv_args(hwnd, &pipe_name, launch.start_paused, None);
                (
                    streamlink_path,
                    build_streamlink_args(&launch.mpv_path, &mpv_args, &url, &quality),
                )
            }
        };
        let session = VideoSession::spawn(&executable, &args, pipe_name, initial_paused)?;
        self.tiles.insert(
            id,
            VideoTile {
                _host: host,
                session,
            },
        );
        Ok(hwnd)
    }

    #[cfg(test)]
    pub fn launch_local(
        &mut self,
        id: PreviewId,
        owner: Option<isize>,
        mpv_path: PathBuf,
        media_path: PathBuf,
        start_paused: bool,
    ) -> Result<isize, String> {
        self.launch(
            id,
            owner,
            (1280, 720),
            VideoLaunch {
                mpv_path,
                source: VideoSource::LocalFile(media_path),
                start_paused,
            },
        )
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn launch_stream(
        &mut self,
        id: PreviewId,
        owner: Option<isize>,
        streamlink_path: PathBuf,
        mpv_path: PathBuf,
        url: String,
        quality: String,
        start_paused: bool,
    ) -> Result<isize, String> {
        self.launch(
            id,
            owner,
            (1280, 720),
            VideoLaunch {
                mpv_path,
                source: VideoSource::Stream {
                    url,
                    quality,
                    streamlink_path,
                },
                start_paused,
            },
        )
    }

    pub fn contains(&self, id: PreviewId) -> bool {
        self.tiles.contains_key(&id)
    }

    pub fn get(&self, id: PreviewId) -> Option<&VideoTile> {
        self.tiles.get(&id)
    }

    pub fn get_mut(&mut self, id: PreviewId) -> Option<&mut VideoTile> {
        self.tiles.get_mut(&id)
    }

    pub fn remove(&mut self, id: PreviewId) -> Option<VideoTile> {
        self.tiles.remove(&id)
    }

    pub fn clear(&mut self) {
        self.tiles.clear();
    }

    pub fn ids(&self) -> impl Iterator<Item = PreviewId> + '_ {
        self.tiles.keys().copied()
    }

    pub fn poll(&mut self) -> Vec<(PreviewId, VideoUpdate)> {
        let mut updates = Vec::new();
        for (id, tile) in &mut self.tiles {
            updates.extend(tile.session.poll().into_iter().map(|update| (*id, update)));
        }
        updates
    }
}

fn build_mpv_args(
    hwnd: isize,
    pipe_name: &str,
    start_paused: bool,
    media_path: Option<&Path>,
) -> Vec<OsString> {
    // mpv adds the Win32 pipe namespace when it is omitted. Passing only the
    // base name also prevents Streamlink's player-argument tokenizer from
    // interpreting backslashes before it launches mpv.
    let pipe_option = pipe_name.strip_prefix(r"\\.\pipe\").unwrap_or(pipe_name);
    let mut args = vec![
        "--no-config".into(),
        "--terminal=no".into(),
        "--osc=no".into(),
        "--input-default-bindings=no".into(),
        "--force-window=yes".into(),
        "--idle=yes".into(),
        "--keep-open=yes".into(),
        "--hwdec=auto-safe".into(),
        format!("--wid={hwnd}").into(),
        format!("--input-ipc-server={pipe_option}").into(),
    ];
    if start_paused {
        args.push("--pause=yes".into());
    }
    if let Some(path) = media_path {
        args.push(path.as_os_str().to_owned());
    }
    args
}

fn build_streamlink_args(
    mpv_path: &Path,
    mpv_args: &[OsString],
    url: &str,
    quality: &str,
) -> Vec<OsString> {
    vec![
        "--player".into(),
        mpv_path.as_os_str().to_owned(),
        "--player-args".into(),
        join_player_args(mpv_args).into(),
        "--player-continuous-http".into(),
        url.into(),
        quality.into(),
    ]
}

fn join_player_args(args: &[OsString]) -> String {
    args.iter()
        .map(|argument| quote_player_argument(&argument.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_player_argument(argument: &str) -> String {
    if !argument.is_empty()
        && !argument
            .chars()
            .any(|character| character.is_whitespace() || character == '"' || character == '\\')
    {
        return argument.to_owned();
    }

    let mut quoted = String::from("\"");
    let mut backslashes = 0;
    for character in argument.chars() {
        if character == '\\' {
            backslashes += 1;
        } else if character == '"' {
            quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
            quoted.push('"');
            backslashes = 0;
        } else {
            quoted.push_str(&"\\".repeat(backslashes));
            backslashes = 0;
            quoted.push(character);
        }
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

fn unique_pipe_name() -> String {
    let sequence = PIPE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        r"\\.\pipe\pluriview-mpv-{}-{sequence}-{nanos}",
        std::process::id()
    )
}

#[derive(Debug)]
pub struct VideoThumbnail {
    pub time: f64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug)]
pub enum VideoThumbnailSource {
    LocalFile(PathBuf),
    Stream {
        streamlink_path: PathBuf,
        url: String,
        quality: String,
    },
}

pub type VideoThumbnailReceiver = Receiver<Result<VideoThumbnail, String>>;

/// Decode one local or seekable stream frame in a background process. The
/// primary playback session is never sought or paused, so timeline hovering
/// cannot disrupt it.
pub fn spawn_video_thumbnail(
    mpv_path: PathBuf,
    source: VideoThumbnailSource,
    time: f64,
) -> VideoThumbnailReceiver {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(extract_video_thumbnail(&mpv_path, &source, time));
    });
    receiver
}

fn extract_video_thumbnail(
    mpv_path: &Path,
    source: &VideoThumbnailSource,
    time: f64,
) -> Result<VideoThumbnail, String> {
    if !time.is_finite() || time < 0.0 {
        return Err("Thumbnail time must be a non-negative finite number".to_owned());
    }
    let sequence = THUMBNAIL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "pluriview-thumbnail-{}-{sequence}-{nanos}",
        std::process::id()
    ));
    fs::create_dir(&directory)
        .map_err(|error| format!("Could not create thumbnail directory: {error}"))?;

    let result = (|| {
        let input = match source {
            VideoThumbnailSource::LocalFile(path) => path.as_os_str().to_owned(),
            VideoThumbnailSource::Stream {
                streamlink_path,
                url,
                quality,
            } => resolve_stream_url(streamlink_path, url, quality)
                .unwrap_or_else(|_| OsString::from(url)),
        };
        let mut command = Command::new(mpv_path);
        command
            .args([
                "--no-config",
                "--terminal=no",
                "--really-quiet",
                "--audio=no",
                "--sub=no",
                "--hwdec=no",
                "--vo=image",
                "--vo-image-format=jpg",
                "--vo-image-jpeg-quality=75",
                "--frames=1",
            ])
            .arg(format!("--vo-image-outdir={}", directory.to_string_lossy()))
            .arg(format!("--start={time:.3}"))
            .arg(input)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
        let mut child = command
            .spawn()
            .map_err(|error| format!("Could not launch mpv for timeline preview: {error}"))?;
        let deadline = Instant::now() + Duration::from_secs(12);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("Timeline preview decoding timed out".to_owned());
                }
                Err(error) => {
                    return Err(format!("Could not query timeline preview process: {error}"));
                }
            }
        };
        if !status.success() {
            return Err(format!("mpv could not decode the frame ({status})"));
        }

        let image_path = fs::read_dir(&directory)
            .map_err(|error| format!("Could not read thumbnail output: {error}"))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("jpg"))
            })
            .ok_or_else(|| "mpv produced no timeline preview image".to_owned())?;
        let image = image::open(&image_path)
            .map_err(|error| format!("Could not decode timeline preview: {error}"))?
            .thumbnail(320, 180)
            .into_rgba8();
        let (width, height) = image.dimensions();
        let _ = fs::remove_file(image_path);
        Ok(VideoThumbnail {
            time,
            width,
            height,
            rgba: image.into_raw(),
        })
    })();

    if let Ok(entries) = fs::read_dir(&directory) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }
    let _ = fs::remove_dir(&directory);
    result
}

/// Resolve Streamlink VODs to their underlying HLS/HTTP URL so a separate MPV
/// process can seek without disturbing the playing Streamlink session. If the
/// plugin cannot expose a URL, the caller can still try MPV's own URL hooks.
pub(crate) fn resolve_stream_url(
    streamlink_path: &Path,
    url: &str,
    quality: &str,
) -> Result<OsString, String> {
    let mut command = Command::new(streamlink_path);
    command
        .arg("--stream-url")
        .arg(url)
        .arg(quality)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not resolve stream preview URL: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Resolving the stream preview URL timed out".to_owned());
            }
            Err(error) => return Err(format!("Could not query Streamlink: {error}")),
        }
    };
    if !status.success() {
        return Err("Streamlink could not expose a seekable preview URL".to_owned());
    }
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .ok_or_else(|| "Streamlink returned no preview URL".to_owned())?
        .read_to_string(&mut stdout)
        .map_err(|error| format!("Could not read Streamlink preview URL: {error}"))?;
    let resolved = stdout.trim();
    if resolved.is_empty() {
        Err("Streamlink returned an empty preview URL".to_owned())
    } else {
        Ok(OsString::from(resolved))
    }
}

fn start_ipc_worker(
    pipe_name: String,
    command_receiver: Receiver<VideoCommand>,
    event_sender: Sender<WorkerEvent>,
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let deadline = Instant::now() + IPC_CONNECT_TIMEOUT;
        let pipe = loop {
            if stop.load(Ordering::Acquire) {
                return;
            }
            match OpenOptions::new().read(true).write(true).open(&pipe_name) {
                Ok(pipe) => break pipe,
                Err(error) if Instant::now() < deadline => {
                    let _ = error;
                    thread::sleep(IPC_RETRY_DELAY);
                }
                Err(error) => {
                    let _ = event_sender.send(WorkerEvent::Error(format!(
                        "Could not connect to mpv IPC at {pipe_name}: {error}"
                    )));
                    return;
                }
            }
        };

        let reader = match pipe.try_clone() {
            Ok(reader) => reader,
            Err(error) => {
                let _ = event_sender.send(WorkerEvent::Error(format!(
                    "Could not clone the mpv IPC pipe: {error}"
                )));
                return;
            }
        };
        let _ = event_sender.send(WorkerEvent::Connected);
        start_ipc_reader(reader, event_sender.clone(), stop.clone());

        let mut writer = pipe;
        for property in OBSERVED_PROPERTIES {
            let command = json!({
                "command": ["observe_property", 0, property]
            });
            if let Err(error) = write_ipc_command(&mut writer, &command) {
                let _ = event_sender.send(WorkerEvent::Error(error));
                return;
            }
        }

        loop {
            if stop.load(Ordering::Acquire) {
                return;
            }
            match command_receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(command) => {
                    for command in coalesce_pending_commands(command, &command_receiver) {
                        if let Err(error) = write_ipc_command(&mut writer, &command) {
                            let _ = event_sender.send(WorkerEvent::Error(error));
                            return;
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
}

/// Drain the commands already waiting for the writer and retain only the most
/// recent volume change. Slider drags can otherwise enqueue dozens of values
/// faster than mpv can acknowledge them, making the audible volume trail the
/// pointer long after the drag has ended.
fn coalesce_pending_commands(first: VideoCommand, receiver: &Receiver<VideoCommand>) -> Vec<Value> {
    let mut commands = vec![first];
    while let Ok(command) = receiver.try_recv() {
        commands.push(command);
    }

    let last_volume = commands
        .iter()
        .rposition(|command| matches!(command, VideoCommand::SetVolume(_)));
    commands
        .into_iter()
        .enumerate()
        .filter_map(|(index, command)| match command {
            VideoCommand::Ipc(command) => Some(command),
            VideoCommand::SetVolume(volume) if Some(index) == last_volume => {
                Some(json!({ "command": ["set_property", "volume", volume] }))
            }
            VideoCommand::SetVolume(_) => None,
        })
        .collect()
}

const OBSERVED_PROPERTIES: [&str; 14] = [
    "pause",
    "time-pos",
    "duration",
    "volume",
    "mute",
    "speed",
    "loop-file",
    "track-list",
    "aid",
    "sid",
    "media-title",
    "paused-for-cache",
    "core-idle",
    "eof-reached",
];

fn start_ipc_reader(pipe: File, event_sender: Sender<WorkerEvent>, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    if !stop.load(Ordering::Acquire) {
                        let _ = event_sender.send(WorkerEvent::Error(
                            "The mpv IPC pipe closed unexpectedly".to_owned(),
                        ));
                    }
                    return;
                }
                Ok(_) => {
                    let line = line.trim_end_matches(['\r', '\n']);
                    if !line.is_empty() {
                        let _ = event_sender.send(WorkerEvent::Line(line.to_owned()));
                    }
                }
                Err(error) => {
                    if !stop.load(Ordering::Acquire) {
                        let _ = event_sender.send(WorkerEvent::Error(format!(
                            "Could not read mpv IPC: {error}"
                        )));
                    }
                    return;
                }
            }
        }
    });
}

fn write_ipc_command(pipe: &mut File, command: &Value) -> Result<(), String> {
    serde_json::to_writer(&mut *pipe, command)
        .map_err(|error| format!("Could not encode an mpv command: {error}"))?;
    pipe.write_all(b"\n")
        .and_then(|_| pipe.flush())
        .map_err(|error| format!("Could not write to mpv IPC: {error}"))
}

fn parse_ipc_line(line: &str, state: &mut VideoState, updates: &mut Vec<VideoUpdate>) {
    let message: Value = match serde_json::from_str(line) {
        Ok(message) => message,
        Err(error) => {
            updates.push(VideoUpdate::Error(format!(
                "mpv returned invalid JSON: {error}"
            )));
            return;
        }
    };

    if let Some(error) = message.get("error").and_then(Value::as_str) {
        if error != "success" {
            updates.push(VideoUpdate::Error(format!("mpv IPC error: {error}")));
        }
    }

    let Some(event) = message.get("event").and_then(Value::as_str) else {
        return;
    };
    if event != "property-change" {
        updates.push(VideoUpdate::Event);
        return;
    }
    let Some(name) = message.get("name").and_then(Value::as_str) else {
        return;
    };
    let data = message.get("data").unwrap_or(&Value::Null);
    if let Some(property) = apply_property(state, name, data) {
        updates.push(VideoUpdate::Property(property));
    }
}

fn apply_property(state: &mut VideoState, name: &str, data: &Value) -> Option<VideoProperty> {
    match name {
        "pause" => {
            state.pause = data.as_bool()?;
            Some(VideoProperty::Pause)
        }
        "time-pos" => {
            state.time_pos = optional_f64(data)?;
            Some(VideoProperty::TimePosition)
        }
        "duration" => {
            state.duration = optional_f64(data)?;
            Some(VideoProperty::Duration)
        }
        "volume" => {
            state.volume = data.as_f64()?;
            Some(VideoProperty::Volume)
        }
        "mute" => {
            state.mute = data.as_bool()?;
            Some(VideoProperty::Mute)
        }
        "speed" => {
            state.speed = data.as_f64()?;
            Some(VideoProperty::Speed)
        }
        "loop-file" => {
            state.loop_file = parse_loop_mode(data)?;
            Some(VideoProperty::LoopFile)
        }
        "track-list" => {
            state.track_list = serde_json::from_value(data.clone()).ok()?;
            Some(VideoProperty::TrackList)
        }
        "aid" => {
            state.audio_track = parse_track_selection(data);
            Some(VideoProperty::AudioTrack)
        }
        "sid" => {
            state.subtitle_track = parse_track_selection(data);
            Some(VideoProperty::SubtitleTrack)
        }
        "media-title" => {
            state.media_title = data.as_str().map(str::to_owned);
            Some(VideoProperty::MediaTitle)
        }
        "paused-for-cache" => {
            state.paused_for_cache = data.as_bool()?;
            Some(VideoProperty::PausedForCache)
        }
        "core-idle" => {
            state.core_idle = data.as_bool()?;
            Some(VideoProperty::CoreIdle)
        }
        "eof-reached" => {
            state.eof_reached = data.as_bool()?;
            Some(VideoProperty::EofReached)
        }
        _ => None,
    }
}

fn optional_f64(value: &Value) -> Option<Option<f64>> {
    if value.is_null() {
        Some(None)
    } else {
        value.as_f64().map(Some)
    }
}

fn parse_loop_mode(value: &Value) -> Option<LoopMode> {
    match value {
        Value::Bool(false) | Value::Null => Some(LoopMode::Off),
        Value::String(value) if value == "no" => Some(LoopMode::Off),
        Value::String(value) if value == "inf" || value == "yes" => Some(LoopMode::Infinite),
        Value::Number(value) => value.as_u64().map(LoopMode::Count),
        Value::String(value) => value.parse().ok().map(LoopMode::Count),
        _ => None,
    }
}

fn parse_track_selection(value: &Value) -> TrackSelection {
    match value {
        Value::Number(value) => value
            .as_i64()
            .map(TrackSelection::Id)
            .unwrap_or(TrackSelection::Disabled),
        Value::String(value) if value == "no" || value == "auto" => TrackSelection::Disabled,
        Value::String(value) => value
            .parse()
            .map(TrackSelection::Id)
            .unwrap_or_else(|_| TrackSelection::Other(value.clone())),
        _ => TrackSelection::Disabled,
    }
}

fn drain_stderr(stderr: impl std::io::Read + Send + 'static, tail: Arc<Mutex<VecDeque<String>>>) {
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let mut tail = lock_tail(&tail);
            if tail.len() == STDERR_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
        }
    });
}

fn lock_tail(tail: &Mutex<VecDeque<String>>) -> std::sync::MutexGuard<'_, VecDeque<String>> {
    tail.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn resume_process(process_id: u32) -> Result<(), String> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }
        .map_err(|error| format!("Could not enumerate the video process threads: {error}"))?;
    let result = (|| {
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        unsafe { Thread32First(snapshot, &mut entry) }
            .map_err(|error| format!("Could not inspect the video process thread: {error}"))?;
        loop {
            if entry.th32OwnerProcessID == process_id {
                let thread =
                    unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID) }
                        .map_err(|error| {
                            format!("Could not open the suspended video process thread: {error}")
                        })?;
                let previous_count = unsafe { ResumeThread(thread) };
                unsafe {
                    let _ = CloseHandle(thread);
                }
                return if previous_count == u32::MAX {
                    Err("Could not resume the suspended video process".to_owned())
                } else {
                    Ok(())
                };
            }
            if unsafe { Thread32Next(snapshot, &mut entry) }.is_err() {
                return Err("Could not find the suspended video process thread".to_owned());
            }
        }
    })();
    unsafe {
        let _ = CloseHandle(snapshot);
    }
    result
}

struct JobObject(HANDLE);

impl JobObject {
    fn new() -> Result<Self, String> {
        let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
            .map_err(|error| format!("Could not create the video process job: {error}"))?;
        let job = Self(handle);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const c_void,
                std::mem::size_of_val(&limits) as u32,
            )
            .map_err(|error| format!("Could not configure the video process job: {error}"))?;
        }
        Ok(job)
    }

    fn assign(&self, child: &Child) -> Result<(), String> {
        let process = HANDLE(child.as_raw_handle());
        unsafe { AssignProcessToJobObject(self.0, process) }
            .map_err(|error| format!("Could not assign the video process to its job: {error}"))
    }

    fn terminate(&self) -> Result<(), String> {
        unsafe { TerminateJobObject(self.0, 1) }
            .map_err(|error| format!("Could not terminate the video process job: {error}"))
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_mpv_args, build_streamlink_args, coalesce_pending_commands, extract_video_thumbnail,
        parse_ipc_line, LoopMode, TrackSelection, VideoCommand, VideoManager, VideoProperty,
        VideoState, VideoThumbnailSource, VideoUpdate,
    };
    use crate::preview::PreviewId;
    use serde_json::json;
    use std::{
        ffi::OsString,
        path::{Path, PathBuf},
        sync::mpsc,
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    use windows::Win32::{
        Foundation::CloseHandle,
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
    };

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn process_snapshot() -> Vec<(u32, u32)> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.unwrap();
        let mut entries = Vec::new();
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok() {
            loop {
                entries.push((entry.th32ProcessID, entry.th32ParentProcessID));
                if unsafe { Process32NextW(snapshot, &mut entry) }.is_err() {
                    break;
                }
            }
        }
        unsafe {
            let _ = CloseHandle(snapshot);
        }
        entries
    }

    fn process_tree(root: u32) -> Vec<u32> {
        let entries = process_snapshot();
        let mut tree = vec![root];
        let mut index = 0;
        while index < tree.len() {
            let parent = tree[index];
            for (pid, parent_pid) in &entries {
                if *parent_pid == parent && !tree.contains(pid) {
                    tree.push(*pid);
                }
            }
            index += 1;
        }
        tree
    }

    fn wait_for_processes_to_exit(processes: &[u32]) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let running: Vec<_> = process_snapshot()
                .into_iter()
                .map(|(pid, _)| pid)
                .filter(|pid| processes.contains(pid))
                .collect();
            if running.is_empty() {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let running: Vec<_> = process_snapshot()
            .into_iter()
            .map(|(pid, _)| pid)
            .filter(|pid| processes.contains(pid))
            .collect();
        assert!(
            running.is_empty(),
            "video processes still running: {running:?}"
        );
    }

    #[test]
    fn mpv_arguments_are_deterministic_and_keep_media_path_atomic() {
        let media = Path::new(r"C:\Video Library\a film.mkv");
        let args = strings(build_mpv_args(
            4242,
            r"\\.\pipe\pluriview-test",
            true,
            Some(media),
        ));

        assert_eq!(
            args,
            vec![
                "--no-config",
                "--terminal=no",
                "--osc=no",
                "--input-default-bindings=no",
                "--force-window=yes",
                "--idle=yes",
                "--keep-open=yes",
                "--hwdec=auto-safe",
                "--wid=4242",
                "--input-ipc-server=pluriview-test",
                "--pause=yes",
                r"C:\Video Library\a film.mkv",
            ]
        );
    }

    #[test]
    fn streamlink_arguments_keep_executable_and_url_with_spaces_atomic() {
        let mpv = Path::new(r"C:\Program Files\mpv\mpv.exe");
        let mpv_args = vec![
            OsString::from("--no-config"),
            OsString::from(r"--script-opts=cache-dir=C:\A Folder"),
            OsString::from(r"--input-ipc-server=\\.\pipe\pluriview-test"),
        ];
        let args = strings(build_streamlink_args(
            mpv,
            &mpv_args,
            "https://example.test/watch?v=hello world",
            "best",
        ));

        assert_eq!(args[0], "--player");
        assert_eq!(args[1], r"C:\Program Files\mpv\mpv.exe");
        assert_eq!(args[2], "--player-args");
        assert_eq!(
            args[3],
            r#"--no-config "--script-opts=cache-dir=C:\A Folder" "--input-ipc-server=\\.\pipe\pluriview-test""#
        );
        assert_eq!(args[4], "--player-continuous-http");
        assert_eq!(args[5], "https://example.test/watch?v=hello world");
        assert_eq!(args[6], "best");
    }

    #[test]
    fn json_property_events_update_video_state() {
        let mut state = VideoState::default();
        let mut updates = Vec::new();
        for line in [
            r#"{"event":"property-change","name":"pause","data":true}"#,
            r#"{"event":"property-change","name":"time-pos","data":12.5}"#,
            r#"{"event":"property-change","name":"duration","data":90.0}"#,
            r#"{"event":"property-change","name":"volume","data":42.0}"#,
            r#"{"event":"property-change","name":"mute","data":true}"#,
            r#"{"event":"property-change","name":"speed","data":1.25}"#,
            r#"{"event":"property-change","name":"loop-file","data":"inf"}"#,
            r#"{"event":"property-change","name":"aid","data":2}"#,
            r#"{"event":"property-change","name":"sid","data":"no"}"#,
            r#"{"event":"property-change","name":"media-title","data":"Demo"}"#,
            r#"{"event":"property-change","name":"paused-for-cache","data":true}"#,
            r#"{"event":"property-change","name":"core-idle","data":false}"#,
            r#"{"event":"property-change","name":"eof-reached","data":false}"#,
            r#"{"event":"property-change","name":"track-list","data":[{"id":2,"type":"audio","lang":"en","selected":true},{"id":3,"type":"sub","title":"English"}]}"#,
        ] {
            parse_ipc_line(line, &mut state, &mut updates);
        }

        assert!(state.pause);
        assert_eq!(state.time_pos, Some(12.5));
        assert_eq!(state.duration, Some(90.0));
        assert_eq!(state.volume, 42.0);
        assert!(state.mute);
        assert_eq!(state.speed, 1.25);
        assert_eq!(state.loop_file, LoopMode::Infinite);
        assert_eq!(state.audio_track, TrackSelection::Id(2));
        assert_eq!(state.subtitle_track, TrackSelection::Disabled);
        assert_eq!(state.media_title.as_deref(), Some("Demo"));
        assert!(state.paused_for_cache);
        assert!(!state.core_idle);
        assert!(!state.eof_reached);
        assert_eq!(state.track_list.len(), 2);
        assert_eq!(state.track_list[0].lang.as_deref(), Some("en"));
        assert!(updates
            .iter()
            .any(|update| matches!(update, VideoUpdate::Property(VideoProperty::TrackList))));
    }

    #[test]
    fn json_null_time_and_protocol_errors_are_surfaced() {
        let mut state = VideoState {
            time_pos: Some(4.0),
            ..Default::default()
        };
        let mut updates = Vec::new();
        parse_ipc_line(
            r#"{"event":"property-change","name":"time-pos","data":null}"#,
            &mut state,
            &mut updates,
        );
        parse_ipc_line(
            r#"{"error":"property unavailable"}"#,
            &mut state,
            &mut updates,
        );
        parse_ipc_line("{broken", &mut state, &mut updates);

        assert_eq!(state.time_pos, None);
        assert_eq!(
            updates
                .iter()
                .filter(|update| matches!(update, VideoUpdate::Error(_)))
                .count(),
            2
        );
    }

    #[test]
    fn queued_volume_changes_are_coalesced_to_the_latest_value() {
        let (sender, receiver) = mpsc::channel();
        sender.send(VideoCommand::SetVolume(20.0)).unwrap();
        sender
            .send(VideoCommand::Ipc(json!({ "command": ["cycle", "pause"] })))
            .unwrap();
        sender.send(VideoCommand::SetVolume(30.0)).unwrap();

        let commands = coalesce_pending_commands(VideoCommand::SetVolume(10.0), &receiver);

        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0], json!({ "command": ["cycle", "pause"] }));
        assert_eq!(
            commands[1],
            json!({ "command": ["set_property", "volume", 30.0] })
        );
    }

    #[test]
    #[ignore = "requires PLURIVIEW_TEST_MPV pointing to an installed mpv.exe"]
    fn installed_mpv_runs_two_embedded_ipc_sessions_and_cleans_up() {
        let mpv = PathBuf::from(
            std::env::var_os("PLURIVIEW_TEST_MPV")
                .expect("set PLURIVIEW_TEST_MPV to an installed mpv.exe"),
        );
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let media = std::env::temp_dir().join(format!(
            "pluriview-mpv-smoke-{}-{nonce}.png",
            std::process::id()
        ));
        image::RgbaImage::from_pixel(8, 8, image::Rgba([40, 120, 220, 255]))
            .save(&media)
            .unwrap();

        let mut manager = VideoManager::new();
        for id in [PreviewId(7001), PreviewId(7002)] {
            manager
                .launch_local(id, None, mpv.clone(), media.clone(), true)
                .unwrap();
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let _ = manager.poll();
            if [PreviewId(7001), PreviewId(7002)].iter().all(|id| {
                manager
                    .get(*id)
                    .is_some_and(|tile| tile.session.state().connected)
            }) {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }

        for id in [PreviewId(7001), PreviewId(7002)] {
            let tile = manager
                .get_mut(id)
                .expect("video session should still exist");
            assert!(tile.session.state().connected, "mpv IPC did not connect");
            tile.session.play().unwrap();
            tile.session.set_volume(37.0).unwrap();
        }
        let play_deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < play_deadline {
            let _ = manager.poll();
            if [PreviewId(7001), PreviewId(7002)].iter().all(|id| {
                manager
                    .get(*id)
                    .is_some_and(|tile| !tile.session.state().pause)
            }) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!([PreviewId(7001), PreviewId(7002)].iter().all(|id| {
            manager
                .get(*id)
                .is_some_and(|tile| !tile.session.state().pause)
        }));

        let thumbnail =
            extract_video_thumbnail(&mpv, &VideoThumbnailSource::LocalFile(media.clone()), 0.0)
                .unwrap();
        assert!(thumbnail.width > 0 && thumbnail.height > 0);
        assert_eq!(
            thumbnail.rgba.len(),
            (thumbnail.width * thumbnail.height * 4) as usize
        );
        let processes: Vec<_> = [PreviewId(7001), PreviewId(7002)]
            .into_iter()
            .flat_map(|id| process_tree(manager.get(id).unwrap().session.child.id()))
            .collect();
        manager.clear();
        assert_eq!(manager.ids().count(), 0);
        wait_for_processes_to_exit(&processes);
        std::fs::remove_file(media).unwrap();
    }

    #[test]
    #[ignore = "requires PLURIVIEW_TEST_MPV and PLURIVIEW_TEST_VIDEO"]
    fn installed_mpv_pause_then_play_advances_a_real_video() {
        let mpv = PathBuf::from(
            std::env::var_os("PLURIVIEW_TEST_MPV")
                .expect("set PLURIVIEW_TEST_MPV to an installed mpv.exe"),
        );
        let media = PathBuf::from(
            std::env::var_os("PLURIVIEW_TEST_VIDEO")
                .expect("set PLURIVIEW_TEST_VIDEO to a multi-second video"),
        );
        let id = PreviewId(7003);
        let mut manager = VideoManager::new();
        manager
            .launch_local(id, None, mpv.clone(), media.clone(), false)
            .unwrap();

        let playing_deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < playing_deadline {
            let _ = manager.poll();
            if manager.get(id).is_some_and(|tile| {
                tile.session.state().connected
                    && tile
                        .session
                        .state()
                        .time_pos
                        .is_some_and(|time| time > 0.25)
            }) {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }

        manager
            .get_mut(id)
            .unwrap()
            .session
            .set_paused(true)
            .unwrap();
        let pause_deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < pause_deadline {
            let _ = manager.poll();
            if manager
                .get(id)
                .is_some_and(|tile| tile.session.state().pause)
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let paused_at = manager
            .get(id)
            .and_then(|tile| tile.session.state().time_pos)
            .expect("real video did not report a paused position");
        thread::sleep(Duration::from_millis(250));
        let _ = manager.poll();

        manager.get_mut(id).unwrap().session.play().unwrap();
        let resume_deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < resume_deadline {
            let _ = manager.poll();
            if manager.get(id).is_some_and(|tile| {
                !tile.session.state().pause
                    && tile
                        .session
                        .state()
                        .time_pos
                        .is_some_and(|time| time > paused_at + 0.2)
            }) {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let state = manager.get(id).unwrap().session.state();
        assert!(!state.pause, "mpv still reports pause after Play");
        assert!(
            state.time_pos.is_some_and(|time| time > paused_at + 0.2),
            "playback did not advance after Play: paused at {paused_at:?}, now {:?}",
            state.time_pos
        );

        manager
            .get_mut(id)
            .unwrap()
            .session
            .set_paused(true)
            .unwrap();
        let second_pause_deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < second_pause_deadline {
            let _ = manager.poll();
            if manager
                .get(id)
                .is_some_and(|tile| tile.session.state().pause)
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let restart_at = manager
            .get(id)
            .and_then(|tile| tile.session.state().time_pos)
            .expect("real video did not report the second paused position");
        manager.remove(id);
        manager.launch_local(id, None, mpv, media, false).unwrap();
        manager
            .get(id)
            .unwrap()
            .session
            .seek_absolute(restart_at)
            .unwrap();
        let restart_deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < restart_deadline {
            let _ = manager.poll();
            if manager.get(id).is_some_and(|tile| {
                tile.session
                    .state()
                    .time_pos
                    .is_some_and(|time| time > restart_at + 0.2)
            }) {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            manager.get(id).is_some_and(|tile| {
                tile.session
                    .state()
                    .time_pos
                    .is_some_and(|time| time > restart_at + 0.2)
            }),
            "restarted playback did not continue after the paused timestamp"
        );
    }

    #[test]
    #[ignore = "requires installed mpv/Streamlink and a live test URL"]
    fn installed_streamlink_launches_embedded_mpv_and_cleans_up() {
        let mpv = PathBuf::from(
            std::env::var_os("PLURIVIEW_TEST_MPV")
                .expect("set PLURIVIEW_TEST_MPV to an installed mpv.exe"),
        );
        let streamlink = PathBuf::from(
            std::env::var_os("PLURIVIEW_TEST_STREAMLINK")
                .expect("set PLURIVIEW_TEST_STREAMLINK to streamlink.exe"),
        );
        let url = std::env::var("PLURIVIEW_TEST_STREAM_URL")
            .expect("set PLURIVIEW_TEST_STREAM_URL to a currently live stream");
        let id = PreviewId(7101);
        let mut manager = VideoManager::new();
        manager
            .launch_stream(id, None, streamlink, mpv, url, "best".to_owned(), true)
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(35);
        let mut errors = Vec::new();
        while Instant::now() < deadline {
            for (_, update) in manager.poll() {
                if let VideoUpdate::Error(error) = update {
                    errors.push(error);
                }
            }
            if manager
                .get(id)
                .is_some_and(|tile| tile.session.state().connected)
            {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }

        let tile = manager.get(id).expect("stream session should still exist");
        assert!(
            tile.session.state().connected,
            "Streamlink did not launch an IPC-connected mpv: {errors:?}\n{}",
            tile.session.stderr_tail()
        );
        let processes = process_tree(tile.session.child.id());
        assert!(
            processes.len() >= 2,
            "expected Streamlink to have launched an mpv child"
        );
        manager.clear();
        assert_eq!(manager.ids().count(), 0);
        wait_for_processes_to_exit(&processes);
    }
}
