# Changelog

All notable changes to Pluriview will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- Windows Task Manager identifies the application as "Pluriview" instead of "Live window preview application".
- Replaced the green leaf branding with the gold, champagne, and olive Canvas P logo on a chocolate background across the title bar, About dialog, taskbar, tray, executable, and README.
- Updated source-picker and canvas accents to the logo's honey gold.

### Added
- A workspace recovery screen pauses editing and saving after a failed load, with retry, previous-save recovery, and options to open another workspace.
- Unavailable image/GIF tiles offer Retry Image and Locate Image actions while retaining their layout settings.
- Stream Audio Monitor displays starting, active, retrying, and failure status for its sources.

### Fixed
- Settings now places Streamlink first and groups Saved Viewpoints shortcuts in a separate collapsible section below the general shortcuts.
- Failed workspace loads cannot be overwritten by autosave or exit saves. Backup rotation validates the document schema, unsupported versions are preserved, and catalog recovery recognizes backup-only workspaces without replacing unrecoverable files with a blank workspace.
- Closed window captures become inactive and reconnect through privacy-approved window matching. Waking or undoing window tiles revalidates window identity instead of reusing stale handles.
- Corrupt images remain recoverable tiles when reopening a workspace instead of disappearing from later saves.
- Undo restores a complete tile deletion batch with its stacking order, aspect-ratio lock, freeze state, and playlist links; switching workspaces clears the old deletion undo.
- Repeated Send to Back actions maintain a strict tile stacking order.
- Stopping audio monitoring no longer blocks the UI waiting for a worker; audio activation waits can be cancelled.

## [0.6.7] - 2026-09-02

### Added
- Official release archives can now be built, tested, provenance-attested, and published from version tags by a public GitHub Actions workflow.
- Saved Viewpoints store camera positions within each workspace and glide between them with interruptible, smoothly eased pan and proportional zoom transitions. Configurable next, previous, and direct 1–9 shortcuts provide quick access; the menu indicates active and modified viewpoints, and deleted viewpoints can be undone.
- Browser and stream URL dialogs now provide a Paste button and a right-click Paste action for the URL field.
- Changed workspaces now save automatically once per minute, while unchanged workspaces produce no disk writes.
- The Workspace menu can restore the previous valid save as a separate recovered workspace.
- Saved Viewpoints are available from the empty-canvas right-click menu as well as the top View menu.

### Changed
- Tiles now stop exactly where they are released instead of continuing with momentum and a spring bounce.
- Removed the retired external-player implementation and consolidated video playback state around the active in-process libmpv backend.

### Fixed
- The Saved Viewpoints submenu now opens to the left near the right edge, keeping the main canvas context menu anchored to the click position without overlap.
- Updated vulnerable transitive dependencies and added automated RustSec and Dependabot checks for newly disclosed dependency issues.
- Workspace, catalog, autosave, and settings JSON now use crash-safe temporary files with a previous-save backup instead of truncating the live file in place.
- Missing or corrupt workspace catalogs are rebuilt from valid workspace files, including layouts orphaned by an interrupted save.

## [0.6.6] - 2026-08-26

### Changed
- The Windows compositor now uses ANGLE's Direct3D 11 backend; all release packages include the required `libEGL.dll` and `libGLESv2.dll` runtime files.
- The bundled ANGLE runtime is built from pinned official standalone source, with reproducible provenance, privacy-safe paths, checksums, and complete runtime license notices.
- Libmpv tiles retain high-quality scaling on the OpenGL ES renderer instead of falling back to visibly blocky restricted scaling.
- Release DLLs and text notices now live in a single `lib` folder beside `pluriview.exe`.

### Fixed
- Capturing Pluriview at 60 FPS with OBS Game Capture or Discord's injected capture hook no longer multiplies GPU usage through per-frame WGL-to-Direct3D synchronization.

## [0.6.5] - 2026-08-24

### Added
- Image, animated GIF, browser, and libmpv video tiles can be cropped with Alt+drag; cropped browser pages keep capture and interaction coordinates aligned.
- Browser tiles can unlock their aspect ratio from the context menu for portrait, square, ultrawide, and other custom viewport shapes.
- Saved window tiles remain visible as inactive when their application is closed and reconnect automatically after its window appears.
- Tiles can disable left-click interaction from their context menu, passing primary clicks and drags through to content underneath while keeping right-click access available.

### Changed
- Image/GIF tiles and image wallpapers use their original file paths instead of creating copies in `pluriview_data/media`; missing originals remain visible with a path-change message.
- Libmpv video tiles show a vertical volume slider above the speaker icon on hover and use a larger time readout.

### Fixed
- Entering browser interaction mode no longer moves that browser tile to the front of the saved canvas stacking order.
- Fully transparent browser overlays waiting for their first visible content no longer capture and repaint at full speed.
- Restored frozen browser tiles no longer keep repainting while their deferred WebView remains suspended.
- Window capture tiles waiting for their first frame no longer keep the canvas repainting at an unrestricted rate.
- Manually frozen tiles remain frozen after closing and reopening Pluriview without restarting their source resources.
- Menus and popups block canvas pointer gestures beneath them, so scrolling a menu no longer zooms the canvas.
- Browser crops persist when a workspace is saved and restored.
- Browser tiles keep their loading card behind fully transparent startup frames until visible page content arrives.

## [0.6.4] - 2026-08-22

### Added
- Any tile can be pinned to the viewport so its position and size stay fixed while the canvas pans or zooms.

### Changed
- Browser tiles preserve transparent page backgrounds for overlay widgets such as Pulsoid.
- Removed the rectangular drop shadow from every tile type so transparent content has no dark plate behind it.
- Window captures that remain far outside the viewport release their capture session after a grace period and recreate it when visible again, reducing idle memory without changing Stream Audio intent.
- Hidden video wallpapers and idle seek-preview decoders release their playback resources after short grace periods and resume on demand.
- Cropping changes the visible tile bounds without stretching the content, and clearing a crop restores the matching full-frame bounds.

### Fixed
- Alt-dragging a crop handle no longer pans or zooms the canvas, including for viewport-pinned tiles.

## [0.6.3] - 2026-08-20

### Added
- Capture live Spout2 senders (VTube Studio and similar) as canvas tiles. Pluriview receives the shared GPU texture; it does not publish a Spout sender.
- Spout2 tiles can be pinned to the viewport so their position and size stay fixed while the canvas pans or zooms.
- Keyboard shortcuts can be reassigned in Settings to one key or any two-key combination, with conflict validation and a restore-defaults button. Mouse controls remain fixed.
- The Window Picker has a configurable keyboard shortcut that defaults to `W`.
- Window-capture tiles can be resized to their native capture resolution from the tile context menu.

### Changed
- Live capture tiles request sharper backing frames as the canvas zooms in, up to the source resolution and existing 4K cap.
- The window-capture audio button now says "Stream Audio" instead of "SA" for clarity.
- The enabled Stream Audio button hides with the rest of the tile controls when the tile is not hovered.
- Browser Back and Forward controls use a persistent blue accent so they are easier to discover.
- Settings moved from the File menu to the View menu.

### Fixed
- Window-capture resolution targets refresh immediately while tiles resize or the canvas zooms.
- A pinned Spout2 tile can be dragged to a new viewport position and remains pinned when released.

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
