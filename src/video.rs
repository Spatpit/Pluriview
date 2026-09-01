//! Shared video launch models, Streamlink resolution, and optional mpv.exe
//! thumbnail extraction. Production playback lives in `crate::libmpv`.

use std::{
    ffi::OsString,
    fs,
    io::Read,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::preview::VideoSource;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
static THUMBNAIL_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub struct VideoLaunch {
    pub source: VideoSource,
    pub streamlink_path: Option<PathBuf>,
    pub start_paused: bool,
    /// Loop muted, fill the paint rectangle, and disable audio output.
    pub wallpaper: bool,
}

#[derive(Clone, Debug)]
pub enum VideoUpdate {
    Connected,
    PauseChanged,
    Event,
    Error(String),
}

#[derive(Clone, Debug)]
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

/// Resolve Streamlink VODs to their underlying HLS/HTTP URL so libmpv or a
/// thumbnail helper can consume the stream directly.
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

#[cfg(test)]
pub struct VideoHost {
    hwnd: windows::Win32::Foundation::HWND,
}

#[cfg(test)]
impl VideoHost {
    pub fn new(owner: Option<isize>, width: i32, height: i32) -> Result<Self, String> {
        use std::ffi::c_void;
        use std::sync::OnceLock;
        use windows::core::w;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, RegisterClassW, ShowWindow, SW_SHOWNOACTIVATE,
            WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
        };

        unsafe extern "system" fn window_proc(
            hwnd: HWND,
            message: u32,
            wparam: windows::Win32::Foundation::WPARAM,
            lparam: windows::Win32::Foundation::LPARAM,
        ) -> windows::Win32::Foundation::LRESULT {
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }

        static REGISTERED: OnceLock<Result<(), String>> = OnceLock::new();
        REGISTERED
            .get_or_init(|| {
                let class = WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    hInstance: unsafe { GetModuleHandleW(None) }
                        .map_err(|error| error.to_string())?
                        .into(),
                    lpszClassName: w!("PluriviewVideoTestHost"),
                    ..Default::default()
                };
                let atom = unsafe { RegisterClassW(&class) };
                if atom == 0 {
                    Err(windows::core::Error::from_win32().to_string())
                } else {
                    Ok(())
                }
            })
            .clone()?;

        let owner = owner.map_or_else(HWND::default, |raw| HWND(raw as *mut c_void));
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("PluriviewVideoTestHost"),
                w!("Pluriview Video Test"),
                WS_POPUP,
                -30_000,
                -30_000,
                width.max(1),
                height.max(1),
                owner,
                None,
                None,
                None,
            )
            .map_err(|error| format!("Could not create the video test window: {error}"))?
        };
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        Ok(Self { hwnd })
    }

    pub fn hwnd(&self) -> isize {
        self.hwnd.0 as isize
    }
}

#[cfg(test)]
impl Drop for VideoHost {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_video_thumbnail, VideoThumbnailSource};
    use std::path::Path;

    #[test]
    fn invalid_thumbnail_time_is_rejected_before_launching_mpv() {
        let result = extract_video_thumbnail(
            Path::new("missing-mpv.exe"),
            &VideoThumbnailSource::LocalFile("missing-video.mp4".into()),
            -1.0,
        );
        assert!(result.is_err());
    }
}
