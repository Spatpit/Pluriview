# Pinned standalone ANGLE runtime

Pluriview distributes the two x64 Windows DLLs in this directory unchanged.
They were built specifically for Pluriview from the official standalone ANGLE
source; they were not extracted from OBS, CEF, Chromium, Electron, or another
application package.

## Provenance

- ANGLE source: `google/angle` commit
  `d9fc4a372074b1079c193c422fc4a180e79b6636` (`chromium/7258`)
- vcpkg build definitions: `microsoft/vcpkg` commit
  `ddd0023b0eee70986e42ed49d9d4afb8098f212e`
- vcpkg package: `angle@chromium_7258#2`
- External zlib: 1.3.2, linked statically into `libGLESv2.dll`
- Build toolchain: Visual Studio 18.9, MSVC 14.51.36231, x64 Release
- Build triplet: `x64-windows-angle-runtime.cmake` in this directory

The triplet keeps ANGLE as two dynamic libraries, links non-ANGLE dependencies
statically, remaps build-drive paths, and embeds only PDB filenames. PDB files
are not distributed.

## Runtime hashes

- `libEGL.dll`:
  `FFC4565AD5839AC9FE6719D399B7E90CF5C7984E609D7876DE1D9FA9BF9EDAF3`
- `libGLESv2.dll`:
  `CB302095BACCBEC3BD30184A837B342E3260970C9934558762A5415FB85D12BA`

The release DLLs have no dependency on CEF, OBS, or a separately deployed
zlib DLL. Their remaining imports are Windows system libraries and the same
Microsoft Visual C++ runtime family already required by `pluriview.exe`.

## Build outline

1. Check out the vcpkg commit above and run `bootstrap-vcpkg.bat
   -disableMetrics`.
2. Run `vcpkg install angle:x64-windows-angle-runtime` with this directory as
   `--overlay-triplets` and with a clean install root.
3. Copy only the Release `libEGL.dll` and `libGLESv2.dll`; never copy PDBs.
4. Verify the dynamic imports, hashes, and absence of local paths before
   updating these pinned assets.

Complete runtime notices are in `ANGLE_LICENSES.txt` and `APACHE-2.0.txt`.
