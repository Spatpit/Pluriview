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
| **Browser Tiles** | Live web pages (YouTube, Twitch, anything) on the canvas with their own audio |
| **Video Tiles** | Drop local videos onto the canvas; play, seek, volume, tracks, loop, reload |
| **Folder Playlists** | Drop a folder to get a player plus a playlist tile (next/prev, shuffle, repeat) |
| **Image & GIF Tiles** | Static images or animated GIFs; portable copies live beside the executable |
| **Live Streams** | Optional Streamlink helper for Twitch and other stream URLs |
| **Ad & Tracker Blocking** | uBlock Origin Lite is built in for browser tiles and on by default |
| **Infinite Canvas** | Pan and zoom freely to organize your workspace |
| **Live Wallpaper** | Image, GIF, or looping muted video behind the canvas; it stays screen-sized |
| **Tile Freeze** | Pause live work on selected tiles and keep the last frame until you resume |
| **Stream Audio Monitor** | Copy tile audio into Pluriview so Discord/OBS window shares pick it up |
| **Crop Regions** | Focus on a part of a captured window with Alt+drag |
| **Adjustable FPS** | 15, 30, or 60 FPS per preview |
| **Auto-Save** | Layouts persist automatically, including browser URLs and video tiles |
| **Named Workspaces** | Create, duplicate, rename, and switch between reusable canvas setups |
| **System Tray** | Minimize to tray for background operation |
| **Quick Focus** | Double-click a preview to bring its source window to the front |
| **Canvas-Only Mode** | Press `H` to hide chrome; right-click menus still work |
| **Tile Focus** | Fit a tile to the canvas, then restore with `Esc` |
| **Auto-hiding Title Bar** | Optional: hide the title bar until the pointer is at the top of the window |

### Video tiles and playlists

Keep `libmpv-2.dll` next to `pluriview.exe`. Then:

- Drop a video file onto the canvas, or use **File → Add Video...**
- Drop a folder of videos to create a linked player and playlist
- Use play/pause, seek, volume, mute, speed, loop, audio/subtitle tracks, and reload
- Hover the seek bar to preview a frame without moving the playing video
- Live streams do not get timeline thumbnails

You do **not** need a separate mpv install for playback. Stream URLs need
[Streamlink](https://streamlink.github.io/) (Settings → Streamlink).

### Image and GIF tiles

Choose **File → Add Image...** or right-click the canvas → **Add Image...**.
Pluriview supports PNG, JPEG, GIF, WebP, and BMP. Animated GIFs keep their
original timing. Imported files are copied into `pluriview_data/media` so the
exe and its data folder can move together. You can also drag files from
Explorer onto the canvas.

Development builds keep `pluriview_data` at the repo root. Release builds keep
it beside the executable.

### Live wallpaper

**View → Set Wallpaper...** or right-click the canvas. Images, GIFs, and local
videos fill the window and do not pan or zoom with the canvas. Video wallpaper
needs `libmpv-2.dll` and loops muted. Image wallpapers are copied into
`pluriview_data/media`; video wallpapers keep their original path. The
wallpaper is saved per workspace. While a tile is in focus mode, video
wallpaper pauses.

### Browser tiles

Right-click the canvas → **Add Browser...** and paste a URL. The page is a
normal tile (move, resize, overlap) while audio keeps playing. Double-click
(or `Ctrl+B`) to use the real page, then `Esc` or click outside to return to
the canvas. Hover for back/forward/reload/mute. Logins live in a Pluriview
WebView2 profile, not your main browser.

Browser tiles use **uBlock Origin Lite**, on by default. Toggle it under
**View → Block Ads & Trackers (uBOL)**. WebView2 has no extension toolbar, so
uBOL's popup is not shown.

**Streaming with audio (Discord/OBS):** browser-tile sound normally belongs to
WebView2, so sharing the Pluriview window has no tile audio. Enable **View →
Stream Audio Monitor** and pick an output you do not listen to (a virtual
cable such as VB-Cable, or an unused output). Pluriview plays a copy from its
own process so window shares pick it up. Off by default.

## Requirements

- **OS:** Windows 10 (version 1903+) or Windows 11
- **GPU:** DirectX 11 compatible graphics card
- **Browser tiles:** [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) (already on Windows 11 and current Windows 10)
- **Ad blocking:** WebView2 Runtime 122 or newer
- **Video tiles, playlists, video wallpaper:** `libmpv-2.dll` next to `pluriview.exe` (included in the Full download)
- **Stream URLs:** [Streamlink](https://streamlink.github.io/), configured in Settings if it is not already on PATH
- **Building from source:** Rust 1.88 or newer

Window capture, browsers, images, GIF wallpaper, and workspaces work without
libmpv or Streamlink.

## Installation

### Download

Get the latest release from [Releases](https://github.com/Spatpit/Pluriview/releases).

| Zip | Contains | Use when |
|-----|----------|----------|
| **Full** (recommended) | `pluriview.exe` + `libmpv-2.dll` + license/notices | You want video tiles, playlists, or video wallpaper |
| **Lite** | `pluriview.exe` only | You only need windows, browsers, and images |

Keep `libmpv-2.dll` in the **same folder** as `pluriview.exe`. You can add the
DLL to a Lite install later. Do not ship or run `pluriview.pdb`.

Streamlink is a separate optional install. Point Pluriview at it in Settings
if Windows does not already find `streamlink.exe`.

### Build from Source

```powershell
git clone https://github.com/Spatpit/Pluriview.git
cd Pluriview
.\scripts\prepare-libmpv.ps1
.\scripts\build-release.ps1
```

`dist` will contain the privacy-safe executable, `libmpv-2.dll`, versioned Full
and Lite zip archives, and `SHA256SUMS.txt`. Keep the DLL beside the executable
when using the Full build. Do not distribute `pluriview.pdb`; debug symbols can
contain local paths.

## Usage

1. **Launch** `pluriview.exe`
2. **Add windows** from the Window Picker (left side)
3. **Add browsers** by right-clicking the canvas → Add Browser...
4. **Add images or GIFs** with File → Add Image... or the canvas context menu
5. **Add videos** by dropping a file or folder, or File → Add Video...
6. **Add a stream** from the canvas menu (needs Streamlink)
7. **Set a wallpaper** with View → Set Wallpaper...
8. **Arrange** by dragging tiles; **resize** from corners or edges
9. **Crop** window previews with Alt+drag on corners
10. **Focus** a tile with right-click → **Focus on This Tile**; `Esc` restores the canvas

The **Workspace** menu holds separate setups. Existing installs are migrated
into a workspace named **Default** the first time workspaces run.

## Keyboard Shortcuts

| Action | Shortcut |
|--------|----------|
| Pan canvas | `Middle Mouse` or `Alt + Drag` |
| Zoom | `Scroll Wheel` |
| Toggle grid | `G` |
| Toggle canvas-only mode | `H` |
| Select all | `Ctrl + A` |
| Multi-select | `Ctrl + Click` |
| Box-select tiles | `Left-drag empty canvas` |
| Add with box selection | `Ctrl + Left-drag empty canvas` |
| Freeze or resume selection | `Right-click selected tile or canvas` |
| Delete selected | `Delete` |
| Crop preview | `Alt + Drag corners` |
| Focus window | `Double-click preview` |
| Exit tile focus | `Esc` |
| Interact with browser tile | `Double-click` or `Ctrl + B` |
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
│   └── window_picker/      # Window enumeration and picker UI
├── assets/
│   ├── icon.ico                  # Application icon
│   ├── pluriview-preview.gif     # README preview
│   └── third_party/ubol/         # Pinned official uBlock Origin Lite package
├── scripts/
│   ├── build-release.ps1   # Privacy-safe Windows release build
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
`libmpv-2.dll` (mpv/FFmpeg, GPLv2+).

## Acknowledgments

- Built with [egui](https://github.com/emilk/egui)
- Window capture via [windows-rs](https://github.com/microsoft/windows-rs)
- Browser tiles via [wry](https://github.com/tauri-apps/wry) (WebView2)
- Video playback via [libmpv](https://github.com/mpv-player/mpv) (shinchiro Windows builds)
- Ad and tracker blocking via [uBlock Origin Lite](https://github.com/gorhill/uBlock) (GPL-3.0)

---

<p align="center">
  Made with Rust
</p>
