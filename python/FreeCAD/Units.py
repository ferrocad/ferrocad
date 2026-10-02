"""``FreeCAD.Units`` — the units facade.

Re-exports ``Quantity``/``Unit`` from ``ferrocad_core`` (identical objects to
``FreeCAD.Base``) and adds the module-level units API: ``parseQuantity``,
``toNumber``, ``listSchemas``, ``schemaTranslate``, ``translateUnit``, the
``NumberFormat`` enum, and the predefined unit/quantity constants.

The unit *parsing/arithmetic* lives in Rust (``ferrocad_core``); this module is a thin
Python layer. ``listSchemas``/``schemaTranslate`` are a minimal single-schema
("Standard") implementation for now.
"""

from __future__ import annotations

from ferrocad import Quantity, Unit

# --- predefined units (canonical internal units) ----------------------------
Length = Unit("mm")
Mass = Unit("kg")
Time = Unit("s")
Angle = Unit("deg")
ElectricCurrent = Unit("A")
ThermodynamicTemperature = Unit("K")
AmountOfSubstance = Unit("mol")
LuminousIntensity = Unit("cd")

# --- quantity constants -----------------------------------------------------
Radian = Quantity("1 rad")


class NumberFormat:
    """Format codes accepted by ``toNumber``."""

    General = "g"
    Fixed = "f"
    Scientific = "e"


def parseQuantity(expression):
    """Parse a unit expression into a :class:`Quantity`."""
    return Quantity(expression)


def translateUnit(expression):
    """Return the value of ``expression`` in internal units."""
    return Quantity(expression).Value


def toNumber(value, format="g", decimals=0):
    """Format a number or Quantity as a string.

    ``format`` is one of ``g`` (significant digits), ``f`` (fixed), ``e``
    (scientific); ``decimals`` is the digit count.
    """
    if isinstance(value, Quantity):
        value = value.Value
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        raise TypeError("value must be a number or a Quantity")
    if format not in ("g", "f", "e"):
        raise ValueError("invalid number format '%s'" % format)
    return ("{:." + str(decimals) + format + "}").format(value)


_SCHEMAS = ("Standard",)


def listSchemas(*args):
    """Return the full schema tuple, or one schema name by index."""
    if args:
        return _SCHEMAS[args[0]]
    return _SCHEMAS


def schemaTranslate(quantity, schema):
    """Translate a quantity into another schema's textual pieces.

    Minimal single-schema implementation: identity translation in the canonical
    internal unit.
    """
    return [quantity.UserString, 1.0, ""]


__all__ = [
    "Quantity",
    "Unit",
    "NumberFormat",
    "Radian",
    "parseQuantity",
    "translateUnit",
    "toNumber",
    "listSchemas",
    "schemaTranslate",
]
