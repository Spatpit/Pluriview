use super::*;

#[test]
#[ignore = "runs real 720p encoding for 70 seconds over both pairing paths"]
fn sustained_video_uses_the_same_transport_after_qr_and_approval() {
    for approved in [false, true] {
        let host = Host::start(Ipv4Addr::LOCALHOST, egui::Context::default()).unwrap();
        let uri = url::Url::parse(&host.invite).unwrap();
        let values: std::collections::HashMap<_, _> = uri.query_pairs().collect();
        let client = pairing::random_id().unwrap();
        let token = if approved {
            let id = pairing::random_id().unwrap();
            let input = || {
                serde_json::from_value(serde_json::json!({"request":id,"name":"Soak test"}))
                    .unwrap()
            };
            let mut pairing = host.shared.pairing.lock();
            assert_eq!(
                pairing
                    .poll(input(), &client, false, false, Instant::now())
                    .0,
                202
            );
            pairing.decide(&id, true, false, Instant::now());
            let (status, body) = pairing.poll(input(), &client, false, false, Instant::now());
            assert_eq!(status, 200);
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["token"]
                .as_str()
                .unwrap()
                .to_owned()
        } else {
            values["token"].to_string()
        };
        let mut stream =
            TcpStream::connect((Ipv4Addr::LOCALHOST, values["port"].parse::<u16>().unwrap()))
                .unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write!(
            stream,
            "GET /v2/video HTTP/1.1\r\nAuthorization: {}\r\nX-Pluriview-Client: {client}\r\n\r\n",
            super::auth::test_authorization(&token, "GET", "/v2/video", &client)
        )
        .unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            let mut b = [0];
            stream.read_exact(&mut b).unwrap();
            header.push(b[0]);
        }
        assert!(header.starts_with(b"HTTP/1.1 200"));
        host.shared.blocked.store(false, Ordering::Relaxed);
        let shared = host.shared.clone();
        let sender = host.encoder.clone();
        let producer = std::thread::spawn(move || {
            let images: Vec<_> = (0..2)
                .map(|phase| {
                    let mut image = ColorImage::new([1280, 720], Color32::BLACK);
                    for (index, pixel) in image.pixels.iter_mut().enumerate() {
                        let x = index % 1280;
                        let y = index / 1280;
                        *pixel = Color32::from_rgb(
                            ((x + phase * 32) % 256) as u8,
                            (y % 256) as u8,
                            ((x ^ y) % 256) as u8,
                        );
                    }
                    Arc::new(image)
                })
                .collect();
            let mut frame = 0;
            let interval = Duration::from_nanos(1_000_000_000 / VIDEO_FPS);
            let mut deadline = Instant::now();
            while shared.running.load(Ordering::Relaxed) {
                if !shared.busy.swap(true, Ordering::Relaxed) {
                    if sender
                        .send(CaptureJob {
                            image: images[(frame / 15) % 2].clone(),
                            rect: Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 720.0)),
                            pixels_per_point: 1.0,
                            epoch: shared.epoch.load(Ordering::Relaxed),
                        })
                        .is_err()
                    {
                        break;
                    }
                    frame += 1;
                }
                deadline = cadence::next(deadline, Instant::now(), interval);
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            }
        });
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let start = Instant::now();
            let mut frames = 0;
            let mut last = 0;
            while start.elapsed() < Duration::from_secs(35) {
                let mut header = [0; 32];
                stream
                    .read_exact(&mut header)
                    .expect("Sustained stream disconnected");
                let length = u32::from_be_bytes(header[0..4].try_into().unwrap()) as usize;
                assert!(length <= 4 * 1024 * 1024);
                let kind = u32::from_be_bytes(header[28..32].try_into().unwrap());
                let mut bytes = vec![0; length];
                stream.read_exact(&mut bytes).unwrap();
                if kind == 2 {
                    assert!(frames == 0, "Unrequested stream reset after playback began");
                    continue;
                }
                assert_eq!(kind, 1);
                assert_eq!(u32::from_be_bytes(header[8..12].try_into().unwrap()), 720);
                let id = u64::from_be_bytes(header[20..28].try_into().unwrap());
                if last != 0 {
                    assert_eq!(id, last + 1);
                }
                last = id;
                frames += 1;
                if frames % 90 == 0 {
                    std::thread::sleep(Duration::from_millis(120));
                }
            }
            assert!(
                frames > 700,
                "Unexpected sustained capture starvation: {frames}"
            );
            eprintln!(
                "{} pairing: {frames} uninterrupted frames in {:?}",
                if approved { "Approved" } else { "QR" },
                start.elapsed()
            );
        }));
        host.shared.running.store(false, Ordering::Relaxed);
        producer.join().unwrap();
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
    }
}
