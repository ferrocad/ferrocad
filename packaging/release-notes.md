## What's in each download

Every artifact is self-contained: it bundles CPython, the `FreeCAD` Python facade
and the `Draft` workbench, so no system Python is required.

| Artifact | `python/` | `mods/Draft/` | `runtime/` | Launcher |
| --- | --- | --- | --- | --- |
| `ferrocad-x86_64.AppImage` | ✅ 7 files | ✅ 502 | ✅ `lib/libpython3.14.so.1.0` | `AppRun` |
| `FerroCAD.dmg` → `FerroCAD.app` | ✅ `Contents/Resources/python` | ✅ 502 | ✅ `lib/libpython3.14.dylib` | `Contents/MacOS/ferrocad` |
| `ferrocad-windows-x86_64.zip` | ✅ 7 files | ✅ 502 | ✅ `python314.dll` | `ferrocad.bat` |

- **Linux**: `chmod +x ferrocad-x86_64.AppImage && ./ferrocad-x86_64.AppImage`.
- **macOS**: arm64 only; the app is ad-hoc signed, so a downloaded DMG needs
  `xattr -dr com.apple.quarantine /Applications/FerroCAD.app` (or right-click →
  Open) on first launch.
- **Windows**: the zip is unsigned, so SmartScreen may warn on first run.

`cargo install ferrocad` is the alternative: that binary embeds the same payload
and unpacks it on first run. See `docs/distribution.md`.
