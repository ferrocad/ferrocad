# Fasteners

A **JS** workbench: ISO 4762 socket-head cap screws.

Each command creates **two** real `Part::Cylinder`s: the shank (`ISO4762_M6x20`) and its
head (`ISO4762_M6x20_head`), the head placed on the shank's tip. Nothing is drawn by the
package: the shapes are the kernel's, which is what makes this a glue package rather than
an engine.

The hex socket is the one part of the specification that is *not* there. A socket is a cut
into the head, and the document API has no boolean verbs — an addon declares what it
creates and what it writes, not a description of how to combine shapes
(`docs/project/tasks/backend-parity.md`, D16). Two primitives give the silhouette; a real
one would need a `cut` verb.

**They are two objects, so they move two ways.** There is no link verb either, so the shank
and the head each carry their own `Placement`: select both and `App_Move` moves the screw,
but grabbing one and dragging it moves only that primitive. A `Part::Compound` over the two
would be the fix, and is not one yet — a container's own `Placement` is not applied by a
recompute, and the compound would be drawn beside the two cylinders rather than replacing
them.

| command | what it does |
| --- | --- |
| `Insert M3 × 10` … `Insert M8 × 30` | create the screw (shank and head) |

Head sizes are ISO 4762's: M3 5.5 × 3, M4 7 × 4, M5 8.5 × 5, M6 10 × 6, M8 13 × 8 mm.

## How it runs

`mods/<Name>/InitGui.js` is the JS half's entry file, the mirror of `InitGui.py`. It is an
ES module exporting `install()`; the host evaluates it before the window boots and reads
`window.__ferrocadJs` once, so `install()` merges into another package's commands rather
than replacing them.

The page is installed automatically when a mod set declares it (B9a):
`--script-host=js` is not needed, and the desktop needs a build with `--features js`.

Every write goes through `window.ferrocad.call`, the JSON API over the document. A host
that refuses writes answers `{"ok": false, …}` and the command returns `false`; nothing is
faked. A refusal on the head leaves the shank in the document — two primitives are two
writes, and there is no transaction verb to wrap them in.
