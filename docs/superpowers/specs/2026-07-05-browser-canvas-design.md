# Browser Canvas Design

## Goal

Add app-owned WebView2 browser tiles that play web media with independent audio and retain Pluriview's infinite-canvas movement, zoom, overlap, z-order, and persistence.

## Architecture

Use a hybrid browser portal:

- Each browser tile owns a borderless Win32 host window containing a Wry/WebView2 view.
- In normal canvas mode, Windows Graphics Capture renders that host into the existing preview texture path.
- Double-clicking a browser tile pauses its capture, brings it to the front, and positions the real host window over the tile for native mouse, keyboard, accessibility, login, and media interaction.
- Escape, focus loss, canvas movement, or canvas zoom parks the host and resumes captured rendering.
- Browser controls share one Pluriview-specific WebView2 user-data folder, separate from the user's main browser profile.

DirectComposition visual hosting is excluded: it would require a second native composition/input system beside egui's OpenGL renderer. An always-visible child WebView is excluded because native HWND layering breaks canvas overlap, clipping, and egui overlays.

## Browser Tile Behavior

- Right-clicking the canvas offers **Add Browser** and requests an HTTP(S) URL.
- Browser tiles default to a 1280x720 backing viewport, 16:9 aspect ratio, and 30 FPS capture.
- Normal mode supports selection, movement, resizing, overlap, z-order, viewport culling, and deletion like existing previews.
- Entering interaction mode first brings the tile to the front. Only one browser tile may be interactive at a time.
- Clicking outside the browser or pressing Escape exits interaction mode.
- Browser cropping is not supported because a cropped page has ambiguous interactive coordinates.
- Direct page navigation is used for YouTube, Twitch, and other sites; no site-specific URL parser or embed layer is added.
- New-window requests navigate the same browser tile instead of opening another window. Downloads are disabled initially.

## Components and Data

Add one browser manager responsible for creating, parking, activating, navigating, and destroying browser host windows and WebViews. It is keyed by the existing `PreviewId` and remains on the UI thread because Wry WebViews are not `Send` or `Sync`.

Extend preview persistence with an optional browser URL. Ordinary previews continue restoring by matching an external window. Browser previews recreate their host and load the saved URL. Cookies, login state, cache, and site preferences remain in WebView2's user-data folder and are not copied into layout JSON.

## Capture Prerequisites

Before adding browser UI, correct the shared capture path:

1. Capture the supplied HWND directly instead of finding a window by title.
2. Request RGBA frames directly instead of allocating a BGRA-to-RGBA conversion.
3. Reuse the existing egui texture rather than creating a new texture every frame.
4. Keep only the newest pending frame so UI stalls cannot accumulate obsolete video frames.

These changes apply to all previews and make multiple media tiles viable without a browser-specific renderer.

## Lifecycle and Error Handling

- If WebView2 Runtime is missing, keep the canvas usable and show an actionable browser-tile error.
- A failed navigation leaves the tile alive so the user can enter another URL.
- Removing a browser tile stops capture before closing its WebView and host HWND.
- Minimizing or closing Pluriview parks or closes browser hosts so they do not appear independently.
- Layout restore skips a browser tile only when host creation fails; other tiles still restore.
- Browser URLs accept only `http` and `https`; `file`, `javascript`, and custom schemes are rejected.

## Technical Spike Gate

Before the full feature, a minimal spike must prove on the target Windows setup that:

1. A same-process WebView host HWND can be captured by Windows Graphics Capture while covered or parked.
2. YouTube and Twitch continue video and audio playback when the host is not foreground.
3. Activating and parking the host aligns with a tile at 100%, 125%, and 150% DPI.

If same-process capture fails, use the same executable in a small `--browser-host` helper mode. Do not introduce Electron, CEF, or a second application.

## Verification

- Unit/self-check URL scheme validation and browser layout serialization.
- Manually verify YouTube, Twitch, and a direct MP4.
- Verify pan, zoom, resize, overlap, bring-to-front, deletion, save/restore, minimize/restore, and interaction exit.
- Verify one and three simultaneous 720p/30 FPS browser tiles while watching CPU, memory, dropped frames, and audio continuity.
- Verify multi-monitor DPI transitions at 100%, 125%, and 150%.

## Initial Scope Exclusions

No tabs, bookmarks, extensions, ad blocking, browser history UI, custom media controls, site-specific CSS, arbitrary downloads, browser crop, or DirectComposition renderer integration.
