"""``FreeCAD.Base`` — the core data types.

Upstream keeps the low-level value types (``Quantity``, ``Unit``, ``Vector``,
``Vector2d``, ``Matrix``, ``Rotation``, ``Placement``, ``TypeId``, ``BoundBox``,
``Material``, …) under ``FreeCAD.Base``; ``FreeCAD.Units`` re-exports the unit
types for convenience. These are all implemented in Rust (``ferrocad_core``) and bound
by ``ferrocad_py``.
"""

from __future__ import annotations

from enum import IntEnum

from ferrocad import (
    BoundBox,
    Material,
    Matrix,
    Placement,
    Quantity,
    Rotation,
    TypeId,
    Unit,
    Vector,
    Vector2d,
)


class ScaleType(IntEnum):
    """Scaling mode returned by ``Matrix.hasScale()`` (``Base::ScaleType``)."""

    Other = -1
    NoScaling = 0
    NonUniformRight = 1
    NonUniformLeft = 2
    Uniform = 3


__all__ = [
    "Quantity",
    "Unit",
    "Vector",
    "Vector2d",
    "Matrix",
    "Rotation",
    "Placement",
    "TypeId",
    "BoundBox",
    "Material",
    "ScaleType",
]
