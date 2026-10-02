"""Console-mode stand-in for FreeCAD's ``Gui`` module.

When FreeCAD runs headless (``FreeCAD.GuiUp == 0``), ``import FreeCADGui``
succeeds but exposes only a minimal, non-interactive surface. Workbenches and
tests import it defensively (e.g. ``if FreeCAD.GuiUp:``) and probe for attributes
like ``getDocument`` — which must *not* exist here, mirroring upstream's dummy
GUI module in console mode.

The real GUI module (M5: ``bite-gpui`` host) will replace this stub.
"""

from __future__ import annotations

__version__ = "0.1.0"

# Deliberately minimal: no `getDocument`, no `showMainWindow`, no viewer. The
# `FreeCAD.GuiUp` flag (0 in the facade) is what tests check before touching GUI.
