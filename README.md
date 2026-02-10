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
  <img src="https://img.shields.io/badge/rust-1.70%2B-orange" alt="Rust">
  <img src="https://img.shields.io/github/license/Spatpit/Pluriview" alt="License">
  <img src="https://img.shields.io/github/v/release/Spatpit/Pluriview?include_prereleases" alt="Release">
</p>

---

<!--
## Screenshot
![Pluriview Screenshot](assets/screenshot.png)
-->

## Features

| Feature | Description |
|---------|-------------|
| **Live Capture** | Real-time window previews using Windows Graphics Capture API |
| **Infinite Canvas** | Pan and zoom freely to organize your workspace |
| **Crop Regions** | Focus on specific parts of windows with Alt+drag |
| **Adjustable FPS** | Choose 5, 15, 30, or 60 FPS per preview |
| **Auto-Save** | Layouts persist automatically between sessions |
| **System Tray** | Minimize to tray for background operation |
| **Quick Focus** | Double-click any preview to bring its window to front |

## Requirements

- **OS:** Windows 10 (version 1903+) or Windows 11
- **GPU:** DirectX 11 compatible graphics card

## Installation

### Download Release
Download the latest `pluriview.exe` from [Releases](https://github.com/Spatpit/Pluriview/releases).

### Build from Source
```bash
git clone https://github.com/Spatpit/Pluriview.git
cd Pluriview
cargo build --release
```
The executable will be at `target/release/pluriview.exe` (~6MB).

## Usage

1. **Launch** `pluriview.exe`
2. **Add windows** from the Window Picker panel (left side)
3. **Arrange** by dragging previews on the canvas
4. **Resize** by dragging corners or edges
5. **Crop** by holding Alt and dragging corners
6. **Right-click** for context menu options

## Keyboard Shortcuts

| Action | Shortcut |
|--------|----------|
| Pan canvas | `Middle Mouse` or `Alt + Drag` |
| Zoom | `Scroll Wheel` |
| Toggle grid | `G` |
| Select all | `Ctrl + A` |
| Multi-select | `Ctrl + Click` |
| Delete selected | `Delete` |
| Crop preview | `Alt + Drag corners` |
| Focus window | `Double-click preview` |
| Show shortcuts | `F1` |

## Project Structure

```
Pluriview/
├── src/
│   ├── app.rs              # Main application state and UI
│   ├── main.rs             # Entry point
│   ├── canvas/             # Infinite canvas (pan, zoom, selection)
│   ├── capture/            # Window capture coordinator
│   ├── persistence/        # Layout save/load
│   ├── preview/            # Preview window management
│   ├── tray/               # System tray integration
│   └── window_picker/      # Window enumeration and picker UI
├── assets/
│   └── icon.ico            # Application icon
├── Cargo.toml              # Dependencies and metadata
├── build.rs                # Windows resource compilation
├── LICENSE                 # MIT License
└── README.md
```

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

- Built with [egui](https://github.com/emilk/egui) - Immediate mode GUI library for Rust
- Window capture via [windows-rs](https://github.com/microsoft/windows-rs)

---

<p align="center">
  Made with Rust
</p>
