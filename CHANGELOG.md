# Changelog

All notable changes to Pluriview will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2026-07-06

### Added
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
