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

## libmpv

Full Pluriview releases ship `libmpv-2.dll` next to `pluriview.exe`. Pluriview
loads that DLL at runtime for local video tiles, video wallpaper, folder
playlists, and seek previews. The DLL is not compiled into the executable, and
it is not stored in this git repository.

The pinned Windows runtime is extracted from the shinchiro mpv-dev archive:

- Build date: `20260610`
- mpv git: `304426c`
- Asset: [`mpv-dev-x86_64-v3-20260610-git-304426c.7z`](https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20260610/mpv-dev-x86_64-v3-20260610-git-304426c.7z)
- Archive SHA-256: `D4B3D6DF9FDB33D5591C4ECE7D0CC24D2F7822B298F6A1528595E0CCFF7424A6`
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
filename, beside `pluriview.exe`).
