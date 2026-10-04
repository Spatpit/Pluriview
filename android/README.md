# Pluriview Viewer for Android

Development preview of a **shared PC canvas camera**. Requires Android 8.0+
and a running Windows Pluriview build with View → Phone Viewer. There is no
cloud service or account. Use only trusted private Wi-Fi: the LAN transport is
unencrypted. Version 0.3.11 fixes audio queue-full reconnects and retains
the optional tile audio introduced in 0.3.9. Use the updated Windows 0.7.1 build
for simultaneous phone audio and desktop Stream Audio Monitor support.
Version 0.3.8 uses a dedicated video socket with
resumable reads: short socket timeouts preserve partially received records and
decoder references, instead of restarting the stream. Reads poll at 250 ms,
with a five-second silence limit and eight-second total record limit. The HTTP
handshake remains bounded to two seconds, and backgrounding closes the socket.
The connection bypasses HTTP proxies and pooling, as the control connection does.
Version 0.3.7 fixes the decoder watchdog counting
network idle time as a stall when new work arrives. Brief reconnects retain the
last picture for up to roughly one second with gestures disabled; longer outages,
PC sharing pauses and backgrounding clear it. The live status retains a reconnect
counter and the last reason so intermittent failures can be diagnosed.
It retains nearby-PC discovery and connection requests approved on the PC,
with QR/link pairing as a backup.
It retains the removal of the 10 FPS JPEG fallback and the interrupted decoder
cleanup fix when backgrounding and returning.
It retains double-tap focus and fixed 720p H.264 quality,
fullscreen Fill/Fit and PC hardware encoding with software encoding as backup.
The status bar shows video resolution and **Received / Shown FPS**. Video failures
show a status and retry H.264; the Android client no longer downloads JPEGs.
Video remains compatible with Windows preview 0.3.6; audio requires 0.3.9.
Nearby discovery requires Windows preview 0.3.6 or newer. QR/link pairing
still works with earlier compatible PC previews.

## Build and install

### Optional tile audio

Version 0.3.11 allows bounded waiting for the decoder to drain a short network
burst, while retaining the six-packet queue and 250 ms packet-age limit. Partial
or zero speaker writes return to decoder-input processing; one output buffer
is retained with a 150 ms deadline. AudioTrack starts in streaming mode without
a hard-coded 8192-byte prefill requirement. Stuck input and speaker output have
separate status labels. The production pump is tested with isolated media API
fixtures for bursts, short/zero writes, stalls and cleanup; these do not replace
device-level codec and listening checks.

Version 0.3.10 prepares MediaCodec and AudioTrack before requesting the live
audio stream, so slower device initialization cannot fill its six-packet queue.
Preparation is cancellable and has a three-second deadline. Decoder stall
timing excludes idle network waits. Reconnect status includes a fixed, safe
failure label rather than hiding every failure behind “Reconnecting.”

Tap the canvas to reveal **Audio off**, then tap it to enable sound. The control
is also available outside fullscreen. Audio is off for each new connection;
turning it off closes the audio socket and releases the PC capture/encoder and
phone decoder/output. It is not just a speaker mute. The stream is AAC-LC,
48 kHz stereo, at a 128 kbps target (roughly 16 KB/s plus transport overhead).
Backgrounding and paused/unavailable video stop audio too. Returning resumes
audio only if it was still enabled. Audio focus loss switches it off; tap again
to enable after the other app finishes. Phone media volume controls loudness.

With desktop monitoring off, native video and owned WebView tile audio comes from Pluriview's process tree.
External window tiles must have their per-tile **Stream Audio** switch enabled.
Only those application trees are added, with duplicate/nested PIDs removed.
Windows captures an application's sound, not an individual window/tab; other
sounds from the same source application can also be heard. The PC's unrelated
applications and system notifications are not included. No microphone is used.

Process loopback requires [Windows build 20348+](https://learn.microsoft.com/en-us/samples/microsoft/windows-classic-samples/applicationloopbackaudio-sample/), including Windows 11; unsupported PCs
show an audio-unavailable status while video continues. Desktop **Stream Audio
Monitor** and phone audio can run together. With monitoring enabled, the phone
captures only Pluriview's application mix, including the monitor's rendered copies,
as application sharing does. It does not add separate captures of those source
applications. The desktop monitor keeps its selected output and existing behavior;
phone streaming does not change desktop audio routing or mute settings.
Changing the monitoring mode reconnects phone audio automatically. Existing
companion 0.3.9 installations support this updated desktop behavior.
The initial implementation has bounded latency buffering, not a shared audio/
video presentation clock; precise lip-sync across devices is not guaranteed.
Missing Windows media components or Android audio codecs also leave video usable.

### Building the APK

Version 0.3.1 fixes an immediate startup crash introduced by fullscreen setup:
the pairing screen is installed before system-bar configuration, and controller
access initializes the window decor before querying it.

The app uses Android platform APIs only, with no third-party Java dependencies.
Install JDK 17+ and Android SDK Platform 35 / Build Tools 35.0.0, then put the
JDK `bin` directory on PATH. From the repository root:

```powershell
.\scripts\build-android.ps1
```

Use `-Sdk`, `-Platform` or `-BuildTools` to select a different installed SDK.
The script compiles Java, runs camera-transform, Fit/Fill geometry, double-tap,
video-framing, playback pacing, measured FPS, persistent-control and interrupted
decoder lifecycle tests, plus partial-timeout video reads and cancellation, creates
DEX with D8, packages with AAPT2, aligns the APK and verifies its signature.
It produces `target/android-viewer/pluriview-viewer-debug.apk`. The local test
signing key and all outputs remain in ignored `target/`; this is not a Play
Store release build. Production distribution needs a separately managed signing key.

Install the APK on a connected Android device:

```powershell
adb install -r target/android-viewer/pluriview-viewer-debug.apk
```

Or copy the APK to the phone and install it there. Start sharing in the PC's
Phone Viewer dialog, select its name nearby on Android and tap **Request connection**.
Compare the six-digit number on both screens, then choose **Allow connection**
on the PC. Requests can be denied or cancelled and expire after one minute.
Busy PCs cannot accept another viewer. Discovery requires a LAN that allows
devices to communicate; use Refresh after switching networks.

QR/link pairing remains a backup: scan the QR or paste the link, tap Connect,
then close the PC dialog. Treat that link as a secret: it grants access without
further approval. Some camera apps do not open custom QR links; use copy/paste
in that case. The app keeps pairing information in memory only,
stops network activity when backgrounded, and reconnects when brought forward.
Backgrounding clears the visible frame and releases the decoder; returning
reconnects 720p video with fresh statistics. There is no lower-quality background
stream. Cleanup waits for a bounded period even if the outgoing stream worker
was interrupted. A new decoder cannot start until the previous one has stopped.

Viewing starts fullscreen with **Fill screen**, preserving proportions while
cropping the PC canvas edges to fill the phone. Tap the canvas to reveal
**Fit whole canvas** and **Exit fullscreen**. Controls hide after 3.5 seconds;
Back exits fullscreen, and an edge swipe temporarily reveals Android's system
bars. Cutout space is used for content while controls avoid the camera cutout.
Fit shows every edge and may leave bars because phone and PC aspect ratios
differ. The Fit/Fill preference is saved; pairing credentials are not.

Double-tap a tile to focus it on both screens; double-tap the focused tile to
restore the previous camera and selection. Empty canvas and viewport-pinned
tiles do not enter focus. Pinches, drags and long presses cancel double-tap
recognition. Single taps reveal controls after the double-tap interval.

Tap once to see **Received / Shown FPS** alongside the fullscreen controls.
Received counts complete H.264 pictures arriving from the PC; Shown counts
TextureView updates observed by Android. Both use a rolling second and drop
during a stall. They measure activity, not the configured target or physical
panel refresh. Low received FPS points to the capture/encode/network path;
received near 30 with low shown FPS points to decoding or phone presentation.

## Protocol and future extension

`src/remote_view.rs` owns the Windows host, frame producer and bounded command
queue. `CanvasState::apply_viewer_camera` is the sole camera mutation entry
point. `CanvasState::focus_viewer_at` adds temporary focus through canvas hit
testing. No source clicks, keyboard input, tile editing, file, URL or workspace
action is exposed.

The invitation is `pluriview://viewer?host=<private-ipv4>&port=<port>&token=<random-128-bit-token>`.
The host binds only the selected local interface and chooses an available port.
Authenticated viewing requests use `Authorization: Bearer <token>` and a
32-character hex `X-Pluriview-Client` session identifier. Tokens never appear in HTTP paths.
The first authenticated frame request claims a four-second renewable lease.
Other phones receive 409 until that lease expires. Restart sharing to revoke
the token immediately. The native client accepts only literal private/loopback
IPv4 addresses, does not follow redirects, and bounds response size and decoded
dimensions. The host rejects browser Origin headers and bounds request sizes,
socket timeouts, connection workers, image jobs and pending commands.

Windows advertises `_pluriview._tcp.local` on the selected private interface
only while sharing. It advertises a generated PC display name, address/port and
protocol version, never a token or canvas content. Android uses `NsdManager`,
bounds results and resolves one service at a time. Discovery and approval stop
when the pairing screen leaves the foreground. Four fixed host workers
accommodate video, control, optional audio and pairing.
Foreground discovery holds a Wi-Fi multicast lock for older Android support;
it releases the lock on failure, backgrounding or connection, as described in
[Android's NSD documentation](https://developer.android.com/reference/android/net/nsd/NsdManager#Wi-Fi-Multicast-Lock).

- `GET /v3/info`: unauthenticated, bounded name/protocol/busy information.
- `POST /v3/pair`: random 32-hex request ID and sanitized phone name, with the
  client ID header. Returns 202 with a six-digit comparison number while awaiting
  PC approval, 403 if denied, 409 if busy, or 429 during the prompt cooldown.
  Only approval produces a 200 with a fresh 128-bit token bound to that client.
  One pending request is retained for at most 60 seconds; prompts are at least
  ten seconds apart. Android cancels after 55 seconds.
- `POST /v3/pair/cancel`: cancels that client/request and revokes its grant if
  approval raced with cancellation. Stopping sharing revokes all grants.

The number helps identify the intended request; it does not authenticate the
network or prevent eavesdropping. This preview still requires a trusted LAN.
Encryption is a separate future change. Approval UI is excluded from the stream
by the desktop-dialog pause guard.

- `GET /v1/frame`: legacy JPEG support for older clients (unused by Android 0.3.5),
  frame ID and desktop canvas dimensions in egui points
  through `X-Frame-Id`, `X-Canvas-Width`, `X-Canvas-Height`. Send `X-Last-Frame`
  to avoid downloading the same frame. A 204 means no new frame; the
  `X-Viewer-State: paused` header means the client should hide stale content.
- `GET /v2/audio`: optional persistent response, `application/x-pluriview-aac`.
  Requires the existing bearer/client authentication, a live viewer lease and
  active unpaused video. It cannot claim or renew that lease. One audio stream
  is allowed. Status 423 means paused;
  503 means capture/codec/source availability prevents audio. A four-byte `PVA1`
  magic plus two-byte AAC AudioSpecificConfig `11 90` announces fixed AAC-LC
  48 kHz stereo. Each record contains a big-endian u32 length (1–8192) followed
  by one raw AAC access unit representing 1024 stereo frames. No HTTP chunking.
  Sources are supplied exclusively by the desktop; no PID or audio-routing
  controls are accepted from the phone. Changed sources, pause, video epoch,
  expired lease/UI source snapshot, client EOF and stale output close capture.
  The phone bounds receive/decode/output queues, cancels the socket on off or
  background, and keeps a single decoder slot across reconnects. Audio errors
  retry independently of video; older PCs report audio as unsupported.
  The codec uses the platform [AAC encoder](https://learn.microsoft.com/en-us/windows/win32/medfound/aac-encoder), without a bundled codec dependency.
- `GET /v2/video`: one persistent HTTP response with content type
  `application/x-pluriview-h264`. Each record starts with a 32-byte header in
  network byte order: payload length (`u32`), coded width/height (`u32` each),
  desktop canvas width/height (`f32` each), frame sequence (`u64`), kind (`u32`).
  Kind 1 carries one Annex-B H.264 access unit, at most 4 MiB and 1280×720.
  Kind 2 is a zero-payload pause/reset: hide the surface and discard the decoder.
  Kind 3 is a zero-payload unavailable-codec notice: show a video error and retry
  H.264 after two seconds. It never switches the current Android app to JPEG.
  A new stream and each resumed/resized encoder begin with parameter sets and
  an IDR. Sequence gaps terminate the stream and require reconnecting. Only one
  video response may be active, including requests from the same client ID.
- `POST /v1/camera`: JSON with `pan: [x,y]` in fractions of the displayed
  canvas, `scale` as a multiplier, `anchor: [x,y]` normalized to that canvas,
  and `viewport: [width,height]` matching the frame's desktop dimensions.
  Zoom around the anchor, then pan. Unknown fields/invalid values are rejected.
  Stale, paused, mismatched-viewport or other-session commands are discarded.
  PC pointer gestures take priority. A 200 accepts into a bounded queue; it
  does not guarantee the camera moved if desktop state changed subsequently.
- `GET /v2/control`: authenticated persistent duplex response with content type
  `application/x-pluriview-control`. Client messages have a two-byte big-endian
  length (1–1024) followed by UTF-8 JSON: `{"type":"camera","value":{...}}`,
  `{"type":"focus","value":{...}}` or `{"type":"ping"}`. Values use the
  corresponding v1 schemas. Each response is a two-byte big-endian status:
  200 accepted, 400 invalid, 409 expired/wrong peer, 423 paused, 429 queue full.
  Only one control connection may be active. Controls/pings never claim or
  renew a viewer lease and cannot bypass the v1 navigation guards. Idle reads
  check shutdown/lease expiry; partial records and writes have bounded timeouts.
  Android uses v1 POST commands only if the PC returns 404 for this endpoint.
- `POST /v2/feedback`: compatibility endpoint for older Android builds. JSON
  `frame` sequence and `decode_ms` (finite, 0–2000) are validated, then ignored.
  Only the active paired viewer may send reports; unknown fields are rejected.
  Reports cannot change quality, reset the encoder, acquire/renew a lease or
  enqueue camera commands. Android 0.3.2 no longer sends these requests.
- `POST /v1/focus`: JSON `at: [x,y]` normalized to the full displayed PC frame
  and `viewport: [width,height]`. The host toggles focus on the topmost tile at
  that point, using the same eligibility as desktop focus (no pinned tiles).
  It shares camera authentication, the bounded queue, 500 ms expiry, viewport
  checks and local-input priority. No source pointer event is generated.

The client coalesces gestures as affine transforms and never retries uncertain
camera requests, avoiding double movement. Video and commands use separate
workers; camera sends are coalesced at an 8 ms interval plus acknowledgement time,
reusing one TCP connection rather than reconnecting for each command.
Focus uses the same ordered command worker and is never retried after an
uncertain response, preventing an accidental second toggle. The PC interpolates
network camera steps with a 25 ms smoothing constant and catches up exactly
within 100 ms after the last command. Gesture composition uses the complete
target camera, so it preserves distance and zoom limits. Local camera changes,
focus, sharing pauses and viewport changes cancel the pending interpolation.
Portrait/landscape, Fit/Fill and fullscreen change only the display mapping;
they do not alter the PC viewport dimensions. Fill gestures use the full
rendered image rectangle, including its cropped portion.

The Windows frame producer currently reads the composed application framebuffer,
crops to the canvas, then scales and encodes on a worker. It includes GPU
video callbacks and the canvas's visible overlays, but excludes the picker and
title bar. Screen readback adds overhead while connected. Only one screenshot/job
is submitted at a time; absolute capture deadlines avoid accumulating wakeup
delay, and late deadlines skip missed slots rather than sending catch-up bursts.
Asynchronous encoder input is bounded to three
pictures, with no screenshot requests when unwatched. The shared
camera uses existing PC capture visibility and resolution decisions. H.264 uses
Windows Media Foundation encoders in baseline, low-latency mode, with no
B-frames and reusable RGB/NV12/codec buffers. Hardware MFTs are tried first;
asynchronous input/output events and driver shutdown are handled on the worker.
A hardware failure restarts with software encoding, then reports video unavailable
if encoding still fails. Android chooses its system decoder (normally hardware).
Android 11+ also receives the low-latency decoder hint. No FFmpeg or
additional installed encoder is needed. Media Foundation is loaded lazily from
Windows' system directory; missing runtime/codecs report video unavailable without
preventing the desktop app from starting. Android passes SPS/PPS explicitly to
MediaCodec before the first picture. The host keeps at most six compressed
frames; missing references or a frame older than 250 ms close the stream rather
than sending an old backlog. TCP/socket buffers and Wi-Fi can still add latency.
Pauses, reconnects and size changes rebuild codec state. Android receives packets
independently of the decoder worker, retaining at most six queued packets, eight
submitted inputs and three decoded outputs. Playback uses a 50 ms cushion with
a 30 FPS clock; late decoded outputs may be skipped, but compressed reference
pictures are never dropped within a stream. Excessive backlog reconnects from
a keyframe, and a long stall resets the playback clock instead of retaining
unbounded delay. Brief input-buffer pressure retries rather than declaring the
codec unavailable. Runtime decode failures reconnect H.264; unavailable codecs
show a status and retry after two seconds. A driver that cannot stop shows a
waiting status until it releases its resources, without spawning more decoders.

Video quality is fixed at the 1280×720 / 30 FPS / 4 Mbps profile. These are
caps/targets, not guaranteed displayed rates. Slow delivery/decoding never
lowers resolution or bitrate; stale streams still reconnect with a fresh
keyframe at the same quality. Canvas proportions are preserved and small PC
canvases are not upscaled. Update Windows to enforce the fixed profile; the
Android update removes the old feedback worker and Auto quality label.

This pass uses a persistent LAN HTTP/TCP stream, not WebRTC. A future WebRTC
transport can reuse the camera protocol. Independent viewing additionally needs a separate render target,
viewer-specific camera and combined source-visibility/resolution accounting.
Those are deliberately not implemented by duplicating the desktop's tile models.

## Device smoke checks

- Find the sharing PC nearby; request, compare numbers and approve. Verify no
  video/control before approval. Deny, cancel, expire, refresh, and try another
  phone while viewing. Stop sharing and check discovery disappears.
- Try QR/link backup and confirm restarting sharing invalidates old approvals.
- Pair; view mixed image/GIF, browser, normal-window and libmpv tiles.
- Confirm the status shows a resolution and measured Received/Shown FPS. A PC
  without video support must show an update message, never a JPEG preview.
- Leave both screens untouched for several minutes. If playback reconnects,
  record the retained Reconnects count and reason. A short recovery should retain
  the picture with gestures disabled; a longer outage must clear it. Opening a
  PC dialog or backgrounding the phone must still hide the picture immediately.
- Interrupt delivery briefly and restore it: partial records must resume on
  the same socket without corrupted framing. Only a prolonged stall, closed
  connection or invalid stream should require a fresh video session.
- Switch apps and return repeatedly, including rapid switches and screen locking;
  verify 720p video resumes and never becomes permanently locked to 10 FPS.
- Drag over a tile and pinch off-center: PC camera changes, tile geometry does not.
- Double-tap overlapping tiles: focus the top visible tile without activating
  its content. Repeat to restore; confirm pinches and long presses do not focus.
- Interrupt a remote pinch with PC navigation or a sharing pause; old movement
  must stop. Check zoom limits and Fit/Fill gesture mapping.
- Navigate on the PC and verify the phone follows; check pinned and cropped tiles.
- Rotate and enter/exit fullscreen; toggle Fit/Fill and pinch off-center in
  both modes. Fit margins must not pan, but taps should reveal controls.
- Check cutouts and system-bar swipes; confirm controls hide and remain reachable.
- Degrade and restore Wi-Fi; check fixed quality and recovery without stale
  pictures. Check Hardware/Software encoding in the PC Phone Viewer dialog.
- Open settings/pairing, enter browser interaction, minimize, then restore:
  stale content hides and fresh viewing resumes.
- Background/resume the phone; disable Wi-Fi and reconnect; stop/restart PC
  sharing; try a second phone and confirm the single-viewer lease.
- Confirm Click Through and a locally held drag suppress remote navigation.

The desktop Rust tests and Java transform tests run without a phone. Android
touch, lifecycle and Wi-Fi behavior still require a physical device or emulator.
