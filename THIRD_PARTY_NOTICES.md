# Third-party notices

## uBlock Origin Lite

Pluriview includes an unmodified official release package of uBlock Origin
Lite, version `2026.714.1952`, by Raymond Hill and contributors.

uBlock Origin Lite is licensed under the GNU General Public License, version
3. Its license text is included inside the embedded package at
`assets/third_party/ubol/uBOLite_2026.714.1952.edge.zip` as `LICENSE.txt`.
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

- Build date: `20260610`
- mpv git: `304426c`
- Asset: [`mpv-dev-x86_64-v3-20260610-git-304426c.7z`](https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20260610/mpv-dev-x86_64-v3-20260610-git-304426c.7z)
- Archive SHA-256: `D4B3D6DF9FDB33D5591C4ECE7D0CC24D2F7822B298F6A1528595E0CCFF7424A6`
- Extracted `libmpv-2.dll` SHA-256: `ADE5CAC46CFC397A3D5CD356A968CDA7ACF0DEBFFB705A16509DAFDF93029F5E`
- Prepare script: `scripts/prepare-libmpv.ps1`

mpv is copyright the mpv-player contributors. The mpv project is GPLv2+ by
default, and can be built as LGPLv2.1+ without GPL-only files. These shinchiro
`mpv-dev` packages are the standard Windows builds (GPLv2+). See
[mpv Copyright](https://github.com/mpv-player/mpv/blob/master/Copyright),
[LICENSE.GPL](https://github.com/mpv-player/mpv/blob/master/LICENSE.GPL), and
the matching source at
[`mpv@304426c`](https://github.com/mpv-player/mpv/tree/304426c).

`libmpv-2.dll` also contains FFmpeg and other libraries with their own
licenses. Typical licenses in this stack include LGPLv2.1+ / GPLv2+ (FFmpeg
and related codecs). Corresponding source for this exact binary is the
shinchiro release above.

You may replace `libmpv-2.dll` with another compatible `libmpv-2.dll` (same
filename, inside the `lib` folder beside `pluriview.exe`).
