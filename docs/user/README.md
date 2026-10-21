# User documentation

Generated documentation for the workflows FerroCAD implements **today**.

Each page is produced by running an ordinary Python script against the real
`FreeCAD` facade: the code blocks are the code that executed and the tables are
the values it read back. A workflow that is not implemented has no script, so it
has no page — this index is the progress list, not a backlog.

The pages are grouped the way Diátaxis groups them: a **tutorial** teaches by
doing, a **how-to guide** answers one task, the **reference** lists what exists,
and an **explanation** page says why. Reference is the generated
[Python API reference](../python-api/README.md).

## Tutorials

Learning-oriented: follow one pass end to end.

| Page | What it shows | Requires |
| --- | --- | --- |
| [Your first model](tutorials/first-model.md) | A single pass from an empty interpreter to a saved, parametric document: a document, a parameter, a second object that depends on it, a recompute, and the save that keeps it. | the `FreeCAD` facade; no workbench module and no GPU |

## How-to guides

Task-oriented: do one specific thing.

| Page | What it shows | Requires |
| --- | --- | --- |
| [Create a document and edit properties](how-to/documents-and-properties.md) | Open a document, add a typed length and an enumeration, write a value through each, and see the error an invalid enumeration value raises. | the `FreeCAD` facade; no workbench module and no GPU |
| [Wire two objects together with an expression](how-to/parametric-recompute.md) | Add an expression that reads one object's length from another, recompute, then change the input and watch the dependent value follow. | the `FreeCAD` facade; no workbench module and no GPU |
| [Undo an edit and reload the document](how-to/undo-and-persistence.md) | Group an edit into a named transaction, undo and redo it, then save the document, close it, open it again, and read the values back. | the `FreeCAD` facade; a writable temporary directory |
| [Create, display and manipulate an object](how-to/toy-workbench.md) | The toy workbench is the kernel-free reference edition. It shows the four things a workbench does: create an object, give it a visual representation, manipulate it through the generic interaction path, and record the edit as a transaction. | the `ferrocad_toy` app script (it registers on import); no GPU: the mesh is read before it would be drawn |
| [Drive a command from the selection](how-to/selection-and-commands.md) | Selection is UI state the shell pushes into the facade, and a command reads it when the shell asks whether it is enabled. This page runs a command with no selection, selects an object, opens a dialog and accepts it, and deletes the selection. | the `ferrocad_toy` app script (it registers on import); no window: the shell's pushes are ordinary function calls |

## Reference

Generated from the live modules by `tools/api_docs.py`:

- [Python API reference](../python-api/README.md)

## Explanation

Understanding-oriented: why it works this way.

- [How a click becomes a document edit](explanation/how-the-shell-talks-to-the-document.md) — This page explains the shape of the system from the outside in. It is the answer to a question the how-to guides skip: *why* does the code look the way it does?

6 workflows.

## Regenerating

```sh
python3 tools/userdocs.py            # refresh every page
python3 tools/userdocs.py --check    # non-zero exit if a page is stale
python3 tools/build_book.py          # assemble the mdBook edition
```

The check is the part that matters: it is cheap, needs no GPU, and it is what
keeps the pages honest in review and in CI. The mdBook edition is assembled from
these pages, so it inherits the same guarantee.

## What these pages are not

- **Not rendered screenshots.** A page shows the mesh the viewport would draw,
  summarised, but no pixels. Capturing a frame needs a GPU and an offscreen
  render path the shell does not expose yet; the `image` hook in
  `tools/workflows/_capture.py` is where that will attach.
- **Not upstream parity.** Only what runs here is documented here; the gap is
  measured by [`../workbenches/conformance.md`](../workbenches/conformance.md).
