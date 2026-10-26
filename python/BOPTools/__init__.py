"""The BOPTools surface FerroCAD needs: ``SplitAPI.slice``.

Upstream's ``BOPTools`` package is the Python layer over FreeCAD's boolean
operations. SimpleCAD's ``modeling/slicing.py`` imports exactly one name,
``from BOPTools import SplitAPI``, and calls::

    result = SplitAPI.slice(shape.copy(), [plane.face(offset)], "Split")

so this package implements that one operation and nothing else. It is a
top-level package (like ``python/TechDraw`` and ``python/Draft``), not a
``Part`` submodule, because that is the import spelling the caller uses.

What this module implements
---------------------------

``SplitAPI.slice(base_shape, tool_shapes, mode="Split", tolerance=None)``
    Split ``base_shape`` with ``tool_shapes`` and return the pieces that came
    from ``base_shape``. Tool fragments are never returned.

    This is the operation upstream ``BOPTools.SplitAPI.slice`` builds from
    ``Shape.generalFuse``: the base shape and the (compound-wrapped) tools go
    through one General Fuse, and the parts of the first argument are the
    result. FerroCAD's ``Shape.generalFuse(shapes, fuzzy_value)`` already
    returns ``(result, per-input parts)``, so ``parts[0]`` *is* upstream's
    ``piecesFromSource(base_shape)``; no Python ``GeneralFuseResult``/element-map
    bookkeeping is needed. The tools are wrapped in one-shape compounds first,
    exactly as upstream does, so their fragments never leak into the result.

    A ``Compound`` (or ``CompSolid``) is split through each of its children,
    because OCCT's ``Modified()`` map is empty for a compound argument (the
    children are what the splitter actually changes). ``childShapes()``
    enumerates the coarsest kind present, so a compound that mixes solids with
    loose faces/edges is split for its solids only; that is the child
    enumeration the ``Part`` facade exposes.

    A ``Shell`` or ``Wire`` is likewise split through its faces or edges, which
    is upstream's ``splitAggregates`` step. The pieces are returned as faces or
    edges; SimpleCAD sews the surfaces back together itself.

    Only ``"Split"`` is implemented: wires, shells and solids are split at the
    intersections. ``"Standard"`` and ``"CompSolid"`` raise
    ``NotImplementedError`` rather than silently returning the Split result.

    ``tolerance=None`` (the default) gives General Fuse no fuzzy value; a number
    is forwarded as the fuzzy value. FerroCAD's binding currently ignores the
    value, and that is the only part of the upstream signature accepted for
    compatibility. Extra keyword arguments are accepted and ignored so callers
    that pass upstream-only options keep importing.

    Returns ``base_shape``'s single piece unchanged when the split produced one
    piece (as upstream does), otherwise a ``Part.Compound`` of the pieces. A
    shape the tools do not touch is its own one piece, because a split never
    removes material. ``Part.Shape()`` (the null shape) is returned only for an
    empty compound, which has no piece to keep.

How this relates to ``Part.makeSplitShape``
--------------------------------------------

``Part.makeSplitShape(shape, splits, checkInterior=True)`` is a *different*
operation. Each ``splits`` item is ``(cutter, face)`` where the cutter must be
an edge or wire lying on that face, so it splits a **face**; it cannot split a
solid with a tool face. A plane-versus-solid slice therefore cannot be built
from ``makeSplitShape`` pairs, which is why ``slice`` uses ``generalFuse`` —
the same operation upstream's ``SplitAPI.slice`` uses.

What this module does **not** implement
---------------------------------------

Everything else in BOPTools: ``booleanFragments``, ``xor``, ``JoinAPI``,
``SplitFeatures``, ``ShapeMerge`` and the GUI commands. A name outside
``__all__`` raises ``AttributeError`` naming this module; it is never silently
replaced by a placeholder.
"""

from __future__ import annotations

import Part

__all__ = ["SplitAPI"]

#: The only upstream mode this module can produce faithfully.
_SUPPORTED_MODES = ("Split",)

#: The aggregate types. ``ShapeType`` cannot be used here: a compound holding
#: exactly one solid reports ``"Solid"``, so only the Python type distinguishes
#: it from the solid itself.
_AGGREGATE_TYPES = tuple(
    kind
    for kind in (getattr(Part, "Compound", None), getattr(Part, "CompSolid", None))
    if isinstance(kind, type)
)


def _split(shape, arguments, fuzzy):
    """One General Fuse of a solid, face or edge; the shape when unchanged.

    General Fuse's ``Modified()`` map is empty for a shape the tools do not
    touch, so an empty group means "this is still one piece" rather than "the
    shape is gone": a split can never remove material.
    """
    if shape.isNull():
        return []
    _fused, split = shape.generalFuse(arguments, fuzzy)
    pieces = list(split[0]) if split else []
    return pieces or [shape]


def _pieces(base_shape, arguments, fuzzy):
    """The pieces of `base_shape` after splitting it with `arguments`.

    A compound is split through each of its children: OCCT's ``Modified()`` map
    is empty for a compound argument but non-empty for the children it changes.

    A shell or wire is split through its faces or edges. General Fuse splits a
    face or edge individually but leaves ``Modified(shell)``/``Modified(wire)``
    empty, which is upstream's ``splitAggregates`` case: the aggregate is split
    at the intersections. Returned as its face/edge pieces (upstream sews them
    back into shells after this step); an aggregate no tool touches is returned
    whole.
    """
    if base_shape.isNull():
        return []

    if _AGGREGATE_TYPES and isinstance(base_shape, _AGGREGATE_TYPES):
        pieces = []
        for child in base_shape.childShapes():
            pieces.extend(_pieces(child, arguments, fuzzy))
        return pieces

    kind = base_shape.ShapeType
    if kind == "Shell":
        parts = list(base_shape.Faces)
        pieces = [piece for part in parts for piece in _split(part, arguments, fuzzy)]
        if len(pieces) == len(parts) and all(
            piece.isSame(part) for piece, part in zip(pieces, parts)
        ):
            return [base_shape]
        return pieces
    if kind == "Wire":
        parts = list(base_shape.Edges)
        pieces = [piece for part in parts for piece in _split(part, arguments, fuzzy)]
        if len(pieces) == len(parts) and all(
            piece.isSame(part) for piece, part in zip(pieces, parts)
        ):
            return [base_shape]
        return pieces

    return _split(base_shape, arguments, fuzzy)


class SplitAPI:
    """The ``slice`` entry point of upstream ``BOPTools.SplitAPI``."""

    @staticmethod
    def slice(base_shape, tool_shapes, mode="Split", tolerance=None, **kwargs):
        """Split ``base_shape`` with ``tool_shapes``; see the module docstring.

        Parameters
        ----------
        base_shape : Part.Shape
            The shape to split. Its pieces are the result.
        tool_shapes : iterable of Part.Shape
            Slicing tools (planes, faces, ...). Each is passed to General Fuse
            wrapped in its own compound.
        mode : str
            Only ``"Split"`` is supported.
        tolerance : float or None
            Optional General Fuse fuzzy value.
        **kwargs
            Accepted and ignored for upstream signature compatibility.

        Returns
        -------
        Part.Shape
            The base shape's single remaining piece, or a ``Part.Compound`` of
            its pieces. ``Part.Shape()`` only for an empty compound.
        """
        del kwargs  # upstream-only options this exact subset does not use

        if mode not in _SUPPORTED_MODES:
            raise NotImplementedError(
                f"BOPTools.SplitAPI.slice supports only {_SUPPORTED_MODES!r} in "
                f"FerroCAD; got mode {mode!r}"
            )

        tools = list(tool_shapes)
        if not tools:
            raise ValueError("No slicing objects supplied!")

        # Upstream wraps every tool in a one-shape compound so the General Fuse
        # cannot return tool fragments as part of the base shape. Taking parts[0]
        # already excludes them, but keep the exact input shape upstream uses.
        arguments = [Part.makeCompound([tool]) for tool in tools]
        fuzzy = 0.0 if tolerance is None else float(tolerance)
        pieces = _pieces(base_shape, arguments, fuzzy)

        if not pieces:
            return Part.Shape()
        return pieces[0] if len(pieces) == 1 else Part.makeCompound(pieces)


def __getattr__(name):
    raise AttributeError(
        f"module {__name__!r} is a compatibility shim, not the BOPTools "
        f"workbench; it implements only {{{', '.join(__all__)}}}"
    )
