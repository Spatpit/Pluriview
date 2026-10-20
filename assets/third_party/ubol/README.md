# uBlock Origin Lite package

Pluriview ships the official Microsoft Edge package of **uBlock Origin Lite**
(uBOL) in the `lib` folder beside `pluriview.exe` and extracts it into the
application's WebView2 profile on first use. The build pins the package's
SHA-256, and the app refuses any other file under that name.

- Version: `2026.714.1952`
- Upstream release: https://github.com/gorhill/uBlock/releases/tag/uBOLite_2026.714.1952
- Upstream source: https://github.com/gorhill/uBlock/tree/uBOLite_2026.714.1952
- Packaged asset: `uBOLite_2026.714.1952.edge.zip`
- SHA-256: `B218ED46118E1E6E8C0ECE23DA8DA373807F26C5A60AFF4322F72730CDA6E39E`
- License: GNU General Public License v3.0 (the upstream `LICENSE.txt` is
  preserved inside the package)

The package is unmodified from the official upstream release asset.
