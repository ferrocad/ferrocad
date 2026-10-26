# `FreeCAD.Units`

Quantity parsing, units and number formatting.

> **Generated page.** `tools/api_docs.py` reads this surface from the
> module itself. Do not edit it by hand.

``FreeCAD.Units`` — the units facade.

## Functions

| Function | Summary |
| --- | --- |
| `listSchemas(*args)` | Return the full schema tuple, or one schema name by index. |
| `parseQuantity(expression)` | Parse a unit expression into a :class:`Quantity`. |
| `schemaTranslate(quantity, schema)` | Translate a quantity into another schema's textual pieces. |
| `toNumber(value, format='g', decimals=0)` | Format a number or Quantity as a string. |
| `translateUnit(expression)` | Return the value of ``expression`` in internal units. |

## Class `NumberFormat`

Format codes accepted by ``toNumber``.

| Member | Summary |
| --- | --- |
| `Fixed` (attribute) | — |
| `General` (attribute) | — |
| `Scientific` (attribute) | — |

## Class `Quantity`

| Member | Summary |
| --- | --- |
| `Format` (attribute) | — |
| `Unit` (attribute) | — |
| `UserString` (attribute) | — |
| `Value` (attribute) | — |
| getValueAs(unit) | — |
| isEqual(other, tol=0.0) | `Quantity.isEqual(other, tol=0)`: equal units and values within `tol`. Upstream requires another `Quantity` (a bare number is a `TypeError`). |
| toStr() | — |
| value_in(unit) | — |
| value_mm() | — |

## Class `Unit`

## Values

| Name | Summary / value |
| --- | --- |
| `Radian` | 57.29577951308232 deg |
