<p align="center">
  <img src="assets/icon.png" width="128" height="128" alt="Pluriview logo">
</p>

<h1 align="center">Pluriview</h1>

<p align="center">
  Your windows, web pages, and media together on an infinite canvas for Windows.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/platform-Windows%2010%2F11-blue" alt="Windows 10 and 11">
  <img src="https://img.shields.io/github/license/Spatpit/Pluriview" alt="License">
  <img src="https://img.shields.io/github/v/release/Spatpit/Pluriview?include_prereleases" alt="Latest release">
</p>

Pluriview gives you a flexible space to arrange live app windows, browser pages,
videos, images, and overlays. Move and resize tiles, crop out distractions, and
save different layouts for different activities.

Watch streams alongside chat, keep reference material nearby, or build a canvas
with transparent widgets and a moving wallpaper.

<p align="center">
  <img src="assets/pluriview-preview.gif" alt="Live windows, browser pages, and images arranged on the Pluriview canvas">
</p>

## Download and start

Get the latest version from **[Releases](https://github.com/Spatpit/Pluriview/releases)**.

| Version | Choose this if… |
|---|---|
| **Full — recommended** | You want all features, including local videos, folder playlists, and video wallpapers. |
| **Lite** | You need live windows, browser tiles, Spout2 sources, images, and GIFs. |

1. Download and extract the ZIP.
2. Keep the included `lib` folder beside `pluriview.exe`.
3. Launch `pluriview.exe` and start adding content.

**Requirements:** Windows 10 version 1903 or newer, or Windows 11, with a
DirectX 11-compatible GPU.

Browser tiles require the [Microsoft WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/).
If browser tiles cannot start because the runtime is missing, install it and
reopen Pluriview. Built-in ad blocking requires WebView2 Runtime 122 or newer.

## Your first canvas

- **Add an app window:** choose it from the Window Picker on the left.
- **Add a web page:** right-click the canvas → **Add Browser...**
- **Add images, GIFs, or videos:** drag files from Explorer onto the canvas.
- **Add a video playlist:** drag a folder of videos onto the canvas.
- **Arrange your tiles:** drag to move them and use their edges or corners to resize.
- **Explore the canvas:** use the middle mouse button to pan and the scroll wheel to zoom.

Hover over tiles for controls, or right-click a tile for more options.
Press `F1` to see shortcuts.

## What you can add

| Source | What it does |
|---|---|
| **App windows** | Displays a live preview of an open application window. |
| **Browser tiles** | Opens a web page inside Pluriview, with navigation, interaction, and audio. |
| **Images and GIFs** | Displays local images and plays animated GIFs. |
| **Videos** | Plays local video files with playback, seeking, volume, and track controls. |
| **Folder playlists** | Creates a video player linked to a playlist of files from a folder. |
| **Stream URLs** | Uses optional Streamlink support to play supported streams in a video tile. |
| **Spout2 sources** | Receives live output from compatible applications such as VTube Studio. |

### App windows and Spout2

Use the **Window Picker** to add an open application as a live tile. You can
resize, crop, freeze, or pin its preview.

Search by window title or application name, then use **+** to add the source.
Clear the search with **×**; hover a shortened name to read it in full.
Right-click a tile for its settings, including the **Frame Rate** submenu.
Menu shortcuts appear on the right and follow your bindings in **Settings**.

Live Spout2 senders appear in the same picker. Select one to display its shared
output on the canvas.

### Browser tiles

Browser tiles let you place live web pages on the canvas, including streams,
chats, dashboards, and overlay widgets.

Right-click the canvas → **Add Browser...**, then paste a web address.
Use **Paste** beside the URL field or select a recent website. Press **Enter**
to add the page, or **Esc** to cancel.

#### Arrange the tile or use the website

While working on the canvas, you can move, resize, layer, crop, and pin a
browser tile.

To interact with the website, **double-click the tile** or press **`Ctrl+B`**.
You can then click links, type, sign in, and use the page's controls.
Press **`Esc`** or click outside to return to arranging your canvas.

Canvas shortcuts, including `F` for tile focus, stay inactive while you interact
with a page. The browser interaction and exit shortcuts remain available.

Hover over the tile for **back, forward, reload, and mute**. Each tile has its
own page and mute control. One browser tile can receive direct interaction at
a time.

#### How browser tiles work

Pluriview uses **Microsoft WebView2** to run web pages inside browser instances
managed by the app. You do not need to keep a separate browser window open for
each tile.

Browser data is stored locally in a **dedicated Pluriview profile**, separate
from your regular browser profile:

- Your regular browser's tabs and sign-ins are not automatically imported.
- All Pluriview browser tiles share cookies and login sessions, including across workspaces.
- Signing into a website in one tile can also sign you into that website in other tiles.
- Login sessions can remain available when you close and reopen Pluriview.

Websites connect to their own services and handle account information as they
normally would when you visit them.

#### Ad and tracker blocking

**uBlock Origin Lite** is included and enabled by default for browser tiles.

Toggle it under **View → Block Ads & Trackers (uBOL)**. The setting applies to
all browser tiles. If blocking interferes with a page, you can turn it off from
this menu.

WebView2 does not provide an extension toolbar, so the usual uBlock popup is
unavailable.

#### Transparent overlays and custom sizes

Pages designed with transparent backgrounds stay transparent. This allows
widgets such as heart-rate monitors to blend into your canvas.

- **Change the page shape:** right-click the tile and turn off **Lock Aspect Ratio** to resize it into a portrait, square, or wide layout.
- **Show part of a page:** hold `Alt` and drag a tile edge or corner to crop.
- **Keep a widget in place:** right-click → **Pin to Viewport**.
- **Click content underneath an overlay:** use the tile's **Disable Left Click** option.

Cropping keeps the page's proportions, and you can still interact with the
visible region.

### Images and GIFs

Drag an image onto the canvas, or choose **File → Add Image...**.

Supported formats include **PNG, JPEG, GIF, WebP, and BMP**. Animated GIFs
retain their original timing and pause when the tile is frozen.

Pluriview uses the original file without creating a separate copy. Keep the
file in its saved location, or use **Locate Image…** from the tile's right-click
menu if you move it.

### Videos and folder playlists

The **Full** download includes the video runtime. No separate mpv installation
is required for playback.

Drag a video onto the canvas, or choose **File → Add Video...**. Controls
include:

- Play/pause, seek, mute, and volume.
- Playback speed and looping.
- Audio and subtitle track selection.
- A seek-bar preview so you can inspect another moment without moving playback.

Hold `Alt` and drag an edge or corner to crop the video.

Drag a **folder of videos** onto the canvas to create a linked player and
playlist. Playlists include previous/next, shuffle, and repeat controls, and
use files directly inside the selected folder.

### Live streams

You can watch a stream in a **browser tile** by opening its website.

To open a supported stream URL in a **video tile**, install
[Streamlink](https://streamlink.github.io/) and add the stream from the canvas
menu. Configure Streamlink in **Settings** if Pluriview does not find it
automatically.

The Add Stream dialog lets you type a quality or use **Choose** for the best,
lowest, or detected qualities. Custom quality values remain supported.

Video-tile streams require the video runtime included in the Full download.
Live streams do not provide seek-bar thumbnails.

## Make the canvas yours

| Option | What it does |
|---|---|
| **Crop** | Shows only the part of a tile you need. Hold `Alt` and drag an edge or corner. Available for windows, browsers, Spout2, images, GIFs, and videos. |
| **Pin to Viewport** | Keeps any tile fixed on screen while you pan or zoom the canvas. |
| **Freeze** | Holds the current frame of selected tiles until you resume them. |
| **Focus on This Tile** | Fits a tile to the canvas. Press `Esc` to return. |
| **Canvas-only mode** | Hides the interface with `H`; right-click menus remain available. |
| **Capture FPS** | Chooses 15, 30, or 60 FPS for live previews. |
| **System tray** | Keeps Pluriview available while minimized to the tray. |

### Wallpapers

Choose **View → Set Wallpaper...**, or use the canvas right-click menu.

Images, GIFs, and local videos can fill the background. The wallpaper stays
fixed to the window while you pan and zoom, and each workspace can have its
own wallpaper.

Video wallpapers require the Full download's video runtime. They loop without
audio and pause while a tile is in focus mode.

Wallpaper files stay in their original locations, so keep them available after
adding them.

### Workspaces and saved viewpoints

Use the **Workspace** menu to create, duplicate, rename, and switch between
canvas setups. Changes save automatically once per minute.

**Saved viewpoints** bookmark places within a workspace. Arrange your camera,
then choose **View → Saved Viewpoints → Save Current Viewpoint...**.

Select a viewpoint to move smoothly back to it. You can rename, update, or
delete viewpoints from the same menu. Pan, zoom, or press `Esc` to interrupt
a transition.

Use `Ctrl+1` through `Ctrl+9` to open a viewpoint directly, or
`Ctrl+Page Up` / `Ctrl+Page Down` to cycle between them.

## Sharing audio with Discord or OBS

Browser audio plays normally while you use Pluriview. However, that sound
belongs to WebView2, so sharing only the Pluriview window may leave browser
audio out.

**Stream Audio Monitor** lets Pluriview replay a copy of the audio through its
own process so window sharing can capture it.

1. Open **View → Stream Audio Monitor**.
2. Choose an output you do not listen to, such as a virtual audio cable or an unused audio output.
3. Share the Pluriview window in your streaming or calling application.

Using a separate output avoids hearing the replayed audio twice. Browser tiles
are monitored automatically when the feature is enabled; captured app windows
use their individual **Stream Audio** toggle.

This feature is off by default.

## Controls and shortcuts

Keyboard shortcuts can be changed under **View → Settings → Keyboard
shortcuts**. Mouse controls are fixed.

Starting with v0.6.9, **Focus current tile** defaults to `F`. Existing saved
shortcuts are preserved when updating; set this action to `F` in Settings if
your installation still uses `Numpad 2`.

| Action | Default control |
|---|---|
| Pan canvas | Middle mouse drag or `Alt` + drag on empty canvas |
| Zoom | Scroll wheel |
| Show/hide Window Picker | `W` |
| Show/hide grid | `G` |
| Toggle canvas-only mode | `H` |
| Select all tiles | `Ctrl+A` |
| Select multiple tiles | `Ctrl` + click |
| Box-select tiles | Left-drag on empty canvas |
| Add to box selection | `Ctrl` + left-drag on empty canvas |
| Crop a supported tile | `Alt` + drag an edge or corner |
| Delete selected tiles | `Delete` |
| Undo the latest tile deletion | `Ctrl+Z` |
| Focus current tile | `F` |
| Interact with browser tile | Double-click, `Ctrl+B`, or `Numpad 1` |
| Exit focus or browser interaction | `Esc` |
| Open saved viewpoints 1–9 | `Ctrl+1` through `Ctrl+9` |
| Previous/next viewpoint | `Ctrl+Page Up` / `Ctrl+Page Down` |
| Show shortcuts | `F1` |

## Technical information

<details>
<summary><strong>Included components and optional tools</strong></summary>

Both downloads include the ANGLE rendering runtime in the `lib` folder:

- `libEGL.dll`
- `libGLESv2.dll`

The **Full** download also includes `libmpv-2.dll` for video tiles, folder
playlists, and video wallpapers.

**WebView2** is required for browser tiles. **Streamlink** is an optional
separate installation for supported stream URLs opened in video tiles.

Window capture, browser tiles, images, GIFs, and workspaces do not require
libmpv or Streamlink.

</details>

<details>
<summary><strong>Verify a release download</strong></summary>

Release downloads include a `SHA256SUMS.txt` file for checking archive integrity.

Official tagged releases are built through the repository's public GitHub
Actions workflow, which runs formatting checks, tests, dependency auditing,
and the release build. Published archives include build-provenance attestations.

With the GitHub CLI installed, verify an archive using:

```powershell
gh attestation verify .\Pluriview-vX.Y.Z-windows-x64-full.zip --repo Spatpit/Pluriview
```

Replace the example filename with the archive you downloaded.

</details>

<details>
<summary><strong>Build from source</strong></summary>

Building requires **Rust 1.88 or newer** and a Windows development environment.

```powershell
git clone https://github.com/Spatpit/Pluriview.git
cd Pluriview
.\scripts\prepare-angle.ps1
.\scripts\prepare-libmpv.ps1
.\scripts\build-release.ps1
```

The release script builds with local path remapping, checks the executable for
local-path and credential-like strings, and creates the packaged output in
`dist`.

Output includes Full and Lite ZIP archives, runtime libraries, third-party
notices, and `SHA256SUMS.txt`.

Keep the `lib` folder beside the executable. Do not distribute `pluriview.pdb`,
which can contain local paths.

The capture resize shader is included as precompiled bytecode, so normal builds
need no shader compiler. After editing `src/capture/shaders/area_average.hlsl`,
run `.\scripts\compile-capture-shader.ps1` with the Windows SDK installed and
include the regenerated `.cso` alongside the shader source.

</details>

<details>
<summary><strong>Project structure</strong></summary>

| Location | Purpose |
|---|---|
| `src/app.rs` | Application state, menus, and source orchestration |
| `src/canvas/` | Canvas rendering, interaction, and wallpaper |
| `src/preview/` | Tile models and lifecycle |
| `src/capture/` | Window and Spout2 capture |
| `src/browser.rs` | WebView2 browser tiles |
| `src/libmpv.rs` | In-process video rendering |
| `src/media.rs` | Images and animated GIFs |
| `src/playlist.rs` | Folder playlists |
| `src/audio.rs` | Stream Audio Monitor |
| `src/persistence/` | Workspaces, layouts, and settings |
| `src/window_picker/` | Source enumeration and picker |
| `assets/` | Branding and bundled third-party assets |
| `scripts/` | Runtime preparation and release packaging |

The editable logo is `assets/logo.svg`. Run `scripts/generate-icons.ps1` on
Windows to regenerate the application's PNG and ICO assets.

</details>

## Support

If you would like to support Pluriview's development, you can visit
[Spatpit on Ko-fi](https://ko-fi.com/spatpit). The same link is available in the
app's **About** dialog. Support is always optional.

## License

Pluriview is licensed under the **MIT License**. See [LICENSE](LICENSE).

Bundled components retain their own licenses. See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). Full releases include
libmpv/FFmpeg under GPLv2+; all releases include ANGLE and its applicable notices.

## Acknowledgments

- [egui](https://github.com/emilk/egui) — application interface.
- [windows-rs](https://github.com/microsoft/windows-rs) — Windows integration and capture.
- [Spout](https://spout.zeal.co/) — shared GPU texture sources.
- [wry](https://github.com/tauri-apps/wry) and Microsoft WebView2 — browser tiles.
- [ANGLE](https://github.com/google/angle) — graphics rendering.
- [libmpv](https://github.com/mpv-player/mpv) — video playback.
- [uBlock Origin Lite](https://github.com/gorhill/uBlock) — ad and tracker blocking.
