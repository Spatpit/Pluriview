# Changelog

All notable changes to Pluriview will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.2] - 2026-08-19

### Fixed
- Window capture on Windows 10 no longer stays on "Connecting...". Border and cursor Graphics Capture settings are requested only when those APIs exist, and tiles show a failed state if capture cannot start.

## [0.6.1] - 2026-08-18

### Changed
- Window-capture resolution follows canvas zoom below 100%, so zoomed-out tiles keep smaller textures. Browser and video tiles are unchanged.
- Tile focus hides the selection border, resize handles, and zoom badge until focus is cleared.
- Cropped window tiles no longer show a bottom-left CROP badge; cropping still works via Alt-drag and Clear Crop.

## [0.6.0] - 2026-08-18

### Added
- Local video tiles play in-process with bundled `libmpv-2.dll` (drop a file, play/pause, seek, volume, mute, speed, loop, tracks, reload)
- Hovering a local or on-demand seek bar shows a frame at that time without moving the playing video
- Drag-and-drop video folders create a linked player and a playlist tile (next/prev, shuffle, repeat, autoplay, natural filename sort, workspace restore)
- Live wallpaper for images, GIFs, and looping muted videos; it fills the window and does not pan or zoom with the canvas
- Freeze Selected / Resume Selected: frozen window captures stop, browsers suspend, video tiles keep a still frame after unloading the player, GIFs stop advancing
- Stream Audio Monitor copies tile audio into the Pluriview process so Discord/OBS window shares can pick it up
- Optional auto-hiding title bar (show it only when the pointer is at the top of the window)
- Left-drag marquee selection from empty canvas; Ctrl adds to the current selection
- Capture-resolution badge on hover for browser and window tiles

### Changed
- Video playback no longer launches `mpv.exe`. Settings only configures Streamlink, which is required only for stream URLs
- Browser tiles capture at 2× the tile size (still capped at 4K). Window captures still see the native HWND, then downscale in the worker so 4K frames are not stored
- Video wallpaper pauses while a tile is in focus mode, then resumes when focus is cleared
- Full GitHub releases include `libmpv-2.dll`, the license, and third-party notices beside the exe; a Lite zip is exe-only, and both downloads have published SHA-256 checksums

### Fixed
- Play/pause and volume follow the control immediately instead of lagging or reversing
- Seek-bar hover previews match the hovered timestamp instead of a nearby keyframe
- Stream tiles show a short title from the original URL instead of CDN query strings

## [0.5.1] - 2026-08-10

### Fixed
- Restored browser tiles now start at two-second intervals, and off-screen browser tiles remain unloaded until viewed, preventing media-heavy workspaces from freezing the app during startup

## [0.5.0] - 2026-08-10

### Added
- Named workspaces with create, duplicate, rename, switch, and confirmed-delete actions
- Backward-compatible migration of the existing autosave into a Default workspace

### Changed
- The active workspace is mirrored to the legacy `autosave.json` so older Pluriview builds can still open the latest canvas
- Development builds keep `pluriview_data` at the repository root so Cargo cleanup cannot delete workspaces or imported media
- Development and test profiles use smaller debug information without incremental caches to limit build-directory growth

## [0.4.0] - 2026-08-10

### Added
- Image tiles for PNG, JPEG, WebP, and BMP files, plus animated GIF tiles with authored frame timing
- Imported tile media is copied into `pluriview_data/media` and restored from portable relative paths in saved layouts
- Image files can be dragged onto the canvas, with pointer placement, multi-file fan-out, and a visible drop target

### Changed
- Animated GIF playback schedules repaints around each frame's authored delay and bounds catch-up work after UI stalls
- uBlock Origin Lite is reused from the existing WebView2 profile instead of being reinstalled on every launch, reducing startup work and preserving active filtering rules

### Fixed
- Restored browser tiles block media such as YouTube videos from autoplaying until that tile receives user interaction
- Hiding the window picker sidebar now survives a restart instead of coming back on every launch
- The main window reopens at the size, position, and maximized state it was closed at, instead of always 1280x720; a position on a monitor that is no longer attached falls back to the default placement
- Ad blocking no longer misses the pages a launch starts with: uBlock Origin Lite is reused from the WebView2 profile instead of being reinstalled every launch, which used to clear its request rules and script injections and leave tiles (YouTube especially) unfiltered for the rest of the session
- Browser tiles wait on a blank page while uBlock Origin Lite starts after an install or after the blocker is switched back on, and pages that already loaded unfiltered reload once it is filtering

## [0.3.0] - 2026-07-06

### Added
- Integrated uBlock Origin Lite ad/tracker blocking for browser tiles, enabled by default with a persistent global View-menu toggle
- Browser tiles: app-owned WebView2 pages (YouTube, Twitch, any site) on the canvas with independent audio; double-click or Ctrl+B for native interaction, Esc or click outside to exit
- Browser tiles are saved and restored with layouts (current URL, position, size, FPS, z-order, mute state)
- Browser hover controls: back, forward, reload, mute/unmute, open in default browser
- Browser context menu: Interact, Mute, Reload, Change URL, Copy URL, Open in Default Browser
- Live page titles on browser tiles (instead of the raw URL)
- Recent-URL suggestions in the Add Browser dialog
- Persistent muted badge on silenced browser tiles
- Green accent outline around the browser tile in interaction mode
- Esc exits browser interaction mode; browser shortcuts listed in the F1 dialog

### Changed
- Interactive browser windows now follow their tile through canvas pan/zoom and window moves
- Entering/leaving browser interaction keeps the page at the same apparent zoom (no size "pop")
- Popup/new-window requests from pages navigate the same tile instead of opening windows
- FPS preset changes apply live without restarting the capture (no black flash)
- Undoing a removed browser tile recreates it from its URL

### Fixed
- Interactive browser windows no longer stay floating over other apps on focus loss or minimize
- WebView is resized to match the tile in interaction mode (was clipped or undersized)
- Browser sizing is DPI-correct (physical pixels) on scaled monitors
- Captured-frame backlog can no longer accumulate unbounded memory if the UI stalls
- Typing in a text field no longer triggers the G/Ctrl+B canvas shortcuts
- Enter reliably submits the Add Browser dialog

## [0.1] - 2025-02-09

### Added
- Initial release
- Live window capture using Windows Graphics Capture API
- Infinite canvas with pan (middle-mouse/Alt+drag) and zoom (scroll wheel)
- Window Picker panel with search and filtering
- Preview management (add, remove, resize, reposition)
- Crop regions with Alt+drag on corners
- Adjustable FPS presets (5, 15, 30, 60 FPS)
- Persistent layout save/restore
- System tray integration with minimize to tray
- Double-click preview to focus source window
- Minimal dark "Void" theme with hover-reveal controls
- Grid toggle (G key)
- Keyboard shortcuts help dialog (F1)
- About dialog with version info
- Application icon
