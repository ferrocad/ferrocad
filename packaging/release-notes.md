## What's in each download

Every artifact is self-contained: it bundles CPython, the `FreeCAD` Python facade
and the `Draft` workbench, so no system Python is required. The
`ferrocad-parametric-*` artifacts add the Part workbench and bundle the OCCT
toolkit closure under `lib/` (beside `ferrocad.exe` on Windows), so `import Part`
works with no system OCCT installed.

<!--ARTIFACTS-->

- **Linux**: `chmod +x ferrocad-x86_64.AppImage && ./ferrocad-x86_64.AppImage`
  (or `ferrocad-parametric-x86_64.AppImage` for the Parametric edition).
- **macOS**: not shipped while the pinned GPUI fork uses `cocoa` without declaring
  it; see `docs/developer/architecture/edition-publishing.md`.
- **Windows**: the zip is unsigned, so SmartScreen may warn on first run.

`cargo install ferrocad` is the alternative: that binary embeds the same payload
and unpacks it on first run. See `docs/developer/architecture/distribution.md`.
