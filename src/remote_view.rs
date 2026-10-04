//! Opt-in LAN viewer. Transport, frame production and camera commands are kept
//! separate so a future video transport/independent camera can reuse the client.
use eframe::egui::{self, Color32, ColorImage, Rect};
use parking_lot::Mutex;
use serde::Deserialize;
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

#[cfg(windows)]
mod aac;
#[cfg(windows)]
mod audio;
mod auth;
mod cadence;
#[cfg(windows)]
mod discovery;
mod motion;
mod pairing;
#[cfg(all(test, windows))]
mod soak_tests;
#[cfg(test)]
mod transport_tests;
#[cfg(windows)]
mod video;

const FRAME_INTERVAL: Duration = Duration::from_millis(100);
const VIDEO_SIZE: [usize; 2] = [1280, 720];
const VIDEO_FPS: u64 = 30;
const VIDEO_BITRATE: u32 = 4_000_000;
const LEASE: Duration = Duration::from_secs(4);
const MAX_REQUEST: usize = 4096;
/// An unused QR/link invitation stops working after this long. Once a phone
/// uses it, it stays bound to that phone until sharing stops or a new link
/// is created.
const INVITE_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// Fixed network workers. Video, control and audio streams can each hold one
/// for as long as they are connected.
const NETWORK_WORKERS: usize = 8;
/// Connections from one address that may be reading headers at once. A
/// device trickling bytes holds at most this many workers for the request
/// deadline, so other devices (and the paired phone's own streams) are never
/// starved.
const MAX_PRE_AUTH_PER_ADDRESS: u8 = 2;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraCommand {
    pub pan: [f32; 2],
    pub scale: f32,
    pub anchor: [f32; 2],
    pub viewport: [f32; 2],
}

impl CameraCommand {
    pub fn is_valid(&self) -> bool {
        self.pan.iter().all(|v| v.is_finite() && v.abs() <= 2.0)
            && self.scale.is_finite()
            && (0.25..=4.0).contains(&self.scale)
            && self
                .anchor
                .iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
            && self
                .viewport
                .iter()
                .all(|v| v.is_finite() && (1.0..=32768.0).contains(v))
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FocusCommand {
    at: [f32; 2],
    viewport: [f32; 2],
}
impl FocusCommand {
    fn is_valid(&self) -> bool {
        CameraCommand {
            pan: [0.0; 2],
            scale: 1.0,
            anchor: self.at,
            viewport: self.viewport,
        }
        .is_valid()
    }
}
enum NavigationCommand {
    Camera(CameraCommand),
    Focus(FocusCommand),
}
impl NavigationCommand {
    fn viewport(&self) -> [f32; 2] {
        match self {
            Self::Camera(c) => c.viewport,
            Self::Focus(c) => c.viewport,
        }
    }
}

struct Peer {
    id: String,
    seen: Instant,
}

struct EncodedFrame {
    jpeg: Vec<u8>,
    id: u64,
    viewport: [f32; 2],
    created: Instant,
}

struct VideoFrame {
    bytes: Vec<u8>,
    size: [u32; 2],
    viewport: [f32; 2],
    id: u64,
    created: Instant,
    stream_start: bool,
}

struct Shared {
    name: String,
    pairing: Mutex<pairing::Pairing>,
    discovery_status: Mutex<&'static str>,
    running: AtomicBool,
    blocked: AtomicBool,
    busy: AtomicBool,
    epoch: AtomicU64,
    sequence: AtomicU64,
    peer: Mutex<Option<Peer>>,
    frame: Mutex<Option<Arc<EncodedFrame>>>,
    video_active: AtomicBool,
    control_active: AtomicBool,
    #[cfg(windows)]
    audio: audio::State,
    video_failed: AtomicBool,
    video_frames: Mutex<VecDeque<Arc<VideoFrame>>>,
    hardware: AtomicBool,
    pre_auth: Mutex<std::collections::HashMap<std::net::IpAddr, u8>>,
    invite: Mutex<Invite>,
}

/// The secret carried by the QR code/pairing link. It never crosses the
/// network: requests prove knowledge of it with `auth` signatures.
struct Invite {
    token: String,
    created: Instant,
    client: Option<String>,
    replay: auth::ReplayGuard,
}

impl Invite {
    fn new() -> io::Result<Self> {
        Ok(Self {
            token: pairing::random_id()?,
            created: Instant::now(),
            client: None,
            replay: auth::ReplayGuard::default(),
        })
    }

    fn expired(&self) -> bool {
        self.client.is_none() && self.created.elapsed() >= INVITE_LIFETIME
    }

    fn authorized(&mut self, request: &Request, credentials: &auth::Credentials) -> bool {
        if self.expired()
            || self
                .client
                .as_ref()
                .is_some_and(|client| *client != request.client)
            || !credentials.signed_by(&self.token, &request.method, &request.path, &request.client)
            || !self.replay.accept(credentials)
        {
            return false;
        }
        // Single use: the first phone to present the link keeps it.
        self.client.get_or_insert_with(|| request.client.clone());
        true
    }
}

/// A connection that has not yet been authenticated or answered.
struct PreAuthSlot<'a> {
    shared: &'a Shared,
    address: std::net::IpAddr,
}

impl<'a> PreAuthSlot<'a> {
    fn acquire(shared: &'a Shared, address: std::net::IpAddr) -> Option<Self> {
        let mut counts = shared.pre_auth.lock();
        let count = counts.entry(address).or_default();
        if *count >= MAX_PRE_AUTH_PER_ADDRESS {
            return None;
        }
        *count += 1;
        Some(Self { shared, address })
    }
}

impl Drop for PreAuthSlot<'_> {
    fn drop(&mut self) {
        let mut counts = self.shared.pre_auth.lock();
        if let Some(count) = counts.get_mut(&self.address) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                counts.remove(&self.address);
            }
        }
    }
}

impl Shared {
    fn watching(&self) -> bool {
        self.peer
            .lock()
            .as_ref()
            .is_some_and(|p| p.seen.elapsed() < LEASE)
    }

    fn invalidate(&self) {
        self.epoch.fetch_add(1, Ordering::Relaxed);
        *self.frame.lock() = None;
        self.video_frames.lock().clear();
    }
}

struct CaptureJob {
    image: Arc<ColorImage>,
    rect: Rect,
    pixels_per_point: f32,
    epoch: u64,
}

struct QueuedCamera {
    command: NavigationCommand,
    client: String,
    received: Instant,
    epoch: u64,
}

struct Host {
    shared: Arc<Shared>,
    commands: mpsc::Receiver<QueuedCamera>,
    encoder: mpsc::SyncSender<CaptureJob>,
    address: SocketAddrV4,
    invite: String,
    qr: qrcode::QrCode,
}

fn invite_link(address: SocketAddrV4, token: &str) -> io::Result<(String, qrcode::QrCode)> {
    let link = format!(
        "pluriview://viewer?host={}&port={}&token={token}",
        address.ip(),
        address.port()
    );
    let qr =
        qrcode::QrCode::new(link.as_bytes()).map_err(|_| io::Error::other("Pairing QR failed"))?;
    Ok((link, qr))
}

impl Drop for Host {
    fn drop(&mut self) {
        self.shared.running.store(false, Ordering::Relaxed);
        self.shared.blocked.store(true, Ordering::Relaxed);
        self.shared.invalidate();
        // Workers have bounded socket deadlines and exit without blocking UI.
    }
}

pub struct RemoteView {
    pub show_dialog: bool,
    approval_drawn: bool,
    address: String,
    error: Option<String>,
    host: Option<Host>,
    pending: Option<(Rect, f32, u64, Instant)>,
    next_capture: Instant,
    motion: Option<motion::Motion>,
    motion_epoch: u64,
}

impl Default for RemoteView {
    fn default() -> Self {
        Self {
            show_dialog: false,
            approval_drawn: false,
            address: String::new(),
            error: None,
            host: None,
            pending: None,
            next_capture: Instant::now(),
            motion: None,
            motion_epoch: 0,
        }
    }
}

impl RemoteView {
    #[cfg(windows)]
    pub fn set_audio_sources(&self, pids: impl IntoIterator<Item = u32>, monitor_enabled: bool) {
        if let Some(host) = &self.host {
            host.shared.audio.update(pids, monitor_enabled);
        }
    }

    pub fn is_blocking(&self) -> bool {
        self.show_dialog
            || self.approval_drawn
            || self
                .host
                .as_ref()
                .is_some_and(|h| h.shared.pairing.lock().pending())
    }

    pub fn stop(&mut self) {
        self.motion = None;
        self.host = None;
        self.pending = None;
    }

    pub fn ui(&mut self, ctx: &egui::Context) {
        self.approval_drawn = false;
        if let Some(host) = &self.host {
            let prompt = host.shared.pairing.lock().prompt(Instant::now());
            if let Some((request, name, code)) = prompt {
                self.approval_drawn = true;
                let mut decision = None;
                egui::Window::new("Phone connection request")
                    .collapsible(false)
                    .resizable(false)
                    .default_width(350.0)
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.label(format!("{name} wants to view and navigate your canvas."));
                        ui.label("The phone can also enable tile audio while connected.");
                        ui.label("Only approve if this number matches the one on your phone:");
                        ui.heading(&code);
                        ui.small("Device names are provided by the requesting phone.");
                        ui.horizontal(|ui| {
                            if ui.button("Allow connection").clicked() {
                                decision = Some(true);
                            }
                            if ui.button("Deny").clicked() {
                                decision = Some(false);
                            }
                        });
                        ui.small("Request expires after one minute. Trusted local networks only.");
                    });
                if let Some(allow) = decision {
                    host.shared.pairing.lock().decide(
                        &request,
                        allow,
                        host.shared.watching(),
                        Instant::now(),
                    );
                    if allow {
                        self.show_dialog = false;
                    }
                }
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }
        if self.approval_drawn || !self.show_dialog {
            return;
        }
        if self.address.is_empty() {
            // UDP connect selects a route; it sends no packet to this address.
            self.address = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
                .and_then(|s| {
                    s.connect((Ipv4Addr::new(192, 0, 2, 1), 9))?;
                    s.local_addr()
                })
                .map(|a| a.ip().to_string())
                .unwrap_or_else(|_| "192.168.1.2".into());
        }
        let mut open = true;
        egui::Window::new("Phone Viewer")
            .open(&mut open).collapsible(false).resizable(false).default_width(390.0)
            .show(ctx, |ui| {
                ui.label("View and navigate this canvas from your Android phone.");
                ui.label("Use trusted private Wi-Fi. This preview uses unencrypted LAN streaming.");
                ui.add_space(8.0);
                if let Some(host) = &self.host {
                    ui.label(format!("Nearby PC name: {}", host.shared.name));
                    ui.small(*host.shared.discovery_status.lock());
                    ui.label("Select this PC in the Android app, then approve its connection request here.");
                    ui.label(if host.shared.watching() { "Phone connected" } else { "Waiting for a phone" });
                    if host.shared.video_active.load(Ordering::Relaxed) {
                        ui.label(format!("{} encoding · fixed 720p / 30 FPS", if host.shared.hardware.load(Ordering::Relaxed) { "Hardware" } else { "Software" }));
                    }
                    let (expired, used) = {
                        let invite = host.shared.invite.lock();
                        (invite.expired(), invite.client.is_some())
                    };
                    let mut new_link = false;
                    if expired {
                        ui.label("The pairing link expired after 10 minutes without use.");
                        new_link = ui.button("New pairing link").clicked();
                    } else {
                    ui.label("Scan with your phone's camera and open Pluriview Viewer, or paste the pairing link in the app.");
                    let count = host.qr.width() + 8;
                    let cell = 240.0 / count as f32;
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(240.0, 240.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 0.0, Color32::WHITE);
                    for y in 0..host.qr.width() {
                        for x in 0..host.qr.width() {
                            if host.qr[(x, y)] == qrcode::Color::Dark {
                                let min = rect.min + egui::vec2((x + 4) as f32 * cell, (y + 4) as f32 * cell);
                                ui.painter().rect_filled(Rect::from_min_size(min, egui::Vec2::splat(cell + 0.1)), 0.0, Color32::BLACK);
                            }
                        }
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Copy pairing link").clicked() { ctx.copy_text(host.invite.clone()); }
                        new_link = ui.button("New pairing link").clicked();
                    });
                    ui.small(if used {
                        "This link is in use by one phone. A new link disconnects it and works for another phone."
                    } else {
                        "The link works for one phone and expires after 10 minutes unless used."
                    });
                    }
                    ui.label("Close this dialog to begin viewing. Drag to pan; pinch to zoom. Both screens share the same view.");
                    ui.label("Sharing pauses for dialogs and interactive browser mode. Optional tile audio is controlled on the phone.");
                    if ui.button("Stop sharing / disconnect phone").clicked() { self.stop(); }
                    else if new_link {
                        if let Some(host) = &mut self.host {
                            if host.refresh_invite().is_err() {
                                self.error = Some("Could not create a new pairing link.".into());
                            }
                        }
                    }
                    ctx.request_repaint_after(Duration::from_secs(1));
                } else {
                    ui.label("PC Wi-Fi IPv4 address");
                    ui.text_edit_singleline(&mut self.address);
                    if ui.button("Start sharing").clicked() {
                        match self.address.trim().parse::<Ipv4Addr>() {
                            Ok(ip) if ip.is_private() || ip.is_loopback() => match Host::start(ip, ctx.clone()) {
                                Ok(host) => { self.host = Some(host); self.error = None; }
                                Err(_) => self.error = Some("Could not start sharing on this address. Check the PC's Wi-Fi IPv4 address.".into()),
                            },
                            _ => self.error = Some("Enter a private IPv4 address belonging to this PC.".into()),
                        }
                    }
                    ui.label("Sharing starts off each time Pluriview opens. Starting again creates a new pairing link.");
                }
                if let Some(error) = &self.error { ui.colored_label(Color32::LIGHT_RED, error); }
            });
        self.show_dialog = open;
    }

    pub fn apply_camera_commands(
        &mut self,
        ctx: &egui::Context,
        canvas: &mut crate::canvas::CanvasState,
        previews: &crate::preview::PreviewManager,
        blocked: bool,
    ) {
        let Some(host) = &self.host else {
            return;
        };
        let local_gesture = ctx.input(|i| i.pointer.any_down()) || canvas.drag_state.is_some();
        let now = ctx.input(|i| i.time);
        let epoch = host.shared.epoch.load(Ordering::Relaxed);
        let suppressed = blocked || local_gesture || !host.shared.watching();
        if suppressed
            || epoch != self.motion_epoch
            || self.motion.as_ref().is_some_and(|m| !m.matches(canvas))
        {
            self.motion = None;
        }
        self.motion_epoch = epoch;
        for queued in host.commands.try_iter().take(16) {
            if suppressed {
                continue;
            }
            if queued.received.elapsed() > Duration::from_millis(500)
                || queued.epoch != host.shared.epoch.load(Ordering::Relaxed)
                || host
                    .shared
                    .peer
                    .lock()
                    .as_ref()
                    .is_none_or(|peer| peer.id != queued.client)
            {
                continue;
            }
            let command = queued.command;
            let viewport = command.viewport();
            // Drop commands drawn against a different viewport after PC resize,
            // sidebar changes or rotation rather than moving the wrong distance.
            if canvas.last_screen_rect.is_some_and(|r| {
                (r.width() - viewport[0]).abs() < 1.0 && (r.height() - viewport[1]).abs() < 1.0
            }) {
                match command {
                    NavigationCommand::Camera(command) => {
                        if self.motion.is_none() {
                            self.motion = motion::Motion::new(canvas, now);
                        }
                        if let Some(motion) = &mut self.motion {
                            motion.push(canvas, command, now);
                        }
                    }
                    NavigationCommand::Focus(command) => {
                        self.motion = None;
                        canvas.focus_viewer_at(command.at, previews);
                    }
                }
                ctx.request_repaint();
            }
        }
        if let Some(motion) = &mut self.motion {
            if motion.advance(canvas, now) {
                ctx.request_repaint();
            } else {
                self.motion = None;
            }
        }
    }

    pub fn receive_screenshot(&mut self, events: &[egui::Event]) {
        let Some((rect, pixels_per_point, epoch, _)) = self.pending else {
            return;
        };
        let image = events.iter().find_map(|event| match event {
            egui::Event::Screenshot { viewport_id, image }
                if *viewport_id == egui::ViewportId::ROOT =>
            {
                Some(image.clone())
            }
            _ => None,
        });
        if let Some(image) = image {
            self.pending = None;
            if let Some(host) = &self.host {
                let job = CaptureJob {
                    image,
                    rect,
                    pixels_per_point,
                    epoch,
                };
                if host.encoder.try_send(job).is_err() {
                    host.shared.busy.store(false, Ordering::Relaxed);
                }
            }
        }
    }

    pub fn capture(&mut self, ctx: &egui::Context, rect: Option<Rect>, blocked: bool) {
        let Some(host) = &self.host else {
            return;
        };
        let blocked = blocked || rect.is_none();
        if host.shared.blocked.swap(blocked, Ordering::Relaxed) != blocked {
            host.shared.invalidate();
        }
        if blocked || !host.shared.watching() {
            self.next_capture = Instant::now();
            return;
        }
        if self
            .pending
            .is_some_and(|p| p.3.elapsed() > Duration::from_secs(2))
        {
            self.pending = None;
            host.shared.busy.store(false, Ordering::Relaxed);
        }
        let interval = if host.shared.video_active.load(Ordering::Relaxed) {
            Duration::from_nanos(1_000_000_000 / VIDEO_FPS)
        } else {
            FRAME_INTERVAL
        };
        if self.pending.is_none()
            && Instant::now() >= self.next_capture
            && !host.shared.busy.swap(true, Ordering::Relaxed)
        {
            self.pending = Some((
                rect.unwrap(),
                ctx.pixels_per_point(),
                host.shared.epoch.load(Ordering::Relaxed),
                Instant::now(),
            ));
            self.next_capture = cadence::next(self.next_capture, Instant::now(), interval);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
        crate::media::request_animation_repaint(
            ctx,
            self.next_capture
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(2)),
        );
    }
}

impl Host {
    /// Replace the QR/link secret. A phone still using the old link loses
    /// access on its next request; approved nearby pairings are unaffected.
    fn refresh_invite(&mut self) -> io::Result<()> {
        let invite = Invite::new()?;
        let (link, qr) = invite_link(self.address, &invite.token)?;
        *self.shared.invite.lock() = invite;
        self.invite = link;
        self.qr = qr;
        Ok(())
    }

    fn start(ip: Ipv4Addr, ctx: egui::Context) -> io::Result<Self> {
        let listener = TcpListener::bind(SocketAddrV4::new(ip, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let address = SocketAddrV4::new(ip, port);
        let first_invite = Invite::new()?;
        let (invite, qr) = invite_link(address, &first_invite.token)?;
        let shared = Arc::new(Shared {
            name: format!("Pluriview PC {}", &pairing::random_id()?[..6]),
            pairing: Mutex::new(pairing::Pairing::default()),
            discovery_status: Mutex::new("Local discovery starting…"),
            running: AtomicBool::new(true),
            blocked: AtomicBool::new(true),
            busy: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            sequence: AtomicU64::new(0),
            peer: Mutex::new(None),
            frame: Mutex::new(None),
            video_active: AtomicBool::new(false),
            control_active: AtomicBool::new(false),
            #[cfg(windows)]
            audio: audio::State::default(),
            video_failed: AtomicBool::new(false),
            video_frames: Mutex::new(VecDeque::with_capacity(6)),
            hardware: AtomicBool::new(false),
            pre_auth: Mutex::new(std::collections::HashMap::new()),
            invite: Mutex::new(first_invite),
        });
        let (command_tx, commands) = mpsc::sync_channel(8);
        let (encoder, jobs) = mpsc::sync_channel::<CaptureJob>(1);
        // Construct the guard before spawning: a partial startup failure must
        // stop any already-created listener workers as well.
        let host = Self {
            shared: shared.clone(),
            commands,
            encoder,
            address,
            invite,
            qr,
        };
        let state = shared.clone();
        std::thread::Builder::new()
            .name("viewer-encoder".into())
            .spawn(move || {
                let mut rgb = Vec::new();
                #[cfg(windows)]
                let mut video_runtime = None;
                #[cfg(windows)]
                let mut video_encoder: Option<(
                    u64,
                    [usize; 2],
                    [f32; 2],
                    video::Encoder,
                )> = None;
                #[cfg(windows)]
                let mut hardware_allowed = true;
                loop {
                    let job = match jobs.recv_timeout(Duration::from_millis(250)) {
                        Ok(job) => job,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if !state.running.load(Ordering::Relaxed) {
                                break;
                            }
                            #[cfg(windows)]
                            if !state.video_active.load(Ordering::Relaxed)
                                || state.blocked.load(Ordering::Relaxed)
                            {
                                video_encoder = None;
                                video_runtime = None;
                            }
                            continue;
                        }
                    };
                    if !state.running.load(Ordering::Relaxed) {
                        break;
                    }
                    if state.video_active.load(Ordering::Relaxed) {
                        #[cfg(windows)]
                        {
                            let viewport = [job.rect.width(), job.rect.height()];
                            if let Some(size) = prepare_frame(&job, &mut rgb, true, VIDEO_SIZE) {
                                let result = (|| -> windows_core::Result<Option<video::Packet>> {
                                    if video_runtime.is_none() {
                                        video_runtime = Some(video::Runtime::new()?);
                                    }
                                    // Async output can lag one input. A PC resize must
                                    // discard those pictures even if the coded cap is unchanged.
                                    if !video_encoder.as_ref().is_some_and(
                                        |(epoch, old_size, old_viewport, _)| {
                                            *epoch == job.epoch
                                                && *old_size == size
                                                && *old_viewport == viewport
                                        },
                                    ) {
                                        video_encoder = None;
                                        video_encoder = Some((
                                            job.epoch,
                                            size,
                                            viewport,
                                            video::Encoder::with_settings(
                                                video_runtime.as_ref().unwrap(),
                                                size[0],
                                                size[1],
                                                VIDEO_BITRATE,
                                                VIDEO_FPS as u32,
                                                hardware_allowed,
                                            )?,
                                        ));
                                        state.hardware.store(
                                            video_encoder.as_ref().unwrap().3.hardware,
                                            Ordering::Relaxed,
                                        );
                                    }
                                    video_encoder.as_mut().unwrap().3.encode(&rgb)
                                })();
                                if result.is_err()
                                    && video_encoder
                                        .as_ref()
                                        .is_some_and(|(_, _, _, encoder)| encoder.hardware)
                                {
                                    // A driver can enumerate successfully and still fail
                                    // on real input. Retry a fresh software stream once.
                                    hardware_allowed = false;
                                    video_encoder = None;
                                    state.hardware.store(false, Ordering::Relaxed);
                                    state.invalidate();
                                    state.busy.store(false, Ordering::Relaxed);
                                    continue;
                                }
                                let mut frames = state.video_frames.lock();
                                if state.epoch.load(Ordering::Relaxed) == job.epoch
                                    && !state.blocked.load(Ordering::Relaxed)
                                {
                                    match result {
                                        Ok(Some(packet))
                                            if packet.bytes.len() <= 4 * 1024 * 1024 =>
                                        {
                                            if frames.len() == 6 {
                                                frames.pop_front();
                                            }
                                            frames.push_back(Arc::new(VideoFrame {
                                                bytes: packet.bytes,
                                                size: size.map(|v| v as u32),
                                                viewport,
                                                id: state.sequence.fetch_add(1, Ordering::Relaxed)
                                                    + 1,
                                                created: packet.created,
                                                stream_start: packet.first,
                                            }));
                                        }
                                        Ok(None) => {}
                                        _ => {
                                            state.video_failed.store(true, Ordering::Relaxed);
                                        }
                                    }
                                }
                            }
                        }
                        #[cfg(not(windows))]
                        state.video_failed.store(true, Ordering::Relaxed);
                    } else if let Some(jpeg) = encode_frame(&job, &mut rgb) {
                        #[cfg(windows)]
                        {
                            video_encoder = None;
                            video_runtime = None;
                        }
                        let mut frame = state.frame.lock();
                        if state.epoch.load(Ordering::Relaxed) == job.epoch
                            && !state.blocked.load(Ordering::Relaxed)
                        {
                            *frame = Some(Arc::new(EncodedFrame {
                                jpeg,
                                id: state.sequence.fetch_add(1, Ordering::Relaxed) + 1,
                                viewport: [job.rect.width(), job.rect.height()],
                                created: Instant::now(),
                            }));
                        }
                    }
                    state.busy.store(false, Ordering::Relaxed);
                }
            })?;
        #[cfg(windows)]
        if !ip.is_loopback() {
            discovery::start(ip, port, shared.clone(), ctx.clone());
        }
        // Fixed workers leave several for pairing and camera requests while the
        // exclusive video, control and optional audio streams are active, and
        // per-address pre-auth limits stop one device from occupying the rest.
        // No per-request thread creation or unbounded connection/task queue.
        for index in 0..NETWORK_WORKERS {
            let listener = listener.try_clone()?;
            // Windows socket duplicates need their own nonblocking mode;
            // otherwise Stop leaves accept() waiting until another client calls.
            listener.set_nonblocking(true)?;
            let state = shared.clone();
            let commands = command_tx.clone();
            let ctx = ctx.clone();
            std::thread::Builder::new()
                .name(format!("viewer-network-{index}"))
                .spawn(move || {
                    while state.running.load(Ordering::Relaxed) {
                        match listener.accept() {
                            Ok((mut stream, address)) => {
                                // Over the limit: close at once instead of
                                // spending a worker on reading its headers.
                                if let Some(slot) = PreAuthSlot::acquire(&state, address.ip()) {
                                    let _ = serve(&mut stream, &state, slot, &commands, &ctx);
                                }
                            }
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                                std::thread::sleep(Duration::from_millis(10))
                            }
                            Err(_) => break,
                        }
                    }
                })?;
        }
        Ok(host)
    }
}

fn encode_frame(job: &CaptureJob, rgb: &mut Vec<u8>) -> Option<Vec<u8>> {
    let [w, h] = prepare_frame(job, rgb, false, [1280, 720])?;
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 75)
        .encode(rgb, w as u32, h as u32, image::ExtendedColorType::Rgb8)
        .ok()?;
    Some(jpeg)
}

fn prepare_frame(
    job: &CaptureJob,
    rgb: &mut Vec<u8>,
    even: bool,
    cap: [usize; 2],
) -> Option<[usize; 2]> {
    let [sw, sh] = job.image.size;
    let rect = job.rect * job.pixels_per_point;
    let x0 = rect.min.x.round().clamp(0.0, sw as f32) as usize;
    let y0 = rect.min.y.round().clamp(0.0, sh as f32) as usize;
    let x1 = rect.max.x.round().clamp(0.0, sw as f32) as usize;
    let y1 = rect.max.y.round().clamp(0.0, sh as f32) as usize;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let factor = (cap[0] as f32 / (x1 - x0) as f32)
        .min(cap[1] as f32 / (y1 - y0) as f32)
        .min(1.0);
    let dimension = |v: usize| if even { (v & !1).max(2) } else { v.max(1) };
    let w = dimension(((x1 - x0) as f32 * factor).round() as usize);
    let h = dimension(((y1 - y0) as f32 * factor).round() as usize);
    rgb.clear();
    rgb.reserve(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let pixel = job.image.pixels[(y0 + y * (y1 - y0) / h) * sw + x0 + x * (x1 - x0) / w];
            rgb.extend_from_slice(&pixel.to_array()[..3]);
        }
    }
    Some([w, h])
}

struct Request {
    method: String,
    path: String,
    authorization: String,
    client: String,
    last_frame: u64,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> io::Result<Request> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut bytes = Vec::with_capacity(1024);
    let mut byte = [0];
    while !bytes.ends_with(b"\r\n\r\n") {
        if bytes.len() >= MAX_REQUEST || Instant::now() >= deadline {
            return Err(io::Error::other("Request too large or slow"));
        }
        stream.read_exact(&mut byte)?;
        bytes.push(byte[0]);
    }
    let header = std::str::from_utf8(&bytes).map_err(|_| io::Error::other("Invalid header"))?;
    let mut lines = header.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split_whitespace();
    let method = first.next().unwrap_or_default().to_owned();
    let path = first.next().unwrap_or_default().to_owned();
    if first.next() != Some("HTTP/1.1") || first.next().is_some() {
        return Err(io::Error::other("Invalid HTTP version"));
    }
    let mut result = Request {
        method,
        path,
        authorization: String::new(),
        client: String::new(),
        last_frame: 0,
        body: Vec::new(),
    };
    let mut length = None;
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| io::Error::other("Invalid header"))?;
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "authorization" => result.authorization = value.to_owned(),
            "x-pluriview-client" => result.client = value.to_owned(),
            "x-last-frame" => result.last_frame = value.parse().unwrap_or(0),
            "content-length" => {
                if length.is_some() {
                    return Err(io::Error::other("Duplicate length"));
                }
                length = Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| io::Error::other("Invalid length"))?,
                );
            }
            // This protocol accepts only native clients and fixed-length bodies.
            "origin" | "transfer-encoding" => return Err(io::Error::other("Unsupported request")),
            _ => {}
        }
    }
    let length = length.unwrap_or(0);
    if length > 1024 {
        return Err(io::Error::other("Body too large"));
    }
    result.body.resize(length, 0);
    let mut offset = 0;
    while offset < length {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::other("Request too slow"));
        }
        stream.set_read_timeout(Some(remaining.min(Duration::from_millis(500))))?;
        let read = stream.read(&mut result.body[offset..])?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        offset += read;
    }
    Ok(result)
}

fn respond(stream: &mut TcpStream, status: u16, extra: &str, body: &[u8]) -> io::Result<()> {
    write!(stream, "HTTP/1.1 {status} {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n{extra}\r\n", if status == 200 { "OK" } else { "Status" }, body.len())?;
    stream.write_all(body)
}

fn serve(
    stream: &mut TcpStream,
    shared: &Shared,
    pre_auth: PreAuthSlot<'_>,
    commands: &mpsc::SyncSender<QueuedCamera>,
    ctx: &egui::Context,
) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    stream.set_nodelay(true)?;
    let request = match read_request(stream) {
        Ok(r) => r,
        Err(_) => return respond(stream, 400, "", &[]),
    };
    // Headers are complete; responses below are bounded by write timeouts,
    // and long-lived streams require authentication first.
    drop(pre_auth);
    if request.client.len() != 32 || !request.client.bytes().all(|v| v.is_ascii_hexdigit()) {
        return respond(stream, 401, "", &[]);
    }
    if shared.running.load(Ordering::Relaxed) && request.path.starts_with("/v3/") {
        return pairing::serve_pairing(stream, shared, &request, ctx);
    }
    let authorized = auth::Credentials::parse(&request.authorization).is_some_and(|credentials| {
        shared.pairing.lock().authorized(&request, &credentials)
            || shared.invite.lock().authorized(&request, &credentials)
    });
    if !authorized {
        return respond(stream, 401, "", &[]);
    }
    if !shared.running.load(Ordering::Relaxed) {
        return respond(stream, 503, "", &[]);
    }
    let frame_request = request.method == "GET" && request.path == "/v1/frame";
    let video_request = request.method == "GET" && request.path == "/v2/video";
    let control_request = request.method == "GET" && request.path == "/v2/control";
    let audio_request = request.method == "GET" && request.path == "/v2/audio";
    let camera_request = request.method == "POST" && request.path == "/v1/camera";
    let focus_request = request.method == "POST" && request.path == "/v1/focus";
    let feedback_request = request.method == "POST" && request.path == "/v2/feedback";
    if !frame_request
        && !camera_request
        && !focus_request
        && !video_request
        && !feedback_request
        && !control_request
        && !audio_request
    {
        return respond(stream, 404, "", &[]);
    }
    let peer_allowed = {
        let mut peer = shared.peer.lock();
        let occupied = peer
            .as_ref()
            .is_some_and(|p| p.id != request.client && p.seen.elapsed() < LEASE);
        if occupied {
            false
        } else if frame_request || video_request {
            *peer = Some(Peer {
                id: request.client.clone(),
                seen: Instant::now(),
            });
            true
        } else {
            peer.as_ref()
                .is_some_and(|p| p.id == request.client && p.seen.elapsed() < LEASE)
        }
    };
    if !peer_allowed {
        return respond(stream, 409, "", &[]);
    }
    if audio_request {
        #[cfg(windows)]
        return audio::serve(stream, shared, &request.client);
        #[cfg(not(windows))]
        return respond(stream, 503, "", &[]);
    }
    if control_request {
        return serve_control(stream, shared, &request.client, commands, ctx);
    }
    if feedback_request {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Feedback {
            #[serde(rename = "frame")]
            _frame: u64,
            decode_ms: f32,
        }
        match serde_json::from_slice::<Feedback>(&request.body) {
            Ok(value)
                if value.decode_ms.is_finite() && (0.0..=2000.0).contains(&value.decode_ms) =>
            {
                // Older Android builds still send reports. Validate them for
                // compatibility, but never alter the fixed video settings.
            }
            _ => return respond(stream, 400, "", &[]),
        };
        if !shared.video_active.load(Ordering::Relaxed) || shared.blocked.load(Ordering::Relaxed) {
            return respond(stream, 204, "", &[]);
        }
        return respond(stream, 200, "", &[]);
    }
    ctx.request_repaint();
    if frame_request && shared.video_active.load(Ordering::Relaxed) {
        return respond(stream, 409, "", &[]);
    }
    if video_request {
        return serve_video(stream, shared, &request.client, ctx);
    }
    if shared.blocked.load(Ordering::Relaxed) {
        return respond(
            stream,
            if frame_request { 204 } else { 423 },
            "X-Viewer-State: paused\r\n",
            &[],
        );
    }
    if frame_request {
        let frame = shared.frame.lock().clone();
        if let Some(frame) = frame
            .filter(|f| f.id != request.last_frame && f.created.elapsed() < Duration::from_secs(2))
        {
            let header = format!("Content-Type: image/jpeg\r\nX-Frame-Id: {}\r\nX-Canvas-Width: {}\r\nX-Canvas-Height: {}\r\n", frame.id, frame.viewport[0], frame.viewport[1]);
            return respond(stream, 200, &header, &frame.jpeg);
        }
        respond(stream, 204, "", &[])
    } else {
        let command = if focus_request {
            serde_json::from_slice::<FocusCommand>(&request.body)
                .ok()
                .filter(FocusCommand::is_valid)
                .map(NavigationCommand::Focus)
        } else {
            serde_json::from_slice::<CameraCommand>(&request.body)
                .ok()
                .filter(CameraCommand::is_valid)
                .map(NavigationCommand::Camera)
        };
        match command {
            Some(command) => {
                let queued = QueuedCamera {
                    command,
                    client: request.client,
                    received: Instant::now(),
                    epoch: shared.epoch.load(Ordering::Relaxed),
                };
                let status = if commands.try_send(queued).is_ok() {
                    ctx.request_repaint();
                    200
                } else {
                    429
                };
                respond(stream, status, "", &[])
            }
            _ => respond(stream, 400, "", &[]),
        }
    }
}

#[derive(Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum ControlRecord {
    Camera(CameraCommand),
    Focus(FocusCommand),
    Ping,
}

/// One authenticated duplex control connection alongside the video socket.
/// Two-byte lengths/statuses avoid a TCP handshake for every gesture update.
fn serve_control(
    stream: &mut TcpStream,
    shared: &Shared,
    client: &str,
    commands: &mpsc::SyncSender<QueuedCamera>,
    ctx: &egui::Context,
) -> io::Result<()> {
    if shared
        .control_active
        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return respond(stream, 409, "", &[]);
    }
    struct Lease<'a>(&'a AtomicBool);
    impl Drop for Lease<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Relaxed);
        }
    }
    let _lease = Lease(&shared.control_active);
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/x-pluriview-control\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n")?;
    let mut payload = [0u8; 1024];
    while shared.running.load(Ordering::Relaxed) {
        let mut header = [0u8; 2];
        match stream.read(&mut header[..1]) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                if !shared.watching() {
                    break;
                }
                continue;
            }
            Err(e) => return Err(e),
        }
        stream.read_exact(&mut header[1..])?;
        let length = u16::from_be_bytes(header) as usize;
        if length == 0 || length > payload.len() {
            stream.write_all(&400u16.to_be_bytes())?;
            break;
        }
        stream.read_exact(&mut payload[..length])?;
        let allowed = shared
            .peer
            .lock()
            .as_ref()
            .is_some_and(|peer| peer.id == client && peer.seen.elapsed() < LEASE);
        if !allowed || !shared.running.load(Ordering::Relaxed) {
            stream.write_all(&409u16.to_be_bytes())?;
            break;
        }
        let command = match serde_json::from_slice::<ControlRecord>(&payload[..length]) {
            Ok(ControlRecord::Camera(c)) if c.is_valid() => Some(NavigationCommand::Camera(c)),
            Ok(ControlRecord::Focus(c)) if c.is_valid() => Some(NavigationCommand::Focus(c)),
            Ok(ControlRecord::Ping) => {
                stream.write_all(&200u16.to_be_bytes())?;
                continue;
            }
            _ => {
                stream.write_all(&400u16.to_be_bytes())?;
                continue;
            }
        };
        let status: u16 = if shared.blocked.load(Ordering::Relaxed) {
            423
        } else {
            let queued = QueuedCamera {
                command: command.unwrap(),
                client: client.into(),
                received: Instant::now(),
                epoch: shared.epoch.load(Ordering::Relaxed),
            };
            if commands.try_send(queued).is_ok() {
                ctx.request_repaint();
                200
            } else {
                429
            }
        };
        stream.write_all(&status.to_be_bytes())?;
    }
    Ok(())
}

// A stream owns one of the fixed network workers. A second stream cannot
// occupy the control worker, even when it presents the same pairing identity.
struct VideoLease<'a>(&'a Shared);
impl Drop for VideoLease<'_> {
    fn drop(&mut self) {
        self.0.invalidate();
        self.0.video_active.store(false, Ordering::Relaxed);
    }
}

fn video_header(length: usize, size: [u32; 2], viewport: [f32; 2], id: u64, kind: u32) -> [u8; 32] {
    let mut header = [0; 32];
    header[0..4].copy_from_slice(&(length as u32).to_be_bytes());
    header[4..8].copy_from_slice(&size[0].to_be_bytes());
    header[8..12].copy_from_slice(&size[1].to_be_bytes());
    header[12..16].copy_from_slice(&viewport[0].to_be_bytes());
    header[16..20].copy_from_slice(&viewport[1].to_be_bytes());
    header[20..28].copy_from_slice(&id.to_be_bytes());
    header[28..32].copy_from_slice(&kind.to_be_bytes());
    header
}

fn serve_video(
    stream: &mut TcpStream,
    shared: &Shared,
    client: &str,
    ctx: &egui::Context,
) -> io::Result<()> {
    if shared
        .video_active
        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return respond(stream, 409, "", &[]);
    }
    let _lease = VideoLease(shared);
    shared.video_failed.store(false, Ordering::Relaxed);
    shared.invalidate();
    ctx.request_repaint();
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/x-pluriview-h264\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n")?;
    let mut last = 0;
    let mut epoch = shared.epoch.load(Ordering::Relaxed);
    let mut heartbeat = Instant::now() - Duration::from_secs(1);
    let mut last_frame = Instant::now();
    let mut wire = Vec::with_capacity(64 * 1024);
    while shared.running.load(Ordering::Relaxed) {
        {
            let mut peer = shared.peer.lock();
            let Some(peer) = peer.as_mut().filter(|p| p.id == client) else {
                break;
            };
            peer.seen = Instant::now();
        }
        let current_epoch = shared.epoch.load(Ordering::Relaxed);
        if epoch != current_epoch {
            // A pause/resume invalidates the codec references too. Tell the
            // phone to discard its decoder before accepting the new keyframe.
            stream.write_all(&video_header(0, [0; 2], [0.0; 2], 0, 2))?;
            epoch = current_epoch;
            last = 0;
            last_frame = Instant::now();
        }
        if shared.video_failed.load(Ordering::Relaxed) {
            stream.write_all(&video_header(0, [0; 2], [0.0; 2], 0, 3))?;
            break;
        }
        if shared.blocked.load(Ordering::Relaxed) {
            if heartbeat.elapsed() >= Duration::from_millis(200) {
                stream.write_all(&video_header(0, [0; 2], [0.0; 2], 0, 2))?;
                heartbeat = Instant::now();
            }
            last_frame = Instant::now();
        } else {
            let frame = shared
                .video_frames
                .lock()
                .iter()
                .find(|f| f.id > last)
                .cloned();
            if let Some(frame) = frame {
                // H.264 depends on preceding frames. Never skip a missing frame
                // and feed corrupt references, or play an old backlog slowly.
                if (last == 0 && !frame.stream_start)
                    || (last != 0 && frame.id != last + 1)
                    || frame.created.elapsed() > Duration::from_millis(250)
                {
                    break;
                }
                if shared.blocked.load(Ordering::Relaxed)
                    || epoch != shared.epoch.load(Ordering::Relaxed)
                {
                    continue;
                }
                // Keep each small record header with its payload instead of
                // sending a separate tiny TCP packet ahead of every picture.
                wire.clear();
                wire.extend_from_slice(&video_header(
                    frame.bytes.len(),
                    frame.size,
                    frame.viewport,
                    frame.id,
                    1,
                ));
                wire.extend_from_slice(&frame.bytes);
                stream.write_all(&wire)?;
                last = frame.id;
                last_frame = Instant::now();
            } else if last_frame.elapsed() > Duration::from_secs(2) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(
            if shared.blocked.load(Ordering::Relaxed) {
                20
            } else {
                2
            },
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command() -> CameraCommand {
        CameraCommand {
            pan: [0.0, 0.0],
            scale: 1.0,
            anchor: [0.5, 0.5],
            viewport: [800.0, 600.0],
        }
    }

    #[test]
    fn camera_commands_reject_invalid_values_and_tile_operations() {
        let mut c = command();
        assert!(c.is_valid());
        c.pan[0] = f32::NAN;
        assert!(!c.is_valid());
        c = command();
        c.scale = 0.0;
        assert!(!c.is_valid());
        c = command();
        c.anchor[1] = 1.1;
        assert!(!c.is_valid());
        assert!(serde_json::from_str::<CameraCommand>(
            r#"{"pan":[0,0],"scale":1,"anchor":[0,0],"viewport":[800,600],"delete":1}"#
        )
        .is_err());
    }

    #[test]
    fn shared_camera_zoom_preserves_anchor_and_pan_uses_viewport_size() {
        let mut canvas = crate::canvas::CanvasState::default();
        let rect = Rect::from_min_size(egui::pos2(80.0, 40.0), egui::vec2(800.0, 600.0));
        canvas.last_screen_rect = Some(rect);
        let before = canvas.camera_center(rect);
        let mut c = command();
        c.scale = 2.0;
        canvas.apply_viewer_camera(c);
        assert!(canvas.camera_center(rect).distance(before) < 0.001);
        c.scale = 1.0;
        c.pan = [0.1, -0.2];
        canvas.apply_viewer_camera(c);
        let expected = before + egui::vec2(-40.0, 60.0);
        assert!(canvas.camera_center(rect).distance(expected) < 0.001);
        assert!(canvas.selection.is_empty());
    }

    #[test]
    fn frame_encoding_crops_chrome_and_caps_dimensions() {
        let mut image = ColorImage::new([1600, 1000], Color32::RED);
        image.pixels[..1600 * 100].fill(Color32::BLUE);
        let job = CaptureJob {
            image: Arc::new(image),
            rect: Rect::from_min_max(egui::pos2(0.0, 50.0), egui::pos2(800.0, 500.0)),
            pixels_per_point: 2.0,
            epoch: 0,
        };
        let jpeg = encode_frame(&job, &mut Vec::new()).unwrap();
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1280, 720));
        let rgb = decoded.to_rgb8();
        assert!(rgb.get_pixel(0, 0)[0] > 240);
        assert!(rgb.get_pixel(0, 0)[2] < 10);
    }

    #[test]
    fn camera_commands_preserve_selection_and_stop_camera_animation() {
        let mut canvas = crate::canvas::CanvasState::default();
        let rect = Rect::from_min_size(egui::pos2(40.0, 20.0), egui::vec2(800.0, 600.0));
        canvas.last_screen_rect = Some(rect);
        canvas.selection = vec![crate::preview::PreviewId(7)];
        canvas.start_camera_transition(egui::pos2(1000.0, 1000.0), 2.0, rect, 0.0);
        let mut command = command();
        command.anchor = [0.25, 0.75];
        command.scale = 4.0;
        let anchor = rect.min + rect.size() * egui::vec2(0.25, 0.75);
        let before = canvas.screen_to_canvas(anchor, rect);
        canvas.apply_viewer_camera(command);
        assert!(canvas.screen_to_canvas(anchor, rect).distance(before) < 0.001);
        assert!(!canvas.is_camera_transitioning());
        assert_eq!(canvas.selection, vec![crate::preview::PreviewId(7)]);
        command.scale = f32::INFINITY;
        let camera = (canvas.pan, canvas.zoom);
        canvas.apply_viewer_camera(command);
        assert_eq!(camera, (canvas.pan, canvas.zoom));
        canvas.zoom = canvas.zoom_max;
        let before = canvas.screen_to_canvas(anchor, rect);
        command.scale = 2.0;
        canvas.apply_viewer_camera(command);
        assert_eq!(canvas.zoom, canvas.zoom_max);
        assert!(canvas.screen_to_canvas(anchor, rect).distance(before) < 0.001);
    }

    #[test]
    fn queue_discards_stale_other_peer_resized_and_blocked_commands() {
        let ctx = egui::Context::default();
        let mut host = Host::start(Ipv4Addr::LOCALHOST, ctx.clone()).unwrap();
        let (tx, rx) = mpsc::sync_channel(8);
        host.commands = rx;
        *host.shared.peer.lock() = Some(Peer {
            id: "phone".into(),
            seen: Instant::now(),
        });
        let mut viewer = RemoteView {
            host: Some(host),
            ..Default::default()
        };
        let mut canvas = crate::canvas::CanvasState::default();
        canvas.last_screen_rect = Some(Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        ));
        let mut c = command();
        c.pan = [0.1, 0.0];
        let previews = crate::preview::PreviewManager::new();
        let send = |command, client: &str, age, epoch| {
            tx.send(QueuedCamera {
                command: NavigationCommand::Camera(command),
                client: client.into(),
                received: Instant::now() - age,
                epoch,
            })
            .unwrap()
        };
        send(c, "phone", Duration::from_secs(1), 0);
        send(c, "other", Duration::ZERO, 0);
        send(c, "phone", Duration::ZERO, 3);
        let mut resized = c;
        resized.viewport[0] = 900.0;
        send(resized, "phone", Duration::ZERO, 0);
        viewer.apply_camera_commands(&ctx, &mut canvas, &previews, false);
        assert_eq!(canvas.pan, egui::Vec2::ZERO);
        send(c, "phone", Duration::ZERO, 0);
        viewer.apply_camera_commands(&ctx, &mut canvas, &previews, true);
        viewer.apply_camera_commands(&ctx, &mut canvas, &previews, false);
        assert_eq!(
            canvas.pan,
            egui::Vec2::ZERO,
            "Blocked input must not replay later"
        );
        send(c, "phone", Duration::ZERO, 0);
        viewer.apply_camera_commands(&ctx, &mut canvas, &previews, false);
        viewer
            .motion
            .as_mut()
            .unwrap()
            .advance(&mut canvas, ctx.input(|i| i.time) + 0.2);
        assert!((canvas.pan.x - 80.0).abs() < 0.001);
    }

    #[test]
    fn unwatched_and_paused_viewers_never_request_screenshots() {
        let ctx = egui::Context::default();
        let host = Host::start(Ipv4Addr::LOCALHOST, ctx.clone()).unwrap();
        let mut viewer = RemoteView {
            host: Some(host),
            next_capture: Instant::now() - Duration::from_secs(1),
            ..Default::default()
        };
        let rect = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let output = ctx.run(Default::default(), |ctx| {
            viewer.capture(ctx, Some(rect), false)
        });
        assert!(!output
            .viewport_output
            .values()
            .any(|v| v.commands.contains(&egui::ViewportCommand::Screenshot)));
        assert!(viewer.pending.is_none());
        *viewer.host.as_ref().unwrap().shared.peer.lock() = Some(Peer {
            id: "phone".into(),
            seen: Instant::now(),
        });
        let output = ctx.run(Default::default(), |ctx| {
            viewer.capture(ctx, Some(rect), true)
        });
        assert!(!output
            .viewport_output
            .values()
            .any(|v| v.commands.contains(&egui::ViewportCommand::Screenshot)));
        let output = ctx.run(Default::default(), |ctx| {
            viewer.capture(ctx, Some(rect), false)
        });
        assert!(output
            .viewport_output
            .values()
            .any(|v| v.commands.contains(&egui::ViewportCommand::Screenshot)));
        assert!(viewer.pending.is_some());
    }

    #[test]
    fn focus_queue_rejects_stale_other_peer_resized_and_blocked_requests() {
        let ctx = egui::Context::default();
        let mut host = Host::start(Ipv4Addr::LOCALHOST, ctx.clone()).unwrap();
        let (tx, rx) = mpsc::sync_channel(8);
        host.commands = rx;
        *host.shared.peer.lock() = Some(Peer {
            id: "phone".into(),
            seen: Instant::now(),
        });
        let mut viewer = RemoteView {
            host: Some(host),
            ..Default::default()
        };
        let mut canvas = crate::canvas::CanvasState::default();
        canvas.last_screen_rect = Some(Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        ));
        let mut previews = crate::preview::PreviewManager::new();
        let id = previews.add("tile".into(), egui::Pos2::ZERO, egui::Vec2::splat(100.0));
        let send = |client: &str, age, epoch, width| {
            tx.send(QueuedCamera {
                command: NavigationCommand::Focus(FocusCommand {
                    at: [0.05, 0.05],
                    viewport: [width, 600.0],
                }),
                client: client.into(),
                received: Instant::now() - age,
                epoch,
            })
            .unwrap()
        };
        send("phone", Duration::from_secs(1), 0, 800.0);
        send("other", Duration::ZERO, 0, 800.0);
        send("phone", Duration::ZERO, 1, 800.0);
        send("phone", Duration::ZERO, 0, 900.0);
        viewer.apply_camera_commands(&ctx, &mut canvas, &previews, false);
        assert!(!canvas.is_focusing_tile());
        send("phone", Duration::ZERO, 0, 800.0);
        viewer.apply_camera_commands(&ctx, &mut canvas, &previews, true);
        viewer.apply_camera_commands(&ctx, &mut canvas, &previews, false);
        assert!(!canvas.is_focusing_tile());
        send("phone", Duration::ZERO, 0, 800.0);
        viewer.apply_camera_commands(&ctx, &mut canvas, &previews, false);
        assert_eq!(canvas.selection, vec![id]);
        assert!(canvas.is_focusing_tile());
        assert!(viewer.motion.is_none());
    }

    #[test]
    fn native_protocol_bounds_requests_and_camera_queue() {
        let host = Host::start(Ipv4Addr::LOCALHOST, egui::Context::default()).unwrap();
        let uri = url::Url::parse(&host.invite).unwrap();
        let values: std::collections::HashMap<_, _> = uri.query_pairs().collect();
        let port = values["port"].parse::<u16>().unwrap();
        let send = |method: &str, route: &str, extra: &str, body: &str| {
            let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            write!(socket, "{method} {route} HTTP/1.1\r\nAuthorization: {}\r\nX-Pluriview-Client: 00000000000000000000000000000001\r\n{extra}\r\n{body}", auth::test_authorization(&values["token"], method, route, "00000000000000000000000000000001")).unwrap();
            let mut reply = String::new();
            socket.read_to_string(&mut reply).unwrap();
            reply
        };
        assert!(send(
            "GET",
            "/v1/frame",
            "Origin: http://untrusted.invalid\r\n",
            ""
        )
        .starts_with("HTTP/1.1 400"));
        assert!(send("POST", "/v1/camera", "Content-Length: 999999\r\n", "")
            .starts_with("HTTP/1.1 400"));
        assert!(send("GET", "/v1/frame", "", "").contains("X-Viewer-State: paused"));
        let body = r#"{"pan":[0.1,0],"scale":1,"anchor":[0,0],"viewport":[800,600]}"#;
        let length = format!("Content-Length: {}\r\n", body.len());
        assert!(send("POST", "/v1/camera", &length, body).starts_with("HTTP/1.1 423"));
        host.shared.blocked.store(false, Ordering::Relaxed);
        for _ in 0..8 {
            assert!(send("POST", "/v1/camera", &length, body).starts_with("HTTP/1.1 200"));
        }
        assert!(send("POST", "/v1/camera", &length, body).starts_with("HTTP/1.1 429"));
        assert_eq!(host.commands.try_iter().count(), 8);
        let focus_body = r#"{"at":[0.25,0.5],"viewport":[800,600]}"#;
        assert!(send(
            "POST",
            "/v1/focus",
            &format!("Content-Length: {}\r\n", focus_body.len()),
            focus_body
        )
        .starts_with("HTTP/1.1 200"));
        assert!(matches!(
            host.commands.try_recv().unwrap().command,
            NavigationCommand::Focus(_)
        ));
        let invalid_focus = r#"{"at":[1.5,0.5],"viewport":[800,600]}"#;
        assert!(send(
            "POST",
            "/v1/focus",
            &format!("Content-Length: {}\r\n", invalid_focus.len()),
            invalid_focus
        )
        .starts_with("HTTP/1.1 400"));
        host.shared.blocked.store(true, Ordering::Relaxed);
        assert!(send(
            "POST",
            "/v1/focus",
            &format!("Content-Length: {}\r\n", focus_body.len()),
            focus_body
        )
        .starts_with("HTTP/1.1 423"));
        host.shared.blocked.store(false, Ordering::Relaxed);
        let feedback = |body: &str| {
            send(
                "POST",
                "/v2/feedback",
                &format!("Content-Length: {}\r\n", body.len()),
                body,
            )
        };
        assert!(feedback(r#"{"frame":1,"decode_ms":5}"#).starts_with("HTTP/1.1 204"));
        host.shared.video_active.store(true, Ordering::Relaxed);
        assert!(feedback(r#"{"frame":1,"decode_ms":-1}"#).starts_with("HTTP/1.1 400"));
        assert!(feedback(r#"{"frame":1,"decode_ms":5,"pan":[1,1]}"#).starts_with("HTTP/1.1 400"));
        assert!(feedback(r#"{"frame":999,"decode_ms":5}"#).starts_with("HTTP/1.1 200"));
        let epoch = host.shared.epoch.load(Ordering::Relaxed);
        assert!(feedback(r#"{"frame":1,"decode_ms":5}"#).starts_with("HTTP/1.1 200"));
        assert_eq!(
            host.shared.epoch.load(Ordering::Relaxed),
            epoch,
            "Legacy feedback must not reset the encoder"
        );
        assert_eq!(
            host.commands.try_iter().count(),
            0,
            "Feedback cannot enqueue camera actions"
        );
        host.shared.peer.lock().as_mut().unwrap().id = "00000000000000000000000000000002".into();
        assert!(feedback(r#"{"frame":1,"decode_ms":5}"#).starts_with("HTTP/1.1 409"));
    }

    #[test]
    #[cfg(windows)]
    #[ignore = "requires installed Windows H.264 encoder and a short localhost streaming run"]
    fn live_encoder_stays_at_720p_despite_slow_decoder_feedback_and_pc_resize() {
        let host = Host::start(Ipv4Addr::LOCALHOST, egui::Context::default()).unwrap();
        let uri = url::Url::parse(&host.invite).unwrap();
        let values: std::collections::HashMap<_, _> = uri.query_pairs().collect();
        let port = values["port"].parse::<u16>().unwrap();
        let client = "00000000000000000000000000000001";
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        write!(
            stream,
            "GET /v2/video HTTP/1.1\r\nAuthorization: {}\r\nX-Pluriview-Client: {client}\r\n\r\n",
            auth::test_authorization(&values["token"], "GET", "/v2/video", client)
        )
        .unwrap();
        let mut response = Vec::new();
        let mut byte = [0];
        while !response.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            response.push(byte[0]);
        }
        assert!(response.starts_with(b"HTTP/1.1 200"));
        host.shared.blocked.store(false, Ordering::Relaxed);
        let shared = host.shared.clone();
        let sender = host.encoder.clone();
        let producer = std::thread::spawn(move || {
            let image = Arc::new(ColorImage::new(
                [1280, 720],
                Color32::from_rgb(50, 100, 200),
            ));
            let start = Instant::now();
            let interval = Duration::from_nanos(1_000_000_000 / VIDEO_FPS);
            let mut deadline = start;
            while shared.running.load(Ordering::Relaxed)
                && start.elapsed() < Duration::from_secs(12)
            {
                if !shared.busy.swap(true, Ordering::Relaxed) {
                    let resized = start.elapsed() > Duration::from_secs(1);
                    sender
                        .send(CaptureJob {
                            image: image.clone(),
                            rect: Rect::from_min_size(
                                egui::Pos2::ZERO,
                                if resized {
                                    egui::vec2(1920.0, 1080.0)
                                } else {
                                    egui::vec2(1280.0, 720.0)
                                },
                            ),
                            pixels_per_point: if resized { 2.0 / 3.0 } else { 1.0 },
                            epoch: shared.epoch.load(Ordering::Relaxed),
                        })
                        .unwrap();
                }
                deadline = cadence::next(deadline, Instant::now(), interval);
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            }
        });
        let mut heights = Vec::new();
        let mut last_feedback = Instant::now();
        let started = Instant::now();
        let mut count = 0;
        let mut first = true;
        let mut viewport = 0.0;
        while started.elapsed() < Duration::from_secs(5) {
            let mut header = [0; 32];
            stream.read_exact(&mut header).unwrap();
            let length = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
            assert!(length <= 4 * 1024 * 1024);
            let kind = u32::from_be_bytes(header[28..].try_into().unwrap());
            let mut bytes = vec![0; length];
            stream.read_exact(&mut bytes).unwrap();
            if kind == 2 {
                first = true;
                continue;
            }
            assert_eq!(kind, 1, "Video must remain available at fixed quality");
            let current_viewport = f32::from_be_bytes(header[12..16].try_into().unwrap());
            if viewport != current_viewport {
                first = true;
                viewport = current_viewport;
            }
            let height = u32::from_be_bytes(header[8..12].try_into().unwrap());
            if first {
                assert!(
                    bytes
                        .windows(4)
                        .any(|n| n[..3] == [0, 0, 1] && n[3] & 31 == 7),
                    "New stream must include SPS"
                );
                assert!(
                    bytes
                        .windows(4)
                        .any(|n| n[..3] == [0, 0, 1] && n[3] & 31 == 5),
                    "New stream must include IDR"
                );
                first = false;
            }
            if !heights.contains(&height) {
                heights.push(height);
            }
            count += 1;
            if last_feedback.elapsed() >= Duration::from_millis(500) {
                let id = u64::from_be_bytes(header[20..28].try_into().unwrap());
                let body = format!("{{\"frame\":{id},\"decode_ms\":80}}");
                let mut feedback = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
                feedback
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                write!(feedback, "POST /v2/feedback HTTP/1.1\r\nAuthorization: {}\r\nX-Pluriview-Client: {client}\r\nContent-Length: {}\r\n\r\n{body}", auth::test_authorization(&values["token"], "POST", "/v2/feedback", client), body.len()).unwrap();
                let mut reply = String::new();
                feedback.read_to_string(&mut reply).unwrap();
                assert!(reply.starts_with("HTTP/1.1 200"));
                last_feedback = Instant::now();
            }
        }
        host.shared.running.store(false, Ordering::Relaxed);
        producer.join().unwrap();
        assert_eq!(
            heights,
            [720],
            "Slow decoder reports must never lower resolution"
        );
        assert_eq!(viewport, 1920.0, "PC resize must refresh viewport metadata");
        assert!(count > 60);
        assert_eq!(host.commands.try_iter().count(), 0);
        eprintln!(
            "Stream delivered {count} frames across {heights:?} in {:?}; hardware={}",
            started.elapsed(),
            host.shared.hardware.load(Ordering::Relaxed)
        );
    }

    #[test]
    fn stopping_host_closes_idle_listener_workers_without_a_wakeup_connection() {
        let host = Host::start(Ipv4Addr::LOCALHOST, egui::Context::default()).unwrap();
        let uri = url::Url::parse(&host.invite).unwrap();
        let port = uri
            .query_pairs()
            .find(|(k, _)| k == "port")
            .unwrap()
            .1
            .parse::<u16>()
            .unwrap();
        // Let the workers enter their idle accept path before stopping.
        std::thread::sleep(Duration::from_millis(100));
        drop(host);
        std::thread::sleep(Duration::from_millis(100));
        // A single first attempt must fail; repeated connects could mask an
        // accept loop that exits only when a new connection wakes each worker.
        assert!(TcpStream::connect_timeout(
            &SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into(),
            Duration::from_millis(500)
        )
        .is_err());
    }

    #[test]
    fn slow_unauthenticated_connections_cannot_occupy_every_worker() {
        let host = Host::start(Ipv4Addr::LOCALHOST, egui::Context::default()).unwrap();
        let uri = url::Url::parse(&host.invite).unwrap();
        let port = uri
            .query_pairs()
            .find(|(k, _)| k == "port")
            .unwrap()
            .1
            .parse::<u16>()
            .unwrap();
        // Two silent connections from one address use its pre-auth budget.
        let _held: Vec<_> = (0..MAX_PRE_AUTH_PER_ADDRESS)
            .map(|_| TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap())
            .collect();
        std::thread::sleep(Duration::from_millis(100));
        // A further one is closed at once instead of waiting for headers.
        let mut extra = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        extra
            .set_read_timeout(Some(Duration::from_millis(1500)))
            .unwrap();
        let started = Instant::now();
        let mut byte = [0];
        let closed = match extra.read(&mut byte) {
            Ok(read) => read == 0,
            Err(error) => {
                error.kind() != io::ErrorKind::WouldBlock && error.kind() != io::ErrorKind::TimedOut
            }
        };
        assert!(closed && started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn host_requires_pairing_and_rejects_tile_routes() {
        let host = Host::start(Ipv4Addr::LOCALHOST, egui::Context::default()).unwrap();
        let uri = url::Url::parse(&host.invite).unwrap();
        let values: std::collections::HashMap<_, _> = uri.query_pairs().collect();
        let port = values["port"].parse::<u16>().unwrap();
        let request = |path: &str, token: &str, client: &str| {
            let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            write!(
                socket,
                "GET {path} HTTP/1.1\r\nAuthorization: {}\r\nX-Pluriview-Client: {client}\r\n\r\n",
                auth::test_authorization(token, "GET", path, client)
            )
            .unwrap();
            let mut reply = String::new();
            socket.read_to_string(&mut reply).unwrap();
            reply
        };
        let client = "00000000000000000000000000000001";
        assert!(request("/v1/frame", "wrong", client).starts_with("HTTP/1.1 401"));
        assert!(request("/v2/video", "wrong", client).starts_with("HTTP/1.1 401"));
        assert!(!host.shared.watching());
        assert!(request("/v1/delete", &values["token"], client).starts_with("HTTP/1.1 404"));
        assert!(request("/v1/frame", &values["token"], client).starts_with("HTTP/1.1 204"));
        assert!(host.shared.watching());
        // The link is single-use: another phone cannot reuse it.
        assert!(request(
            "/v1/frame",
            &values["token"],
            "00000000000000000000000000000002"
        )
        .starts_with("HTTP/1.1 401"));
    }

    #[test]
    fn pairing_link_secret_never_crosses_the_network_and_expires() {
        let mut host = Host::start(Ipv4Addr::LOCALHOST, egui::Context::default()).unwrap();
        let uri = url::Url::parse(&host.invite).unwrap();
        let values: std::collections::HashMap<_, _> = uri.query_pairs().collect();
        let port = values["port"].parse::<u16>().unwrap();
        let client = "00000000000000000000000000000001";
        let send = |authorization: &str| {
            let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            write!(socket, "GET /v1/frame HTTP/1.1\r\nAuthorization: {authorization}\r\nX-Pluriview-Client: {client}\r\n\r\n").unwrap();
            let mut reply = String::new();
            socket.read_to_string(&mut reply).unwrap();
            reply
        };
        // The old bearer form is no longer accepted.
        let reply = send(&format!("Bearer {}", values["token"]));
        assert!(reply.starts_with("HTTP/1.1 401"), "{reply}");
        // A captured signed request cannot be replayed.
        let signed = auth::test_authorization(&values["token"], "GET", "/v1/frame", client);
        assert!(send(&signed).starts_with("HTTP/1.1 204"));
        assert!(send(&signed).starts_with("HTTP/1.1 401"));

        // An unused link expires; a new link replaces the old secret.
        host.refresh_invite().unwrap();
        host.shared.invite.lock().created -= INVITE_LIFETIME;
        let fresh = url::Url::parse(&host.invite).unwrap();
        let fresh: std::collections::HashMap<_, _> = fresh.query_pairs().collect();
        assert!(send(&auth::test_authorization(
            &fresh["token"],
            "GET",
            "/v1/frame",
            client
        ))
        .starts_with("HTTP/1.1 401"));
        host.refresh_invite().unwrap();
        let fresh = url::Url::parse(&host.invite).unwrap();
        let fresh: std::collections::HashMap<_, _> = fresh.query_pairs().collect();
        assert!(send(&auth::test_authorization(
            &values["token"],
            "GET",
            "/v1/frame",
            client
        ))
        .starts_with("HTTP/1.1 401"));
        assert!(send(&auth::test_authorization(
            &fresh["token"],
            "GET",
            "/v1/frame",
            client
        ))
        .starts_with("HTTP/1.1 204"));
    }

    #[test]
    fn video_stream_is_exclusive_resets_on_pause_and_rejects_missing_references() {
        let host = Host::start(Ipv4Addr::LOCALHOST, egui::Context::default()).unwrap();
        let uri = url::Url::parse(&host.invite).unwrap();
        let values: std::collections::HashMap<_, _> = uri.query_pairs().collect();
        let connect = || {
            let mut socket =
                TcpStream::connect((Ipv4Addr::LOCALHOST, values["port"].parse::<u16>().unwrap()))
                    .unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            write!(socket, "GET /v2/video HTTP/1.1\r\nAuthorization: {}\r\nX-Pluriview-Client: 00000000000000000000000000000001\r\n\r\n", auth::test_authorization(&values["token"], "GET", "/v2/video", "00000000000000000000000000000001")).unwrap();
            let mut header = Vec::new();
            let mut byte = [0];
            while !header.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
            }
            (socket, String::from_utf8(header).unwrap())
        };
        let (mut stream, header) = connect();
        assert!(header.starts_with("HTTP/1.1 200"));
        assert!(
            connect().1.starts_with("HTTP/1.1 409"),
            "A duplicate stream must leave the command worker free"
        );
        let record = |stream: &mut TcpStream| {
            let mut header = [0; 32];
            stream.read_exact(&mut header).unwrap();
            let length = u32::from_be_bytes(header[..4].try_into().unwrap());
            let mut body = vec![0; length as usize];
            stream.read_exact(&mut body).unwrap();
            (u32::from_be_bytes(header[28..].try_into().unwrap()), body)
        };
        assert_eq!(record(&mut stream), (2, vec![]));
        host.shared.blocked.store(false, Ordering::Relaxed);
        host.shared.invalidate();
        host.shared
            .video_frames
            .lock()
            .push_back(Arc::new(VideoFrame {
                bytes: vec![0, 0, 1, 0x65],
                size: [1280, 680],
                viewport: [1280.0, 680.0],
                id: 1,
                created: Instant::now(),
                stream_start: true,
            }));
        loop {
            let (kind, body) = record(&mut stream);
            if kind == 1 {
                assert_eq!(body, [0, 0, 1, 0x65]);
                break;
            }
        }
        host.shared
            .video_frames
            .lock()
            .push_back(Arc::new(VideoFrame {
                bytes: vec![0, 0, 1, 0x41],
                size: [1280, 680],
                viewport: [1280.0, 680.0],
                id: 3,
                created: Instant::now(),
                stream_start: false,
            }));
        let mut rest = Vec::new();
        stream.read_to_end(&mut rest).unwrap();
        assert!(
            rest.is_empty(),
            "Missing frame 2 must close the stream instead of sending frame 3"
        );
        assert!(!host.shared.video_active.load(Ordering::Relaxed));
        assert!(host.shared.video_frames.lock().is_empty());
        let (mut fallback, header) = connect();
        assert!(header.starts_with("HTTP/1.1 200"));
        host.shared.video_failed.store(true, Ordering::Relaxed);
        assert_eq!(record(&mut fallback), (3, vec![]));
        let mut rest = Vec::new();
        fallback.read_to_end(&mut rest).unwrap();
        assert!(rest.is_empty());
    }
}
