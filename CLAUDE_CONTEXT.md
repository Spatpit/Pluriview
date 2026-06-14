# Pluriview - Claude Context

Use this file to quickly get Claude up to speed on the project.

---

## Project Overview

**Pluriview** is a Windows application (similar to PureRef/Kosmik) that displays live window previews on an infinite canvas. Built with Rust + egui.

- **GitHub:** https://github.com/Spatpit/Pluriview
- **Version:** 0.1
- **Author:** Spatpit
- **License:** MIT

---

## Quick Start for Claude

Copy this into a new chat:

```
I'm continuing work on Pluriview, a Rust + egui Windows app for live window previews on an infinite canvas. The project is at S:\AI\My Own apps\pluriview

Please read CLAUDE_CONTEXT.md for the full project context.
```

---

## Key Features

1. **Infinite Canvas** - Pan (middle-click drag) and zoom (scroll wheel)
2. **Live Window Capture** - Uses Windows Graphics Capture API for real-time previews
3. **Modern Dark UI** - "Minimal Void" theme with dark colors
4. **Card-Based Window Picker** - Search, rounded cards, green accent + button
5. **System Tray** - Custom leaf icon, minimize to tray
6. **Window Cropping** - Alt+drag on preview OR right-click → Crop → Select Region
7. **ShareX-Style Region Selector** - Native overlay for precise crop selection
8. **Layout Persistence** - Saves/loads canvas state automatically
9. **Portable .exe** - Single file with embedded icon

---

## Project Structure

```
pluriview/
├── Cargo.toml              # Dependencies and build config
├── build.rs                # Embeds icon.ico into .exe
├── assets/
│   ├── icon.ico            # Leaf icon (multi-size)
│   └── icon_options/       # Alternative icon designs
├── src/
│   ├── main.rs             # Entry point, window creation, tray setup
│   ├── app.rs              # Main app state, UI layout (side panel + canvas)
│   ├── canvas/
│   │   ├── mod.rs
│   │   ├── state.rs        # Canvas rendering, pan/zoom, context menu, crop
│   │   ├── input.rs        # Mouse/keyboard input handling
│   │   └── animation.rs    # Smooth pan/zoom animations
│   ├── preview/
│   │   ├── mod.rs
│   │   ├── manager.rs      # Collection of previews, add/remove/get
│   │   └── preview.rs      # Individual preview state (position, size, crop)
│   ├── window_picker/
│   │   ├── mod.rs
│   │   ├── picker.rs       # Modern card-based UI with search
│   │   └── enumerator.rs   # Win32 EnumWindows, GetWindowText, etc.
│   ├── capture/
│   │   ├── mod.rs
│   │   └── coordinator.rs  # Spawns capture threads, manages frame channels
│   ├── overlay/
│   │   ├── mod.rs
│   │   └── region_selector.rs  # Native Win32 overlay for region selection
│   ├── tray/
│   │   ├── mod.rs
│   │   └── icon.rs         # Creates leaf icon for system tray
│   └── persistence/
│       ├── mod.rs
│       ├── layout.rs       # Serializable layout structures
│       └── storage.rs      # Save/load to AppData
├── README.md
├── LICENSE
├── CHANGELOG.md
└── CLAUDE_CONTEXT.md       # This file
```

---

## Key Files to Know

| File | Purpose |
|------|---------|
| `src/app.rs` | Main app struct, side panel layout, region selection state |
| `src/canvas/state.rs` | Canvas rendering, context menu (Crop submenu), preview display |
| `src/window_picker/picker.rs` | Modern card-based window list UI |
| `src/capture/coordinator.rs` | Windows Graphics Capture API integration |
| `src/overlay/region_selector.rs` | ShareX-style native overlay for crop selection |
| `Cargo.toml` | Dependencies including `windows`, `eframe`, `windows-capture` |

---

## Dependencies

```toml
# UI
eframe = "0.29"
egui = "0.29"

# Window Capture
windows-capture = "1.4"

# Win32 API
windows = "0.58"  # Features: Win32_Foundation, Win32_UI_WindowsAndMessaging,
                  # Win32_Graphics_Gdi, Win32_System_*, Win32_UI_Input_KeyboardAndMouse

# System Tray
tray-icon = "0.19"

# Async
tokio = "1"

# Serialization
serde = "1.0"
serde_json = "1.0"

# Utilities
parking_lot = "0.12"
image = "0.25"
```

---

## Build Commands

```bash
# Development build (faster, with debug info)
cargo build

# Release build (optimized, portable .exe)
cargo build --release

# Run development version
cargo run

# Output locations
# Debug:   target/debug/pluriview.exe
# Release: target/release/pluriview.exe
```

---

## UI Theme Colors

The app uses a "Minimal Void" dark theme:

```rust
// Backgrounds
let bg_darkest = Color32::from_rgb(18, 18, 20);      // Canvas background
let bg_dark = Color32::from_rgb(22, 22, 26);         // Panels
let bg_card = Color32::from_rgb(28, 28, 32);         // Cards
let bg_hover = Color32::from_rgb(38, 38, 45);        // Hover state

// Accent
let accent_green = Color32::from_rgb(107, 170, 75);  // Leaf green (buttons, highlights)

// Text
let text_primary = Color32::WHITE;
let text_secondary = Color32::from_rgb(140, 140, 150);
```

---

## How Things Work

### Window Capture Flow
1. User clicks + on a window card in picker
2. `WindowPicker::add_window_to_canvas()` creates preview and starts capture
3. `CaptureCoordinator::start_capture()` spawns a thread
4. Thread uses `windows-capture` crate (Windows Graphics Capture API)
5. Frames sent via channel to main thread
6. `CaptureCoordinator::process_frames()` updates preview textures

### Region Selection (Crop)
1. Right-click preview → Crop → Select Region
2. `CanvasState` sets `pending_region_select` with preview ID
3. `App` detects this, creates `RegionSelector` targeting the source window
4. Native Win32 overlay appears over source window
5. User draws rectangle, presses Enter or clicks
6. Selection converted to UV coordinates, applied to preview's `crop_uv`

### Canvas Rendering
1. `CanvasState::ui()` draws the infinite canvas
2. Each preview rendered with current frame texture
3. Crop applied via UV coordinates in shader
4. Pan offset and zoom applied to all positions

---

## Git Info

- **Main branch:** `main`
- **Remote:** `https://github.com/Spatpit/Pluriview.git`
- **Last release:** v0.1

---

## Skills (Shortcuts)

The project has `.claude/skills.md` with these shortcuts:
- `/build` - Build debug version
- `/release` - Build release version
- `/run` - Run the app
- `/push` - Git add, commit, push

---

## Known Issues / TODOs

- Minor unused code warnings in overlay module
- Region selector could use keyboard shortcut activation
- Consider adding undo/redo for canvas operations

---

## Recent Changes (as of last session)

1. Set up GitHub repository with proper README, LICENSE, CHANGELOG
2. Created custom leaf icon for all contexts (tray, taskbar, title bar, .exe)
3. Changed author name from "Javier" to "Spatpit" throughout
4. Implemented ShareX-style region selector for cropping
5. Redesigned Window Picker with modern card-based UI
6. Fixed cursor issues in region selector (crosshair instead of loading)

---

## Starting a New Session

Just tell Claude:

> "I'm working on Pluriview at `S:\AI\My Own apps\pluriview`. Read CLAUDE_CONTEXT.md to get up to speed."

Then describe what you want to work on!
