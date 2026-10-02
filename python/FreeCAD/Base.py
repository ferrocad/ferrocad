"""``FreeCAD.Base`` — the core data types.

Upstream keeps the low-level value types (``Quantity``, ``Unit``, ``Vector``,
``Matrix``, …) under ``FreeCAD.Base``; ``FreeCAD.Units`` re-exports the unit
types for convenience. This POC implements ``Quantity``/``Unit`` (Rust
``fc-core``) now; the remaining geometry types land here as they are added.
"""

from __future__ import annotations

from fc import Quantity, Unit

__all__ = ["Quantity", "Unit"]
