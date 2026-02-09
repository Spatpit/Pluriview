# Pluriview

A Windows application for creating live window previews on an infinite canvas. Similar to PureRef but for live windows instead of static images.

## Features

- **Live Window Capture** - Real-time previews of any window using Windows Graphics Capture API
- **Infinite Canvas** - Pan and zoom freely to organize your previews
- **Crop Regions** - Alt+drag corners to crop previews to specific areas
- **Adjustable FPS** - Choose from 5, 15, 30, or 60 FPS per preview
- **Persistent Layouts** - Automatically saves and restores your layout on restart
- **System Tray** - Minimize to tray for background operation
- **Double-Click Focus** - Double-click a preview to bring the source window to front
- **Minimal Dark Theme** - Clean, borderless design with hover-reveal controls

## Requirements

- Windows 10 (1903+) or Windows 11
- Graphics card with DirectX 11 support

## Building

```bash
# Debug build
cargo build

# Release build (optimized, ~6MB)
cargo build --release
```

The executable will be at `target/release/pluriview.exe`.

## Usage

1. Run `pluriview.exe`
2. Use the Window Picker (left panel) to add windows to the canvas
3. Drag previews to arrange them
4. Resize by dragging corners/edges
5. Crop by Alt+dragging corners
6. Right-click for context menu options

### Keyboard Shortcuts

| Action | Shortcut |
|--------|----------|
| Pan canvas | Middle Mouse / Alt+Drag |
| Zoom | Scroll Wheel |
| Toggle grid | G |
| Select all | Ctrl+A |
| Multi-select | Ctrl+Click |
| Delete selected | Delete |
| Crop preview | Alt+Drag corners |
| Focus source window | Double-click preview |
| Show shortcuts | F1 |

## Project Structure

```
src/
  app.rs           # Main application state and UI
  main.rs          # Entry point
  canvas/          # Infinite canvas (pan, zoom, selection)
  capture/         # Window capture coordinator
  persistence/     # Layout save/load
  preview/         # Preview window management
  tray/            # System tray integration
  window_picker/   # Window enumeration and picker UI
```

## License

MIT

## Author

Javier
