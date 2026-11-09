# Fasteners

A **JS** workbench: ISO 4762 socket-head cap screws.

Each command creates a real `Part::Cylinder` with the shank's radius and length, named
`ISO4762_M6x20` and so on, in one transaction. Nothing is drawn by the package: the shape
is the kernel's, which is what makes this a ten-line glue package rather than an engine.

| command | what it does |
| --- | --- |
| `Insert M3 × 10` … `Insert M8 × 30` | create the screw |

## How it runs

`mods/<Name>/InitGui.js` is the JS half's entry file, the mirror of `InitGui.py`. It is an
ES module exporting `install()`; the host evaluates it before the window boots and reads
`window.__ferrocadJs` once, so `install()` merges into another package's commands rather
than replacing them.

The page is installed automatically when a mod set declares it (B9a):
`--script-host=js` is not needed, and the desktop needs a build with `--features js`.

Every write goes through `window.ferrocad.call`, the JSON API over the document. A host
that refuses writes answers `{"ok": false, …}` and the command returns `false`; nothing is
faked.
