# Changelog

All notable changes to Pluriview will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

- Added left-drag marquee selection from empty canvas, with Ctrl for additive
  selection and group movement through the existing multi-select behavior.
- Added Freeze Selected and Resume Selected context actions. Frozen window
  captures stop their capture sessions, browser tiles use WebView2 suspension,
  mpv and Streamlink tiles keep a still frame while fully unloading their player
  and cache, and animated GIFs stop advancing. Resumed on-demand videos restore
  their position and playback settings; live streams reconnect at the live edge.
- Added drag-and-drop video folders, creating a linked libmpv player and a
  resizable canvas playlist with lazy thumbnails, natural filename sorting,
  previous/next, autoplay, shuffle, repeat, scrolling, and workspace restore.

### Added
- Local video files can be dropped onto the canvas and start in MPV immediately
- Hovering a local or on-demand video's seek bar reuses a persistent libmpv decoder, keeps the last frame while the next one loads, prefetches nearby seconds, and skips livestreams
- MPV and Streamlink tiles have an in-place reload action for stopped or failed playback

### Fixed
- Play/pause and volume controls update immediately and ignore stale MPV acknowledgements instead of lagging or reversing the requested state
- Play resumes the existing MPV process instead of recreating it, and leaves a healthy Windows capture session running so pause/play does not freeze or blank the tile
- Seek-bar hover previews seek to the exact hovered time instead of a nearby keyframe, so the popup frame matches the timestamp
- Stream tiles no longer use CDN query strings as their name; the menu and overlay show a short title from the original URL

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
