# Packaging FerroCAD

Platform scripts that wrap the payload staged by `cargo xtask bundle`
(`target/dist/ferrocad/`) into a distributable artifact:

| Platform | Script | Artifact | Needs |
| --- | --- | --- | --- |
| Linux | `linux/appimage.sh` | `target/dist/ferrocad-<arch>.AppImage` | `appimagetool` |
| macOS | `macos/app.sh` | `target/dist/FerroCAD.app` (+ `FerroCAD.dmg`) | macOS; `hdiutil` for the dmg |
| Windows | `windows/portable.ps1` | `target/dist/ferrocad-windows-x86_64/` + `.zip` | PowerShell |

Each script runs `cargo xtask bundle` first, so one command produces the artifact.
Pass `--debug` (or `-Debug` on Windows) for a fast, unstripped build.

## The payload

`cargo xtask bundle` stages a platform-neutral directory, with native names for
the current host:

```
target/dist/ferrocad/
├── bin/ferrocad[.exe]        the app binary
├── lib/ferrocad.abi3.so      the PyO3 extension (`ferrocad.pyd` on Windows)
├── python/                   FreeCAD, FreeCADGui, ferrocad_shell, ferrocad_spike
├── mods/                     workbenches
└── LICENSES/                 LGPL-2.1-or-later
```

Each packager adds its own launcher (AppRun, the `.app` launcher, `ferrocad.bat`)
that sets `FERROCAD_PYTHON_PATH` and `PYTHONPATH`, so the embedded interpreter
finds `python/`, `lib/` and `mods/`.

## Notes

- **CPython is not bundled yet.** The payload uses the system interpreter.
  Bundling python-build-standalone is the next step (see `docs/distribution.md`).
- **Icons are optional.** Each script uses one if you provide it
  (`linux/ferrocad.png`, `macos/FerroCAD.icns`, `windows/ferrocad.ico`) and warns
  otherwise. The source is `icon/ferrocad.svg`; convert it with `rsvg-convert` or
  `magick` (png/ico) and `iconutil` (icns).
- **Linux `appimagetool`** is found on `PATH`, via `APPIMAGETOOL`, or as
  `tools/appimagetool-<arch>.AppImage`. An `.AppImage` tool is run with
  `--appimage-extract-and-run`, so FUSE is not required.
- **macOS and Windows scripts are untested here** (this project builds on Linux);
  they are written to be read and adjusted on their platforms.
