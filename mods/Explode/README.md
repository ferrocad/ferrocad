# Explode

A **JS** view tool: spread the model's objects out, and put them back.

| command | what it does |
| --- | --- |
| `Explode` | move each object onto a ring around the origin, offset from where it is |
| `Collapse` | restore every placement **exactly** as it was |

Collapse is exact because `Explode` remembers the placement strings it replaced rather than
dividing by a factor. It is a *view* operation: the addon writes `Placement` through the
document API, so it is undoable and the property pane shows it.

## Why this needed a doorway change

The document API's placement text used to be `(x, y, z)` — the base only, so the rotation was
unreadable and `set` refused a placement outright. `present::format` now prints
`(x, y, z; ax, ay, az; angle)` when the object is rotated, and `coerce` parses both forms, so
this addon can read a placement, rewrite the base and hand the rotation back untouched.

## Running it

`mods/<Name>/InitGui.js` is the JS half's entry file; the page is derived from the mod set
(B9a), so a desktop build with `--features js` loads it with no switch. Writes go through
`window.ferrocad.call`; a host that refuses them answers `{"ok": false, …}` and the command
returns `false`.
