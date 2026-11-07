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

Arrange live app windows, web pages, videos, and images on one canvas. Move,
resize, and crop them, and save different layouts for different activities:
watch a stream next to its chat, keep reference material nearby, or build an
overlay of transparent widgets.

<p align="center">
  <img src="assets/pluriview-preview.gif" alt="Live windows, browser pages, and images arranged on the Pluriview canvas">
</p>

## Download

1. Download the ZIP from **[Releases](https://github.com/Spatpit/Pluriview/releases)** and extract it.
2. Keep the `lib` folder beside `pluriview.exe`.
3. Run `pluriview.exe`.

**Requirements:** Windows 10 (1903 or newer) or Windows 11 with a DirectX 11
graphics card. Browser tiles need the
[Microsoft WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/),
which most Windows PCs already have.

**Updating:** close Pluriview, then replace `pluriview.exe` and the `lib`
folder. Your workspaces, settings, and sign-ins live in the `pluriview_data`
folder, so keep it. See the [changelog](CHANGELOG.md) for what's new.

## Getting started

- **Add an app window:** pick it from the Window Picker on the left (`W`).
- **Add a web page:** right-click the canvas → **Add Browser...**
- **Add images, GIFs, or videos:** drag files onto the canvas. Drag a folder of videos to make a playlist.
- **Arrange:** drag tiles to move them, and drag their edges or corners to resize.
- **Move around:** drag with the middle mouse button to pan, and scroll to zoom.

Hover a tile for its controls, or right-click it for everything else. Press
`F1` to see all shortcuts. A short tour of the main features opens the first
time you start Pluriview; reopen it from **Help → Getting Started**.

## What you can add

| Source | What it is |
|---|---|
| **App windows** | A live view of any open window, which you can also use in place. |
| **Web pages** | A full browser inside the tile, with tabs, sign-ins, and sound. |
| **Videos and playlists** | Local video files, or a whole folder played as a playlist. |
| **Images and GIFs** | PNG, JPEG, GIF, WebP, and BMP files. |
| **Live streams** | Any stream site in a web page tile, or stream links in a video tile with [Streamlink](https://streamlink.github.io/) installed. |
| **Spout2** | Live output from apps such as VTube Studio, listed in the Window Picker. |

## Features

### Use an app from its tile

Double-click a window tile (or right-click → **Interact**) to use the app right
on the canvas. Pluriview moves the app's own window over the tile, so your
clicks and typing go to the app. Drag the gold bar above it to move the tile,
and click outside the app or **Done** when you're finished; the window goes back
where it was. People watching through Discord or OBS see your mouse pointer
meanwhile.

This doesn't work for apps running as administrator or windows on another
virtual desktop.

### Web pages

Double-click a browser tile or press `Ctrl+B` to use the page, and `Esc` to go
back to arranging.

- **Toolbar and tabs:** address bar, back, forward, reload, and tabs down the
  side. Drag a tab to another browser tile, or onto the canvas to give it its
  own tile.
- **Sign-ins:** browser tiles share one profile, separate from your regular
  browser. **Tools → Clear Browser Data...** signs you out everywhere.
- **Ad blocking:** uBlock Origin Lite is built in and on by default
  (**Tools → Block Ads & Trackers (uBOL)**).
- **Extras:** `Shift`+scroll scrolls a page without clicking into it,
  **Page Zoom** resizes its text, and **Page → Fit to Video** crops the tile to
  the page's video. Transparent pages stay transparent, great for overlay
  widgets.

### Videos, playlists, and streams

Video tiles have play/pause, seeking, volume, speed, looping, and audio and
subtitle tracks, and each tile remembers its settings. A folder playlist adds
previous/next, shuffle, and repeat. For stream links, install
[Streamlink](https://streamlink.github.io/), or open the stream site in a web
page tile.

Images, GIFs, and videos are used from where they are, not copied, so keep them
in place. If you move an image, right-click its tile → **Locate Image…**.

### Control Tile Center

Press `C` to see every tile in a separate window. Click a card to focus that
tile, the **eye** to hide or show it, or the **speaker** to mute it. **Map**
shows the whole canvas in miniature, where you can drag tiles to new places.
Hidden tiles keep playing; right-click empty canvas to show them again.

### Groups

Select tiles (`Ctrl`+click or drag a box) and press `Ctrl+G` to group them, so
they move and resize together. `Alt+1`–`Alt+4` lay a group out with one, two,
or three large tiles, or all equal, and it keeps that layout as tiles come and
go. `Shift`+drag a tile onto another to swap them, and `Shift+F` makes the
selected tile the large one.

### Tile tools

| Tool | What it does |
|---|---|
| **Crop** | Shows only the part you need. Hold `Alt` and drag an edge, or right-click → **Tile Settings → Edit Crop**. |
| **Focus** | `F` fills the view with the selected tile; `Esc` goes back. |
| **Pin to Viewport** | Keeps a tile in place on screen while you pan and zoom. |
| **Picture in Picture** | `Shift+P` pops tiles into a small window that stays above other apps, including borderless or windowed games. |
| **Freeze** | Holds a tile's current frame until you resume it. |
| **Frame rate** | Click the FPS number on a live tile to switch between 15, 30, and 60 FPS. |

### Screenshots and recording

- `Shift+S` saves a screenshot of the selected tiles, laid out as on the canvas.
- `Shift+R` records them to an MP4 with sound (`P` pauses), and `Shift+G`
  records a GIF of up to 10 seconds.
- `S` opens the **Screenshots panel**, where you sort shots into folders or drag
  them back onto the canvas.

### Workspaces, viewpoints, and wallpapers

- The **Workspace** menu switches between separate canvases, which save
  automatically.
- **View → Saved Viewpoints** bookmarks spots on the canvas; jump to them with
  `Ctrl+1`–`Ctrl+9`.
- **View → Set Wallpaper...** puts an image, GIF, or looping video behind the
  canvas.

### Overlay mode

Turn on **Always on Top** (right-click empty canvas), then **Click Through**
from the tray icon so your clicks reach the apps underneath. Turn Click Through
off from the tray to use Pluriview again. `H` hides the interface.

## Sharing audio with Discord or OBS

Web page sound comes from a separate browser process, so sharing just the
Pluriview window can leave it out. **Stream Audio Monitor** fixes that:

1. Open **Tools → Stream Audio Monitor**.
2. Pick an output you don't listen to, such as a virtual audio cable.
3. Share the Pluriview window in Discord or OBS.

Web pages are included automatically. For app windows, turn on their **Stream
Audio** button.

## Shortcuts

Change any shortcut in **Tools → Settings → Keyboard Shortcuts**. Press `F1` in
the app for the full list.

| Action | Shortcut |
|---|---|
| Pan / zoom | Middle mouse drag / scroll wheel |
| Window Picker | `W` |
| Select all / box-select | `Ctrl+A` / drag on empty canvas |
| Crop | `Alt` + drag an edge |
| Delete / undo delete | `Delete` / `Ctrl+Z` |
| Focus tile | `F` |
| Use a web page | Double-click or `Ctrl+B` |
| Use an app window | Double-click |
| Back / exit | `Esc` |
| Control Tile Center | `C` |
| Picture in Picture | `Shift+P` |
| Group / ungroup | `Ctrl+G` / `Ctrl+U` |
| Group layouts | `Alt+1`–`Alt+4` |
| Make main tile / next / previous | `Shift+F` / `Shift+→` / `Shift+←` |
| Screenshot / Screenshots panel | `Shift+S` / `S` |
| Record / pause / record a GIF | `Shift+R` / `P` / `Shift+G` |
| Hide interface | `H` |
| Saved viewpoints | `Ctrl+1`–`Ctrl+9` |

## Technical information

<details>
<summary><strong>What's in the download</strong></summary>

The `lib` folder holds the ANGLE graphics runtime (`libEGL.dll`,
`libGLESv2.dll`), `libmpv-2.dll` for video, and the uBlock Origin Lite package
for ad blocking. WebView2 and Streamlink are separate installs; Streamlink is
optional.

Only one Pluriview can use a `pluriview_data` folder at a time. Separate
portable copies can run side by side.

</details>

<details>
<summary><strong>Verify a download</strong></summary>

Each release includes `SHA256SUMS.txt`, and releases are built by the public
GitHub Actions workflow with build-provenance attestations. With the GitHub CLI:

```powershell
gh attestation verify .\Pluriview-vX.Y.Z-windows-x64-full.zip --repo Spatpit/Pluriview
```

</details>

<details>
<summary><strong>Build from source</strong></summary>

Requires **Rust 1.92 or newer** on Windows.

```powershell
git clone https://github.com/Spatpit/Pluriview.git
cd Pluriview
.\scripts\prepare-angle.ps1
.\scripts\prepare-libmpv.ps1
.\scripts\build-release.ps1
```

The package lands in `dist`. Don't share `pluriview.pdb`, which can contain
local paths. After editing `src/capture/shaders/area_average.hlsl`, run
`.\scripts\compile-capture-shader.ps1` (needs the Windows SDK).

</details>

## Support

If you'd like to support Pluriview, you can visit
[Spatpit on Ko-fi](https://ko-fi.com/spatpit). It's always optional.

## License

Pluriview is licensed under the **MIT License**. See [LICENSE](LICENSE).
Bundled components keep their own licenses, including libmpv/FFmpeg under
GPLv2+; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Acknowledgments

- [egui](https://github.com/emilk/egui) — interface
- [windows-rs](https://github.com/microsoft/windows-rs) — Windows integration and capture
- [Spout](https://spout.zeal.co/) — shared GPU textures
- [wry](https://github.com/tauri-apps/wry) and Microsoft WebView2 — browser tiles
- [ANGLE](https://github.com/google/angle) — graphics rendering
- [libmpv](https://github.com/mpv-player/mpv) — video playback
- [uBlock Origin Lite](https://github.com/gorhill/uBlock) — ad and tracker blocking
