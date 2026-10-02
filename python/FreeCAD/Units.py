"""``FreeCAD.Units`` — the units facade.

Re-exports the unit/quantity types from ``fc-core`` (the same objects as
``FreeCAD.Base``, preserving type identity). The full upstream units system
(``parseQuantity``, ``translateUnit``, scheme handling, dimensional analysis)
is not implemented yet; it is the gating surface for `UnitTests.py`.
"""

from __future__ import annotations

from fc import Quantity

__all__ = ["Quantity"]
