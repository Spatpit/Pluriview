<p align="center">
  <h1 align="center">Pluriview</h1>
  <p align="center">
    Live window previews on an infinite canvas for Windows
    <br />
    <em>Like PureRef, but for live windows</em>
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
| **Live Capture** | Real-time window previews using Windows Graphics Capture API |
| **Browser Tiles** | Put live web pages (YouTube, Twitch, anything) on the canvas with their own audio — no separate browser needed |
| **Image & GIF Tiles** | Add static images or animated GIFs; portable copies are managed beside the executable |
| **Ad & Tracker Blocking** | uBlock Origin Lite is integrated into the shared browser-tile profile and enabled by default |
| **Infinite Canvas** | Pan and zoom freely to organize your workspace |
| **Crop Regions** | Focus on specific parts of windows with Alt+drag |
| **Adjustable FPS** | Choose 5, 15, 30, or 60 FPS per preview |
| **Auto-Save** | Layouts persist automatically between sessions, including browser URLs |
| **System Tray** | Minimize to tray for background operation |
| **Quick Focus** | Double-click any preview to bring its window to front |
| **Canvas-Only Mode** | Press `H` to hide all visual chrome while keeping right-click menus available |
| **Tile Focus** | Fit any captured window or browser tile to the canvas, then restore with `Esc` |

### Image and GIF tiles

Choose **File → Add Image...** or right-click the canvas → **Add Image...**.
Pluriview supports PNG, JPEG, GIF, WebP, and BMP files. Animated GIFs retain
their original frame timing. Imported files are copied into
`pluriview_data/media` and layouts store only the managed filename, so the
executable and its data folder can be moved together without breaking tiles.
You can also drag one or several supported image files from File Explorer (or
another app that provides local files) and drop them directly onto the canvas.

### Browser tiles

Right-click the canvas → **Add Browser...** and paste a URL. The page renders as a
normal tile (move, resize, overlap, z-order) while audio keeps playing — ideal for
music, streams, or reference pages. Double-click (or `Ctrl+B`) to interact with the
real page — log in, scroll, pick a video — and press `Esc` or click outside to go
back to canvas mode. Hover a browser tile for back/forward/reload/mute controls;
logins and cookies persist in a dedicated WebView2 profile, separate from your main
browser.

Browser tiles use the official **uBlock Origin Lite** extension to block ads,
trackers, and known malicious URLs. It is enabled by default for the shared
Pluriview browser profile and can be toggled globally under **View → Block Ads
& Trackers (uBOL)**. The extension loads locally from the executable; no
separate browser installation is needed. WebView2 does not expose a normal
extension toolbar, so uBOL's per-site popup and dashboard are not shown.

**Streaming with audio (Discord/OBS):** browser-tile sound normally belongs to
WebView2's own processes, so sharing the Pluriview window carries no tile audio.
To fix that, enable **View → Stream Audio Monitor** and pick an output device you
don't listen to (a virtual cable like VB-Cable, or an unconnected output).
Pluriview then plays a copy of the tile audio from its own process — window
shares pick it up, you don't hear it twice, and nothing outside Pluriview is
modified. Off by default.

## Requirements

- **OS:** Windows 10 (version 1903+) or Windows 11
- **GPU:** DirectX 11 compatible graphics card
- **Browser tiles:** [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) (preinstalled on Windows 11 and current Windows 10; everything else works without it)
- **Integrated ad blocking:** WebView2 Runtime 122 or newer (update the runtime if the View menu reports uBOL as unavailable)
- **Building from source:** Rust 1.88 or newer

## Installation

### Download Release
Download the latest `pluriview.exe` from [Releases](https://github.com/Spatpit/Pluriview/releases).

### Build from Source
```powershell
git clone https://github.com/Spatpit/Pluriview.git
cd Pluriview
.\scripts\build-release.ps1
```
The privacy-safe executable will be at `target/release/pluriview.exe`. Publish only
that file. Do not distribute `pluriview.pdb`; Windows debug symbols can contain
local source paths.

## Usage

1. **Launch** `pluriview.exe`
2. **Add windows** from the Window Picker panel (left side)
3. **Add browsers** by right-clicking the canvas → Add Browser...
4. **Add images or GIFs** with File → Add Image... or the canvas context menu
5. **Arrange** by dragging previews on the canvas
6. **Resize** by dragging corners or edges
7. **Crop** window previews by holding Alt and dragging corners
8. **Focus** a tile with right-click → **Focus on This Tile**; press `Esc` to restore the canvas
9. **Right-click** for other context menu options

## Keyboard Shortcuts

| Action | Shortcut |
|--------|----------|
| Pan canvas | `Middle Mouse` or `Alt + Drag` |
| Zoom | `Scroll Wheel` |
| Toggle grid | `G` |
| Toggle canvas-only mode | `H` |
| Select all | `Ctrl + A` |
| Multi-select | `Ctrl + Click` |
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
│   ├── browser.rs          # WebView2 browser tiles
│   ├── media.rs            # Static image and animated GIF decoding
│   ├── canvas/             # Infinite canvas (pan, zoom, selection)
│   ├── capture/            # Window capture coordinator
│   ├── overlay/            # Region selector overlay (crop)
│   ├── persistence/        # Layout save/load
│   ├── preview/            # Preview window management
│   ├── tray/               # System tray integration
│   └── window_picker/      # Window enumeration and picker UI
├── assets/
│   ├── icon.ico                  # Application icon
│   ├── pluriview-preview.gif     # README preview
│   └── third_party/ubol/         # Pinned official uBlock Origin Lite package
├── scripts/
│   └── build-release.ps1   # Privacy-safe Windows release build
├── Cargo.toml              # Dependencies and metadata
├── build.rs                # Windows resource compilation
├── LICENSE                 # MIT License
└── README.md
```

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
Bundled third-party components retain their own licenses; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Acknowledgments

- Built with [egui](https://github.com/emilk/egui) - Immediate mode GUI library for Rust
- Window capture via [windows-rs](https://github.com/microsoft/windows-rs)
- Browser tiles via [wry](https://github.com/tauri-apps/wry) (WebView2)
- Ad and tracker blocking via [uBlock Origin Lite](https://github.com/gorhill/uBlock) (GPL-3.0)

---

<p align="center">
  Made with Rust
</p>
