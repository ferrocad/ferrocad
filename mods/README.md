# FerroCAD workbenches (`mods/`)

Each subdirectory is a workbench, the equivalent of FreeCAD's `Mod/`: Python the
app loads at startup that adds commands, panels and document types, and customizes
the UI.

The packaging step (`cargo xtask bundle`) copies this directory into the
distribution beside `python/`, and `AppRun` puts it on `PYTHONPATH`. A user can
drop a new workbench into `mods/` after install, which is why the scripts ship as
loose files rather than embedded in the binary (see `docs/architecture/distribution.md` §8).

Populate it with `cargo xtask mods`, which fetches selected upstream FreeCAD
workbenches (default `Draft`, at the pinned revision `FERROCAD_FREECAD_TAG`) and
copies them in as loose files. Authored workbenches are part of the tree and are
tracked normally; fetched upstream trees are gitignored (the `.gitignore`
negates `mods/GridOverlay/`, `mods/GridOverlayCoin/` and `mods/AxisOverlay/`,
the authored workbenches). The authored workbenches are
the grid backends (`GridOverlay`, `GridOverlayCoin`) and `AxisOverlay`, the
native axis-cross settings surface added when the axis cross moved out of the
grid capability. The release workflow runs `cargo xtask mods` before packaging.
