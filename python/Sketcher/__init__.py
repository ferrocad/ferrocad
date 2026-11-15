"""The ``Sketcher`` module's Python surface: constraint records, not a solver.

FerroCAD ships the *model* a sketch constraint lives in, and not the numerical
solver that moves geometry to satisfy it. This module therefore provides the one
type upstream scripts construct by hand — :class:`Constraint` — and nothing else:
``Sketcher::SketchObject`` receives these objects through its own
``addConstraint``/``delConstraint`` methods (see
``docs/developer/workbenches/sketch-constraints.md``).

What is here
------------

``Constraint(type, *args, **kwargs)``
    A constraint record: the upstream type name, the geometry/point references
    and the datum value. It is a data holder. The document object stores it and
    reports it back; nothing is solved.

    ``First``/``Second``/``Third`` are upstream's GeoIds: a 0-based index into
    the sketch's geometry list, ``-1`` for the X axis (whose start point is the
    sketch root point) or ``-2`` for the Y axis. An unused reference is
    :data:`GeoUndef` (``-2000``), as upstream's ``GeoEnum``; it is *not* ``0``,
    because geometry ``0`` is a real curve.

    Accepted positional shapes, matching upstream:

    ``(type, geometry)``
        A unary constraint (``Vertical``, ``Horizontal``, ``Block``).
    ``(type, first, second)``
        Two geometry ids (``Equal``, ``Parallel``, ``Perpendicular``, …).
    ``(type, first, first_pos, second, second_pos)``
        Two geometry ids with point positions (``Coincident``, ``PointOnObject``,
        ``Symmetric``).
    ``(type, geometry, value)``
        A dimensional constraint on a whole curve (``Radius``, ``Diameter``).
    ``(type, geometry, point_pos, value)``
        A dimensional constraint on a point (``Distance`` and its X/Y forms,
        ``Weight``).
    ``(type, first, second, value)``
        An angle between two curves (``Angle``).

    Keyword arguments override the positional ones: ``First``, ``FirstPos``,
    ``Second``, ``SecondPos``, ``Third``, ``ThirdPos``, ``Value``, ``Name``,
    ``Driving``, ``IsActive``. ``Driving``/``IsActive`` default to ``True``.

``PointPos``
    Upstream's point-position numbering as plain class attributes:
    ``none = 0``, ``start = 1``, ``end = 2``, ``mid = 3``.

``GeoUndef``
    Upstream's ``GeoEnum::GeoUndef`` (``-2000``): the value of a reference the
    constraint does not use.

What is deliberately **not** here
---------------------------------

* No solver. ``SketchObject.solve()`` checks that every constraint references a
  geometry that exists and reports that; it never moves geometry. The numerical
  solve is the PlanGCS/OndselSolver binding in the backlog (P4 SK2).
* No ``RedundantConstraints`` analysis for the same reason: with no solve there
  is no rank information to report, so the list is empty rather than invented.
* No constraint *labels* (``LabelPosition``/``LabelDistance``). They position a
  drawing annotation, and the sketch constraint labels are not drawn; passing
  either is a ``TypeError`` naming the gap rather than silently dropping it.
* No ``InternalAlignment`` family and no ``SnellsLaw``. ``SketchObject`` refuses
  them too, with a ``ValueError`` that lists what it does store.
"""

from __future__ import annotations

__all__ = ["Constraint", "GeoUndef", "PointPos"]

#: Upstream's ``GeoEnum::GeoUndef``: the reference a constraint does not use.
GeoUndef = -2000


class PointPos:
    """Upstream's ``PointPos`` values, as the integers the API carries."""

    none = 0
    start = 1
    end = 2
    mid = 3


#: Constraint kinds whose three-integer-plus-value form puts a ``PointPos``
#: second (``Distance(geometry, point, value)``) rather than a geometry id.
_POINT_VALUE_KINDS = {"Distance", "DistanceX", "DistanceY", "Weight"}

_KEYS = (
    "First",
    "FirstPos",
    "Second",
    "SecondPos",
    "Third",
    "ThirdPos",
    "Value",
    "Name",
    "Driving",
    "IsActive",
)

_REFUSED_KEYS = ("LabelPosition", "LabelDistance")


class Constraint:
    """One sketch constraint record (see the module docstring for the shapes).

    Attributes mirror upstream's: ``Type`` is the constraint kind, ``First``/
    ``Second``/``Third`` are geometry ids, ``FirstPos``/``SecondPos``/``ThirdPos``
    are :class:`PointPos` values, ``Value`` is the datum (0 when the kind has
    none), ``Name`` is an optional label and ``Driving``/``IsActive`` say whether
    the constraint is a dimension and whether it is switched on.
    """

    def __init__(self, *args, **kwargs):
        if not args:
            raise TypeError(
                "Constraint() needs at least the constraint type, "
                'for example Constraint("Radius", 0, 5.0)'
            )
        type_name = args[0]
        if not isinstance(type_name, str):
            raise TypeError(
                "Constraint(): the first argument must be the constraint type "
                f"name, not {type(type_name).__name__}"
            )

        self.Type = type_name
        self.First = GeoUndef
        self.FirstPos = PointPos.none
        self.Second = GeoUndef
        self.SecondPos = PointPos.none
        self.Third = GeoUndef
        self.ThirdPos = PointPos.none
        self.Value = 0.0
        self.Name = ""
        self.Driving = True
        self.IsActive = True

        self._parse_positional(type_name, args[1:])

        for key, value in kwargs.items():
            if key in _REFUSED_KEYS:
                raise TypeError(
                    f"Constraint({type_name!r}): {key} is not part of FerroCAD's "
                    "sketch constraint model (constraint labels are not drawn)"
                )
            if key not in _KEYS:
                raise TypeError(
                    f"Constraint({type_name!r}) got an unexpected keyword "
                    f"argument {key!r}"
                )
            setattr(self, key, value)

    def _parse_positional(self, type_name, rest):
        if not rest:
            return

        ints = [arg for arg in rest if isinstance(arg, int) and not isinstance(arg, bool)]
        floats = [arg for arg in rest if isinstance(arg, float)]
        others = [
            arg
            for arg in rest
            if not isinstance(arg, (int, float)) or isinstance(arg, bool)
        ]
        if others:
            raise TypeError(
                f"Constraint({type_name!r}): positional arguments must be geometry "
                "ids (ints) and at most one datum value (float); got "
                + ", ".join(type(arg).__name__ for arg in rest)
            )
        if len(floats) > 1:
            raise TypeError(
                f"Constraint({type_name!r}): at most one datum value, got {rest!r}"
            )

        # The datum value, when present, is always the last argument.
        value_given = bool(floats)
        if value_given:
            if not isinstance(rest[-1], float):
                raise TypeError(
                    f"Constraint({type_name!r}): the datum value must be the last "
                    f"argument, got {rest[-1]!r}"
                )
            self.Value = floats[0]

        count = len(ints)
        if count == 1:
            self.First = ints[0]
        elif count == 2:
            if value_given:
                # `Distance(geometry, point, value)` names a point; every other
                # two-id form names two curves (`Angle(first, second, value)`).
                if type_name in _POINT_VALUE_KINDS:
                    self.First, self.FirstPos = ints
                else:
                    self.First, self.Second = ints
            else:
                self.First, self.Second = ints
        elif count == 3:
            self.First, self.Second, self.Third = ints
        elif count == 4:
            self.First, self.FirstPos, self.Second, self.SecondPos = ints
        else:
            raise TypeError(
                f"Constraint({type_name!r}): {count} geometry ids is not a shape "
                "FerroCAD understands (use 1, 2, 3 or 4)"
            )

    def __repr__(self):
        parts = [repr(self.Type), f"first={self.First}"]
        if self.FirstPos:
            parts.append(f"firstPos={self.FirstPos}")
        if self.Second != GeoUndef or self.SecondPos:
            parts.append(f"second={self.Second}")
        if self.SecondPos:
            parts.append(f"secondPos={self.SecondPos}")
        if self.Third != GeoUndef or self.ThirdPos:
            parts.append(f"third={self.Third}")
        if self.ThirdPos:
            parts.append(f"thirdPos={self.ThirdPos}")
        if self.Value:
            parts.append(f"value={self.Value}")
        if self.Name:
            parts.append(f"name={self.Name!r}")
        if not self.Driving:
            parts.append("driving=False")
        if not self.IsActive:
            parts.append("active=False")
        return f"Constraint({', '.join(parts)})"

    def __str__(self):
        return f"<Constraint {self.Type}>"


def __getattr__(name):
    raise AttributeError(
        f"module {__name__!r} ships only {{{', '.join(__all__)}}}; the Sketcher "
        "solver and its GUI tools are not part of FerroCAD (see "
        "docs/developer/workbenches/sketch-constraints.md)"
    )
