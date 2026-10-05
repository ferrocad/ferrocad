## What's in each download

Every artifact is self-contained: it bundles CPython, the `FreeCAD` Python facade
and the `Draft` workbench, so no system Python is required.

<!--ARTIFACTS-->

- **Linux**: `chmod +x ferrocad-x86_64.AppImage && ./ferrocad-x86_64.AppImage`.
- **macOS**: arm64 only; the app is ad-hoc signed, so a downloaded DMG needs
  `xattr -dr com.apple.quarantine /Applications/FerroCAD.app` (or right-click →
  Open) on first launch.
- **Windows**: the zip is unsigned, so SmartScreen may warn on first run.

`cargo install ferrocad` is the alternative: that binary embeds the same payload
and unpacks it on first run. See `docs/distribution.md`.
