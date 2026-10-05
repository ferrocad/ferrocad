# FerroCAD — MVP app shell (dev-tool vision)

Status: vision/planning (2026-10-03). Companion to
[`mvp-path.md`](mvp-path.md) (the headless engine MVP),
[`python-ui-research.md`](python-ui-research.md) (how the UI is built) and
[`rewrite-strategy.md`](rewrite-strategy.md) (direction). This note defines the
**first usable window** and the slices that get there.

---

## 1. What the MVP app shell is

> **FerroCAD MVP app shell = one native window that drives the Rust document
> engine through the embedded CPython interpreter, with five capabilities: a
> DOM-like inspector, a Python console, a property editor, undo/redo, and
> open/close/load/save.**

It is a *developer tool*, not a CAD workbench: it lets you create, inspect,
edit, script and persist documents. It is deliberately geometry-free and has no
3D viewport — it is the **container** the 3D viewport later docks into.

**Definition of done (the "hello, world" of the shell):** open the app, create a
document, add an object in the Python console, watch it appear in the inspector,
edit one of its properties in the property editor, undo the edit, then save and
reopen the document — all without a terminal.

---

## 2. Why this is the right next chunk

The headless MVP ([`mvp-path.md`](mvp-path.md)) proved the riskiest assumption:
*an unmodified FreeCAD `App`-level Python script runs on the Rust core*. The app
shell proves the **second** assumption: *that a real interactive UI can be built
on `bite-gpui` while all scripting, commands and edits flow through the same
`FreeCAD` API.* Every later workbench UI is a set of panels and commands hung
off this scaffold, so getting it right once pays off everywhere.

It also turns the engine's existing data into a **product surface**: the engine
already has documents, typed properties, dependencies, observers, undo/redo and
save/load — the shell is what makes them *visible and usable*.

The `ferrocad_gpui` spike already demonstrated the hard parts in the small:
Rust embeds CPython, Python *declares* a UI tree, `bite-gpui` renders it, a click
round-trips through a Python callback, and a raising handler is isolated without
crashing the host ([`python-ui-research.md`](python-ui-research.md) §E,§F).

---

## 3. The five pillars

Each pillar is mapped to what the engine already provides and the surface the
shell must add.

| Pillar | Engine support (today) | New surface needed | Slice |
| --- | --- | --- | --- |
| **Open / close / load / save** | `newDocument`, `openDocument`, `closeDocument`, `save`, `saveAs`, document observers | File menu + command registry; dirty-state tracking; error dialogs; recent files | S2 |
| **Undo / redo** | Full engine (MVP slice B2): `openTransaction`/`commit`/`abort`, `undo`/`redo`, `UndoNames`/`RedoNames`, `UndoMode`, `getAvailableUndos`/`getAvailableRedos`, `clearUndos` | Edit menu + toolbar commands with enable/disable from `getAvailableUndos`; wrap every UI edit in a named transaction | S2 |
| **DOM-like inspector** | `Doc.Objects`, object `Name`/`Label`/`TypeId`, `PropertiesList`, `InList`/`OutList`, observers (`slotCreatedObject`, `slotDeletedObject`, `slotChangedObject`, `slotRecomputedDocument`) | A document-tree model + selection service; lazy expansion; refresh on observer signals | S3 |
| **Property editor** | Typed properties (`Bool`/`Int`/`Float`/`String`/`Enumeration`/`Quantity`/`Placement`/`Vector`/`Matrix`/`ColorList`/`Link`/`LinkList`/`LinkSub`), `getPropertyStatus`/`setPropertyStatus`, property `Group` | A property-type → widget registry; grouping; read-only/output badges; commit-on-enter wrapped in a transaction | S4 |
| **Python console** | Embedded CPython (spike), `FreeCAD` / `FreeCADGui` facade, `FreeCAD.Console` | An in-window REPL bound to the same interpreter as the app; `stdout`/`stderr` capture; history; multi-line input | S5 |

---

## 4. Architecture

The engine is the **single source of truth**; every panel is a *derived view*
kept fresh by observers. Nothing mutates the model except through the public
`FreeCAD` API, so the console, the menus and the property editor all take the
same path — and every edit is one more undoable transaction.

```text
┌─────────────────────────── ferrocad host process (Rust) ───────────────────────────┐
│                                                                                    │
│  bite-gpui window                                                                   │
│    ├── menu bar / toolbar   ── command ids ──┐                                      │
│    ├── inspector pane       ── selection ────┤                                      │
│    ├── viewport (placeholder now, M5 later)  │                                      │
│    ├── property editor pane                   │                                     │
│    └── Python console pane                    │                                     │
│                                               ▼                                     │
│        Python-declarative UI (retained tree + Patch diff stream, per               │
│        python-ui-research.md §B/C)  ──►  WireView / element builder                 │
│                                               │                                     │
│                                               ▼                                     │
│        Controller / services (Python): commands, selection, property binding        │
│                                               │                                     │
│                                               ▼                                     │
│        FreeCAD facade  ──►  ferrocad (PyO3)  ──►  ferrocad_core (Rust engine)      │
│            ▲                                                                        │
│            └──────── embedded CPython (GIL) ────────────────────────────────────────│
└────────────────────────────────────────────────────────────────────────────────────┘
```

Key decisions (carried from the spike and the UI research):

- **Python-declarative, Rust-rendered (Blender style).** Workbenches and panels
  declare a tree in Python; Rust renders it with `bite-gpui`. The tree is
  *retained* and *diffed* — re-evaluated on dirty events, not every frame — so
  the GIL is not contended per frame (§B of the UI research).
- **One command layer.** Menus, toolbar, shortcuts and the console all dispatch
  the same command ids. Commands live in Python so workbenches can register
  their own; the shell owns the chrome.
- **Edits are transactions.** The property editor, inspector renames and
  console-driven changes all run inside `openTransaction(name)` /
  `commitTransaction()`, which is what makes undo/redo uniform and free.
- **Observers drive refresh.** The inspector and property editor subscribe to
  document/object signals and re-render only what changed.
- **The interpreter owns the app's namespace.** The console, the panels and the
  `FreeCAD` facade share one CPython instance; the console exposes the active
  document and `FreeCAD`/`FreeCADGui` directly.

---

## 5. Slice plan

Ordered by value; each slice is independently demoable. Sizes are rough.

- **S1 · App-shell skeleton. ✅ done (first cut).** `ferrocad_gpui` boots an
  embedded CPython interpreter, imports `FreeCAD` and creates a **sample
  document** (`Params` + `Derived` wired by an expression + a `Group`), then
  renders a titled window: a **model inspector** (documents → objects, selectable),
  a **property editor** (read-only rows for the selection), a **viewport
  placeholder**, a **Python console** (a log plus runnable snippets), and a
  **status bar**. The window chrome is real; the panes are driven live through
  `python/ferrocad_shell`. It draws **client-side decorations** through the
  reusable `window_frame` in the `ferrocad_widgets` crate: an in-window
  title bar (drag to move, right-click for the window menu, min/max/close) and
  border resize grips, so the window is correctly framed and resizable even under
  GNOME/Wayland, which ignores the server-decoration protocol. The frame insets
  the content by the grip width so the top edge and corners stay grabbable.
  *Acceptance:* the headless `#[gpui::test]`
  (`shell_boots_lists_the_sample_document_and_is_renderable`) asserts the booted
  model, the property rows, a console round-trip, and that the shell renders with
  no display or GPU; plus pure-geometry tests for the resize-grip hit regions
  (`resize_edge_detects_corners_edges_and_content`, `cursors_match_each_resize_direction`).
  *Still S1/S2:* menus/shortcuts and file commands. (Real console text input landed
  as the S5 preview, below.)
- **S2 · Document lifecycle + commands.** File (New/Open/Close/Save/Save As/
  Quit) and Edit (Undo/Redo, with `getAvailableUndos`-driven enablement) menus,
  keyboard shortcuts, a command registry, and dirty-state tracking.
  *Acceptance:* a headless test drives commands by id; save→close→open
  round-trips a document; Undo/Redo light up correctly.
- **S3 · Inspector.** A document tree (Documents → Objects → Properties) built
  from the model, with a selection service and refresh on observer signals.
  *Acceptance:* `addObject` in the console makes a node appear; `removeObject`
  makes it disappear; selecting a node shows its properties in S4.
- **S4 · Property editor.** Editors for the core property types, grouped by
  property `Group`, with read-only/output badges from `getPropertyStatus`;
  edits commit inside a transaction and are immediately undoable.
  *Preview:* editable scalar properties already render an uncontrolled
  `ferrocad_widgets::TextInput` that commits on `Enter` through
  `ferrocad_shell.set_property`, which wraps the change in
  `openTransaction`/`commitTransaction` and recomputes. S4 completes the other
  control kinds (`bool` toggle, `enum` picker), grouping, badges and blur commit.
  *Acceptance:* edit an `Integer`/`Enumeration`/`Quantity`/`Link`; the model
  updates and Undo reverts it.
- **S5 · Python console.** An in-window REPL bound to the app interpreter,
  exposing `FreeCAD`/`FreeCADGui` and the active document, with history and
  captured `stdout`/`stderr`.
  *Preview:* the console is already one **text area**
  (`ferrocad_widgets::TextAreaState`): its read-only prefix is the transcript and its
  editable tail is the current line (which may span several lines via Shift+Enter
  or a pasted snippet), run through the interpreter on `Enter`. S5 completes
  history navigation and streaming `stdout`/`stderr`.
  *Acceptance:* `doc.addObject("App::FeaturePython", "Box")` typed at the prompt
  updates the inspector and property editor.
- **S6 · Polish.** Dockable/resizable panes, error dialogs, `UndoMode` and other
  preferences, recent files, About.

Dependencies: S1 gates S2–S5; S3 and S4 are coupled (selection → editor); S5
shares the interpreter with S1. S2 (undo/redo + open/save) builds directly on
engine work already done.

---

## 6. Open questions and risks

- **`bite-gpui` maturity.** The spike found `debug_bounds("…")` returning `None`
  in 1.21.0 (the rendered-frame map was not committed when read). Text input and
  IME are now exercised headlessly (S5 preview, `ferrocad_widgets`); scrolling and
  virtualised lists in a large inspector tree still need to be verified early in
  S3.
- **GIL and frame time.** The UI thread must not hold the GIL while rendering; a
  recompute triggered from the console or a property edit should run without
  stalling the frame. Reserve a worker thread + channel for long recomputes.
- **Console plumbing.** Capturing `stdout`/`stderr`, multi-line/paste handling
  and clipboard integration are more fiddly than they look.
- **Retained-tree cost.** The per-frame GIL bottleneck is the reason for the
  retained+diff design; confirm the patch stream stays small for large documents.
- **Where commands live.** Python (recommended, for workbench reuse) vs Rust
  (faster, less flexible). Start in Python; move hot paths to Rust if needed.

---

## 7. Relationship to the tracks

This is where the **[`mvp-path.md`](mvp-path.md) engine** meets the **M4/M5 UI
track** of [`rewrite-strategy.md`](rewrite-strategy.md): M4 supplies the
declarative UI + PySide shim, M5 supplies the `scenix`/`wgpu` 3D viewport that
docks into the placeholder pane. The app shell is the umbrella that ties them
together.

**Out of scope here:** the geometry kernel (OCCT), the 3D viewport (M5), and any
PySide/Qt surface beyond the ~15-widget shim needed to host workbench panels.
