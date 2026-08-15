use std::cmp::Ordering;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering as AtomicOrdering},
    mpsc::{self, Receiver, Sender, TryRecvError},
    Arc,
};
use std::time::{Duration, Instant};

const VALIDATION_TIMEOUT: Duration = Duration::from_secs(5);
const QUALITY_PROBE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolKind {
    Mpv,
    Streamlink,
}

impl ToolKind {
    pub const ALL: [Self; 2] = [Self::Mpv, Self::Streamlink];

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Mpv => "mpv",
            Self::Streamlink => "Streamlink",
        }
    }

    fn executable_name(self) -> &'static str {
        match self {
            Self::Mpv => "mpv.exe",
            Self::Streamlink => "streamlink.exe",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiscoverySource {
    Override,
    ExecutableDirectory,
    Path,
    CommonLocation,
}

impl DiscoverySource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Override => "Manual override",
            Self::ExecutableDirectory => "Beside Pluriview",
            Self::Path => "PATH",
            Self::CommonLocation => "Common install location",
        }
    }
}

#[derive(Clone, Debug)]
pub enum ToolStatus {
    Checking,
    Available {
        path: PathBuf,
        version: String,
        source: DiscoverySource,
    },
    Invalid {
        path: PathBuf,
        error: String,
        source: DiscoverySource,
    },
    Missing,
}

impl ToolStatus {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Available { path, .. } | Self::Invalid { path, .. } => Some(path),
            Self::Checking | Self::Missing => None,
        }
    }
}

#[derive(Clone, Debug)]
struct Candidate {
    path: PathBuf,
    source: DiscoverySource,
}

#[derive(Clone, Debug)]
struct ToolState {
    override_path: Option<PathBuf>,
    status: ToolStatus,
    generation: u64,
}

impl ToolState {
    fn new(override_path: Option<PathBuf>) -> Self {
        Self {
            override_path,
            status: ToolStatus::Checking,
            generation: 0,
        }
    }
}

struct ScanResult {
    kind: ToolKind,
    generation: u64,
    status: ToolStatus,
}

/// App-global discovery and validation state for optional media helpers.
///
/// Every scan runs on a worker thread. In particular, a slow or broken
/// executable cannot hold up startup or an egui frame while `--version` runs.
pub struct ExternalTools {
    mpv: ToolState,
    streamlink: ToolState,
    sender: Sender<ScanResult>,
    receiver: Receiver<ScanResult>,
}

impl ExternalTools {
    pub fn new(mpv_override: Option<PathBuf>, streamlink_override: Option<PathBuf>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let mut tools = Self {
            mpv: ToolState::new(mpv_override),
            streamlink: ToolState::new(streamlink_override),
            sender,
            receiver,
        };
        tools.rescan_all();
        tools
    }

    pub fn status(&self, kind: ToolKind) -> &ToolStatus {
        &self.state(kind).status
    }

    pub fn override_path(&self, kind: ToolKind) -> Option<&Path> {
        self.state(kind).override_path.as_deref()
    }

    pub fn is_scanning(&self) -> bool {
        ToolKind::ALL
            .iter()
            .any(|kind| matches!(self.status(*kind), ToolStatus::Checking))
    }

    pub fn set_override(&mut self, kind: ToolKind, path: Option<PathBuf>) {
        self.state_mut(kind).override_path = path;
        self.rescan(kind);
    }

    pub fn rescan_all(&mut self) {
        for kind in ToolKind::ALL {
            self.rescan(kind);
        }
    }

    pub fn rescan(&mut self, kind: ToolKind) {
        let state = self.state_mut(kind);
        state.generation = state.generation.wrapping_add(1);
        state.status = ToolStatus::Checking;
        let generation = state.generation;
        let override_path = state.override_path.clone();
        let sender = self.sender.clone();

        std::thread::spawn(move || {
            let status = match discover(kind, override_path.as_deref()) {
                Some(candidate) => validate(kind, candidate),
                None => ToolStatus::Missing,
            };
            let _ = sender.send(ScanResult {
                kind,
                generation,
                status,
            });
        });
    }

    /// Apply completed scans, ignoring stale results from superseded scans.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.receiver.try_recv() {
            let state = self.state_mut(result.kind);
            if state.generation == result.generation {
                state.status = result.status;
                changed = true;
            }
        }
        changed
    }

    fn state(&self, kind: ToolKind) -> &ToolState {
        match kind {
            ToolKind::Mpv => &self.mpv,
            ToolKind::Streamlink => &self.streamlink,
        }
    }

    fn state_mut(&mut self, kind: ToolKind) -> &mut ToolState {
        match kind {
            ToolKind::Mpv => &mut self.mpv,
            ToolKind::Streamlink => &mut self.streamlink,
        }
    }
}

pub struct StreamQualityProbe {
    receiver: Receiver<Result<Vec<String>, String>>,
    cancelled: Arc<AtomicBool>,
}

impl StreamQualityProbe {
    pub fn try_recv(&self) -> Result<Result<Vec<String>, String>, TryRecvError> {
        self.receiver.try_recv()
    }
}

impl Drop for StreamQualityProbe {
    fn drop(&mut self) {
        self.cancelled.store(true, AtomicOrdering::Release);
    }
}

/// Start a non-blocking Streamlink quality probe for a URL.
pub fn probe_stream_qualities(
    streamlink_path: PathBuf,
    url: String,
) -> StreamQualityProbe {
    let (sender, receiver) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = cancelled.clone();
    std::thread::spawn(move || {
        let mut command = Command::new(streamlink_path);
        command.arg("--json").arg(url);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let result =
            command_output_with_timeout(command, QUALITY_PROBE_TIMEOUT, Some(&worker_cancelled))
                .map_err(|error| format!("Could not run Streamlink: {error}"))
                .and_then(|output| {
                if output.status.success() {
                    parse_stream_qualities(&String::from_utf8_lossy(&output.stdout))
                } else {
                    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                    if detail.is_empty() {
                        Err(format!("Streamlink exited with {}.", output.status))
                    } else {
                        Err(detail)
                    }
                }
            });
        if !worker_cancelled.load(AtomicOrdering::Acquire) {
            let _ = sender.send(result);
        }
    });
    StreamQualityProbe {
        receiver,
        cancelled,
    }
}

fn command_output_with_timeout(
    mut command: Command,
    timeout: Duration,
    cancelled: Option<&AtomicBool>,
) -> Result<Output, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start the process: {error}"))?;
    #[cfg(windows)]
    let _job = match HelperProcessJob::new(&child) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let mut stdout = child.stdout.take().expect("stdout was configured as piped");
    let mut stderr = child.stderr.take().expect("stderr was configured as piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        if cancelled.is_some_and(|cancelled| cancelled.load(AtomicOrdering::Acquire)) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err("operation cancelled".to_owned());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("process timed out after {} seconds", timeout.as_secs()));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("could not query process status: {error}"));
            }
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| "stdout reader stopped unexpectedly".to_owned())?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "stderr reader stopped unexpectedly".to_owned())?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(windows)]
struct HelperProcessJob(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl HelperProcessJob {
    fn new(child: &std::process::Child) -> Result<Self, String> {
        use std::ffi::c_void;
        use std::os::windows::io::AsRawHandle;
        use windows::core::PCWSTR;
        use windows::Win32::{
            Foundation::HANDLE,
            System::JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            },
        };

        let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
            .map_err(|error| format!("could not create helper process job: {error}"))?;
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
            .map_err(|error| format!("could not configure helper process job: {error}"))?;
            AssignProcessToJobObject(job.0, HANDLE(child.as_raw_handle()))
                .map_err(|error| format!("could not assign helper process to its job: {error}"))?;
        }
        Ok(job)
    }
}

#[cfg(windows)]
impl Drop for HelperProcessJob {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

pub fn normalize_stream_quality(quality: &str) -> String {
    let quality = quality.trim();
    if quality.is_empty() {
        "best".to_owned()
    } else {
        quality.to_owned()
    }
}

fn parse_stream_qualities(json: &str) -> Result<Vec<String>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| format!("Invalid Streamlink JSON: {error}"))?;
    let streams = value
        .get("streams")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "Streamlink returned no streams object.".to_owned())?;
    let mut qualities: Vec<String> = streams
        .keys()
        .filter(|quality| !quality.trim().is_empty())
        .cloned()
        .collect();
    qualities.sort_by(|left, right| compare_stream_qualities(left, right));
    if qualities.is_empty() {
        Err("Streamlink found no available qualities.".to_owned())
    } else {
        Ok(qualities)
    }
}

fn compare_stream_qualities(left: &str, right: &str) -> Ordering {
    match (left, right) {
        ("best", "best") | ("worst", "worst") => Ordering::Equal,
        ("best", _) | (_, "worst") => Ordering::Less,
        (_, "best") | ("worst", _) => Ordering::Greater,
        _ => match (quality_metrics(left), quality_metrics(right)) {
            (Some(left_metrics), Some(right_metrics)) => right_metrics
                .cmp(&left_metrics)
                .then_with(|| left.cmp(right)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => left.cmp(right),
        },
    }
}

fn quality_metrics(quality: &str) -> Option<(u32, u32)> {
    let p = quality.find('p')?;
    let height = quality[..p].parse().ok()?;
    let suffix = &quality[p + 1..];
    let frame_digits = suffix
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    let frame_rate = frame_digits.parse().unwrap_or(0);
    Some((height, frame_rate))
}

fn discover(kind: ToolKind, override_path: Option<&Path>) -> Option<Candidate> {
    let executable_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let common_locations = common_windows_locations(kind);
    discover_from_locations(
        kind,
        override_path,
        executable_dir.as_deref(),
        &path_dirs,
        &common_locations,
    )
}

/// Resolve a candidate in deterministic precedence order. A manual override is
/// returned even when invalid so the UI reports its problem rather than
/// silently falling back to a different installation.
fn discover_from_locations(
    kind: ToolKind,
    override_path: Option<&Path>,
    executable_dir: Option<&Path>,
    path_dirs: &[PathBuf],
    common_locations: &[PathBuf],
) -> Option<Candidate> {
    if let Some(path) = override_path {
        return Some(Candidate {
            path: path.to_path_buf(),
            source: DiscoverySource::Override,
        });
    }

    let executable_name = kind.executable_name();
    if let Some(path) = executable_dir
        .map(|directory| directory.join(executable_name))
        .filter(|path| path.is_file())
    {
        return Some(Candidate {
            path,
            source: DiscoverySource::ExecutableDirectory,
        });
    }

    if let Some(path) = path_dirs
        .iter()
        .map(|directory| directory.join(executable_name))
        .find(|path| path.is_file())
    {
        return Some(Candidate {
            path,
            source: DiscoverySource::Path,
        });
    }

    common_locations
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .map(|path| Candidate {
            path,
            source: DiscoverySource::CommonLocation,
        })
}

fn validate(kind: ToolKind, candidate: Candidate) -> ToolStatus {
    if !candidate.path.is_file() {
        return ToolStatus::Invalid {
            path: candidate.path,
            error: "The selected file does not exist or is not a file.".to_owned(),
            source: candidate.source,
        };
    }

    let mut command = Command::new(&candidate.path);
    command.arg("--version");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    match command_output_with_timeout(command, VALIDATION_TIMEOUT, None) {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let version = stdout
                .lines()
                .chain(stderr.lines())
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(str::to_owned);
            let expected_name = kind.display_name().to_ascii_lowercase();
            match version {
                Some(version) if version.to_ascii_lowercase().contains(&expected_name) => {
                    ToolStatus::Available {
                        path: candidate.path,
                        version,
                        source: candidate.source,
                    }
                }
                Some(version) => ToolStatus::Invalid {
                    path: candidate.path,
                    error: format!(
                        "`--version` returned unexpected output for {}: {version}",
                        kind.display_name()
                    ),
                    source: candidate.source,
                },
                None => ToolStatus::Invalid {
                    path: candidate.path,
                    error: "`--version` succeeded but returned no version information.".to_owned(),
                    source: candidate.source,
                },
            }
        }
        Ok(output) => ToolStatus::Invalid {
            path: candidate.path,
            error: format!(
                "`--version` exited with {}.",
                output
                    .status
                    .code()
                    .map_or_else(|| "no status code".to_owned(), |code| code.to_string())
            ),
            source: candidate.source,
        },
        Err(error) => ToolStatus::Invalid {
            path: candidate.path,
            error: format!("Could not run `--version`: {error}"),
            source: candidate.source,
        },
    }
}

fn common_windows_locations(kind: ToolKind) -> Vec<PathBuf> {
    let mut locations = Vec::new();
    let executable = kind.executable_name();
    let product = match kind {
        ToolKind::Mpv => "mpv",
        ToolKind::Streamlink => "Streamlink",
    };

    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        locations.push(
            local_app_data
                .join("Programs")
                .join(product)
                .join(executable),
        );
        locations.push(
            local_app_data
                .join("Programs")
                .join(product)
                .join("bin")
                .join(executable),
        );
        locations.push(
            local_app_data
                .join("Microsoft")
                .join("WinGet")
                .join("Links")
                .join(executable),
        );
    }

    for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(program_files) = std::env::var_os(variable).map(PathBuf::from) {
            locations.push(program_files.join(product).join(executable));
            locations.push(program_files.join(product).join("bin").join(executable));
        }
    }

    if let Some(user_profile) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        locations.push(
            user_profile
                .join("scoop")
                .join("apps")
                .join(product.to_ascii_lowercase())
                .join("current")
                .join(executable),
        );
    }

    if let Some(program_data) = std::env::var_os("ProgramData").map(PathBuf::from) {
        locations.push(program_data.join("chocolatey").join("bin").join(executable));
    }

    let home_variable = match kind {
        ToolKind::Mpv => "MPV_HOME",
        ToolKind::Streamlink => "STREAMLINK_HOME",
    };
    if let Some(home) = std::env::var_os(home_variable).map(PathBuf::from) {
        locations.push(home.join(executable));
        locations.push(home.join("bin").join(executable));
    }

    #[cfg(windows)]
    locations.extend(registry_install_locations(kind));

    locations
}

/// Include nonstandard install drives recorded by Windows installers.
#[cfg(windows)]
fn registry_install_locations(kind: ToolKind) -> Vec<PathBuf> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
        KEY_READ,
    };

    const UNINSTALL_KEYS: [(HKEY, &str); 3] = [
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            HKEY_CURRENT_USER,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
    ];

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn read_string(key: HKEY, value_name: &str) -> Option<String> {
        use windows::core::PCWSTR;
        use windows::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_SZ};

        let name = wide(value_name);
        let mut bytes = 0u32;
        if RegGetValueW(
            key,
            PCWSTR::null(),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut bytes),
        )
        .0
            != 0
            || bytes < 2
        {
            return None;
        }
        let mut buffer = vec![0u16; bytes as usize / 2];
        if RegGetValueW(
            key,
            PCWSTR::null(),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
        .0
            != 0
        {
            return None;
        }
        let length = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..length]))
    }

    let wanted_name = kind.display_name().to_ascii_lowercase();
    let executable = kind.executable_name();
    let mut locations = Vec::new();

    for (root, uninstall_path) in UNINSTALL_KEYS {
        let path = wide(uninstall_path);
        let mut uninstall_key = HKEY::default();
        if unsafe {
            RegOpenKeyExW(
                root,
                PCWSTR(path.as_ptr()),
                0,
                KEY_READ,
                &mut uninstall_key,
            )
        }
        .0
            != 0
        {
            continue;
        }

        let mut index = 0u32;
        loop {
            let mut name = vec![0u16; 512];
            let mut name_len = name.len() as u32;
            let result = unsafe {
                RegEnumKeyExW(
                    uninstall_key,
                    index,
                    PWSTR(name.as_mut_ptr()),
                    &mut name_len,
                    None,
                    PWSTR::null(),
                    None,
                    None,
                )
            };
            if result.0 != 0 {
                break;
            }
            index += 1;

            let subkey_name = String::from_utf16_lossy(&name[..name_len as usize]);
            let subkey_path = wide(&subkey_name);
            let mut app_key = HKEY::default();
            if unsafe {
                RegOpenKeyExW(
                    uninstall_key,
                    PCWSTR(subkey_path.as_ptr()),
                    0,
                    KEY_READ,
                    &mut app_key,
                )
            }
            .0
                != 0
            {
                continue;
            }

            let display_name = unsafe { read_string(app_key, "DisplayName") };
            let matches = display_name
                .as_deref()
                .is_some_and(|name| name.to_ascii_lowercase().contains(&wanted_name));
            if matches {
                if let Some(install_location) = unsafe { read_string(app_key, "InstallLocation") } {
                    let install_location = install_location.trim().trim_matches('"');
                    if !install_location.is_empty() {
                        let root = PathBuf::from(install_location);
                        locations.push(root.join(executable));
                        locations.push(root.join("bin").join(executable));
                    }
                }
                if let Some(display_icon) = unsafe { read_string(app_key, "DisplayIcon") } {
                    let icon = display_icon
                        .split(',')
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .trim_matches('"');
                    let icon = PathBuf::from(icon);
                    if icon.file_name().is_some_and(|name| {
                        name.to_string_lossy().eq_ignore_ascii_case(executable)
                    }) {
                        locations.push(icon);
                    }
                }
            }
            unsafe {
                let _ = RegCloseKey(app_key);
            }
        }
        unsafe {
            let _ = RegCloseKey(uninstall_key);
        }
    }

    locations
}

/// Show a native Windows picker restricted to executable files.
#[cfg(windows)]
pub fn pick_executable(owner: Option<isize>, kind: ToolKind) -> Option<PathBuf> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    let mut path = vec![0u16; 32_768];
    let filter_text = format!(
        "{} ({} executable)\0{}\0Executables (*.exe)\0*.exe\0All files (*.*)\0*.*\0\0",
        kind.display_name(),
        kind.display_name(),
        kind.executable_name()
    );
    let filter: Vec<u16> = filter_text.encode_utf16().collect();
    let title: Vec<u16> = format!("Select {} executable\0", kind.display_name())
        .encode_utf16()
        .collect();
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
        Some(PathBuf::from(String::from_utf16_lossy(&path[..length])))
    } else {
        None
    }
}

#[cfg(not(windows))]
pub fn pick_executable(_owner: Option<isize>, _kind: ToolKind) -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::{
        discover_from_locations, normalize_stream_quality, parse_stream_qualities, DiscoverySource,
        ToolKind,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn discovery_uses_override_before_every_automatic_location() {
        let root = temp_root("override");
        let executable_dir = root.join("app");
        let path_dir = root.join("path");
        let common_dir = root.join("common");
        for directory in [&executable_dir, &path_dir, &common_dir] {
            fs::create_dir_all(directory).unwrap();
            fs::write(directory.join("mpv.exe"), b"test").unwrap();
        }
        let override_path = root.join("manual").join("mpv.exe");

        let candidate = discover_from_locations(
            ToolKind::Mpv,
            Some(&override_path),
            Some(&executable_dir),
            &[path_dir],
            &[common_dir.join("mpv.exe")],
        )
        .unwrap();

        assert_eq!(candidate.path, override_path);
        assert_eq!(candidate.source, DiscoverySource::Override);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn automatic_discovery_follows_directory_path_then_common_precedence() {
        let root = temp_root("automatic");
        let executable_dir = root.join("app");
        let path_dir = root.join("path");
        let common_path = root.join("common").join("streamlink.exe");
        fs::create_dir_all(&executable_dir).unwrap();
        fs::create_dir_all(&path_dir).unwrap();
        fs::create_dir_all(common_path.parent().unwrap()).unwrap();
        fs::write(executable_dir.join("streamlink.exe"), b"test").unwrap();
        fs::write(path_dir.join("streamlink.exe"), b"test").unwrap();
        fs::write(&common_path, b"test").unwrap();

        let first = discover_from_locations(
            ToolKind::Streamlink,
            None,
            Some(&executable_dir),
            std::slice::from_ref(&path_dir),
            std::slice::from_ref(&common_path),
        )
        .unwrap();
        assert_eq!(first.source, DiscoverySource::ExecutableDirectory);

        fs::remove_file(executable_dir.join("streamlink.exe")).unwrap();
        let second = discover_from_locations(
            ToolKind::Streamlink,
            None,
            Some(&executable_dir),
            std::slice::from_ref(&path_dir),
            std::slice::from_ref(&common_path),
        )
        .unwrap();
        assert_eq!(second.source, DiscoverySource::Path);

        fs::remove_file(path_dir.join("streamlink.exe")).unwrap();
        let third = discover_from_locations(
            ToolKind::Streamlink,
            None,
            Some(&executable_dir),
            &[PathBuf::from("missing")],
            std::slice::from_ref(&common_path),
        )
        .unwrap();
        assert_eq!(third.source, DiscoverySource::CommonLocation);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stream_qualities_are_read_from_top_level_streams_and_sorted() {
        let json = r#"{
            "metadata": {"title": "Example"},
            "streams": {
                "worst": {"type": "http"},
                "audio_only": {"type": "http"},
                "720p": {"type": "http"},
                "1080p60": {"type": "http"},
                "best": {"type": "http"},
                "1080p": {"type": "http"}
            }
        }"#;

        assert_eq!(
            parse_stream_qualities(json).unwrap(),
            vec!["best", "1080p60", "1080p", "720p", "audio_only", "worst"]
        );
    }

    #[test]
    fn empty_quality_normalizes_to_best() {
        assert_eq!(normalize_stream_quality(""), "best");
        assert_eq!(normalize_stream_quality("   "), "best");
        assert_eq!(normalize_stream_quality(" 720p60 "), "720p60");
    }

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pluriview-tools-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }
}
