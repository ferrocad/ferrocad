# Iconography: what the UI draws, what we can reuse, and what we must draw

Status: research and decision proposal (2026-11-04). Companion to
[`visual-language.md`](visual-language.md) (the token, type and stroke contract),
[`../../project/tasks/ux-research.md`](../../project/tasks/ux-research.md)
(U3 snap feedback, U6 numeric entry) and
[`../../mods/mod-anatomy.md`](../../mods/mod-anatomy.md) (the `Resources/` convention).

This note answers three questions:

1. What iconography does the code have **today**?
2. What can we take from **upstream FreeCAD** and from **[IconMCP](https://iconmcp.com/)**?
3. Which icons must FerroCAD **draw itself** because neither source states the action honestly?

The short answer: IconMCP is a good source for **chrome and geometric primitives**
(MIT, 16-unit grid, outline style that matches the visual language), upstream is a
good source for **workbench provenance** but not for a coherent picture set, and
**CAD verbs** (extrude, loft, sweep, pocket, workplane, snap modes) do not exist in
either and must be drawn.

---

## 0. What exists today

Until 2026-11-05 there was no iconography decision and no icon name reached the
shell. The inventory below is the starting point; the "as built" column records what
now renders (§6.2).

| Surface | Drawn before | As built (2026-11-05) | Evidence |
| --- | --- | --- | --- |
| App menu | Unicode `☰` text | embedded `menu` glyph; rows carry the command's glyph | `crates/ferrocad_shell_core/src/ui/chrome.rs` |
| Header controls | status dot + text + `▾` | embedded glyph + text + `▾` | `chrome.rs`; `screenshot.png` |
| Action dock | text labels | glyph + text label | `chrome.rs` |
| Active-action bar (tool strip) | `Cancel` / `Finish` words | `close` / `check` glyph + word | `crates/ferrocad_shell_core/src/shell/render.rs` |
| Loadout bar | hotkey + hand label | hotkey + hand glyph + label | `crates/ferrocad_shell_core/src/ui/chrome.rs` |
| Header popovers | `PopoverRow { label, accel, checked, … }` | glyph + label + accel + status dot | `chrome.rs` |
| Radial wheel | text label per wedge; tone only in the inner arc | glyph above the text label | `crates/ferrocad_widgets/src/wheel.rs` |
| Palette | text rows | glyph + text row | `crates/ferrocad_shell_core/src/shell/render.rs` |
| Model tree | `▾` + label + object `type_id` as text | type glyph + label + `type_id` | `crates/ferrocad_shell_core/src/shell/render.rs` |
| Properties panel | rows of `name` + `type_id` | value-kind glyph + `name` + `type_id` | `crates/ferrocad_shell_core/src/shell/render.rs` |
| Dev Tools command list | label + owning surface | glyph + label + owning surface | `crates/ferrocad_shell_core/src/ui/chrome.rs` |
| Dev Tools | **one embedded SVG bug**, 16×16 | unchanged | `crates/ferrocad_widgets/src/icon.rs` |
| Title bar | painted minimize/maximize/close paths | unchanged | `crates/ferrocad_widgets/src/window_chrome.rs:241` |
| NaviCube | 3-D glass cube with `F/R/T` letters | unchanged | `crates/ferrocad_viewport/src/navicube.rs` |

Three facts matter:

- `ferrocad_widgets::icon` is the seed of a real system: embedded SVG bytes, a
  16×16 box, `text_color` ink (`MUTED` / `HOVER_RIM`), a `{key}-icon` debug selector
  and a headless render test. [`icon::named`] resolves an **icon key** for any other
  surface, so a menu row, a dock button and a wheel slot share one asset registry.
- The control glyphs are **embedded SVGs** (`include_bytes!` + `gpui::svg().data()`),
  so there is no `AssetSource`, no filesystem lookup and no network dependency. The
  pinned `bite-gpui` fork also provides `AssetSource`/`svg().path()`, which the larger
  vendored set can use when it lands.
- The command id is the join key. `crates/ferrocad_shell_core/src/ui/command_icons.rs`
  is the curated **command id → icon key** table (the note's alias table in code); a
  command with no row keeps its label and draws no glyph, and a test keeps every
  curated key pointing at a vendored asset. `Pixmap` is now forwarded through the
  bridge and resolved **before** the id, so upstream's own resource works and a mod can
  name a vendored glyph directly. The model tree (`icon_for_type`) and the properties
  panel (`icon_for_property`) are keyed by FreeCAD type instead, because their rows are
  objects and values, not commands.

Until this round the command metadata carried no icon:

- `CommandDef` (`crates/ferrocad_shell_api/src/catalog.rs`) still has no icon field: the
  built-ins resolve by command id through the curated table.
- `CommandInfo` (`crates/ferrocad_shell_api/src/commands.rs`) now carries `pixmap`,
  forwarded from `GetResources()["Pixmap"]`.
- `FreeCADGui._command_list` (`python/FreeCADGui/__init__.py`) forwards `MenuText`,
  `ToolTip`, `Accel`, `Group`, `CmdType` and now `Pixmap`, which is the key that names
  an icon (`"Pixmap": "Draft_Move"`,
  `freecad-upstream/src/Mod/Draft/draftguitools/gui_move.py:57`).

The command id remains the **primary** join key: the ids already travel end to end
(`Draft_Snap_Grid`, `App_Move`), so the curated table works without a mod doing
anything. `Pixmap` is the override, resolved first: a mod that wants a specific glyph
sets `"Pixmap": "cube"`, and upstream commands keep working because their `Pixmap`
values (`Draft_Snap_Endpoint`) are ids the same table knows.

---

## 1. The upstream inventory (FreeCAD 1.1.4)

The pinned fetch is **1.1.4** (`crates/xtask/src/main.rs:64-66`). At that tag the
repository has **1,625 SVG icons under `icons/` directories**, in 26 locations:

| Location | Files | In this checkout | Shipped by `cargo xtask mods` |
| --- | ---: | --- | --- |
| `src/Gui/Icons` | 315 | no (sparse checkout) | no |
| `src/Mod/Sketcher` | 288 | no | no |
| `src/Mod/TechDraw` | 189 | no | no |
| `src/Mod/BIM` | 164 | yes (150 at `33adf61`) | no |
| `src/Mod/Draft` | 110 | yes | **yes** |
| `src/Mod/Fem` | 104 | no | no |
| `src/Mod/CAM` | 89 | no | no |
| `src/Mod/Part` | 76 | yes (17 top-level) | no |
| `src/Mod/Tux` | 57 | no | no |
| `src/Mod/PartDesign` | 55 | no | no |
| `src/Mod/Mesh` | 44 | no | no |
| `src/Mod/Assembly` | 32 | no | no |
| `src/Mod/Spreadsheet` | 19 | no | no |
| `src/Mod/OpenSCAD` | 17 | no | no |
| `src/Mod/Robot` | 14 | no | no |
| `src/Mod/Surface` | 12 | no | no |
| `src/Mod/Measure` | 11 | no | no |
| `src/Mod/Material` | 8 | no | no |
| `src/Mod/Points` | 6 | no | no |
| `src/Mod/MeshPart` | 4 | no | no |
| `src/Mod/Start`, `Inspection`, `ReverseEngineering` | 7 | no | no |
| `src/Mod/Cloud`, `Test` | 2 | no | no |
| Other (the two `_TEMPLATE_` icons) | 2 | no | no |

The table counts the **vector** set. Upstream also keeps 60 legacy PNGs, two ICO/
BMP pairs and one XCF under the same directories; they are not reusable at our
stroke weight and are excluded.

Two consequences are easy to miss:

- **The Draft icons are already in a shipped app.** `cargo xtask mods` copies
  `src/Mod/Draft` into `mods/Draft`, including `Resources/icons/*.svg`, and packaging
  copies `mods/` into the payload on `PYTHONPATH` (`mods/README.md:13`). Reuse is a
  *resolution* problem, not a vendoring problem.
- **The fetched tree is gitignored** (`/mods/*` with negations only for the authored
  mods, `.gitignore:27`). A development checkout without `cargo xtask mods` has no
  Draft icons, so every lookup must fall back to an embedded glyph or the text label.

### Upstream naming is already the command id

Upstream's `GetResources()["Pixmap"]` is the icon key, and it is the command identity:
`Draft_Line.svg`, `Draft_Move.svg`, `Part_Cut.svg`, `Sketcher_ConstrainParallel.svg`.
Of 86 `Draft_*` command ids in the checkout (`33adf61`), **70 have a same-named SVG**;
the 16 misses are tool-group aliases (`Draft_ArcTools`, `Draft_ToggleGrid`, …) that
point at another `Pixmap`. The extra 40 icons are object/tree variants (`_Tree`,
`_on`/`_off`, `DraftWorkbench`).

That gives the icon contract without inventing a vocabulary: **the icon key is the
command id**.

---

## 2. IconMCP: what it is, and what it has

| Property | Value |
| --- | --- |
| Library | 6,481 icons in 58 sets (generic, interface, symbols, editor, tools, industry, architecture, …), re-checked 2026-11-05 |
| Grid | **16 units**, matching our 16 px box |
| Styles | outline (default), solid, duotone |
| Defaults | stroke `1.25`, corner radius `2` — the same weight as `visual-language.md` §3 |
| Colour | `currentColor`, so it takes the UI token directly |
| Licence | **MIT**, Copyright (c) 2026 UI8 |
| Interface | MCP at `https://iconmcp.com/mcp`: `search_icons`, `get_icon`, `list_sets`, `get_set`, `request_icon`, `report_icon` |
| Geometry | each icon is a list of `path`/`line`/`rect`/`poly` elements with `body`/`inner` roles |

**Strengths for FerroCAD.** Chrome and file actions are effectively complete:
`menu`, `grid`, `save`, `folder-open`, `printer`, `undo`, `redo`, `copy`, `paste`,
`trash`, `search`, `settings`, `check`, `close`, `info`, `alert`, `eye`, `lock`,
`chain-link`, `tree-view`, `list`, `terminal`, `camera`, `zoom-fit`, `bug`, `filter`,
`palette`. Geometry primitives and set operations are also strong: `cube`, `cylinder`,
`sphere`, `cone`, `torus`, `prism`, `cuboid`, `frustum`, `hemisphere`, `spiral`,
`spring-coil`, `set-union`, `set-intersection`, `boolean-subtract`, `venn-two`,
`corner-radius`, `bevel-gauge`, `matrix`, `axes-3d`, `perpendicular`, `parallel`,
`angle`, `angle-right`, `intersection`, `anchor-point`, `crosshair`, `ruler`,
`protractor`, `circle-radius`, `measure-tool`, `distance-ruler`, `map-measure-area`,
`volumetric-flask`, `pin`, `physics`, `bezier`, `shape-polyline`, `pattern-crosshatch`,
`square-dashed`, `sparkles`, `paint-bucket`.

**Gaps.** IconMCP does not model CAD verbs or CAD state:

| Missing concept | What IconMCP returned for the plain query |
| --- | --- |
| extrude | **no match** (`weak: true`) |
| loft | `loft-bed`, `dormer-window` |
| sweep | **no match** |
| pocket | `pocket-watch`, `pocket-knife` |
| thickness / shell | `shell` (weather), `terminal` |
| workplane / datum plane | `plane` (an aircraft), `planet` |
| snap midpoint | `api-endpoint`, `network-node` |
| snap near | **no match** |
| offset | `carbon-offset`, `margin` |
| tube | `cream-tube`, `paint-tube` |
| wedge | `prism`, `cheese` |
| view orientation (front/top/right) | `plane-front`, `sea-view`, `human-eye` |

### Word-sense collisions to guard against

IconMCP's search is by meaning, but several CAD words have a stronger everyday sense.
These were observed in the run and must never be accepted blindly:

| Query | Wrong but plausible result | Use instead |
| --- | --- | --- |
| `pad` | `order-pad`, `lily-pad` | draw (extrude) |
| `snap` | `window-snap-left`, `heap-snapshot` | `magnet` |
| `plane` | `plane` (aircraft) | draw (workplane) |
| `array` | `code-array` | `matrix` |
| `point` | `arrow-up-from-dot` | `bullet` |
| `arc` | `crescent`, `rainbow` | `quarter-circle` |
| `finish` | `finish-line`, `race-finish` | `check` |
| `volume` | `volume` (media) | `volumetric-flask` |
| `explode` | no match | `expand` |
| `switch` | no match | `swap-horizontal` |

**This is the central finding: IconMCP covers the *vocabulary of interface*, not the
*vocabulary of modelling*.** It is the right source for the chrome and for solids,
booleans and measurement; it is the wrong source for the verbs that define a CAD tool.

---

## 3. The semantic pass over all 1,625 upstream icons

Method: every SVG name at 1.1.4 was translated (CamelCase split, FreeCAD vocabulary
expanded, e.g. `Trimex` → trim scissors, `ShapeString` → text, `Facebinder` → surface)
and sent to `search_icons` with `limit: 5`. A row counts as "returned a candidate"
when IconMCP did not answer `weak: true`.

| Workbench | Files | Candidate returned | Share |
| --- | ---: | ---: | ---: |
| `Gui` | 315 | 115 | 37% |
| `Sketcher` | 288 | 20 | 7% |
| `TechDraw` | 189 | 51 | 27% |
| `BIM` | 164 | 81 | 49% |
| `Draft` | 110 | 61 | 55% |
| `Fem` | 104 | 0 | 0% |
| `CAM` | 89 | 43 | 48% |
| `Part` | 76 | 39 | 51% |
| `Tux` | 57 | 1 | 2% |
| `PartDesign` | 55 | 19 | 35% |
| `Mesh` | 44 | 20 | 45% |
| `Assembly` | 32 | 3 | 9% |
| `Spreadsheet` | 19 | 5 | 26% |
| `OpenSCAD` | 17 | 1 | 6% |
| `Robot` | 14 | 4 | 29% |
| `Surface` | 12 | 5 | 42% |
| `Measure` | 11 | 2 | 18% |
| `Material` | 8 | 5 | 62% |
| `Points` | 6 | 4 | 67% |
| `MeshPart`, `Start`, `Inspection`, `ReverseEngineering`, `Cloud`, `Test`, templates | 15 | 6 | 40% |
| **Total** | **1,625** | **485** | **29.8%** |

What the other workbenches would need, if they are ever surfaced:

- **Sketcher** (7%) — the generic set has `perpendicular`, `parallel` and `angle`, but
  not `tangent`, `coincident`, `equal`, `symmetric`, `horizontal`, `vertical` or a
  diameter glyph. The 288 files are mostly tool-mode and solver variants; iconifying
  the sketcher is its own project and should wait for the solver work.
- **PartDesign** (35%) — primitives and booleans map cleanly; `pad`, `pocket`,
  `groove`, `revolution`, `loft`, `sweep` and `thickness` are the same draw-list gaps
  as Part.
- **Fem** (0%) — no domain vocabulary in IconMCP. Out of scope.
- **CAM** (48%) — tool and path concepts partially map; the workbench is not fetched.
- **TechDraw** (27%) — `dimension-line`, `section-break` and `projector` cover the
  annotation family; the drafting-specific symbols do not.
- **BIM** (49%) — the `architecture` set helps, but BIM is not in `DEFAULT_MODS`.


The low shares are informative, not a failure of the run: Sketcher is 288 constraint
and tool-mode variants whose names have no everyday equivalent, and FEM is a
domain vocabulary IconMCP does not carry. The number of **distinct concepts** that
matter to FerroCAD is far smaller than 1,625, and that is what the curated table
below covers.

A machine mapping of all 1,625 rows was produced for this research; it is not
committed because a guessed match is not a contract. The curated table is. The pass
is reproducible: split the command name on CamelCase, expand FreeCAD vocabulary
(`Trimex`, `ShapeString`, `Facebinder`, …), then call `search_icons` with
`limit: 5`; a `weak: true` answer is a gap, not a match.

---

## 4. Curated mapping for FerroCAD's own surfaces

**Verdict key**

- **use** — an honest, direct semantic match; take the IconMCP icon.
- **metaphor** — the closest honest picture, but the shape reads as a symbol rather
  than the CAD operation; acceptable only with the text label present.
- **compose** — an IconMCP base plus one drawn marker (for example a circle marker
  on `matrix` for a polar array).
- **draw** — no honest match; draw it in `ferrocad_widgets::icon` on the 16-unit grid.

### Set policy

IconMCP's set is a **taxonomy**, not a renderer: every set is drawn by the same
16-unit outline engine, so `symbols`, `editor` and `generic` icons share a stroke.
The inconsistency is semantic, not painted. Each FerroCAD surface therefore declares
a **primary set**:

| Surface | Primary set | Accepted neighbours |
| --- | --- | --- |
| App chrome | `generic` / `interface` | `files`, `editor`, `media` |
| Draft geometry | `symbols` (shapes) | `editor` (drawing/annotation), `architecture` / `education` (dimensions) |
| Draft edit | `editor` | `arrows` (join/split), `files` (layers), `interface` (arrays) |
| Snap | `symbols` (relations) | `generic` (toggles), `editor` / `interface` (node, crosshair, dimension), `auto` (intersection) |
| Part primitives | `symbols` | `industry` (tube), `science` (helix), `editor` (menu) |
| Part operations | `editor` | `symbols` (common), `industry` (revolve), `tools` (chamfer), `interface` / `files` (section), `arrows` (join/split), `devices` (projection) |
| Measure | `editor` | `symbols` (radius), `education` (angle, inertia), `science` (volume), `travel` (area, centre), `generic` (position) |

A pick outside these families is a **borrow**. The contact sheet labels every
IconMCP pick with its set, so a borrow is visible rather than silent. The policy is
a target, not a claim: the card's set label is the current truth. Where a same-family
icon was as good, it replaced the borrow (`line-segment`, `label-pill`,
`boolean-intersect`, `measure-tool`, `wave-line`). The 27 requests name the target
set so the remaining gaps close in the right family.

### App chrome and navigation

**How to read the table.** The first column is the FerroCAD control; **Surface** says
where the shell renders it today (`header`, `dock`, `wheel`, `app menu`, `panel`,
`dialog`, `tree`, `settings`, `title`, `navicube`). A `—` means the shell does **not**
render it yet: the row is upstream inventory kept as a candidate, not a requirement.
The third column is FreeCAD's icon file (provenance); the fourth is the IconMCP
candidate.

| FerroCAD control | Surface | Upstream icon (1.1.4) | IconMCP | Verdict |
| --- | --- | --- | --- | --- |
| Application menu | title | — | `menu` | use |
| Grid toggle | header | — | `grid-4x4` | use |
| Axis cross | header | `Std_AxisCross` | `axes-3d` | use |
| Workplane | header | `Std_Plane` | — | draw |
| Transform gizmo | header | `Std_TransformManip` | `axes-3d + move` | compose |
| Snap (global) | header | `Draft_Snap_Lock` | `magnet + lock` | compose |
| Open | app menu | `document-open` | `folder-open` | use |
| Save | app menu | `document-save` | `save` | use |
| Save all | — | `Std_SaveAll` | `save-all` | use |
| Print | — | `document-print` | `printer` | use |
| Undo | — | `edit-undo` | `undo` | use |
| Redo | — | `edit-redo` | `redo` | use |
| Copy | — | `edit-copy` | `copy` | use |
| Cut | — | `edit-cut` | `scissors` | use |
| Paste | — | `edit-paste` | `paste` | use |
| Delete | wheel | `edit-delete` | `trash` | use |
| Search commands | dock | — | `search` | use |
| Preferences | settings | `preferences-general` | `settings` | use |
| Dev Tools | app menu | — | `bug` | use |
| Confirm | dialog | `edit_OK` | `check` | use |
| Cancel | dialog | `edit_Cancel` | `close` | use |
| Info | dialog / status | `info` | `info` | use |
| Warning | dialog / status | `Warning` | `alert` | use |
| Visibility on | tree | `TreeItemVisible` | `eye` | use |
| Visibility off | tree | `TreeItemInvisible` | `eye-off` | use |
| Object lock | — | — | `lock` | use |
| Link | — | `Link` | `chain-link` | use |
| Unlink | — | `Unlink` | `unlink` | use |
| Model tree | panel | `tree-doc-single` | `tree-view` | use |
| Properties | panel | `document-properties` | `list` | use |
| Python console | panel | `utilities-terminal` | `terminal` | use |
| Screenshot | — | `Std_ViewScreenShot` | `camera` | use |
| Zoom fit | — | `zoom-all` | `zoom-fit` | use |
| Zoom in | — | `zoom-in` | `zoom-in` | use |
| Zoom out | — | `zoom-out` | `zoom-out` | use |
| Isometric view | navicube | `view-isometric` | `cube` | metaphor |
| Front view | navicube | `view-front` | — | draw |
| Top view | navicube | `view-top` | — | draw |
| Right view | navicube | `view-right` | — | draw |
| Rear view | navicube | `view-rear` | — | draw |
| Select all | — | `edit-select-all` | `select-all` | use |
| Box select | — | `edit-select-box` | `rectangle-select` | use |
| Clear selection | — | `clear-selection` | `deselect` | use |
| Filter | — | — | `filter` | use |
| Colour | settings | `Std_SetAppearance` | `palette` | use |
| Add | — | `list-add` | `plus` | use |
| Remove | — | `list-remove` | `minus` | use |
| Edit | — | `edit-edit` | `edit` | use |
| Macro editor | — | `MacroEditor` | `code` | use |
| Spreadsheet | — | `TextDocument` | `table` | metaphor |

Seven corrections matter:

- **Grid** was a 2 × 2 `grid`; upstream `Draft_Grid` is a 4 × 4 lattice, so the
  header uses `grid-4x4`.
- **Gizmo** was `free-transform` (a 2-D corner handle). `axes-3d` is the closest
  3-D triad, but it collides with Axis cross; a dedicated `transform gizmo` request
  is filed (§5). The marker is `move`, not `hand-grab`: the hand's fingers merge
  into a blob at 16 px, and the four-way arrows still read at toolbar size.
- **Snap (global)** is `Draft_Snap_Lock`, which upstream describes as "enables or
  disables snapping globally" (`gui_snaps.py:70-72`), not a padlock. The glyph is a
  composition, `magnet + lock`.
- **Cut is the clipboard verb.** `scissors` is the conventional cut glyph; the CAD
  solid operation is Part `Cut`, mapped to `boolean-subtract` below.
- **`debug-start` is not a bug.** Upstream's `debug-start` is a run/debug action.
  FerroCAD's app-menu button opens the Dev Tools window, so `bug` is right for it.
  A future Debug Start command should use `play`.
- **`Tree_Annotation` is an object icon, not the model tree.** The panel is
  `tree-doc-single`; `Tree_Annotation` is the tree glyph for an annotation object.
  If annotation objects are surfaced, they use `label-pill`.
- **Clear, not invert.** The facade exposes `clearSelection` and the shell has no
  invert command; `clear-selection` maps to `deselect`. If invert is added later, it
  gets its own row and `invert-selection`.

### Draft geometry

| Need | Upstream icon (1.1.4) | IconMCP | Verdict |
| --- | --- | --- | --- |
| Line | `Draft_Line` | `line-segment` | use |
| Polyline / wire | `Draft_Wire` | `shape-polyline` | use |
| Arc | `Draft_Arc` | — | draw |
| Arc through 3 points | `Draft_Arc_3Points` | — | draw |
| Circle | `Draft_Circle` | `circle` | use |
| Ellipse | `Draft_Ellipse` | `oval` | use |
| Rectangle | `Draft_Rectangle` | `rectangle` | use |
| Polygon | `Draft_Polygon` | `polygon` | use |
| B-spline | `Draft_BSpline` | `wave-line` | use |
| Bezier curve | `Draft_BezCurve` | `bezier` | use |
| Point (toolbar) | `Draft_Point` | `bullet` | use |
| Point (tree) | `Draft_Dot` | `bullet` | use |
| Text (annotation) | `Draft_Text` | `text-box` | use |
| 3-D text | `Draft_ShapeString` | `letter-block` | use |
| Dimension (linear) | `Draft_Dimension` | `dimension-line` | use |
| Dimension (angular) | `Draft_DimensionAngular` | `dimension-line + angle` | compose |
| Dimension (radius) | `Draft_DimensionRadius` | `dimension-line + circle-radius` | compose |
| Label | `Draft_Label` | `tag` | use |
| Hatch | `Draft_Hatch` | `pattern-crosshatch` | use |
| Facebinder | `Draft_Facebinder` | — | draw |

### Draft edit, layers and groups

| Need | Upstream icon (1.1.4) | IconMCP | Verdict |
| --- | --- | --- | --- |
| Move | `Draft_Move` | `move` | use |
| Rotate | `Draft_Rotate` | `rotate-object` | use |
| Scale | `Draft_Scale` | `resize-handle` | use |
| Mirror | `Draft_Mirror` | `copy + flip-horizontal` | compose |
| Offset | `Draft_Offset` | — | draw |
| Clone | `Draft_Clone` | `sheep` | metaphor |
| Array | `Draft_OrthoArray` | `grid-4x4` | use |
| Polar array | `Draft_PolarArray` | `copy + circle-dotted` | compose |
| Path array | `Draft_PathArray` | `copy + shape-polyline` | compose |
| Point array | `Draft_PointArray` | `copy + bullet` | compose |
| Trim | `Draft_Trimex` | `cut-line` | use |
| Extend | `Draft_Trimex` | — | draw |
| Fillet | `Draft_Fillet` | `corner-radius` | use |
| Join | `Draft_Join` | `merge` | use |
| Split | `Draft_Split` | `split` | use |
| Upgrade | `Draft_Upgrade` | `merge` | use |
| Downgrade | `Draft_Downgrade` | `split` | use |
| Stretch | `Draft_Stretch` | `expand-diagonal` | metaphor |
| Heal | `Draft_Heal` | `wrench` | metaphor |
| Wipe | `Draft_Wipe` | `trash` | use |
| Finish | `Draft_Finish` | `check` | use |
| Apply | `Draft_Apply` | `check` | use |
| Construction | `Draft_ToggleConstructionMode` | `square-dashed` | metaphor |
| Close polyline | `Draft_Lock` | `shape-polyline + lock` | compose |
| Layer | `Draft_Layer` | `layers` | use |
| New layer | `Draft_NewLayer` | `layers-plus` | use |
| Add to layer | `Draft_AddToLayer` | `layers-plus` | use |
| Group | `Draft_AddToGroup` | `group` | use |
| Select group | `Draft_SelectGroup` | `group` | use |
| Select plane | `Draft_SelectPlane` | — | draw |
| Flip dimension | `Draft_FlipDimension` | `flip-horizontal` | use |
| Sub-element highlight | `Draft_SubelementHighlight` | `highlighter` | metaphor |
| Slope | `Draft_Slope` | — | draw |
| Switch mode | `Draft_ToggleDisplayMode` | `swap-horizontal` | metaphor |
| VisGroup | `Draft_VisGroup` | `group` | compose |
| AutoGroup | `Draft_AutoGroup` | `group` | use |

The task-panel buttons `Draft_Wipe`, `Draft_Finish`, `Draft_Apply` and `Draft_Lock`
(close polyline) are **icon names, not commands**, and `Draft_VisGroup` /
`Draft_NewLayer` are not registered in 1.1.4, so none reaches the command table.
`Array` is registered as `Draft_OrthoArray`, and the two mode toggles are registered
under their command ids with those pixmaps: `Draft_ToggleDisplayMode`
(`Pixmap: Draft_SwitchMode`) and `Draft_ToggleConstructionMode`
(`Pixmap: Draft_Construction`).

`Draft_Lock` is not a padlock: upstream uses it for the task panel's
close-line button (`DraftGui.py:569,806-809`, "Finishes and closes the current
line"). The glyph is a closed polyline, hence `shape-polyline + lock`.

### Snap modes

| Need | Upstream icon (1.1.4) | IconMCP | Verdict |
| --- | --- | --- | --- |
| Snap | `Draft_Snap` | `magnet` | use |
| Endpoint | `Draft_Snap_Endpoint` | `anchor-point` | use |
| Midpoint | `Draft_Snap_Midpoint` | — | draw |
| Center | `Draft_Snap_Center` | `cursor-crosshair` | use |
| Intersection | `Draft_Snap_Intersection` | `intersection` | use |
| Perpendicular | `Draft_Snap_Perpendicular` | `perpendicular` | use |
| Parallel | `Draft_Snap_Parallel` | `parallel` | use |
| Angle | `Draft_Snap_Angle` | `angle` | use |
| Extension | `Draft_Snap_Extension` | — | draw |
| Near | `Draft_Snap_Near` | — | draw |
| Grid | `Draft_Snap_Grid` | `grid` | use |
| Ortho | `Draft_Snap_Ortho` | `angle-right` | use |
| Working plane | `Draft_Snap_WorkingPlane` | — | draw |
| Lock | `Draft_Snap_Lock` | `lock` | use |
| Dimensions | `Draft_Snap_Dimensions` | `ruler` | use |
| Special | `Draft_Snap_Special` | `star` | use |

### Part primitives

| Need | Upstream icon (1.1.4) | IconMCP | Verdict |
| --- | --- | --- | --- |
| Box | `Part_Box_Parametric` | `cube` | use |
| Cylinder | `Part_Cylinder_Parametric` | `cylinder` | use |
| Sphere | `Part_Sphere_Parametric` | `sphere` | use |
| Cone | `Part_Cone_Parametric` | `cone` | use |
| Torus | `Part_Torus_Parametric` | `torus` | use |
| Tube | `Part_Tube_Parametric` | `pipe-flange` | metaphor |
| Prism | `Part_Prism_Parametric` | `prism` | use |
| Wedge | `Part_Wedge_Parametric` | `circle-sector` | metaphor |
| Helix | `Part_Helix_Parametric` | `spring-coil` | use |
| Spiral | `Part_Spiral_Parametric` | `spiral` | use |
| Ellipsoid | `Part_Ellipsoid_Parametric` | `sphere` | metaphor |
| Plane datum | `Part_Plane_Parametric` | `workplane` | use |
| Line datum | `Part_Line_Parametric` | `line-segment` | use |
| Point datum | `Part_Point_Parametric` | `bullet` | use |
| Primitives menu | `Part_Primitives` | `shapes` | use |

The second column is FreeCAD's `Pixmap` file, **not** the command id. In 1.1.4 the GUI
primitive commands are `Part_Box`, `Part_Cylinder`, `Part_Sphere`, `Part_Cone`,
`Part_Torus`, `Part_Tube` (in `Part/BasicShapes/CommandShapes.py`) and the
`Part_Primitives` dialog; the datum commands are `Part_DatumPlane`, `Part_DatumLine`
and `Part_DatumPoint` (`Command.cpp`). **Prism, Wedge, Helix, Spiral and Ellipsoid
have no GUI command** — they exist as `Part::*` objects, so they are covered by the
model tree's type table instead. Plane datum reuses the authored `workplane` glyph, so
it leaves the draw list.

### Part operations

| Need | Upstream icon (1.1.4) | IconMCP | Verdict |
| --- | --- | --- | --- |
| Extrude | `Part_Extrude` | — | draw |
| Revolve | `Part_Revolve` | `lathe` | metaphor |
| Loft | `Part_Loft` | — | draw |
| Sweep | `Part_Sweep` | — | draw |
| Pocket | — | — | draw |
| Hole | — | — | draw |
| Fillet | `Part_Fillet` | `corner-radius` | use |
| Chamfer | `Part_Chamfer` | `bevel-gauge` | metaphor |
| Thickness | `Part_Thickness` | — | draw |
| Union (fuse) | `Part_Fuse` | `union` | use |
| Cut | `Part_Cut` | `boolean-subtract` | use |
| Common | `Part_Common` | `boolean-intersect` | use |
| Section | `Part_Section` | `section` | use |
| Cross sections | `Part_CrossSections` | `layers` | metaphor |
| Compound | `Part_Compound` | `group` | use |
| Compound filter | `Part_CompoundFilter` | `filter` | use |
| Explode compound | `Part_ExplodeCompound` | `expand` | metaphor |
| Shape builder | `Part_Builder` | `shapes` | use |
| Make face | `Part_MakeFace` | — | draw |
| Make solid | `Part_MakeSolid` | — | draw |
| Refine shape | `Part_RefineShape` | `sparkles` | metaphor |
| Colour face | `Part_ColorPerFace` | `paint-bucket` | use |
| Projection on surface | `Part_ProjectionOnSurface` | `projector` | metaphor |
| Reverse shape | `Part_ReverseShape` | `flip-horizontal` | use |
| Mirror | `Part_Mirror` | `copy + flip-horizontal` | compose |
| Scale | `Part_Scale` | `resize-handle` | use |
| Check geometry | `Part_CheckGeometry` | `check` | use |
| Defeaturing | `Part_Defeaturing` | `eraser` | use |
| Boolean fragments | `Part_BooleanFragments` | `split` | use |
| Slice apart | `Part_SliceApart` | `split` | use |
| Join connect | `Part_JoinConnect` | `merge` | use |
| Join embed | `Part_JoinEmbed` | `merge` | use |
| Join cutout | `Part_JoinCutout` | `merge` | metaphor |
| Join bypass | — | `merge` | metaphor |

Every id above is the registered command (`Command(...)` in `Part/Gui`, `addCommand` in
`Part/BOPTools` and `Part/CompoundTools`). The note previously carried four `Pixmap`
names instead: `Part_Shapebuilder` → **`Part_Builder`**, `Part_Refine_Shape` →
**`Part_RefineShape`**, `Part_ColorFace` → **`Part_ColorPerFace`**, `Part_Reverse_Shape`
→ **`Part_ReverseShape`**. `Part_JoinBypass` is not registered in 1.1.4, so it has no
command id. **Pocket** and **Hole** are PartDesign verbs, not Part commands, and no Part
extrude/loft/sweep/thickness command has an honest glyph yet, so those rows stay on the
draw list.

Mirror bakes the note's `copy + flip-horizontal` composition into the same `mirror`
asset the Draft row uses.

### Measure

| Need | Upstream icon (1.1.4) | IconMCP | Verdict |
| --- | --- | --- | --- |
| Distance | `Measurement-Distance` | `measure-tool` | use |
| Angle | `Measurement-Angle` | `protractor` | use |
| Radius | `Measurement-Radius` | `circle-radius` | use |
| Area | `Measurement-Area` | `map-measure-area` | use |
| Volume | `Measurement-Volume` | `volumetric-flask` | use |
| Centre of mass | `Measurement-CenterOfMass` | `crosshair` | use |
| Position | `Measurement-Position` | `pin` | use |
| Inertia | `Measurement-Inertia` | `physics` | use |

In 1.1.4 the Measure module registers **one** command, `Std_Measure`
(`Measure/Gui/Command.cpp`); the eight `Measurement-*` files are the icons of its
measurement types (`Measure/Gui/Resources/icons`), not command ids. This table is
therefore the measurement **mode** vocabulary, kept for a future measure panel; only
`Std_Measure` is wired today (`measure-tool`).

**Totals: 179 needs — 125 `use`, 19 `metaphor`, 11 `compose`, 24 `draw`.**

### Second semantic review (2026-11-05)

The rendered sheet (upstream icon + candidates at 32 px) resolved the remaining
word-sense questions:

- **Dev Tools has no upstream equivalent.** FreeCAD always shows its panes, so
  `debug-start` was the wrong provenance; the row is `—` and the glyph is a bug.
- **Gizmo vs axis cross.** The gizmo is `axes-3d + move`; the axis cross stays the
  plain `axes-3d`. The marker is the same `move` glyph the Move verb uses, which is
  honest: the gizmo is how that verb is applied to the axes. The first marker was
  `hand-grab`, but at 16 px its fingers merge into a disc (checked at 1×, 2× and 4×),
  so it distinguished the two buttons only by adding noise.
- **Point vs Dot.** They are one concept: `Draft_Point` is the toolbar command and
  `Draft_Dot` is the tree-view icon for the same point object. The rows are named
  "Point (toolbar)" and "Point (tree)".
- **Text vs 3-D text.** Text is a 2-D annotation (`text-box`); Shape From Text is a
  3-D shape (`letter-block`, the block-letter metaphor).
- **Dimension family.** Angular and radius dimensions compose on `dimension-line`
  with `angle` and `circle-radius` markers; dedicated `dimension angle` and
  `dimension radius` requests are filed.
- **Label.** `tag` exists and is a better metaphor than `label-pill`; it is a
  generic-set borrow.
- **Scale.** `resize-handle` is the 2-D corner scale; Stretch uses `expand-diagonal`.
- **Mirror vs flip.** Mirror is `copy + flip-horizontal`; flip alone stays for Flip
  dimension and Reverse shape.
- **Clone.** `sheep` is the deliberate clone metaphor (animals set), distinct from
  `copy`.
- **Trim.** `cut-line` (a line with a cut) replaces scissors; Extend stays a request.
- **Arrays.** Rectangular stays `grid-4x4`; polar, path and point are a generic
  `copy` base plus `circle-dotted`, `shape-polyline` and `bullet` markers.



These rows were checked on a rendered contact sheet at 28 px and at toolbar size
(16 px), not only by name. That review changed a dozen first-pass guesses:
`Line` moved from `cut-line` to `line-segment`; `Arc` from `quarter-circle` to
**draw** (a quarter disc is not an arc); `Array` from `matrix` to `grid-4x4`;
`Polar array` and `VisGroup` gained their compose bases (`grid-4x4 + circle`,
`group + eye`); `Point array` moved to `app-grid`; `Layer` from `layers-panel` to
the stacked `layers`; snap `Intersection` to the crossing-roads `intersection`;
`Union` from `set-union` to `union`; `Common` to `boolean-intersect`; `Section` /
`Cross sections` from the typographic `section-break` pair to `section` and
`layers`. A second, category-consistency pass also moved `Label` to `label-pill`,
`Distance` to `measure-tool`, the datum line to `line-segment`, and the B-spline to
`wave-line`, so each sits in its surface's primary set (see below). The lesson: a
name match is a hypothesis; the picture is the evidence.

---

## 5. The draw list

These are the drawings FerroCAD owns. Each is a 16-unit embedded SVG in
`ferrocad_widgets::icon`, a 1.25 px stroke, round caps, one ink token — the same
recipe as `bug`. The workplane and the three snap glyphs are authored:
`workplane.svg`, `midpoint.svg` (a half-tick on the segment), `extension.svg` (a
dashed continuation from a known endpoint) and `near.svg` (a proximity asterisk).
The rest are open.

| Group | Glyphs |
| --- | --- |
| Planes | workplane, select plane, plane datum, snap working plane |
| View orientation | front, top, right, rear |
| Geometry | arc, arc through 3 points, facebinder |
| Edit | offset (concentric outline), extend (line with arrow), slope (raised wedge) |
| Snap | midpoint (half-tick), extension (dashed continuation), near (proximity asterisk) |
| Solid verbs | extrude, loft, sweep, pocket, hole, thickness, make face, make solid |

Two **compose** pairs rather than new drawings:

- Polar array = `matrix` + a drawn radial marker.
- VisGroup = `group` + `eye`.

That is 26 drawings and 11 compositions, instead of 179.

### The composition rule for solid verbs

A drawn CAD verb should be **the operation, not the object**:

| Verb | Composition on the 16-unit grid |
| --- | --- |
| Extrude | profile square + a bold up arrow + a ghost top face |
| Pocket | same, arrow into the body |
| Loft | two offset profiles + two joining side lines |
| Sweep | a profile circle + a curved path arrow |
| Hole | a circle with a centre tick and a depth tick |
| Thickness | a closed outline and its inset twin |
| Make face | a closed wire with a filled interior |
| Make solid | a wire becoming a shaded cube |

The arrows reuse the existing `AXIS_X/Y/Z` and `PUSH_PULL_COLOR` semantics rather than
introducing new tones. The ghost face is the existing glass-shade ink at low alpha.

### Request log (filed 2026-11-05)

The 26 draw-list needs collapse to **23 requests**: `select plane`, `plane datum` and
`snap working plane` reuse the `workplane` glyph. Four further requests separate
colliding or composed glyphs and complete the dimension family: `transform gizmo`
(because `axes-3d` is also the axis cross), `closed polyline`, `dimension angle`
and `dimension radius`. **27 requests in total.** `request_icon` has no category
argument, so each `why` names the primary set from §4 and its neighbours; the ask is
for the glyph to land in that family, not merely for a picture.

| Request | Target set |
| --- | --- |
| workplane | `symbols` |
| front view | `symbols` |
| top view | `symbols` |
| right view | `symbols` |
| rear view | `symbols` |
| arc | `symbols` |
| arc through three points | `symbols` |
| facebinder | `symbols` |
| snap midpoint | `symbols` |
| snap extension | `symbols` |
| snap nearest | `symbols` |
| offset outline | `editor` |
| extend line | `editor` |
| slope angle | `editor` |
| extrude solid | `editor` |
| pocket cavity | `editor` |
| loft profiles | `editor` |
| sweep along path | `editor` |
| shell solid | `editor` |
| hole | `editor` |
| make face | `editor` |
| make solid | `editor` |
| transform gizmo | `symbols` |
| closed polyline | `symbols` |
| spline curve | `symbols` |
| dimension angle | `architecture` |
| dimension radius | `architecture` |

All 27 were accepted on 2026-11-05; IconMCP says requests are usually drawn within
a day. Re-run the search before adopting one, because the delivered glyph may differ
from the sketch.

**Re-check, 2026-11-05.** The catalogue grew from **6,462 to 6,481** icons; the 19
additions are unrelated to the 27 requests (bank settings, bindle, data logger,
decision, dungeon, fish scale, delete branch, green energy, kilometre post, laptop
settings, notepad, parking bay, plate compactor, power steering, pumice stone,
`terminal-x`, tiltrotor, `utility-knife`, wildlife protection). Re-searching every
requested concept returned no new match, so 23 requests remain open (the spline
request and the three snap draws were withdrawn on 2026-11-06; see below).
`utility-knife` (`home`) is the only newcomer near this table; it was considered for
Cut and Trim but rejected: clipboard Cut is the conventional `scissors`, and the CAD
Trim is the line-with-a-cut `cut-line`, not a blade.

**Design pass, 2026-11-06.** Re-reviewing the `symbols` geometry set (325 icons, all
already indexed) resolved two rows without a new request:

- **B-spline** adopts `wave-line`, a smooth curve, instead of waiting for the
  `spline curve` request. The row was still `draw` even though the second review's
  prose already said `wave-line`; that is corrected here and the request withdrawn.
- **Polar array**'s marker becomes `circle-dotted`, a ring of placements, rather than a
  plain `circle` that only repeated the wedge tint.
- **Midpoint, extension and near are drawn locally** (`midpoint.svg`,
  `extension.svg`, `near.svg`) rather than requested, and are wired to
  `Draft_Snap_Midpoint`, `Draft_Snap_Extension` and `Draft_Snap_Near`. Working plane
  already reuses the authored `workplane`. Those three requests are withdrawn, so
  **23 remain open**.

`quarter-circle`, `circle-segment` and `semicircle` were checked again for **Arc** and
still rejected: the first two carry a radius or chord, and the third is a half disc
with a straight diameter, so Arc stays on the draw list.

**Re-check, 2026-11-06.** The catalogue grew **6,515 → 6,590**: 76 added, 1 removed
(`arrow-down-thick`, unused here). None of the additions is CAD-adjacent — the nearest
are `measuring-wheel` (`industry`) and `photo-rotate` (`photo`) — and re-searching the
26 open requests returned no new match. The `symbols`, `editor`, `interface`, `generic`
and `files` sets are unchanged, so all 115 icon names in this note and all 79 vendored
assets still resolve. The snapshot is refreshed to 6,590.

### Contact sheets

The rendered comparison is checked in beside the design notes. Every card labels the
IconMCP **set**, so a borrow from another family is visible at a glance.

| Surface | Sheet |
| --- | --- |
| App chrome and navigation | [`gallery-chrome.png`](../images/iconography/gallery-chrome.png) |
| Draft geometry | [`gallery-draft-geometry.png`](../images/iconography/gallery-draft-geometry.png) |
| Draft edit, layers and groups | [`gallery-draft-edit.png`](../images/iconography/gallery-draft-edit.png) |
| Snap modes | [`gallery-snap.png`](../images/iconography/gallery-snap.png) |
| Part primitives | [`gallery-part-primitives.png`](../images/iconography/gallery-part-primitives.png) |
| Part operations | [`gallery-part-ops.png`](../images/iconography/gallery-part-ops.png) |
| Measure | [`gallery-measure.png`](../images/iconography/gallery-measure.png) |
| Licence and provenance | [`CREDITS.md`](../images/iconography/CREDITS.md) |

---

## 6. Integration design

### 6.1 The key: command id, with upstream `Pixmap` as the bridge

Adopt upstream's own resource key rather than inventing one:

1. `FreeCADGui._command_list` forwards `"pixmap": resources.get("Pixmap")`.
2. `CommandInfo` and `catalog.json` gain `pixmap`.
3. The shell resolves an icon name in this order:

```text
Pixmap (or command id)
  → curated alias table (Draft_Line → line → cut-line)
  → vendored IconMCP icon
  → embedded core glyph
  → upstream <Pixmap>.svg in mods/*/Resources/icons (provenance fallback)
  → no icon; the label keeps the slot width
```

The last fallback matters: a third-party mod that ships no icon must still produce a
legible row, and a development checkout without `cargo xtask mods` must not lose the
Draft icons it can draw.

### 6.2 Two tiers, deliberately

- **Embedded SVG core** (`ferrocad_widgets::icon`): the 26 draw-list glyphs plus the
  ~20 chrome glyphs that must never fail to load (`menu`, `check`, `close`, `grid`,
  `magnet`, `lock`, `eye`, `search`). Each asset is `include_bytes!`-embedded and
  rendered by `gpui::svg().data()`, with `text_color` as the ink: no asset source, no
  filesystem lookup, deterministic at any DPR. The bug and the five header controls
  use it. **As built (2026-11-05):** the header's `grid`, `axes`, `workplane`, `gizmo`
  and `snap` are embedded SVGs in `crates/ferrocad_widgets/assets/icons/` at 16 px.
  `grid` and `axes` are the IconMCP `grid-4x4` and `axes-3d`; `gizmo` composes
  `axes-3d + move`; `snap` composes `magnet + lock`; `workplane` is the authored
  draw-list glyph. The ink carries the state — `MUTED` off, `HOVER_RIM` on — so there
  is no separate state dot.

  **Surfaces (2026-11-05).** `icon::named(key, size, ink)` is the resolver the rest of
  the chrome uses, and `ui::command_icons` supplies the key: `resolve(command)` first
  honours a vendored `Pixmap`, then the curated command-id table, then the id itself;
  `icon_for_type(type_id)` covers the model tree.

  | Surface | Glyph | Size |
  | --- | --- | --- |
  | App menu button | `menu`, or `☰` only if the asset is missing | 16 px |
  | App menu / header popover rows | `resolve(row command)` | 14 px (`icon::ROW`) |
  | Action dock buttons | `resolve(command)`, followed by the label | 14 px |
  | Action dock Search | `search` | 14 px |
  | Active-action bar (`Cancel` / `Finish`) | `close` / `check` | 14 px |
  | Loadout bar slots | `move` / `rotate-object` (by gizmo mode), `arrow-up-down`, `corner-radius`, `select-pointer` | 14 px (`FG` idle, `CHROME_ACTIVE` equipped) |
  | Command palette rows | `resolve(command)`, before the label | 14 px |
  | Model tree rows | `icon_for_type(type_id)`, before the label | 14 px (`FG` when selected, else `MUTED`) |
  | Properties panel rows | `icon_for_property(type_id)`, or the row command's glyph | 12 px (`FG` when editable, else `MUTED`) |
  | Dev Tools command list | `resolve(command)`, before the label | 14 px |
  | Radial wheel slots | `resolve(command)`, above the label | 14 px (`icon::WHEEL`) |

  The wheel grew with the glyph: a 30 px deadzone, a 92 px outer ring and a 62 px label
  ring (`SIZE` 200), so the icon + label chip is no longer squeezed into the wedge.

  The curated command rows are the built-ins (`App_Move` → `move`, `App_Delete` →
  `trash`, `App_Open` → `folder-open`, …), the dock's creation verbs (`Toy_MakeSlab` →
  `cube`, `Toy_Polyline` → `shape-polyline`, `App_SaveDatumPlane` → `layers-plus`), the
  snap modes (`Draft_Snap_Endpoint` → `anchor-point`, …), the **Draft geometry slice**
  (`Draft_Line` → `line-segment`, `Draft_Wire` → `shape-polyline`, `Draft_BezCurve` →
  `bezier`, `Draft_Dimension` → `dimension-line`, `Draft_Hatch` →
  `pattern-crosshatch`, …), the **Draft edit/layers/groups slice** (`Draft_Move` →
  `move`, `Draft_Scale` → `resize-handle`, `Draft_Trimex` → `cut-line`, `Draft_Layer` →
  `layers`, …) and the **Part primitives/datums slice** (`Part_Box` → `cube`,
  `Part_Tube` → `pipe-flange`, `Part_DatumPlane` → `workplane`, `Part_DatumLine` →
  `line-segment`, …) and the **Part operations slice** (`Part_Fuse` → `union`,
  `Part_Cut` → `boolean-subtract`, `Part_Common` → `boolean-intersect`, `Part_Fillet` →
  `corner-radius`, `Part_Mirror` → the baked `mirror`, …), the **measure
  command** (`Std_Measure` → `measure-tool`) and the **authored snap draws**
  (`Draft_Snap_Midpoint` → `midpoint`, `Draft_Snap_Extension` → `extension`,
  `Draft_Snap_Near` → `near`; `Draft_Snap_WorkingPlane` reuses `workplane`).

  Every workbench id was checked against upstream: `addCommand` in
  `freecad-upstream/src/Mod/Draft/draftguitools`, and `Command(...)` in
  `src/Mod/Part/Gui` and `BasicShapes/CommandShapes.py`. Several note ids turned out to
  be `Pixmap` file names rather than commands and have no row: the Draft view-provider
  names (`Draft_Dot`, `Draft_DimensionAngular`, `Draft_DimensionRadius`) and the
  task-panel buttons (`Draft_Wipe`, `Draft_Finish`, `Draft_Apply`, `Draft_Lock`), plus
  the removed `Draft_VisGroup`/`Draft_NewLayer`. The mode toggles are registered as
  `Draft_ToggleDisplayMode` and `Draft_ToggleConstructionMode` (their `Pixmap` names
  are `Draft_SwitchMode`/`Draft_Construction`), so those two are curated. In Part,
  Prism, Wedge, Helix, Spiral and Ellipsoid have no GUI command and are covered by the
  tree table. Arc, Arc through 3 points and Facebinder stay on the draw list; the snap
  modes the note draws (midpoint, origin, vertex, extension, near) keep their label
  until that glyph exists.

  **Compositions are baked.** The note's `compose` rows (`Mirror` =
  `copy + flip-horizontal`, the three arrays, `VisGroup`, the dimension pair) ship as
  one SVG named after the verb (`mirror`, `polar-array`, `path-array`, `point-array`),
  with the marker's stroke widened to `1.25 / 0.5` so it keeps the base's weight. A
  runtime overlay was tried first and rejected: gpui's SVG element has no
  `stroke-width` override, so a half-size marker draws at ~0.55 px at toolbar size and
  the marker nearly vanishes.

  The tree table is category-level: exact primitives (`Part::Cylinder` → `cylinder`,
  `Part::Torus` → `torus`, …) then `Sketcher::*` → `pencil-ruler`, `Draft::*` →
  `shape-polyline`, `Spreadsheet::*` → `table`, `App::Link*` → `chain-link`,
  `*Group` → `group`, any other `Part::*` → `cube` and `App::*` → `shapes`. A type the
  table has never seen keeps its label alone rather than guessing.

  The property table is by value kind, most-derived first (a `PropertyLength` and a
  `PropertyAngle` both derive from `PropertyQuantity`): length/distance/area/volume →
  `ruler`, angle → `angle`, bool → `toggle-on`, colour → `palette`, vector/placement/
  rotation → `axes-3d`, link → `chain-link`, enumeration → `list`, string → `type`,
  integer/float/quantity → `hash`. A row that runs a command (the settings card's
  reset) takes that command's glyph instead; an unclassified property keeps the space
  so the names stay aligned.
- **Vendored IconMCP set**: the 123 `use` icons, as outline SVGs with their MIT
  licence, rasterised by `gpui::svg()` through an `AssetSource`. Vendored, not fetched:
  the build must work offline and the same binary must render the same icons in CI.

Upstream SVGs stay **verbatim and in place** (`mods/Draft/Resources/icons`). They are
never recoloured or inlined: their metadata is mixed (see §7), and a recoloured
derivative is a different licensing question from shipping the file as FreeCAD does.

### 6.3 Tokens, geometry and states

- **Grid** — 16 × 16 logical px, 1 px safe margin; IconMCP's 16-unit grid maps 1:1.
- **Stroke** — 1.25 px outline only. Solid and duotone are rejected: the visual
  language is dielectric and never uses an opaque fill for chrome.
- **Ink** — `currentColor` from the UI token: `ui::FG` at rest, `ui::MUTED` for
  secondary, `ui::HOVER_RIM` on hover, `ui::CHROME_ACTIVE` when checked,
  `ui::SUBTRACT_COLOR` for destructive, 45 % `MUTED` when disabled. No new palette.
- **Sizes** — 14 px in tree and popover rows, 16 px in the header, dock and palette,
  20 px in the radial wheel. A 24 × 24 px hit target for any icon-only control
  (WCAG 2.5.8, the same floor as `visual-language.md` §5).
- **Labels** — icon + text is the default. Icon-only is allowed only where a tooltip
  exists and the target is 24 px. The status line stays text.

### 6.4 What not to do

- Do not pull IconMCP at runtime. It is a network dependency, a rate limit and an
  offline failure in one.
- Do not render upstream's 64 × 64 Inkscape icons in chrome. They carry gradients,
  shadows and four or five colours; at 16 px they read as foreign.
- Do not use IconMCP's `snap`, `pad`, `plane` or `array` results literally; the word
  senses collide (§2).
- Do not icon-only a destructive action to save space. The wheel's tone arc already
  carries that meaning.

---

## 7. Licensing and provenance

| Source | Licence | Posture |
| --- | --- | --- |
| IconMCP | **MIT** (Copyright © 2026 UI8) | Vendor the chosen outline SVGs with the licence text; attribution is satisfied by the notice. |
| FreeCAD icons | LGPL-2.1+ overall; per-file metadata is mixed | Ship verbatim in `mods/`, exactly as the app already does. Record authors. |
| FerroCAD-drawn glyphs | project licence | Authored here. |

The metadata in this checkout is not uniform: of **359 icon SVGs** under
`src/Mod/**/icons/`, **346** declare `FreeCAD LGPL2+`, **41** additionally carry a
`CC-BY-SA 4.0` badge and **4** a `CC-BY-SA 2.0` badge. CC-BY-SA is share-alike:
**referencing** the file in place is what FreeCAD already distributes, but a
**derived** monochrome set would be a derivative work with attribution and
share-alike obligations. That is the reason the recommendation is "verbatim
provenance, IconMCP for chrome", not "redraw all 1,625 upstream icons".

If we vendor any upstream file into `crates/`, add a `CREDITS.md` listing the
`dc:creator` values (`wmayer`, `Yorik van Havre`, …) per file.

---

## 8. Verification

- **Resolution completeness** — a test over `catalog.json`: every command id maps to a
  curated alias, a vendored IconMCP name, an embedded glyph, or an explicit
  `no-icon`. A new command with no icon fails CI rather than silently losing its slot.
- **Vendored-name validity** — a test that every referenced IconMCP name exists in the
  vendored set, so a typo cannot ship a blank square.
- **Embedded glyphs** — `the_icon_assets_are_embedded` asserts every asset is an SVG
  with strokes; `the_bug_renders_at_its_side` and
  `the_control_glyphs_render_at_their_side` assert the 16 × 16 wrapper and its
  `{key}-{name}` selector.
- **`Pixmap` plumbing** — a Python test that `GetResources()["Pixmap"]` reaches
  `catalog.json`, the same way the existing registry/JSON check guards labels.
- **Licence** — the vendored MIT text and the upstream attribution list are present.

The 1,625-row semantic pass is a research artifact, not a CI check: it needs network
access and its matches are heuristic. The curated alias table is the contract.

---

## 9. Decisions

1. **Adopt IconMCP outline as the chrome and primitive source.** MIT, 16-unit grid,
   1.25 stroke and `currentColor` match the visual language; 133 of the 179 needs map
   directly. *Recommend yes.*
2. **Vendor, do not fetch.** The chosen outline SVGs and their licence ship with the
   widget crate; the runtime never calls iconmcp.com. *Recommend yes.*
3. **Draw 26 CAD glyphs and compose 11.** Neither library states the modelling verbs
   honestly. *Recommend yes.*
4. **Keep upstream FreeCAD SVGs in place, verbatim.** They are workbench provenance,
   not chrome; the Draft set already ships through `mods/`. *Recommend yes.*
5. **Key icons by command id + `Pixmap`.** Add `pixmap` to `CommandInfo` and
   `catalog.json`; do not invent a second id space. *Recommend yes.*
6. **Icon + label everywhere except the title bar.** The status line stays text; the
   wheel keeps its tone arc. *Recommend yes.*

## 10. Open questions

- Do we ask IconMCP to **draw** the CAD gaps through `request_icon` (the service says
  it usually answers within a day), or draw them locally? Local drawing keeps the
  glyphs consistent with the stage ink and the 16-unit grid; a request creates a
  dependency on someone else's interpretation of "pocket".
- The bug is now IconMCP's `bug.svg`, so the widget crate paints no pictograms with
  `PathBuilder` any more (the radial wheel and the scrub track still do, as chrome
  rather than icons). The next decision is whether the rest of the `use` set is
  vendored eagerly or loaded through an `AssetSource`.
- Do we need upstream orientation icons at all, or does the NaviCube remain the only
  orientation control? If it remains, front/top/right/rear drop off the draw list.
- Is the dock icon + text, or icon-only with tooltips? The dock currently has room for
  text, and the wheel does not.

## 11. Non-goals

- A per-workbench icon palette or user-supplied icon themes.
- Solid, duotone or multicolour chrome icons.
- An icon font.
- Redrawing the whole 1,625-file upstream inventory.
- A runtime dependency on the IconMCP service.
