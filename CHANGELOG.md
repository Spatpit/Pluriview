# Changelog

All notable changes to Pluriview will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- Phone Viewer requests are now signed with HMAC-SHA256 instead of sending the pairing token, and captured requests cannot be replayed. The QR/link works for one phone, expires after ten unused minutes, and can be replaced with **New pairing link**. Requires Android Viewer 0.3.12 together with this PC build.
- Prepared local testing packages as v0.7.1.
- Android audio can run alongside desktop Stream Audio Monitor. While monitoring is enabled, the phone captures Pluriview's application mix once, including the existing monitor output, without separately adding the original window sources or changing Discord/OBS routing. Switching monitoring modes reconnects phone audio automatically.
- Image/GIF decoding and folder-playlist scans now run in the background with immediate placeholders. Workspace changes, source replacements and deletion discard stale completions; loading does not overwrite edited tile geometry or playlist controls.
- Split workspace operations, media-source handling, settings, and canvas navigation into focused modules; moved canvas regression tests out of the production module.
- Added Windows pull-request checks for formatting, the full Rust test suite and Clippy correctness lints, with pinned runtime preparation.

### Added
- Tile screenshots. The new **Capture** section in the top bar (camera button or `Shift+S`, configurable) saves the selected or focused tiles as PNG files in the `screenshots` data folder, keeping crops and source resolution where available, with a flash and a thumbnail that flies into the Capture button. **Capture → Open Screenshots Folder** opens the folder.
- Screenshots panel on the right (`S`, the gallery button in the Capture section, or **View → Screenshots Panel**). It lists screenshots newest first in collapsible folders with thumbnails decoded in the background. Create and rename folders by typing the name in place, choose the folder new screenshots are saved to, select several screenshots with Ctrl/Shift+click (clearly outlined, with a count and bulk Move/Recycle Bin actions), drag them onto a folder to move them, or open, show in Explorer, or move them to the Recycle Bin. New captures fly into their folder in the open panel. Drag screenshots, a selection, or a whole folder onto the canvas (or use **Add to Canvas**) to add them as image tiles in a grid; each one flies in from the panel.
- Integrated the existing optional Android tile audio (companion preview 0.3.9) into the v0.7.1 desktop build. A phone on/off control starts a separate authenticated AAC-LC stereo stream at a 128 kbps target. Off/background/pause releases capture and playback; audio failures leave video usable. Includes native/browser tile audio and opted-in window applications. Requires updated PC and APK builds.
- Remember per-video volume, mute, playback speed, looping, and explicit audio/subtitle choices across workspace saves, reloads, freeze/resume, and deletion undo. Apply settings before playback starts, preserve them while sources are unavailable, and fall back to automatic selection when a preferred track is missing.
- Persistent named tile groups with shared movement, proportional resize, group pinning, membership editing, and deletion undo. Alt+1–4 choose one, two, or three large tiles, or equal tiles; Alt+5 is unassigned. Spotlight supports drag-and-drop swaps between any slots, with a drop-target highlight and smooth, interruptible 180 ms transitions for swaps and preset changes. Layouts keep the group's total size, start from its top-left corner, slide back inside the window instead of spilling past it, and preserve proportions and crops. Window, screen, image and Spout tiles that were resized out of shape go back to their source's proportions when a layout is applied or Spotlight slots are swapped, so no tile is shown stretched. Equal tiles pick the number of rows that best fits the group's outline. **Arrange → Fit Group to View** scales a group to fill the window, and dragging a group's resize handle stops before any tile gets smaller than the 100-unit single-tile minimum. Exiting saves the final arrangement and member order; F focuses the selected tile and returns to Spotlight. Autosave retains the previous arrangement until Spotlight exits.
- Added nearby-PC discovery and explicit PC approval for Android connection requests (preview 0.3.6). Both screens show a matching number; requests expire, cancel or deny without granting access. QR/link pairing remains available; encryption remains deferred.
- Experimental Android companion viewer with opt-in LAN sharing, QR/link pairing, fullscreen viewing, and shared PC canvas pan/pinch-zoom. Phone commands cannot edit or interact with tiles. Fullscreen Fill/Fit preserves proportions and gesture mapping, with auto-hiding controls and cutout support. Continuous H.264 video uses a fixed 720p profile (1280×720 cap, 30 FPS / 4 Mbps targets), prefers PC hardware encoding, and retains software/JPEG fallbacks. Native Android decoding uses bounded buffering and fresh-stream recovery. Sharing is unencrypted and pauses for desktop dialogs or interactive browser mode; companion 0.3.9 adds optional tile audio.

### Fixed
- Switching a tile group between layouts no longer shrinks it a little each time until the tiles become tiny.
- Phone tile audio no longer cycles through "reconnecting" on ordinary Wi-Fi hiccups. The PC keeps the audio stream through video restarts and brief PC delays, and Android Viewer 0.3.13 skips audio that arrives too late instead of restarting, keeps audio playing while video briefly recovers, and uses a slightly larger jitter buffer. Pauses, source changes and disconnects still stop audio.
- Android Viewer 0.3.11 fixes immediate audio queue-full reconnects on short packet bursts and prevents speaker writes from monopolizing the decoder loop. Retains bounded queues and deadlines, starts streaming playback without a fixed prefill count, and distinguishes stuck decoder input from speaker output. Added production audio-pump regression tests; the PC 0.7.1 build and Discord/OBS routing are unchanged.
- Android Viewer 0.3.10 initializes audio playback before starting PC capture, preventing slow decoder startup from overflowing the live queue. Audio stall detection excludes network idle time, startup cancellation/deadlines are bounded, and reconnects show safe failure reasons. Compatible with the updated 0.7.1 PC build; desktop audio routing is unchanged.
- Preserve wallpaper paths after image decoding failures or missing video files; failed video wallpapers no longer request continuous immediate redraws.
- Prevent multiple Pluriview instances from writing the same data folder by holding an exclusive Windows file handle through shutdown. Separate data folders remain independent.
- Reduced excessive space between grouped tiles with different aspect ratios. Spotlight packs visible tile edges closely without stretching or changing crops, keeps its top-left anchor, and sizes companions from the group's height so a portrait selection does not miniaturize its neighbors.
- Separated layout presets and group movement from Spotlight activation. Entering Spotlight, selecting tiles, and resizing the viewport preserve the existing arrangement. Dragging onto another tile exchanges only that pair's actual positions and sizes, independent of member order.
- Replaced Android video HTTP-client reads with a dedicated, proxy-free socket that resumes partial records after short read timeouts. Brief delivery pauses no longer immediately discard the connection and decoder; silence, total record time and background cancellation remain bounded (APK 0.3.8; compatible with Windows preview 0.3.6).
- Fixed the Android decoder watchdog counting idle network time against newly arrived work. Brief reconnects retain the last texture for a bounded interval with navigation disabled; explicit sharing pauses still clear immediately. Status retains the reconnect count and last reason for diagnosis (APK 0.3.7; compatible with Windows preview 0.3.6).
- Removed Android's 10 FPS JPEG fallback and fixed background/resume treating interrupted decoder cleanup as a permanent video failure. Hidden apps pause streaming; returning reconnects fixed-quality H.264, with bounded decoder shutdown and no overlapping decoder workers (APK 0.3.5).
- Improved Android stream pacing with absolute PC capture deadlines, independent receive/decode work and a bounded 50 ms playback cushion. Gesture commands reuse a persistent connection, transient decoder stalls recover H.264 instead of dropping to 10 FPS JPEG, and the viewer shows measured received/displayed frame rates (APK 0.3.4; fixed 720p retained).
- Smoothed Android viewer pan/pinch updates with a short, interruptible PC camera interpolation; added double-tap tile focus/restore without source input (APK 0.3.3).
- Fixed Android Viewer 0.3.0 crashing on launch when fullscreen setup queried the system-bar controller before the window decor existed (APK 0.3.1).

## [0.7.0] - 2026-09-11

### Added
- App-wide Always on Top in the empty-canvas and tray menus, remembered across launches. Tray-only Click Through lets mouse input reach apps underneath while previews keep playing; turn it off or use Show Pluriview in the tray to restore interaction. Click-through starts disabled on launch.

### Changed
- Disabled automatic Dependabot version-update pull requests for Cargo and GitHub Actions. Dependency updates are reviewed manually; scheduled and release-time RustSec audits remain enabled.
- Starting with v0.7.0, releases provide only the Full package with the video runtime included. Discontinued Lite builds and their GitHub release assets.
- Menu shortcut hints now use a separate right-aligned column and reflect configured bindings, including saved viewpoints and matching canvas/tile actions.
- Replaced the playlist header's brown folder badge with a pale-blue filmstrip on a subtle blue background.
- Refreshed the top bar, window picker, quick-add popup, and context menus with coordinated charcoal surfaces, warm gold accents, clearer headings, and roomier controls.
- Window picker search now has a clear button and visible empty states; long window and Spout names are ellipsized with full-name tooltips. The Spout section stays out of the way when Spout is absent.
- Tile menus put frame-rate choices in a submenu and scroll when needed to keep every action reachable on smaller windows.
- Updated Windows Graphics Capture integration to windows-capture 2.0.1, retaining GPU downsampling and reusable frame-readback storage.
- Crop menus now offer an in-tile editing mode with highlighted edge/corner handles. Click empty canvas, press Escape, or choose Finish Cropping to leave the mode; Alt+drag remains available. Replaces the external source-window region selector.
- Redesigned the browser and stream source dialogs with consistent headers, URL fields with adjacent Paste controls, clear primary actions, compact recent website rows, and editable stream quality with presets. Added Enter submission for streams and Escape cancellation for both dialogs.

### Fixed
- Fixed Add Window occasionally closing immediately after selection from the canvas menu when popup placement shifted near a window edge. The picker now opens after the menu click finishes.
- Prevented shortcut rows from expanding popups to their provisional width, aligned toggle labels, and kept saved-viewpoint names, shortcut hints, and overflow controls within compact menu bounds.
- Release packaging rejects local performance builds and verifies the pinned libmpv runtime checksum alongside the ANGLE checksums.
- Canvas shortcuts no longer act behind an open browser or stream source dialog.
- Reduced CPU work when resizing captured windows by scaling frames on the capture GPU before readback, preserving the existing pixel averages and falling back to CPU resizing on unsupported devices.
- Browser tiles no longer take keyboard focus while their background hosts are created, avoiding a startup stall observed when adding several tiles together.
- Reduced redundant GIF and video redraws, including high-frequency canvas redraws driven by off-screen videos, while preserving playback and authored animation timing.

## [0.6.9] - 2026-09-06

### Added
- An optional Support on Ko-fi link in the About dialog opens the creator's page in your browser.

### Changed
- Focus current tile now defaults to F, making it available on keyboards without a number pad. Existing saved bindings are preserved; change the action in Settings to adopt the new default.
- Reorganized the README around setup and everyday use, clarified browser profiles, shared logins, ad blocking, and audio sharing, and moved technical details into expandable sections.
- Updated the About description to reflect windows, web pages, and media on the canvas.

### Fixed
- Canvas shortcuts no longer trigger while interacting with a browser page, so typing F or using a site's player shortcuts does not change the canvas. Browser interaction and exit shortcuts remain available.

## [0.6.8] - 2026-09-05

### Changed
- Updated image decoding, logging, and JSON dependencies, plus the pinned checkout, artifact-download, and attestation actions used by the release workflows.
- Windows Task Manager identifies the application as "Pluriview" instead of "Live window preview application".
- Replaced the green leaf branding with the gold, champagne, and olive Canvas P logo on a chocolate background across the title bar, About dialog, taskbar, tray, executable, and README.
- Updated source-picker and canvas accents to the logo's honey gold.

### Added
- Ctrl+Z restores the latest deleted tile batch even after its Undo toast disappears. The shortcut is configurable in Settings and leaves text-field and interactive-browser undo to their editors.
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
