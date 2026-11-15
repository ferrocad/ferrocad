"""App-shell script for **FerroCAD: Sculpt**.

The Sculpt edition is the Parametric composition plus the bundle in
``python/Sculpt``: the same shell data surface, the same Part display and
interaction providers, and a command set for the cage workflow. Re-exporting
``ferrocad_parametric`` rather than copying it keeps one definition of "how a
Part object is drawn and edited" — a `Part::FeaturePython` skin is a Part object,
so it needs exactly those hooks.

What this script adds:

* the six commands from ``Sculpt.commands`` (``Sculpt_MakeCage``,
  ``Sculpt_MakeSkin``, ``Sculpt_NestInBody``, ``Sculpt_LevelUp``,
  ``Sculpt_LevelDown``, ``Sculpt_AttachProxies``), grouped under ``"Sculpt"``;
* a sculpted sample beside the canonical one, so the edition opens on the
  workflow rather than on an empty view: a 30 mm box cage at x = 40, its skin as
  a body's base feature, in `Sample` as ``SculptCage``/``SculptSkin``/``SculptBody``.

The GUI half of the cage interaction (a point/edge manipulator) is not here yet;
the edition's ``main`` is where it will be contributed, exactly as the Parametric
edition contributes the dress-up and push/pull manipulators.

The shell imports this module by name (the edition's ``HostConfig.entry_module``),
so it must re-export every entry point the shell calls.
"""

from __future__ import annotations

import FreeCAD

# The shell's data functions (`bootstrap`, `model_tree`, `properties`,
# `set_property`, `evaluate`, `stage_meshes`, `interaction`, ...) and the Part
# providers/commands all arrive through the Parametric script.
from ferrocad_parametric import *  # noqa: F401,F403
from ferrocad_parametric import bootstrap as _parametric_bootstrap

import FerroCAD
import Sculpt
import Sculpt.commands
import Sculpt.display
import Sculpt.interaction

#: The demo objects this edition adds to the canonical sample.
SAMPLE_CAGE = "SculptCage"
SAMPLE_SKIN = "SculptSkin"
SAMPLE_BODY = "SculptBody"

#: The demo cage's side and offset (beside the sample's filleted box).
SAMPLE_SIZE = 30.0
SAMPLE_OFFSET = (40.0, 0.0, 0.0)


def bootstrap(doc_name: str = "Sample") -> str:
    """Open the canonical sample, add this edition's demo, report host status."""
    summary = _parametric_bootstrap(doc_name)
    seed_sample()
    return summary


def seed_sample():
    """Add the sculpted demo to the active document (a no-op when it is there).

    Safe to call twice: the objects are named and the guard reads the document.
    Everything is one transaction, so a failing seed leaves no half bundle.
    """
    document = FreeCAD.ActiveDocument
    if document is None or document.getObject(SAMPLE_CAGE) is not None:
        return []
    document.openTransaction("Sculpt sample")
    try:
        points, faces = Sculpt.box_cage(
            SAMPLE_SIZE, SAMPLE_SIZE, SAMPLE_SIZE, origin=SAMPLE_OFFSET
        )
        cage = Sculpt.makeCage(document, SAMPLE_CAGE, points, faces, levels=2)
        skin = Sculpt.makeSkin(document, SAMPLE_SKIN, cage)
        body = document.addObject("PartDesign::Body", SAMPLE_BODY)
        Sculpt.nest(body, cage, skin)
        document.recompute()
        document.commitTransaction()
    except Exception:
        document.abortTransaction()
        raise
    return [cage.Name, skin.Name, body.Name]


# The command surface is part of importing the edition, as it is for Parametric.
Sculpt.commands.register()
# The cage grip's two halves: the display hook publishes the control points as stage
# vertices (`Point0`…), and the interaction hook applies `cage_point/move`. Both are
# idempotent and both defer to the Parametric hooks for everything else.
FerroCAD.register_display_provider(Sculpt.display.cage_display)
FerroCAD.register_interaction_provider(Sculpt.interaction.interaction)
