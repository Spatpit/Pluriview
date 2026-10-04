//! One authenticated, optional audio stream. Capture/codec resources live only
//! for its connection; no audio jobs or queues exist while the phone has it off.
use super::*;
use crate::audio::TileAudioCapture;
use std::collections::HashMap;
use windows::Win32::{Foundation::CloseHandle, System::Diagnostics::ToolHelp::*};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Sources {
    pids: [u32; 16],
    count: usize,
    unavailable: bool,
    monitor: bool,
}
pub(super) struct State {
    active: AtomicBool,
    sources: Mutex<(Sources, Instant)>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            active: AtomicBool::new(false),
            sources: Mutex::new((Sources::default(), Instant::now())),
        }
    }
}
impl State {
    pub(super) fn update(&self, pids: impl IntoIterator<Item = u32>, monitor: bool) {
        let mut next = Sources {
            monitor,
            ..Default::default()
        };
        if monitor {
            // Match application sharing: capture Pluriview's emitted mix once.
            // The desktop monitor already renders the opted-in window/browser
            // copies here. Adding their original process trees would add them
            // a second time. Never change the monitor device or source routing.
            next.pids[0] = std::process::id();
            next.count = 1;
            *self.sources.lock() = (next, Instant::now());
            return;
        }
        for pid in pids {
            if pid == 0 || next.pids[..next.count].contains(&pid) {
                continue;
            }
            if next.count == next.pids.len() {
                next.unavailable = true;
                break;
            }
            next.pids[next.count] = pid;
            next.count += 1;
        }
        next.pids[..next.count].sort_unstable();
        *self.sources.lock() = (next, Instant::now());
    }
}

fn allowed(shared: &Shared, client: &str, sources: Sources, epoch: u64) -> bool {
    let latest = shared.audio.sources.lock();
    shared.running.load(Ordering::Relaxed)
        && !shared.blocked.load(Ordering::Relaxed)
        && shared.video_active.load(Ordering::Relaxed)
        && shared.epoch.load(Ordering::Relaxed) == epoch
        && latest.0 == sources
        && latest.1.elapsed() < Duration::from_secs(2)
        && shared
            .peer
            .lock()
            .as_ref()
            .is_some_and(|p| p.id == client && p.seen.elapsed() < LEASE)
}

// No input is legal after GET. Polling EOF makes toggle-off release WASAPI even
// during activation or silence, without adding a receiver thread per stream.
fn connected(stream: &TcpStream) -> bool {
    match stream.peek(&mut [0; 1]) {
        Err(e) => matches!(
            e.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ),
        Ok(_) => false,
    }
}

struct Lease<'a>(&'a AtomicBool);
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

pub(super) fn serve(stream: &mut TcpStream, shared: &Shared, client: &str) -> io::Result<()> {
    let sources = shared.audio.sources.lock().0;
    let epoch = shared.epoch.load(Ordering::Relaxed);
    if sources.unavailable || sources.count == 0 {
        return respond(stream, 503, "", &[]);
    }
    if !allowed(shared, client, sources, epoch) {
        return respond(stream, 423, "", &[]);
    }
    if shared
        .audio
        .active
        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return respond(stream, 409, "", &[]);
    }
    let _lease = Lease(&shared.audio.active);
    stream.set_read_timeout(Some(Duration::from_millis(1)))?;
    let started = Instant::now();
    let setup = || -> Result<_, String> {
        let runtime = video::Runtime::new().map_err(|_| "Audio runtime unavailable")?;
        let encoder = aac::Encoder::new(&runtime).map_err(|_| "Audio codec unavailable")?;
        let roots = process_roots(&sources.pids[..sources.count])?;
        let mut captures = Vec::new();
        for pid in roots {
            let capture = TileAudioCapture::start(pid, &|| {
                started.elapsed() > Duration::from_secs(4)
                    || !allowed(shared, client, sources, epoch)
                    || !connected(stream)
            })?;
            captures.push((capture, VecDeque::<f32>::with_capacity(8192)));
        }
        // COM capture objects must drop before the encoder's final Runtime Rc.
        Ok((encoder, captures))
    };
    let (mut encoder, mut captures) = match setup() {
        Ok(result) => result,
        Err(_) => return respond(stream, 503, "", &[]),
    };
    if !allowed(shared, client, sources, epoch) || !connected(stream) {
        return Ok(());
    }
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/x-pluriview-aac\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n")?;
    // Version 1, AAC-LC 48 kHz stereo, ASC 0x1190. Each record is a u32 BE
    // length followed by one raw 1024-sample AAC access unit (maximum 8192 B).
    stream.write_all(b"PVA1\x11\x90")?;
    let mut stereo = Vec::new();
    // Drop activation history so adding several sources cannot start with old sound.
    for (capture, _) in &mut captures {
        for _ in 0..64 {
            if !capture.read(&mut stereo).map_err(io::Error::other)? {
                break;
            }
        }
    }
    let mut next = Instant::now() + Duration::from_millis(22);
    let mut last_output = Instant::now();
    let mut wire = Vec::with_capacity(8196);
    while allowed(shared, client, sources, epoch) && connected(stream) {
        for (capture, queue) in &mut captures {
            for _ in 0..32 {
                if !capture.read(&mut stereo).map_err(io::Error::other)? {
                    break;
                }
                if queue.len() + stereo.len() > 8192 {
                    return Ok(());
                }
                queue.extend(stereo.iter().copied());
            }
        }
        if Instant::now() >= next {
            // Never catch up a stalled socket by replaying a burst of old audio.
            if next.elapsed() > Duration::from_millis(100) {
                break;
            }
            let mut mix = [0.0f32; 2048];
            for (_, queue) in &mut captures {
                for sample in &mut mix {
                    *sample += queue.pop_front().unwrap_or(0.0);
                }
            }
            let pcm = mix.map(to_pcm);
            if let Some(packet) = encoder.encode(&pcm).map_err(io::Error::other)? {
                if !allowed(shared, client, sources, epoch) {
                    break;
                }
                wire.clear();
                wire.extend_from_slice(&(packet.len() as u32).to_be_bytes());
                wire.extend_from_slice(&packet);
                stream.write_all(&wire)?;
                last_output = Instant::now();
            }
            if last_output.elapsed() > Duration::from_millis(500) {
                break;
            }
            next += Duration::from_nanos(1_024_000_000_000 / 48_000);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

fn to_pcm(value: f32) -> i16 {
    if value.is_finite() {
        (value.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
    } else {
        0
    }
}

fn roots(pids: &[u32], parents: &HashMap<u32, u32>) -> Vec<u32> {
    pids.iter()
        .copied()
        .filter(|pid| {
            let mut ancestor = *pid;
            for _ in 0..128 {
                let Some(&parent) = parents.get(&ancestor) else {
                    break;
                };
                if parent == 0 || parent == ancestor {
                    break;
                }
                if pids.contains(&parent) {
                    return false;
                }
                ancestor = parent;
            }
            true
        })
        .collect()
}

fn process_roots(pids: &[u32]) -> Result<Vec<u32>, String> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
            .map_err(|_| "Process list unavailable")?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut parents = HashMap::new();
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
        if pids.iter().any(|pid| !parents.contains_key(pid)) {
            return Err("Tile process unavailable".into());
        }
        Ok(roots(pids, &parents))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audio_sources_are_bounded_deduplicated_and_fail_closed() {
        let state = State::default();
        state.update([4, 0, 4, 2], false);
        let sources = state.sources.lock().0;
        assert_eq!(&sources.pids[..sources.count], &[2, 4]);
        state.update(1..=17, false);
        assert!(state.sources.lock().0.unavailable);
    }
    #[test]
    fn desktop_monitor_mix_replaces_separate_tile_captures() {
        let state = State::default();
        // Even a large list of monitored windows needs only one capture. In
        // particular, no external app is tapped again alongside its replay.
        state.update(1..=17, true);
        let monitored = state.sources.lock().0;
        assert_eq!(&monitored.pids[..monitored.count], &[std::process::id()]);
        assert!(!monitored.unavailable);
        assert!(monitored.monitor);
        // Source churn is reflected by the desktop mix without interrupting
        // phone audio. Switching modes does invalidate the old connection.
        state.update([42, 43], true);
        assert!(state.sources.lock().0 == monitored);
        state.update([std::process::id()], false);
        assert!(state.sources.lock().0 != monitored);
        state.update([42, 43], false);
        let direct = state.sources.lock().0;
        assert_eq!(&direct.pids[..direct.count], &[42, 43]);
        assert!(!direct.monitor);
    }
    #[test]
    fn nested_tile_processes_are_captured_once() {
        let parents = HashMap::from([(5, 4), (4, 3), (3, 1), (9, 1)]);
        assert_eq!(roots(&[3, 5, 9], &parents), vec![3, 9]);
    }
    #[test]
    fn mixer_clips_and_sanitizes_invalid_samples() {
        assert_eq!(to_pcm(f32::NAN), 0);
        assert_eq!(to_pcm(f32::INFINITY), 0);
        assert_eq!(to_pcm(2.0), 32767);
        assert_eq!(to_pcm(-2.0), -32767);
    }
}
