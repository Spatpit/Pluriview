use super::*;

const CLIENT: &str = "00000000000000000000000000000001";
const CAMERA: &str = r#"{"type":"camera","value":{"pan":[0.01,0],"scale":1,"anchor":[0.5,0.5],"viewport":[1280,720]}}"#;
const FOCUS: &str = r#"{"type":"focus","value":{"at":[0.5,0.5],"viewport":[1280,720]}}"#;
const PING: &str = r#"{"type":"ping"}"#;

fn connect(invite: &str, route: &str, client: &str, authorized: bool) -> (TcpStream, String) {
    let uri = url::Url::parse(invite).unwrap();
    let values: std::collections::HashMap<_, _> = uri.query_pairs().collect();
    let port = values["port"].parse::<u16>().unwrap();
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream.set_nodelay(true).unwrap();
    let token = if authorized {
        &values["token"]
    } else {
        "invalid"
    };
    write!(
        stream,
        "GET {route} HTTP/1.1\r\nAuthorization: {}\r\nX-Pluriview-Client: {client}\r\n\r\n",
        super::auth::test_authorization(token, "GET", route, client)
    )
    .unwrap();
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        header.push(byte[0]);
        assert!(header.len() <= 4096);
    }
    (stream, String::from_utf8(header).unwrap())
}

fn send(stream: &mut TcpStream, body: &str) -> u16 {
    let mut record = Vec::from((body.len() as u16).to_be_bytes());
    record.extend_from_slice(body.as_bytes());
    stream.write_all(&record).unwrap();
    let mut status = [0; 2];
    stream.read_exact(&mut status).unwrap();
    u16::from_be_bytes(status)
}

#[test]
#[cfg(windows)]
fn optional_audio_preserves_authentication_lease_and_pause_guards() {
    let host = Host::start(Ipv4Addr::LOCALHOST, egui::Context::default()).unwrap();
    host.shared.audio.update([std::process::id()], false);
    assert!(connect(&host.invite, "/v2/audio", CLIENT, false)
        .1
        .starts_with("HTTP/1.1 401"));
    assert!(connect(&host.invite, "/v2/audio", CLIENT, true)
        .1
        .starts_with("HTTP/1.1 409"));
    assert!(
        !host.shared.watching(),
        "Audio cannot claim the video lease"
    );
    connect(&host.invite, "/v1/frame", CLIENT, true);
    let seen = host.shared.peer.lock().as_ref().unwrap().seen;
    assert!(connect(&host.invite, "/v2/audio", CLIENT, true)
        .1
        .starts_with("HTTP/1.1 423"));
    host.shared.blocked.store(false, Ordering::Relaxed);
    assert!(
        connect(&host.invite, "/v2/audio", CLIENT, true)
            .1
            .starts_with("HTTP/1.1 423"),
        "Audio requires active video"
    );
    host.shared.audio.update([std::process::id()], true);
    assert!(connect(&host.invite, "/v2/audio", CLIENT, true)
        .1
        .starts_with("HTTP/1.1 423"));
    assert_eq!(
        host.shared.peer.lock().as_ref().unwrap().seen,
        seen,
        "Audio cannot renew the lease"
    );
}

#[test]
#[cfg(windows)]
#[ignore = "requires Windows process loopback and the installed AAC encoder"]
fn native_audio_stream_stops_on_toggle_off_and_source_change() {
    for monitor in [false, true] {
        native_audio_stream_lifecycle(monitor);
    }
}

#[cfg(windows)]
fn native_audio_stream_lifecycle(monitor: bool) {
    let host = Host::start(Ipv4Addr::LOCALHOST, egui::Context::default()).unwrap();
    host.shared.audio.update([std::process::id()], monitor);
    connect(&host.invite, "/v1/frame", CLIENT, true);
    host.shared.blocked.store(false, Ordering::Relaxed);
    host.shared.video_active.store(true, Ordering::Relaxed);
    let (mut stream, header) = connect(&host.invite, "/v2/audio", CLIENT, true);
    assert!(header.starts_with("HTTP/1.1 200"), "{header}");
    let mut format = [0; 6];
    stream.read_exact(&mut format).unwrap();
    assert_eq!(&format, b"PVA1\x11\x90");
    let packets = if monitor { 750 } else { 20 };
    for _ in 0..packets {
        // Model the UI/source heartbeat and live video's lease renewal. A
        // sub-second test misses capture queue drift and stale-source expiry.
        host.shared.audio.update([std::process::id()], monitor);
        host.shared.peer.lock().as_mut().unwrap().seen = Instant::now();
        let mut length = [0; 4];
        stream.read_exact(&mut length).unwrap();
        let length = u32::from_be_bytes(length) as usize;
        assert!((1..=8192).contains(&length));
        stream.read_exact(&mut vec![0; length]).unwrap();
    }
    assert!(connect(&host.invite, "/v2/audio", CLIENT, true)
        .1
        .starts_with("HTTP/1.1 409"));
    // Video + audio + control still leave a worker for status/pairing requests.
    let (_control, header) = connect(&host.invite, "/v2/control", CLIENT, true);
    assert!(header.starts_with("HTTP/1.1 200"));
    assert!(connect(&host.invite, "/v3/info", CLIENT, true)
        .1
        .starts_with("HTTP/1.1 200"));
    drop(stream);
    std::thread::sleep(Duration::from_millis(150));
    host.shared.audio.update([std::process::id()], monitor);
    let (mut stream, header) = connect(&host.invite, "/v2/audio", CLIENT, true);
    assert!(
        header.starts_with("HTTP/1.1 200"),
        "Toggle-off did not release capture: {header}"
    );
    stream.read_exact(&mut format).unwrap();
    host.shared.audio.update([std::process::id()], !monitor);
    let mut tail = Vec::new();
    stream.read_to_end(&mut tail).unwrap();
    assert!(
        tail.len() < 8196 * 6,
        "Old audio accumulated after source change"
    );
}

#[test]
fn persistent_control_preserves_auth_bounds_pause_and_peer_lease() {
    let host = Host::start(Ipv4Addr::LOCALHOST, egui::Context::default()).unwrap();
    assert!(connect(&host.invite, "/v2/control", CLIENT, false)
        .1
        .starts_with("HTTP/1.1 401"));
    assert!(connect(&host.invite, "/v2/control", CLIENT, true)
        .1
        .starts_with("HTTP/1.1 409"));
    assert!(
        !host.shared.watching(),
        "Controls cannot claim the viewer lease"
    );
    assert!(connect(&host.invite, "/v1/frame", CLIENT, true)
        .1
        .starts_with("HTTP/1.1 204"));
    let (mut control, header) = connect(&host.invite, "/v2/control", CLIENT, true);
    assert!(header.starts_with("HTTP/1.1 200"));
    assert!(connect(&host.invite, "/v2/control", CLIENT, true)
        .1
        .starts_with("HTTP/1.1 409"));
    // The pairing link belongs to the first phone that used it.
    assert!(connect(
        &host.invite,
        "/v2/control",
        "00000000000000000000000000000002",
        true
    )
    .1
    .starts_with("HTTP/1.1 401"));
    let seen = host.shared.peer.lock().as_ref().unwrap().seen;
    assert_eq!(send(&mut control, PING), 200);
    assert_eq!(
        host.shared.peer.lock().as_ref().unwrap().seen,
        seen,
        "Controls must not renew the video lease"
    );
    assert_eq!(send(&mut control, CAMERA), 423);
    assert_eq!(send(&mut control, FOCUS), 423);
    host.shared.blocked.store(false, Ordering::Relaxed);
    for body in [
        r#"{"type":"delete","value":1}"#,
        r#"{"type":"camera","value":{"pan":[0,0],"scale":99,"anchor":[0,0],"viewport":[1280,720]}}"#,
        r#"{"type":"focus","value":{"at":[2,0],"viewport":[1280,720]}}"#,
        r#"{"type":"ping","delete":1}"#,
        "malformed",
    ] {
        assert_eq!(send(&mut control, body), 400);
    }
    for i in 0..8 {
        assert_eq!(
            send(&mut control, if i % 2 == 0 { CAMERA } else { FOCUS }),
            200
        );
    }
    assert_eq!(send(&mut control, CAMERA), 429);
    assert_eq!(send(&mut control, PING), 200);
    let commands: Vec<_> = host.commands.try_iter().collect();
    assert_eq!(commands.len(), 8);
    assert!(matches!(commands[0].command, NavigationCommand::Camera(_)));
    assert!(matches!(commands[1].command, NavigationCommand::Focus(_)));
    host.shared.peer.lock().as_mut().unwrap().seen = Instant::now() - LEASE;
    assert_eq!(send(&mut control, CAMERA), 409);
    let mut byte = [0];
    assert_eq!(control.read(&mut byte).unwrap(), 0);
    assert_eq!(host.commands.try_iter().count(), 0);
}

#[test]
fn persistent_control_rejects_oversized_records_and_stops_when_revoked() {
    let host = Host::start(Ipv4Addr::LOCALHOST, egui::Context::default()).unwrap();
    connect(&host.invite, "/v1/frame", CLIENT, true);
    let (mut control, _) = connect(&host.invite, "/v2/control", CLIENT, true);
    control.write_all(&1025u16.to_be_bytes()).unwrap();
    let mut status = [0; 2];
    control.read_exact(&mut status).unwrap();
    assert_eq!(u16::from_be_bytes(status), 400);
    let mut byte = [0];
    assert_eq!(control.read(&mut byte).unwrap(), 0);
    let (mut control, header) = connect(&host.invite, "/v2/control", CLIENT, true);
    assert!(header.starts_with("HTTP/1.1 200"));
    host.shared.running.store(false, Ordering::Relaxed);
    assert_eq!(control.read(&mut byte).unwrap(), 0);
    assert!(!host.shared.control_active.load(Ordering::Relaxed));
}

#[test]
#[cfg(windows)]
#[ignore = "requires Windows H.264 encoder; measures real localhost video alongside continuous controls"]
fn live_video_cadence_with_persistent_gestures_and_receive_jitter() {
    let mut host = Host::start(Ipv4Addr::LOCALHOST, egui::Context::default()).unwrap();
    // Model the UI consuming commands independently of codec startup/readback.
    let commands = std::mem::replace(&mut host.commands, mpsc::sync_channel(8).1);
    let shared = host.shared.clone();
    let consumer = std::thread::spawn(move || {
        while shared.running.load(Ordering::Relaxed) {
            commands.try_iter().for_each(drop);
            std::thread::sleep(Duration::from_millis(8));
        }
    });
    let (mut stream, response) = connect(&host.invite, "/v2/video", CLIENT, true);
    assert!(response.starts_with("HTTP/1.1 200"));
    host.shared.blocked.store(false, Ordering::Relaxed);
    let (mut control, response) = connect(&host.invite, "/v2/control", CLIENT, true);
    assert!(response.starts_with("HTTP/1.1 200"));
    let shared = host.shared.clone();
    let controls = std::thread::spawn(move || {
        let mut count = 0;
        let mut deadline = Instant::now();
        while shared.running.load(Ordering::Relaxed) {
            // The last message may race an intentional host shutdown.
            let mut record = Vec::from((CAMERA.len() as u16).to_be_bytes());
            record.extend_from_slice(CAMERA.as_bytes());
            if control.write_all(&record).is_err() {
                break;
            }
            let mut status = [0; 2];
            if control.read_exact(&mut status).is_err() {
                break;
            }
            let status = u16::from_be_bytes(status);
            if status == 409 {
                break;
            }
            assert_eq!(status, 200);
            count += 1;
            deadline = cadence::next(
                deadline,
                Instant::now(),
                Duration::from_nanos(1_000_000_000 / 60),
            );
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
        }
        count
    });
    let shared = host.shared.clone();
    let sender = host.encoder.clone();
    let producer = std::thread::spawn(move || {
        let mut image = Arc::new(ColorImage::new([1280, 720], Color32::BLACK));
        let mut frame = 0usize;
        let started = Instant::now();
        let mut deadline = started;
        while shared.running.load(Ordering::Relaxed) && started.elapsed() < Duration::from_secs(12)
        {
            if !shared.busy.swap(true, Ordering::Relaxed) {
                // Moving detail exercises payload bandwidth, not just tiny
                // unchanged pictures. All content is a generated test fixture.
                for (i, pixel) in Arc::make_mut(&mut image).pixels.iter_mut().enumerate() {
                    let x = i % 1280 + frame * 5;
                    let y = i / 1280 + frame * 3;
                    *pixel = Color32::from_rgb(
                        (x * 7 + y * 3) as u8,
                        (x ^ y) as u8,
                        (x * 3 + y * 11) as u8,
                    );
                }
                frame += 1;
                sender
                    .send(CaptureJob {
                        image: image.clone(),
                        rect: Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 720.0)),
                        pixels_per_point: 1.0,
                        epoch: shared.epoch.load(Ordering::Relaxed),
                    })
                    .unwrap();
            }
            deadline = cadence::next(
                deadline,
                Instant::now(),
                Duration::from_nanos(1_000_000_000 / VIDEO_FPS),
            );
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
        }
    });
    let started = Instant::now();
    let mut received = Vec::new();
    let mut last_id = 0;
    let mut encoded = Vec::new();
    while started.elapsed() < Duration::from_secs(7) {
        let mut header = [0; 32];
        stream.read_exact(&mut header).unwrap();
        let length = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        assert!(length <= 4 * 1024 * 1024);
        let mut bytes = vec![0; length];
        stream.read_exact(&mut bytes).unwrap();
        let kind = u32::from_be_bytes(header[28..].try_into().unwrap());
        if kind == 2 {
            continue;
        }
        assert_eq!(kind, 1, "Video fell back during normal control traffic");
        assert_eq!(u32::from_be_bytes(header[4..8].try_into().unwrap()), 1280);
        assert_eq!(u32::from_be_bytes(header[8..12].try_into().unwrap()), 720);
        let id = u64::from_be_bytes(header[20..28].try_into().unwrap());
        if last_id != 0 {
            assert_eq!(id, last_id + 1, "Lost an H.264 reference");
        }
        last_id = id;
        received.push(Instant::now());
        encoded.extend(bytes);
        if started.elapsed() > Duration::from_secs(3) && received.len() % 20 == 0 {
            std::thread::sleep(Duration::from_millis(60));
        }
    }
    host.shared.running.store(false, Ordering::Relaxed);
    producer.join().unwrap();
    consumer.join().unwrap();
    let controls = controls.join().unwrap();
    let fps = (received.len() - 1) as f64
        / received
            .last()
            .unwrap()
            .duration_since(received[0])
            .as_secs_f64();
    let mut gaps: Vec<_> = received
        .windows(2)
        .map(|v| v[1].duration_since(v[0]).as_secs_f64() * 1000.0)
        .collect();
    gaps.sort_by(f64::total_cmp);
    let p95 = gaps[gaps.len() * 95 / 100];
    let mbps = encoded.len() as f64 * 8.0
        / received
            .last()
            .unwrap()
            .duration_since(received[0])
            .as_secs_f64()
        / 1_000_000.0;
    eprintln!("720p loopback: {fps:.2} received FPS, {mbps:.2} Mbps, p95 gap {p95:.1} ms, {} video frames, {controls} accepted controls; hardware={}", received.len(), host.shared.hardware.load(Ordering::Relaxed));
    assert!(fps >= 28.0, "Steady stream failed to sustain near-30 FPS");
    assert!(controls >= 300, "Insufficient concurrent gesture traffic");
    if let Some(path) = std::env::var_os("PLURIVIEW_TEST_H264_OUTPUT") {
        std::fs::write(path, encoded).unwrap();
    }
}
