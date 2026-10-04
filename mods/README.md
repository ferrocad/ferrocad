# FerroCAD workbenches (`mods/`)

Each subdirectory is a workbench, the equivalent of FreeCAD's `Mod/`: Python the
app loads at startup that adds commands, panels and document types, and customizes
the UI.

The packaging step (`cargo xtask bundle`) copies this directory into the
distribution beside `python/`, and `AppRun` puts it on `PYTHONPATH`. A user can
drop a new workbench into `mods/` after install, which is why the scripts ship as
loose files rather than embedded in the binary (see `docs/distribution.md` §8).

The directory is intentionally empty for now: the first workbench arrives with the
workbench/runtime slice.
