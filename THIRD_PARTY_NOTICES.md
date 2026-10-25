# Third-party notices

## QR code encoder

The phone viewer uses the `qrcode` Rust crate, version 0.14.1
([source](https://github.com/kennytm/qrcode-rust)), under its MIT license:

Copyright (c) 2016 kennytm

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## uBlock Origin Lite

Pluriview ships an unmodified official release package of uBlock Origin
Lite, version `2026.714.1952`, by Raymond Hill and contributors, as
`lib/uBOLite_2026.714.1952.edge.zip` beside `pluriview.exe`.

uBlock Origin Lite is licensed under the GNU General Public License, version
3. Its license text is included inside the package as `LICENSE.txt`. The
source tree keeps the package at
`assets/third_party/ubol/uBOLite_2026.714.1952.edge.zip`.
The corresponding upstream source is available from the
[`uBOLite_2026.714.1952` tag](https://github.com/gorhill/uBlock/tree/uBOLite_2026.714.1952).

Package provenance and checksum are recorded in
`assets/third_party/ubol/README.md`.

## ANGLE

Every Pluriview release ships `libEGL.dll` and `libGLESv2.dll` in the `lib`
folder beside `pluriview.exe`. ANGLE translates Pluriview's OpenGL ES
compositor to Direct3D 11 on Windows. These pinned DLLs were built specifically
for Pluriview from the official standalone ANGLE source; they were not
extracted from OBS, CEF, Chromium, Electron, or another application package.

- ANGLE source: [`google/angle@d9fc4a372074`](https://github.com/google/angle/tree/d9fc4a372074b1079c193c422fc4a180e79b6636), Chromium branch 7258
- Build definitions: [`microsoft/vcpkg@ddd0023b0eee`](https://github.com/microsoft/vcpkg/tree/ddd0023b0eee70986e42ed49d9d4afb8098f212e), `angle@chromium_7258#2`
- `libEGL.dll` SHA-256: `FFC4565AD5839AC9FE6719D399B7E90CF5C7984E609D7876DE1D9FA9BF9EDAF3`
- `libGLESv2.dll` SHA-256: `CB302095BACCBEC3BD30184A837B342E3260970C9934558762A5415FB85D12BA`
- Build provenance: `assets/third_party/angle/README.md`

The runtime incorporates ANGLE (BSD-3-Clause), Chromium base helpers
(BSD-3-Clause), xxHash (BSD-2-Clause), zlib (zlib License), and Khronos
registry headers (MIT/Apache-2.0). Complete copies of their applicable terms
are retained in `assets/third_party/angle/ANGLE_LICENSES.txt` and
`assets/third_party/angle/APACHE-2.0.txt` and included in release packages.

## glutin-winit EGL compatibility fork

Pluriview carries a focused fork of `glutin-winit` 0.5.0 under
`third_party/glutin-winit-egl`. The only behavioral change selects EGL on
Windows so eframe can use the bundled ANGLE runtime. The original project is
licensed under MIT; its license is retained in that directory.

## libmpv

Full Pluriview releases ship `libmpv-2.dll` in the `lib` folder beside
`pluriview.exe`. Pluriview loads that DLL at runtime for local video tiles,
video wallpaper, folder playlists, and seek previews. The DLL is not compiled
into the executable, and it is not stored in this git repository.

The pinned Windows runtime is extracted from the shinchiro mpv-dev archive:

- Build date: `20261018`
- mpv git: `01e2533f31`
- Asset: [`mpv-dev-x86_64-v3-20261018-git-01e2533f31.7z`](https://sourceforge.net/projects/mpv-player-windows/files/libmpv/mpv-dev-x86_64-v3-20261018-git-01e2533f31.7z) (SourceForge mirror of the shinchiro GitHub release, which removes old builds)
- Archive SHA-256: `49533E80D681672E42AB38FCCFDB9DB0CC9FC423F86D34F5F9634EBA71109912`
- Extracted `libmpv-2.dll` SHA-256: `5C5C9D094859BDC60B8B27B2D832DB8FA8C2E487EFBD35F5AEA5AFE6960DAD39`
- Prepare script: `scripts/prepare-libmpv.ps1`

mpv is copyright the mpv-player contributors. The mpv project is GPLv2+ by
default, and can be built as LGPLv2.1+ without GPL-only files. These shinchiro
`mpv-dev` packages are the standard Windows builds (GPLv2+). See
[mpv Copyright](https://github.com/mpv-player/mpv/blob/master/Copyright),
[LICENSE.GPL](https://github.com/mpv-player/mpv/blob/master/LICENSE.GPL), and
the matching source at
[`mpv@01e2533f31`](https://github.com/mpv-player/mpv/tree/01e2533f31).

`libmpv-2.dll` also contains FFmpeg and other libraries with their own
licenses. Typical licenses in this stack include LGPLv2.1+ / GPLv2+ (FFmpeg
and related codecs). Corresponding source for this exact binary is the
shinchiro release above.

You may replace `libmpv-2.dll` with another compatible `libmpv-2.dll` (same
filename, inside the `lib` folder beside `pluriview.exe`).
