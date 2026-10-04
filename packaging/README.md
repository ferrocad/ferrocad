# Packaging FerroCAD

Platform scripts that wrap the payload staged by `cargo xtask bundle`
(`target/dist/ferrocad/`) into a distributable artifact:

| Platform | Script | Artifact | Needs |
| --- | --- | --- | --- |
| Linux | `linux/appimage.sh` | `target/dist/ferrocad-<arch>.AppImage` | `appimagetool` |
| macOS | `macos/app.sh` | `target/dist/FerroCAD.app` (+ `FerroCAD.dmg`) | macOS; `hdiutil` for the dmg |
| Windows | `windows/portable.ps1` | `target/dist/ferrocad-windows-x86_64/` + `.zip` | PowerShell |

Workbench scripts are fetched first with `cargo xtask mods` (a pinned upstream
FreeCAD revision, default `Draft`) into `mods/`; the packagers ship them loose.
The release workflow runs it before packaging.

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
├── runtime/                  bundled CPython (when fetched; self-contained)
└── LICENSES/                 LGPL-2.1-or-later
```

Each packager adds its own launcher (AppRun, the `.app` launcher, `ferrocad.bat`)
that sets `FERROCAD_PYTHON_PATH` and `PYTHONPATH`, so the embedded interpreter
finds `python/`, `lib/` and `mods/`.

## Notes

- **CPython is bundled when available.** Run `cargo xtask python` once to fetch a
  pinned `python-build-standalone` runtime (cached under `target/python-runtime/`);
  `bundle` then includes it as `runtime/` and builds the app against it. The
  launchers set `PYTHONHOME` and the library path, so `libpython` and the stdlib
  come from the bundle. Pin a different version with `FERROCAD_PYTHON_VERSION` and
  `FERROCAD_PBS_DATE`. Without a runtime, the payload uses the system interpreter.
- **Icons are optional.** Each script uses one if you provide it
  (`linux/ferrocad.png`, `macos/FerroCAD.icns`, `windows/ferrocad.ico`) and warns
  otherwise. The source is `icon/ferrocad.svg`; convert it with `rsvg-convert` or
  `magick` (png/ico) and `iconutil` (icns).
- **Linux `appimagetool`** is found on `PATH`, via `APPIMAGETOOL`, or as
  `tools/appimagetool-<arch>.AppImage`. An `.AppImage` tool is run with
  `--appimage-extract-and-run`, so FUSE is not required.
- **macOS and Windows scripts are untested here** (this project builds on Linux);
  they are written to be read and adjusted on their platforms.
