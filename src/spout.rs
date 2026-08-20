//! Discover live [Spout2](https://github.com/leadedge/Spout2) senders on Windows.
//!
//! Spout is a same-machine GPU texture sharing system (the Windows counterpart
//! to Syphon on macOS). Apps such as VTube Studio publish a named sender;
//! Pluriview reads the sender registry and copies the shared DirectX texture
//! into a canvas tile. Pluriview does not publish a Spout sender of its own.

use std::ffi::CString;
use std::time::Duration;

use windows::core::PCSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows::Win32::System::Memory::{
    MapViewOfFile, OpenFileMappingA, UnmapViewOfFile, VirtualQuery, FILE_MAP_READ,
    MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::System::Threading::{CreateMutexA, ReleaseMutex, WaitForSingleObject};

/// Shared-memory map that lists every registered sender name.
const SENDER_LIST_MAP: &str = "SpoutSenderNames";
/// Shared-memory map holding the currently selected sender name.
const ACTIVE_SENDER_MAP: &str = "ActiveSenderName";
/// HKCU key written by SpoutSettings / the Spout installer.
const SPOUT_REGISTRY_KEY: &str = r"Software\Leading Edge\Spout";
/// Slot size used by the sender-name registry.
pub const MAX_SENDER_NAME_LEN: usize = 256;
/// Default maximum sender count when the registry has no override.
const DEFAULT_MAX_SENDERS: u32 = 64;
/// Size of Spout's `SharedTextureInfo` struct.
const TEXTURE_INFO_SIZE: usize = 280;
/// Mutex wait used by the Spout SDK when locking a map.
const LOCK_TIMEOUT: Duration = Duration::from_millis(67);

/// Snapshot of Spout2 on this machine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpoutDetection {
    /// SpoutSettings (or the installer) has written the HKCU key.
    pub installed: bool,
    /// Registry `Version` DWORD, when present.
    pub version: Option<u32>,
    /// Registry `MaxSenders`, or the SDK default of 64.
    pub max_senders: u32,
    /// The `SpoutSenderNames` map exists, meaning a sender has published it.
    pub sender_map_present: bool,
    /// Name from the `ActiveSenderName` map, if it is set and non-empty.
    pub active_sender: Option<String>,
    /// Live senders whose info maps could be opened.
    pub senders: Vec<SpoutSender>,
}

impl SpoutDetection {
    pub fn is_present(&self) -> bool {
        self.installed || self.sender_map_present || !self.senders.is_empty()
    }

    pub fn status_label(&self) -> String {
        let version = self.version.map(format_spout_version);
        match (self.is_present(), self.senders.len(), version.as_deref()) {
            (false, _, _) => "Not detected".to_owned(),
            (true, 0, Some(version)) => format!("{version} · no live senders"),
            (true, 0, None) => "No live senders".to_owned(),
            (true, 1, Some(version)) => format!("{version} · 1 sender"),
            (true, 1, None) => "1 sender".to_owned(),
            (true, count, Some(version)) => format!("{version} · {count} senders"),
            (true, count, None) => format!("{count} senders"),
        }
    }
}

/// One live Spout sender.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpoutSender {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    /// Host executable path stored in the sender info map, when present.
    pub host_path: String,
    pub cpu_share: bool,
    pub gldx_compatible: bool,
    /// Truncated D3D11 share handle from Spout's `SharedTextureInfo`.
    pub share_handle: u32,
}

impl SpoutSender {
    pub fn matches_filter(&self, normalized_filter: &str) -> bool {
        if normalized_filter.is_empty() {
            return true;
        }
        self.name.to_lowercase().contains(normalized_filter)
            || self
                .host_filename()
                .to_lowercase()
                .contains(normalized_filter)
    }

    pub fn host_filename(&self) -> &str {
        self.host_path
            .rsplit(['\\', '/'])
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("Spout2")
    }

    pub fn size_label(&self) -> String {
        if self.width == 0 || self.height == 0 {
            format_dxgi_format(self.format).to_owned()
        } else {
            format!(
                "{}×{} · {}",
                self.width,
                self.height,
                format_dxgi_format(self.format)
            )
        }
    }
}

/// Probe the Spout registry and shared-memory sender list.
pub fn detect() -> SpoutDetection {
    let (installed, version, max_senders) = read_spout_registry();
    let mut detection = SpoutDetection {
        installed,
        version,
        max_senders,
        sender_map_present: false,
        active_sender: None,
        senders: Vec::new(),
    };

    if let Some(map) = SharedMap::open(SENDER_LIST_MAP) {
        detection.sender_map_present = true;
        if let Some(locked) = map.lock() {
            let slot_count = (locked.bytes().len() / MAX_SENDER_NAME_LEN).clamp(1, 256);
            let names = parse_sender_names(locked.bytes(), slot_count);
            drop(locked);
            detection.senders = names
                .into_iter()
                .filter_map(|name| sender_info(&name))
                .collect();
            detection
                .senders
                .sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
        }
    }

    detection.active_sender = read_active_sender();
    detection
}

/// Read one sender's shared-memory info map.
pub fn sender_info(name: &str) -> Option<SpoutSender> {
    let map = SharedMap::open(name)?;
    let locked = map.lock()?;
    let info = parse_texture_info(locked.bytes());
    Some(SpoutSender {
        name: name.to_owned(),
        width: info.width,
        height: info.height,
        format: info.format,
        host_path: info.host_path,
        cpu_share: info.cpu_share,
        gldx_compatible: info.gldx_compatible,
        share_handle: info.share_handle,
    })
}

/// Reconstruct the HANDLE Spout stored as a 32-bit value.
pub fn share_handle_from_u32(value: u32) -> windows::Win32::Foundation::HANDLE {
    windows::Win32::Foundation::HANDLE((value as i32 as isize) as *mut core::ffi::c_void)
}

fn read_active_sender() -> Option<String> {
    let map = SharedMap::open(ACTIVE_SENDER_MAP)?;
    let locked = map.lock()?;
    let name = c_string_from_bytes(
        locked
            .bytes()
            .get(..MAX_SENDER_NAME_LEN)
            .unwrap_or(locked.bytes()),
    );
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn read_spout_registry() -> (bool, Option<u32>, u32) {
    let version = read_registry_dword("Version");
    let max_senders = read_registry_dword("MaxSenders").unwrap_or(DEFAULT_MAX_SENDERS);
    let installed = version.is_some() || read_registry_dword("MaxSenders").is_some();
    (installed, version, max_senders.max(1))
}

fn read_registry_dword(value: &str) -> Option<u32> {
    let subkey: Vec<u16> = SPOUT_REGISTRY_KEY
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let name: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            windows::core::PCWSTR(subkey.as_ptr()),
            windows::core::PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut data as *mut u32).cast()),
            Some(&mut size),
        )
    };
    if status.0 == 0 {
        Some(data)
    } else {
        None
    }
}

/// Parse the `SpoutSenderNames` map: 256-byte zero-padded slots, terminated
/// by an empty slot.
pub fn parse_sender_names(buffer: &[u8], max_senders: usize) -> Vec<String> {
    let mut names = Vec::new();
    for index in 0..max_senders {
        let start = index.saturating_mul(MAX_SENDER_NAME_LEN);
        if start >= buffer.len() {
            break;
        }
        let end = (start + MAX_SENDER_NAME_LEN).min(buffer.len());
        let name = c_string_from_bytes(&buffer[start..end]);
        if name.is_empty() {
            break;
        }
        names.push(name);
    }
    names
}

struct TextureInfo {
    share_handle: u32,
    width: u32,
    height: u32,
    format: u32,
    host_path: String,
    cpu_share: bool,
    gldx_compatible: bool,
}

fn parse_texture_info(buffer: &[u8]) -> TextureInfo {
    let share_handle = read_u32(buffer, 0);
    let width = read_u32(buffer, 4);
    let height = read_u32(buffer, 8);
    let format = read_u32(buffer, 12);
    let partner_id = read_u32(buffer, 276);
    let host_path = if buffer.len() >= 20 {
        c_string_from_bytes(&buffer[20..buffer.len().min(276)])
    } else {
        String::new()
    };
    TextureInfo {
        share_handle,
        width,
        height,
        format,
        host_path,
        cpu_share: partner_id & 0x8000_0000 != 0,
        gldx_compatible: partner_id & 0x4000_0000 != 0,
    }
}

fn read_u32(buffer: &[u8], offset: usize) -> u32 {
    buffer
        .get(offset..offset + 4)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_le_bytes)
        .unwrap_or(0)
}

fn c_string_from_bytes(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

/// Render the Spout registry Version DWORD as `2.007` / `2.007.017`.
pub fn format_spout_version(version: u32) -> String {
    if version >= 1_000_000 {
        let major = version / 1_000_000;
        let rest = version % 1_000_000;
        format!("{major}.{:03}.{:03}", rest / 1000, rest % 1000)
    } else if version >= 1000 {
        format!("{}.{:03}", version / 1000, version % 1000)
    } else {
        version.to_string()
    }
}

pub fn format_dxgi_format(format: u32) -> &'static str {
    match format {
        0 => "DX9",
        10 => "RGBA16F",
        24 => "RGB10A2",
        28 => "RGBA8",
        29 => "RGBA8 sRGB",
        87 => "BGRA8",
        91 => "BGRA8 sRGB",
        _ => "DXGI",
    }
}

struct SharedMap {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    size: usize,
    mutex: HANDLE,
}

impl SharedMap {
    fn open(name: &str) -> Option<Self> {
        let c_name = CString::new(name).ok()?;
        let handle =
            unsafe { OpenFileMappingA(FILE_MAP_READ.0, false, PCSTR(c_name.as_ptr().cast())) }
                .ok()?;
        if handle.is_invalid() {
            return None;
        }

        let view = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 0) };
        if view.Value.is_null() {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return None;
        }

        let mut info = MEMORY_BASIC_INFORMATION::default();
        let queried = unsafe {
            VirtualQuery(
                Some(view.Value),
                &mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        let size = if queried == 0 {
            TEXTURE_INFO_SIZE.max(MAX_SENDER_NAME_LEN)
        } else {
            info.RegionSize
        };

        let mutex_name = match CString::new(format!("{name}_mutex")) {
            Ok(name) => name,
            Err(_) => {
                unsafe {
                    let _ = UnmapViewOfFile(view);
                    let _ = CloseHandle(handle);
                }
                return None;
            }
        };
        let mutex = unsafe { CreateMutexA(None, false, PCSTR(mutex_name.as_ptr().cast())) }.ok();
        let mutex = match mutex {
            Some(handle) if !handle.is_invalid() => handle,
            _ => {
                unsafe {
                    let _ = UnmapViewOfFile(view);
                    let _ = CloseHandle(handle);
                }
                return None;
            }
        };

        Some(Self {
            handle,
            view,
            size,
            mutex,
        })
    }

    fn lock(&self) -> Option<LockedMap<'_>> {
        let wait = unsafe { WaitForSingleObject(self.mutex, LOCK_TIMEOUT.as_millis() as u32) };
        if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED {
            return None;
        }
        if self.view.Value.is_null() || self.size == 0 {
            self.unlock();
            return None;
        }
        Some(LockedMap {
            map: self,
            data: unsafe { std::slice::from_raw_parts(self.view.Value as *const u8, self.size) },
        })
    }

    fn unlock(&self) {
        unsafe {
            let _ = ReleaseMutex(self.mutex);
        }
    }
}

struct LockedMap<'a> {
    map: &'a SharedMap,
    data: &'a [u8],
}

impl LockedMap<'_> {
    fn bytes(&self) -> &[u8] {
        self.data
    }
}

impl Drop for LockedMap<'_> {
    fn drop(&mut self) {
        self.map.unlock();
    }
}

impl Drop for SharedMap {
    fn drop(&mut self) {
        unsafe {
            if !self.view.Value.is_null() {
                let _ = UnmapViewOfFile(self.view);
            }
            if !self.handle.is_invalid() {
                let _ = CloseHandle(self.handle);
            }
            if !self.mutex.is_invalid() {
                let _ = CloseHandle(self.mutex);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_name_slots_stop_at_the_first_empty_entry() {
        let mut buffer = vec![0u8; MAX_SENDER_NAME_LEN * 3];
        buffer[..5].copy_from_slice(b"Arena");
        buffer[MAX_SENDER_NAME_LEN..MAX_SENDER_NAME_LEN + 7].copy_from_slice(b"TD_OUT1");

        assert_eq!(
            parse_sender_names(&buffer, 3),
            vec!["Arena".to_owned(), "TD_OUT1".to_owned()]
        );
    }

    #[test]
    fn empty_sender_map_has_no_names() {
        let buffer = vec![0u8; MAX_SENDER_NAME_LEN];
        assert!(parse_sender_names(&buffer, 1).is_empty());
    }

    #[test]
    fn spout_version_dwords_render_as_dotted_sdk_numbers() {
        assert_eq!(format_spout_version(2007), "2.007");
        assert_eq!(format_spout_version(2_007_017), "2.007.017");
        assert_eq!(format_spout_version(42), "42");
    }

    #[test]
    fn texture_info_reads_size_format_and_partner_flags() {
        let mut buffer = vec![0u8; TEXTURE_INFO_SIZE];
        buffer[4..8].copy_from_slice(&1920u32.to_le_bytes());
        buffer[8..12].copy_from_slice(&1080u32.to_le_bytes());
        buffer[12..16].copy_from_slice(&87u32.to_le_bytes());
        buffer[20..40].copy_from_slice(b"C:\\Apps\\Arena.exe\0xx");
        buffer[276..280].copy_from_slice(&0xC000_0000u32.to_le_bytes());

        let info = parse_texture_info(&buffer);
        assert_eq!(info.share_handle, 0);
        assert_eq!(info.width, 1920);
        assert_eq!(info.height, 1080);
        assert_eq!(info.format, 87);
        assert_eq!(info.host_path, r"C:\Apps\Arena.exe");
        assert!(info.cpu_share);
        assert!(info.gldx_compatible);
    }

    #[test]
    fn detection_status_covers_missing_and_live_senders() {
        let missing = SpoutDetection::default();
        assert_eq!(missing.status_label(), "Not detected");

        let installed = SpoutDetection {
            installed: true,
            version: Some(2007),
            max_senders: 64,
            ..SpoutDetection::default()
        };
        assert_eq!(installed.status_label(), "2.007 · no live senders");

        let live = SpoutDetection {
            sender_map_present: true,
            senders: vec![SpoutSender {
                name: "Arena".to_owned(),
                width: 1920,
                height: 1080,
                format: 87,
                host_path: r"C:\Arena.exe".to_owned(),
                cpu_share: false,
                gldx_compatible: true,
                share_handle: 1,
            }],
            ..SpoutDetection::default()
        };
        assert_eq!(live.status_label(), "1 sender");
        assert!(live.senders[0].matches_filter("arena"));
        assert_eq!(live.senders[0].size_label(), "1920×1080 · BGRA8");
    }

    #[test]
    fn detect_does_not_panic_without_a_running_sender() {
        let detection = detect();
        assert!(detection.max_senders >= 1);
        if !detection.is_present() {
            assert!(detection.senders.is_empty());
            assert_eq!(detection.status_label(), "Not detected");
        }
    }
}
