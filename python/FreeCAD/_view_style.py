"""The headless ``ViewObject`` style surface.

``ferrocad.ViewObject`` is the native, GUI-free view-provider model: it exposes
the *App* state a script reads headlessly (``Visibility``, ``Transparency``,
``ShapeColor``, ``ShapeAppearance``, ``ShapeMaterial``, ``DiffuseColor`` and
``DisplayMode``).  Draft's ``Draft_SetStyle`` (and several workbenches) also
read and write the line/point style properties of a GUI ``ViewProvider``:

``LineColor``, ``LineWidth``, ``PointColor``, ``PointSize`` and ``DrawStyle``.

The native class lives in the Rust extension, so this module adds the missing
properties there from Python.  Each value is process state, like a GUI
``ViewProvider``'s style: it starts at the ``View`` parameter-group default and
is read back through the same attribute.  The store is keyed by
``(document name, object name)`` and forgotten when the document closes, so a
closed document is not kept alive.

``ShapeColor``, ``Transparency`` and ``DisplayMode`` stay native — they are part
of ``ferrocad.ViewState`` and persist with the document.  Only the properties
whose native model does not exist yet are shimmed here.
"""

from __future__ import annotations

#: The View preference group the defaults come from (Draft's own defaults).
_VIEW_GROUP = "User parameter:BaseApp/Preferences/View"

#: The property names this module installs (in ``PropertiesList`` order).
_STYLE_PROPERTIES = ("LineColor", "LineWidth", "PointColor", "PointSize", "DrawStyle")

#: The view properties the native class already exposes.
_NATIVE_PROPERTIES = (
    "Visibility",
    "Transparency",
    "ShapeColor",
    "ShapeAppearance",
    "ShapeMaterial",
    "DiffuseColor",
    "DisplayMode",
)

#: FreeCAD's (and Draft's) draw-style enumeration, in index order.
DRAW_STYLES = ("Solid", "Dashed", "Dotted", "Dashdot")

#: The display modes a Part view provider advertises.
DISPLAY_MODES = ("Flat Lines", "Shaded", "Wireframe", "Points")

#: ``(document, object) -> {property: value}``.  Keyed by names rather than by
#: the ``ViewObject`` so a closed document can be released.
_STYLES: dict = {}

#: Set once the class was extended, so a reload is a no-op.
_INSTALLED = False

_DEFAULT_COLOUR = 0x000000FF
_DEFAULT_WIDTH = 2


def _view_group():
    import FreeCAD

    return FreeCAD.ParamGet(_VIEW_GROUP)


def _preference(name, default, *, unsigned=False):
    """Read a ``View`` preference, falling back to ``default``."""
    try:
        group = _view_group()
        if unsigned:
            return int(group.GetUnsigned(name, int(default)))
        return int(group.GetInt(name, int(default)))
    except Exception:  # noqa: BLE001 - the parameter store is optional
        return int(default)


def _rgba(value):
    """Coerce a colour (3/4-sequence or packed ``0xRRGGBBAA``) to RGBA floats."""
    if isinstance(value, (tuple, list)):
        if len(value) not in (3, 4):
            raise TypeError(
                "a colour must be an (r, g, b) or (r, g, b, a) sequence"
            )
        red, green, blue = (float(component) for component in value[:3])
        alpha = float(value[3]) if len(value) == 4 else 1.0
        return (red, green, blue, alpha)
    if isinstance(value, int):
        packed = value & 0xFFFFFFFF
        return (
            ((packed >> 24) & 0xFF) / 255.0,
            ((packed >> 16) & 0xFF) / 255.0,
            ((packed >> 8) & 0xFF) / 255.0,
            (packed & 0xFF) / 255.0,
        )
    raise TypeError(
        "a colour must be an (r, g, b[, a]) sequence or a packed 0xRRGGBBAA integer"
    )


def _key(view):
    """The ``(document, object)`` key of a view object, or ``None``."""
    obj = getattr(view, "Object", None)
    name = getattr(obj, "Name", None)
    document = getattr(obj, "Document", None)
    document_name = getattr(document, "Name", None)
    if not name or not document_name:
        return None
    return (str(document_name), str(name))


def _state(view, *, create=False):
    key = _key(view)
    if key is None:
        return None
    state = _STYLES.get(key)
    if state is None and create:
        state = {}
        _STYLES[key] = state
    return state


def _default(name):
    if name == "LineColor":
        return _rgba(_preference("DefaultShapeLineColor", _DEFAULT_COLOUR, unsigned=True))
    if name == "PointColor":
        return _rgba(
            _preference("DefaultShapeVertexColor", _DEFAULT_COLOUR, unsigned=True)
        )
    if name == "LineWidth":
        return _preference("DefaultShapeLineWidth", _DEFAULT_WIDTH)
    if name == "PointSize":
        return _preference("DefaultShapePointSize", _DEFAULT_WIDTH)
    if name == "DrawStyle":
        index = _preference("DefaultDrawStyle", 0)
        return DRAW_STYLES[index] if 0 <= index < len(DRAW_STYLES) else DRAW_STYLES[0]
    raise AttributeError(f"no headless style property {name!r}")


def _coerce(name, value):
    if name in ("LineColor", "PointColor"):
        return _rgba(value)
    if name in ("LineWidth", "PointSize"):
        return max(0, int(value))
    if name == "DrawStyle":
        style = str(value)
        if style not in DRAW_STYLES:
            raise ValueError(
                f"'{style}' is not a draw style; use one of {DRAW_STYLES!r}"
            )
        return style
    raise AttributeError(f"no headless style property {name!r}")


class _StyleProperty:
    """A data descriptor over the per-object style store."""

    __slots__ = ("name",)

    def __init__(self, name):
        self.name = name

    def __get__(self, instance, owner=None):
        if instance is None:
            return self
        state = _state(instance)
        if state is not None and self.name in state:
            return state[self.name]
        return _default(self.name)

    def __set__(self, instance, value):
        state = _state(instance, create=True)
        if state is None:
            raise AttributeError(
                "cannot store a view style on an object that is not in a document"
            )
        state[self.name] = _coerce(self.name, value)


def _properties_list(self):
    """The view object's property names, including the headless style set."""
    return list(_NATIVE_PROPERTIES) + list(_STYLE_PROPERTIES)


def _list_display_modes(self):
    """The display modes this view provider offers."""
    return list(DISPLAY_MODES)


def install(view_class=None) -> bool:
    """Install the missing style properties on ``ViewObject`` (idempotent)."""
    global _INSTALLED
    import ferrocad

    cls = view_class if view_class is not None else getattr(ferrocad, "ViewObject", None)
    if cls is None:
        return False
    if _INSTALLED:
        return True
    for name in _STYLE_PROPERTIES:
        if not hasattr(cls, name):
            setattr(cls, name, _StyleProperty(name))
    if not hasattr(cls, "PropertiesList"):
        cls.PropertiesList = property(_properties_list)
    if not hasattr(cls, "listDisplayModes"):
        cls.listDisplayModes = _list_display_modes
    _INSTALLED = True
    return True


def forget_document(name: str) -> None:
    """Drop the style state of a document that is closing."""
    for key in [key for key in _STYLES if key[0] == str(name)]:
        del _STYLES[key]


__all__ = ["DRAW_STYLES", "DISPLAY_MODES", "forget_document", "install"]
