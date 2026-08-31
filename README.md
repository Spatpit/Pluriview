<p align="center">
  <h1 align="center">Pluriview</h1>
  <p align="center">
    Live window previews on an infinite canvas for Windows
  </p>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/platform-Windows%2010%2F11-blue" alt="Platform">
  <img src="https://img.shields.io/badge/rust-1.88%2B-orange" alt="Rust">
  <img src="https://img.shields.io/github/license/Spatpit/Pluriview" alt="License">
  <img src="https://img.shields.io/github/v/release/Spatpit/Pluriview?include_prereleases" alt="Release">
</p>

<p align="center">
  <img src="assets/pluriview-preview.gif" alt="Pluriview arranging live windows, browser pages, and image tiles on the infinite canvas">
</p>

---

## Features

| Feature | Description |
|---------|-------------|
| **Live Capture** | Real-time window previews using Windows Graphics Capture |
| **Spout2 Capture** | Receive senders from apps such as VTube Studio as live canvas tiles |
| **Browser Tiles** | Live web pages (YouTube, Twitch, anything) on the canvas with their own audio |
| **Video Tiles** | Drop local videos onto the canvas; play, seek, volume, tracks, loop, reload |
| **Folder Playlists** | Drop a folder to get a player plus a playlist tile (next/prev, shuffle, repeat) |
| **Image & GIF Tiles** | Static images or animated GIFs loaded directly from their original local paths |
| **Live Streams** | Optional Streamlink helper for Twitch and other stream URLs |
| **Ad & Tracker Blocking** | uBlock Origin Lite is built in for browser tiles and on by default |
| **Infinite Canvas** | Pan and zoom freely to organize your workspace |
| **Viewport Pinning** | Pin any tile so it stays fixed on screen while the canvas pans or zooms |
| **Click Pass-Through** | Disable left click on an overlay tile to interact with tiles underneath it |
| **Live Wallpaper** | Image, GIF, or looping muted video behind the canvas; it stays screen-sized |
| **Tile Freeze** | Pause live work on selected tiles and keep the last frame until you resume |
| **Stream Audio Monitor** | Copy tile audio into Pluriview so Discord/OBS window shares pick it up |
| **Crop Regions** | Focus on part of a window, browser page, image, GIF, or video with Alt+drag |
| **Adjustable FPS** | 15, 30, or 60 FPS per preview |
| **Auto-Save** | Changed layouts save once per minute with crash-safe previous-save backups; unchanged layouts do not write to disk |
| **Named Workspaces** | Create, duplicate, rename, and switch between reusable canvas setups |
| **System Tray** | Minimize to tray for background operation |
| **Quick Focus** | Double-click a preview to bring its source window to the front |
| **Canvas-Only Mode** | Press `H` to hide chrome; right-click menus still work |
| **Tile Focus** | Fit a tile to the canvas, then restore with `Esc` |
| **Auto-hiding Title Bar** | Optional: hide the title bar until the pointer is at the top of the window |

### Video tiles and playlists

Keep `libmpv-2.dll` inside the `lib` folder next to `pluriview.exe`. Then:

- Drop a video file onto the canvas, or use **File → Add Video...**
- Drop a folder of videos to create a linked player and playlist
- Use play/pause, seek, mute, speed, loop, audio/subtitle tracks, and reload; hover the speaker icon for a vertical volume slider
- Crop the visible video region with Alt+drag on tile edges or corners
- Hover the seek bar to preview a frame without moving the playing video
- Live streams do not get timeline thumbnails

You do **not** need a separate mpv install for playback. Stream URLs need
[Streamlink](https://streamlink.github.io/) (Settings → Streamlink).

### Image and GIF tiles

Choose **File → Add Image...** or right-click the canvas → **Add Image...**.
Pluriview supports PNG, JPEG, GIF, WebP, and BMP. Animated GIFs keep their
original timing. Tiles use the image's original path and do not create a copy
inside `pluriview_data`. If the original is moved or removed, the tile remains
on the canvas and explains that its saved path can no longer be found. You can
also drag files from Explorer onto the canvas.

Development builds keep `pluriview_data` at the repo root. Release builds keep
it beside the executable.

### Live wallpaper

**View → Set Wallpaper...** or right-click the canvas. Images, GIFs, and local
videos fill the window and do not pan or zoom with the canvas. Video wallpaper
needs `libmpv-2.dll` and loops muted. Image, GIF, and video wallpapers keep
their original paths instead of creating managed copies. A missing image
wallpaper shows a path-change message. The wallpaper is saved per workspace.
While a tile is in focus mode, video wallpaper pauses.

### Browser tiles

Right-click the canvas → **Add Browser...** and paste a URL. The page is a
normal tile (move, resize, overlap) while audio keeps playing. Double-click
(or `Ctrl+B`) to use the real page, then `Esc` or click outside to return to
the canvas. Hover for back/forward/reload/mute. Logins live in a Pluriview
WebView2 profile, not your main browser.

Browser tiles use **uBlock Origin Lite**, on by default. Toggle it under
**View → Block Ads & Trackers (uBOL)**. WebView2 has no extension toolbar, so
uBOL's popup is not shown.

Turn off **Lock Aspect Ratio** from a browser tile's right-click menu to resize
its WebView into portrait, square, ultrawide, or another custom viewport shape.

Pages with transparent backgrounds stay transparent in browser tiles, which
makes overlay widgets such as heart-rate monitors blend into the canvas.
Browser tiles can also be cropped with Alt+drag while in canvas mode; entering
interaction mode keeps the visible page region and pointer coordinates aligned,
and the crop is restored with the workspace.

**Streaming with audio (Discord/OBS):** browser-tile sound normally belongs to
WebView2, so sharing the Pluriview window has no tile audio. Enable **View →
Stream Audio Monitor** and pick an output you do not listen to (a virtual
cable such as VB-Cable, or an unused output). Pluriview plays a copy from its
own process so window shares pick it up. Off by default.

## Requirements

- **OS:** Windows 10 (version 1903+) or Windows 11
- **GPU:** DirectX 11 compatible graphics card
- **Renderer:** `lib\libEGL.dll` and `lib\libGLESv2.dll` beside `pluriview.exe` (included in every download)
- **Browser tiles:** [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) (already on Windows 11 and current Windows 10)
- **Ad blocking:** WebView2 Runtime 122 or newer
- **Video tiles, playlists, video wallpaper:** `lib\libmpv-2.dll` beside `pluriview.exe` (included in the Full download)
- **Stream URLs:** [Streamlink](https://streamlink.github.io/), configured in Settings if it is not already on PATH
- **Building from source:** Rust 1.88 or newer

Window capture, browsers, images, GIF wallpaper, and workspaces work without
libmpv or Streamlink.

## Installation

### Download

Get the latest release from [Releases](https://github.com/Spatpit/Pluriview/releases).

| Zip | Contains | Use when |
|-----|----------|----------|
| **Full** (recommended) | `pluriview.exe` + a `lib` folder containing ANGLE, `libmpv-2.dll`, and license/notices | You want video tiles, playlists, or video wallpaper |
| **Lite** | `pluriview.exe` + a `lib` folder containing ANGLE and license/notices | You only need windows, browsers, and images |

Keep the included `lib` folder next to `pluriview.exe`. Full installs also
include `libmpv-2.dll` inside that folder; you can add that DLL to a Lite
install later. Do not ship or run `pluriview.pdb`.

Streamlink is a separate optional install. Point Pluriview at it in Settings
if Windows does not already find `streamlink.exe`.

### Build from Source

```powershell
git clone https://github.com/Spatpit/Pluriview.git
cd Pluriview
.\scripts\prepare-angle.ps1
.\scripts\prepare-libmpv.ps1
.\scripts\build-release.ps1
```

`dist` will contain the privacy-safe executable, a `lib` folder with the
runtime DLLs and text notices, versioned Full and Lite zip archives, and
`SHA256SUMS.txt`. Keep the `lib` folder beside the executable. Do not
distribute `pluriview.pdb`; debug symbols can contain local paths.

## Usage

1. **Launch** `pluriview.exe`
2. **Add windows** from the Window Picker (left side). Live Spout2 senders such as VTube Studio appear in the same list; add them to capture the shared texture.
3. **Add browsers** by right-clicking the canvas → Add Browser...
4. **Add images or GIFs** with File → Add Image... or the canvas context menu
5. **Add videos** by dropping a file or folder, or File → Add Video...
6. **Add a stream** from the canvas menu (needs Streamlink)
7. **Set a wallpaper** with View → Set Wallpaper...
8. **Arrange** by dragging tiles; **resize** from corners or edges
9. **Crop** window, browser, image, GIF, or video tiles with Alt+drag on edges or corners
10. **Pin** any tile with right-click → **Pin to Viewport** to keep it fixed while navigating the canvas
11. **Focus** a tile with right-click → **Focus on This Tile**; `Esc` restores the canvas

The **Workspace** menu holds separate setups. Existing installs are migrated
into a workspace named **Default** the first time workspaces run. Pluriview
checks once per minute and saves only when persisted workspace state changed.
Use **Workspace → Restore Previous Save as New Workspace** to inspect the prior
valid save without overwriting the current workspace. If the workspace catalog
is interrupted or damaged, valid unlisted workspace files are returned to the
catalog automatically.

## Keyboard Shortcuts

Keyboard shortcuts can be changed under **View → Settings → Keyboard
shortcuts**. A binding can be one key or any two keys held together. Mouse
controls remain fixed. **Restore Default Keys** returns the keyboard bindings
to those shown below.

| Action | Shortcut |
|--------|----------|
| Pan canvas | `Middle Mouse` or `Alt + Drag` |
| Zoom | `Scroll Wheel` |
| Toggle Window Picker | `W` |
| Toggle grid | `G` |
| Toggle canvas-only mode | `H` |
| Select all | `Ctrl + A` |
| Multi-select | `Ctrl + Click` |
| Box-select tiles | `Left-drag empty canvas` |
| Add with box selection | `Ctrl + Left-drag empty canvas` |
| Freeze or resume selection | `Right-click selected tile or canvas` |
| Delete selected | `Delete` |
| Crop tile | `Alt + Drag edges or corners` |
| Focus current tile | `Numpad 2` or `Double-click preview` |
| Exit tile focus | `Esc` |
| Interact with browser tile | `Ctrl + B`, `Numpad 1`, or `Double-click` |
| Exit browser interaction | `Esc` or click outside |
| Show shortcuts | `F1` |

## Project Structure

```
Pluriview/
├── src/
│   ├── app.rs              # Main application state and UI
│   ├── main.rs             # Entry point
│   ├── audio.rs            # Stream audio monitor
│   ├── browser.rs          # WebView2 browser tiles
│   ├── libmpv.rs           # In-process video playback
│   ├── media.rs            # Static image and animated GIF decoding
│   ├── playlist.rs         # Folder playlist tiles
│   ├── video.rs            # Video sources and playlist thumbnails
│   ├── canvas/             # Infinite canvas, wallpaper, selection
│   ├── capture/            # Window capture and downscale
│   ├── overlay/            # Region selector overlay (crop)
│   ├── persistence/        # Layout, workspaces, settings
│   ├── preview/            # Preview window management
│   ├── tray/               # System tray integration
│   ├── window_picker/      # Window enumeration and picker UI
│   └── spout.rs            # Spout2 sender detection
├── assets/
│   ├── icon.ico                  # Application icon
│   ├── pluriview-preview.gif     # README preview
│   └── third_party/
│       ├── angle/                 # Pinned standalone ANGLE runtime and provenance
│       └── ubol/                  # Pinned official uBlock Origin Lite package
├── scripts/
│   ├── build-release.ps1   # Privacy-safe Windows release build
│   ├── prepare-angle.ps1   # Verify and prepare the pinned standalone ANGLE runtime
│   └── prepare-libmpv.ps1  # Download the pinned libmpv runtime
├── Cargo.toml
├── build.rs
├── LICENSE                 # MIT License
├── THIRD_PARTY_NOTICES.md
└── README.md
```

`vendor/` and `dist/` are local build outputs. They are not committed.

## License

This project is licensed under the MIT License — see [LICENSE](LICENSE).
Bundled third-party components keep their own licenses; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). Full releases include
`libmpv-2.dll` (mpv/FFmpeg, GPLv2+); every release includes standalone ANGLE
and the applicable permissive runtime notices.

## Acknowledgments

- Built with [egui](https://github.com/emilk/egui)
- Window capture via [windows-rs](https://github.com/microsoft/windows-rs)
- Spout2 sender capture via the public [Spout](https://spout.zeal.co/) shared-memory registry and DirectX 11
- Browser tiles via [wry](https://github.com/tauri-apps/wry) (WebView2)
- OpenGL ES translation via [ANGLE](https://github.com/google/angle) (Direct3D 11 on Windows)
- Video playback via [libmpv](https://github.com/mpv-player/mpv) (shinchiro Windows builds)
- Ad and tracker blocking via [uBlock Origin Lite](https://github.com/gorhill/uBlock) (GPL-3.0)

---

<p align="center">
  Made with Rust
</p>
