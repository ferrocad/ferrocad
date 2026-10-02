"""``FreeCAD.Base`` — the core data types.

Upstream keeps the low-level value types (``Quantity``, ``Unit``, ``Vector``,
``Matrix``, ``Rotation``, ``Placement``, ``TypeId``, …) under ``FreeCAD.Base``;
``FreeCAD.Units`` re-exports the unit types for convenience. These are all
implemented in Rust (``fc-core``) and bound by ``fc-python``.
"""

from __future__ import annotations

from fc import Matrix, Placement, Quantity, Rotation, TypeId, Unit, Vector

__all__ = [
    "Quantity",
    "Unit",
    "Vector",
    "Matrix",
    "Rotation",
    "Placement",
    "TypeId",
]
