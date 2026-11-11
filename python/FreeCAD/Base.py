"""``FreeCAD.Base`` — the core data types.

Upstream keeps the low-level value types (``Quantity``, ``Unit``, ``Vector``,
``Vector2d``, ``Matrix``, ``Rotation``, ``Placement``, ``TypeId``, ``BoundBox``,
``Material``, …) under ``FreeCAD.Base``; ``FreeCAD.Units`` re-exports the unit
types for convenience. These are all implemented in Rust (``ferrocad_core``) and bound
by ``ferrocad_py``.
"""

from __future__ import annotations

from enum import IntEnum

import ferrocad as _fc

# The value types this module re-exports. A native binding may not provide all of them
# (the read-only wasm half binds the ones the App surface uses); a missing one is absent
# from this module rather than a second implementation. `tools/api_docs.py` documents the
# complete CPython surface.
_VALUE_TYPES = (
    "BoundBox",
    "Material",
    "Matrix",
    "Placement",
    "Quantity",
    "Rotation",
    "TypeId",
    "Unit",
    "Vector",
    "Vector2d",
)
for _name in _VALUE_TYPES:
    if hasattr(_fc, _name):
        globals()[_name] = getattr(_fc, _name)
del _name

try:
    # Upstream reports kernel failures as ``Base.CADKernelError``; this image
    # raises ``Part.OCCError`` for the same condition, so the upstream tests'
    # ``assertRaises(Base.CADKernelError)`` keeps its meaning. ``Part`` is a
    # separate module, so a core-only install falls back to a local class.
    from Part import OCCError as CADKernelError
except ImportError:  # pragma: no cover - core-only installs

    class CADKernelError(Exception):
        """Kernel failure (``Part.OCCError`` when the Part module is absent)."""


class ScaleType(IntEnum):
    """Scaling mode returned by ``Matrix.hasScale()`` (``Base::ScaleType``)."""

    Other = -1
    NoScaling = 0
    NonUniformRight = 1
    NonUniformLeft = 2
    Uniform = 3


class Precision:
    """``Base::Precision`` — the global tolerance defaults OCCT uses.

    Values match OCCT's ``Precision`` (``confusion`` is the linear tolerance,
    ``angular`` the angular one); a kernel backend may refine them later.
    """

    @staticmethod
    def confusion() -> float:
        return 1e-7

    @staticmethod
    def squareConfusion() -> float:
        return 1e-14

    @staticmethod
    def angular() -> float:
        return 1e-12

    @staticmethod
    def approximation() -> float:
        return 1e-12

    @staticmethod
    def intersection() -> float:
        return 1e-9

    @staticmethod
    def firstParameter() -> float:
        return 1e-7

    @staticmethod
    def lastParameter() -> float:
        return 1.0 - 1e-7


__all__ = [name for name in _VALUE_TYPES if name in globals()] + [
    "ScaleType",
    "Precision",
    "CADKernelError",
]
