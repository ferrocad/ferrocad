# Input components: architecture (working note)

Status: design + first slice (2026-10-04). Companion to
[`app-shell-vision.md`](app-shell-vision.md) (S4 property editor, S5 console) and
[`milestones.md`](milestones.md). This note covers the editable components: the
single-line field, the console text area with a read-only transcript, and how the
property editor commits edits.

## 1. Research: the `View` pattern

`bite-gpui` 1.21 has the `View` trait
(`bite-gp-authoring-1.21.0/src/view.rs`):

```rust
pub trait View: 'static + Sized {
    fn entity_id(&self) -> Option<EntityId>;
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement;
}
```

with two blanket impls: `Entity<T: Render>` is a view keyed on its own id, and any
`T: RenderOnce` is a stateless view with no id. You implement `View` by hand "only
when a component needs both parent-supplied props **and** a backing entity for
identity" (the crate's own words).

The mental model, from `astrimid/gpui_book2` (`entity-view-foundation.md`):

1. A parent entity owns a handle to the component's persistent state
   (`Entity<InputState>`).
2. The widget is an **ephemeral wrapper** constructed on the stack each render,
   holding the state handle plus props (`placeholder`, width, style).
3. Its `entity_id()` returns the backing entity's id, so layout/paint caching
   treat the wrapper with the state's reactive identity.
4. Inside `render(self, …)` it reads the state through the handle.

This is exactly the split we want: **state** (buffer, selection, focus) lives in
an entity; **appearance and per-call-site props** live in a tiny view struct.

## 2. Component model

The widgets live in one crate, `ferrocad_widgets`, which the shell and the book both
depend on (there is one copy, not one per project).

```
                        ferrocad_widgets
  ┌──────────────────────────────┐   ┌─────────────────────────────────┐
  │ TextBuffer                   │   │ TextInput (entity)              │
  │  • text, selection, IME mark │◄──│  • TextBuffer, placeholder      │
  │  • read-only-length boundary │   │  • FocusHandle                  │
  └──────────────────────────────┘   │  • EntityInputHandler, Render   │
  ┌──────────────────────────────┐   │  • ChangeEvent / SubmitEvent    │
  │ TextAreaState (entity)       │◄──└─────────────────────────────────┘
  │  • TextBuffer (read-only len)│
  │  • FocusHandle, ScrollHandle │
  │  • EntityInputHandler, Render│
  │  • SubmitEvent               │
  └──────────────────────────────┘
```

Both share `TextBuffer` (`ferrocad_widgets::text_buffer`), which owns the text, the
selection, the IME marked range and the read-only boundary. All editing rules
(including the read-only clamp) are plain Rust and unit-tested there. The
components own only the `Window`-facing parts: the custom `Element`, the
`EntityInputHandler` delegation and the focus/scroll handles.

Both are plain entities that implement `Render`; a call site passes
`entity.clone()` to `.child()`. We tried the `View` wrapper from §1 to carry a
`placeholder` prop, but the placeholder is the only prop and it belongs in the
entity, so the wrapper was dropped. Section 1 stays the reference for when a
component really needs parent-supplied props *and* a backing entity.

## 3. Controlled and uncontrolled fields

| | Uncontrolled | Controlled |
| --- | --- | --- |
| Who owns the text | the `TextInput` entity | the parent (model/property) |
| Parent API | `state.update(..).text()`, `SubmitEvent` | `value` prop + `on_change(new)` |
| Re-render per keystroke | no | yes (parent notifies) |
| Best for | the console line, search boxes, any field whose owner acts on commit | fields whose value must always mirror external state |
| Cost | the owner reads on commit | a round trip and a notification per keystroke |

Decision for the property editor: **uncontrolled**. The field holds the typed
value; commit happens on `Enter`/blur and wraps one transaction. A controlled
field would open a transaction or round-trip through the model on every
keystroke, which is wrong for CAD and for the undo stack. The model pushes fresh
values in only when the selection changes or an observer fires, via an explicit
`set_content`.

The console line is uncontrolled for the same reason; the text area is a special
uncontrolled buffer whose prefix the owner appends to (see below).

## 4. The console as a text area with a read-only region

The console is not an input plus a log; it is **one buffer**:

```
content        = "FerroCAD shell · …\n>>> len(doc.Objects)\n3\n>>> "
read_only_len  = up to and including the last ">>> " ────┐
editable       = ""                                       │ everything before
                                                          ▼ is read-only
```

- `TextBuffer.read_only_len` marks the boundary. Edits, the caret and the
  selection are all clamped to `>= read_only_len` (`text_area.rs`).
- Rendering splits the content on `\n`. Each line is split at `read_only_len`
  into a muted read-only run and a normal editable run (a line can be entirely
  read-only, entirely editable, or both). The caret and selection are drawn only
  in the editable region, per line, so a selection can span lines.
- The editable tail may contain newlines: `Shift+Enter` inserts one and a pasted
  snippet keeps its own. That lets the user paste and edit a block before running
  it. Plain `Enter` emits `SubmitEvent { text }` with the whole tail; the owner
  runs it and rebuilds the transcript with `set_transcript`, which clears the tail
  and scrolls to the bottom.

## 5. Slices

| Slice | Deliverable | State |
| --- | --- | --- |
| A | `text.rs`: `TextBuffer` with read-only boundary + tests | **done** |
| B | `input.rs` on `TextBuffer`; `TextInputState` + the `Edit` view wrapper (`View` + props) | **done** |
| C | `textarea.rs`: `TextAreaState` (multi-line element, read-only runs) | **done** |
| D | Replace the console's input + log with one `TextArea`; update the shell test | **done** |
| E | Property editor: uncontrolled field committing on Enter inside one transaction | **done** |
| F | Book chapter: "Controlled and Uncontrolled Components" (property editor) | **done** |
| G | Extract the widgets into a shared `ferrocad_widgets` crate; shell and book use it | **done** |
| H | Multi-line console: Shift+Enter and pasted newlines in the editable tail | **done** |

What landed in B–D:

- `TextInput` is one entity: buffer, focus, `EntityInputHandler`, `Render` and the
  two events. It was briefly split into a state entity plus an `Edit` view wrapper;
  slice G dropped the wrapper (see §2).
- `TextAreaState` renders the buffer line by line, splitting each line at
  `read_only_len`. The element registers the input handler and reports geometry
  back to the state for hit-testing.
- The console is one `TextAreaState`: `ShellModel.console` is still the logical log,
  and `run()` rebuilds the transcript (`log + "\n>>> "`) after each command.
  `Enter` submits the editable tail; the rebuild clears it.

What landed in E:

- `ferrocad_shell.set_property(object, property, text)` marks each property row
  `editable` and a `kind`, refuses expression-driven properties, coerces the text
  by type (`float`, `int`, `bool`, `Quantity` for length/angle, string for
  enumeration), and applies it inside `openTransaction` / `commitTransaction`
  before `recompute()`.
- The shell keeps one uncontrolled `TextInput` per editable property, keyed by
  `object.property`. It is re-seeded from the model only when the value changed
  underneath and the field does not have focus. On `Enter` the field calls
  `set_property`, then the shell re-reads the tree and the selected object's rows.
- Commit is on `Enter` only. Commit-on-blur is deferred until the component emits
  a blur event.

What landed in F–H:

- The book's chapter 11 teaches the two ownership models; its example uses
  `ferrocad_widgets::TextInput`, which emits `ChangeEvent` on every edit and
  `SubmitEvent` on `Enter`, so one component serves both a controlled title field
  and an uncontrolled tag field. Chapter 9 keeps an inline copy for teaching.
- Slice G moved `window_frame`, `TextBuffer`, `TextInput` and `TextAreaState` into
  `ferrocad_widgets`; the host and the book both depend on it and their copies are gone.
- Slice H lets the console's editable tail span lines: `Shift+Enter` inserts a
  newline, paste keeps newlines, and `Enter` submits the whole tail. The element
  renders the read-only and editable runs per line and draws multi-line
  selections.

Still out of scope: soft wrapping of long lines and horizontal scrolling. The
console scrolls vertically only; a general code editor would extend the element.
