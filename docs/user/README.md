# User documentation

Generated documentation for the workflows FerroCAD implements **today**.

Each page is produced by running an ordinary Python script against the real
`FreeCAD` facade: the code blocks are the code that executed and the tables are
the values it read back. A workflow that is not implemented has no script, so it
has no page — this index is the progress list, not a backlog.

| Workflow | What it shows | Needs |
| --- | --- | --- |
| [Create a document and edit properties](documents-and-properties.md) | Open a document, add a typed length and an enumeration, write a value through each, and see the error an invalid enumeration value raises. | the `FreeCAD` facade; no workbench module and no GPU |
| [Wire two objects together with an expression](parametric-recompute.md) | Add an expression that reads one object's length from another, recompute, then change the input and watch the dependent value follow. | the `FreeCAD` facade; no workbench module and no GPU |
| [Undo an edit and reload the document](undo-and-persistence.md) | Group an edit into a named transaction, undo and redo it, then save the document, close it, open it again, and read the values back. | the `FreeCAD` facade; a writable temporary directory |
| [Create, display and manipulate an object](toy-workbench.md) | The toy workbench is the kernel-free reference edition. It shows the four things a workbench does: create an object, give it a visual representation, manipulate it through the generic interaction path, and record the edit as a transaction. | the `ferrocad_toy` app script (it registers on import); no GPU: the mesh is read before it would be drawn |
| [Drive a command from the selection](selection-and-commands.md) | Selection is UI state the shell pushes into the facade, and a command reads it when the shell asks whether it is enabled. This page runs a command with no selection, selects an object, opens a dialog and accepts it, and deletes the selection. | the `ferrocad_toy` app script (it registers on import); no window: the shell's pushes are ordinary function calls |

5 workflows.

## Regenerating

```sh
python3 tools/userdocs.py            # refresh every page
python3 tools/userdocs.py --check    # non-zero exit if a page is stale
```

The check is the part that matters: it is cheap, needs no GPU, and it is what
keeps the pages honest in review and in CI.

## What these pages are not

- **Not rendered screenshots.** A page shows the mesh the viewport would draw,
  summarised, but no pixels. Capturing a frame needs a GPU and an offscreen
  render path the shell does not expose yet; the `image` hook in
  `tools/workflows/_capture.py` is where that will attach.
- **Not the API reference.** For signatures see [`../python-api/`](../python-api/).
- **Not upstream parity.** Only what runs here is documented here; the gap is
  measured by [`../workbenches/conformance.md`](../workbenches/conformance.md).
