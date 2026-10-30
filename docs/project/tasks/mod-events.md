# Mod events: advertise, subscribe, and stay out of the frame

Status: **design and plan (2026-10-30).** *Plan only — nothing here is built.* Stage **E1** of
the transform overhaul ([`transform-session.md`](transform-session.md) §5), written up separately
because it is about the host/mod boundary rather than the transform itself.

## 0. Why this exists

The host already calls **into** mods at three points (a display mesh, a manipulator commit, a
declared handle), and mods already push **out** one thing (preview items, `FreeCADGui.Preview`).
What is missing is the opposite direction: a mod cannot observe the host. So a mod that wants to
react to a selection or a document edit has to be pulled by the host at a point the host already
knows about, and anything else is impossible.

The transform session makes this concrete. A workbench that owns `transform` (D0) receives the
commit, but it may want to know *when a session starts and ends* — to tint its handles, to open
its own readout, to log. That is an observation, not an edit, and it should not require the host
to grow a special case for that mod.

## 1. The contract, in one paragraph

**The host advertises a fixed, versioned set of events. A mod subscribes to the ones it wants by
name, at import time, and gets a synchronous callback. Nothing is emitted per frame.**

```python
# at import, after the providers are registered
FerroCAD.subscribe(["selection.changed", "document.edited"], handler)
# handler(event) with event = {"id": "selection.changed", "document": "Unnamed", "selection": [...]}
```

- **Fixed set, not a bus.** The host publishes the list with each release
  (`FerroCAD.events()` → `["selection.changed", "document.edited", "document.recomputed",
  "transform.committed"]`). A mod cannot invent an event id; a mod that needs one gets it added
  to the host, which is a one-line change and a reviewable decision.
- **Synchronous, after the change.** A handler runs on the UI thread once the host state is
  consistent (selection replaced, document committed and recomputed). It must not block: the
  contract says "observe and schedule", not "do work".
- **Low frequency only.** A frame-driven or tick-driven event is out of scope by construction —
  that is what the `Preview` channel and the declarative handle path are for. The event set is
  the set of things that happen at human speed.
- **Exceptions are logged, not propagated.** A raising handler cannot break the caller; it is
  reported once per subscriber and the subscription stays (the mod can fix itself).

## 2. Why not the richer lifecycle

The tempting version is: each mod declares its capabilities, and mods subscribe to *each
other's* events with a dependency graph and an ordering rule. It was considered and rejected for
now, for three reasons:

1. **Ordering is a new problem with no current consumer.** Today the host already imposes an
   order on the calls it makes (display providers in registration order, manipulator factories
   newest-first). A mod-to-mod bus needs a topological order, cycle detection, teardown on
   unload, and a policy for a mod that subscribes to itself indirectly.
2. **The host is the only party that knows when something happened.** A selection change is the
   shell's event; a document edit is the document's. Mods do not produce events the host does
   not already see.
3. **Capability advertisement is mostly already there.** What a mod provides is observable from
   its registrations (a display provider, an interaction provider, N handles), and the registry
   already keeps the order. Declaring it again in a manifest is only worth doing when something
   *consumes* the declaration — e.g. a UI that lists "this mod adds a fillet tool".

**Trigger to revisit:** a second mod that needs to react to a *first mod's* action, or a mod
manager UI that must show capabilities before loading. Until then this stays a host-only event
set.

## 3. The first four events

| id | When | Payload | Consumers today |
| --- | --- | --- | --- |
| `selection.changed` | after a pick, a tree click, or a programmatic selection | the selection refs and the active object name | the transform session (arm/disarm), a workbench that wants to tint handles |
| `document.edited` | after a committed transaction, before recompute | the transaction name and the touched object names | anything that caches per-document data |
| `document.recomputed` | after `recompute()` returns | the objects that executed, and failure state | the same, plus diagnostics |
| `transform.committed` | after a transform commit is applied | the target, the accumulated delta, the transaction result | a workbench that wants to log or chain a repeat |

`transform.committed` is the one this plan needs: it lets a mod see the session boundary without
the host calling into it twice, and it is what makes D0's "mods own the commit" observable rather
than a one-way call.

## 4. Where it lands in the code

- `ferrocad_stage_py` (or a sibling cell) holds the subscriber list, like `preview_items` does
  today: process-wide, behind a mutex, called with the GIL held.
- The shell emits from the places it already treats as boundaries: `on_selection`,
  `after_document_edit`, and the commit path in `on_interaction`.
- `FerroCAD.subscribe` / `FerroCAD.events()` are thin Python wrappers over that cell; no new
  crate.

## 5. Acceptance (E1)

- A test mod subscribes to `selection.changed` and `document.edited`, and receives exactly one
  event per action — no frames, no duplicates.
- A handler that raises is logged and does not break the selection or the commit.
- An unknown event id is rejected at subscribe time with the list of valid ids.
- `FerroCAD.events()` and this table agree (a test reads both).

## 6. Open questions

| # | Question | Leaning |
| --- | --- | --- |
| E1a | Do subscriptions survive a document close, or are they process-wide? | Process-wide: a mod is built to serve every document, and the payload names the document |
| E1b | Is there a `document.opened` / `document.closed`? | Yes, once something needs it — a mod that keeps per-document caches is exactly the "second consumer" trigger in §2 |
| E1c | Does the host emit `transform.committed` for *auto-commits* too? | Yes; a session boundary is a boundary, however it was triggered |
