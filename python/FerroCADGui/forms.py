"""Headless drivers for the four remaining Draft forms (Q5).

FerroCAD does not render Qt. ``FreeCADGui.PySideUic.loadUi`` returns a
``FormProxy`` model instead (Q2a), and this module drives that model: it seeds
the widgets from the Draft parameter group and from the active document, then
reads them back on accept. There is no window, no event loop and no Qt import.

The four commands registered at import time are upstream's remaining Draft
forms:

============================ ==========================================
Command                      Form
============================ ==========================================
``Draft_SetStyle``           ``TaskPanel_SetStyle.ui``
``Draft_Hatch``              ``dialogHatch.ui``
``Draft_AnnotationStyleEditor`` ``dialog_AnnotationStyleEditor.ui``
``Draft_Scale``              ``TaskPanel_Scale.ui`` (local, converted)
============================ ==========================================

``TaskPanel_Scale.ui`` is the one panel upstream builds in code
(``drafttaskpanels/task_scale.py``); the local copy is the declarative
equivalent described by ``docs/workbenches/qt-strategy.md`` §2.5. The other
three files are read from the pinned upstream checkout, resolved exactly as the
conformance harness resolves it (``FERROCAD_UPSTREAM``, then the sibling
``freecad-upstream`` of this repository). An explicit path always wins.

Programmatic entry point
------------------------

``open_panel(name, path=None)`` loads a form and returns a ``PanelController``:

``panel.form``
    The ``FormProxy``; widget names are the ``.ui`` object names.
``panel.fields()``
    JSON-ready field descriptions for a shell dialog spec.
``panel.values()``
    The current form values, in the same shape ``fields()`` reports.
``panel.apply(values)``
    Write shell values into the form (unknown names raise ``ValueError``).
``panel.accept(values=None)``
    Apply (optionally), commit, and return ``{"status": ..., "value": ...}``.
``panel.reject()``
    Discard the form without writing anything.

The SetStyle panel also exposes ``presets()``, ``save_preset(name)``,
``delete_preset(name)``, ``load_preset(name)`` and ``select_preset(name)``. The
annotation editor exposes ``styles()``, ``style_names()``, ``select_style(name)``,
``save_style(name)`` and ``editor_values()``. The hatch panel exposes
``patterns()``.

What is deliberately *not* modelled
-----------------------------------

* The forms are never rendered; layout/tab/scroll widgets are loaded (so their
  widget counts and names match upstream) but have no behaviour here.
* The annotation editor cannot push a style into live annotation objects: the
  headless ``ViewObject`` models only the shape-view properties
  (``ShapeColor``, ``Transparency``, ``DisplayMode``, ...), not upstream's
  ``FontName``/``TextColor``/``AnnotationStyle``. Accept therefore persists the
  style in the document's ``Meta`` store and in the Draft parameter group, and
  reports that no annotation view objects were updated. This is an explicit
  limitation, not a silent no-op.
* Upstream stores SetStyle presets in ``<user>/Draft/StylePresets.json``. Here
  they live in the Draft parameter group as one JSON string, as Q5 requires.
* ``QFontComboBox`` has no font enumeration in the proxy, so seeding a font
  adds that family to the combo's items before selecting it.
* The SetStyle ``applyButton``/``annotButton`` and the preset ``saveButton``
  are not wired to signals. Accept **does** apply the shape-view subset
  (``LineColor``, ``LineWidth``, ``PointColor``, ``PointSize``, ``DrawStyle``,
  ``ShapeColor``, ``Transparency``, ``DisplayMode``) to the selected objects'
  headless ``ViewObject``; annotation fonts need properties the headless model
  does not carry, so those fields only persist as Draft defaults. The
  ``saveButton`` needs ``QInputDialog``, which the shim raises on. Use
  :meth:`SetStylePanel.save_preset` and :meth:`SetStylePanel.select_preset`
  instead.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import NamedTuple

import FreeCAD
import FreeCADGui

#: The Draft preference group every command reads and writes.
DRAFT_PREFERENCES = "User parameter:BaseApp/Preferences/Mod/Draft"

#: The preferred key of the SetStyle preset store (a JSON object as a string).
PRESET_KEY = "StylePresets"

#: The four commands this module owns.
COMMANDS = (
    "Draft_SetStyle",
    "Draft_Hatch",
    "Draft_AnnotationStyleEditor",
    "Draft_Scale",
)

#: The upstream form each non-local command drives.
UPSTREAM_FORMS = {
    "Draft_SetStyle": "TaskPanel_SetStyle.ui",
    "Draft_Hatch": "dialogHatch.ui",
    "Draft_AnnotationStyleEditor": "dialog_AnnotationStyleEditor.ui",
    # Converted from code; the local copy is authoritative for this one.
    "Draft_Scale": "TaskPanel_Scale.ui",
}

#: Forms shipped with FerroCAD rather than read from the upstream checkout.
LOCAL_FORMS = frozenset({"Draft_Scale"})

_MODULE_DIR = Path(__file__).resolve().parent
_LOCAL_UI_DIR = _MODULE_DIR / "Resources" / "ui"

#: Upstream's arrow/dimension enumerations (``draftutils/utils.py``).
ARROW_TYPES = ("Dot", "Circle", "Arrow", "Tick", "Tick-2", "None")

#: Upstream's draw styles, in ``DefaultDrawStyle`` index order.
DRAW_STYLES = ("Solid", "Dashed", "Dotted", "Dashdot")

#: Upstream's shape display modes, in ``DefaultDisplayMode`` index order.
DISPLAY_MODES = ("Flat Lines", "Shaded", "Wireframe", "Points")


# ---------------------------------------------------------------------------
# Locating the .ui files
# ---------------------------------------------------------------------------


def _find_ui_dir(root: Path):
    """Return the Draft ``Resources/ui`` directory under ``root``, or ``None``."""
    for candidate in (
        root / "src" / "Mod" / "Draft",
        root / "Mod" / "Draft",
        root,
    ):
        ui_dir = candidate / "Resources" / "ui"
        if ui_dir.is_dir():
            return ui_dir
    return None


def _candidate_upstream_roots():
    """The checkout locations the conformance harness also understands."""
    override = os.environ.get("FERROCAD_UPSTREAM")
    if override:
        yield Path(override).expanduser()
    # ``python/FerroCADGui/forms.py`` -> repository root.
    repository = _MODULE_DIR.parents[1]
    yield repository.parent / "freecad-upstream"
    yield repository.parent.parent / "freecad-upstream"
    yield repository / "freecad-upstream"


def ui_directory(path=None) -> Path:
    """Resolve a Draft ``Resources/ui`` directory.

    ``path`` wins when given: it may name the directory, the Draft module or
    the upstream checkout root. Otherwise ``FERROCAD_UPSTREAM`` and the sibling
    ``freecad-upstream`` of this repository are tried. A missing directory is an
    actionable ``FileNotFoundError``, never an empty form.
    """
    if path is not None:
        candidate = Path(path).expanduser()
        if candidate.is_file():
            candidate = candidate.parent
        # A Draft module directory, the ui directory itself, or a checkout.
        for check in (candidate / "Resources" / "ui", candidate):
            if check.is_dir() and any(check.glob("*.ui")):
                return check
        found = _find_ui_dir(candidate)
        if found is not None:
            return found
        raise FileNotFoundError(
            f"Draft forms: {path!r} is not a Draft Resources/ui directory "
            "and contains no Draft .ui files"
        )

    tried = []
    for root in _candidate_upstream_roots():
        tried.append(str(root))
        found = _find_ui_dir(root)
        if found is not None:
            return found
    raise FileNotFoundError(
        "Draft forms: cannot find the upstream Draft .ui resources. "
        "Set FERROCAD_UPSTREAM to the freecad-upstream checkout, or pass an "
        "explicit path. Tried: " + ", ".join(tried)
    )


def ui_path(name: str, path=None) -> Path:
    """Resolve one form's ``.ui`` file.

    ``name`` is a command id (``"Draft_Hatch"``) or a file name. ``path`` may be
    the file itself, its directory, or the upstream checkout; it always wins.
    ``TaskPanel_Scale.ui`` resolves against the local ``Resources/ui`` unless an
    explicit path is given.
    """
    filename = UPSTREAM_FORMS.get(name, name)

    if path is not None:
        candidate = Path(path).expanduser()
        if candidate.is_dir():
            direct = candidate / filename
            if direct.is_file():
                return direct
            candidate = ui_directory(candidate) / filename
        if not candidate.is_file():
            raise FileNotFoundError(
                f"Draft forms: no form {filename!r} at {candidate}"
            )
        return candidate

    if name in LOCAL_FORMS or name == filename:
        local = _LOCAL_UI_DIR / filename
        if local.is_file():
            return local

    directory = ui_directory()
    candidate = directory / filename
    if not candidate.is_file():
        raise FileNotFoundError(
            f"Draft forms: {filename!r} is missing from {directory}; "
            "the upstream checkout may be incomplete"
        )
    return candidate


# ---------------------------------------------------------------------------
# Parameter and colour helpers
# ---------------------------------------------------------------------------


def _draft_group():
    """A fresh handle on the Draft preference group."""
    return FreeCAD.ParamGet(DRAFT_PREFERENCES)


def _rgba_int(value) -> int:
    """Pack an ``(r, g, b, a)`` proxy colour into a FreeCAD RGBA integer."""
    red, green, blue, alpha = (int(component) & 0xFF for component in value)
    return (red << 24) | (green << 16) | (blue << 8) | alpha


def _rgba_tuple(value) -> tuple:
    """Unpack a FreeCAD RGBA integer into the proxy's tuple form."""
    value = int(value) & 0xFFFFFFFF
    return (
        (value >> 24) & 0xFF,
        (value >> 16) & 0xFF,
        (value >> 8) & 0xFF,
        value & 0xFF,
    )


def _parse_hex_colour(text: str) -> int:
    text = str(text).strip().lstrip("#")
    if len(text) == 6:
        text += "ff"
    if len(text) != 8:
        raise ValueError(f"expected a #rrggbb or #rrggbbaa colour, got {text!r}")
    return int(text, 16)


def _hex_colour(value: int) -> str:
    return "#%08x" % (int(value) & 0xFFFFFFFF)


class _Field(NamedTuple):
    """One form field: widget name, label, value kind and parameter key."""

    name: str
    label: str
    kind: str
    param: str = ""
    default: object = None


#: Field kinds map onto the shell's dialog kinds in ``fields()``.
_SHELL_KIND = {
    "color": "text",
    "font": "text",
    "file": "text",
}


# ---------------------------------------------------------------------------
# Generic form access
# ---------------------------------------------------------------------------


def _read_storage(form, field: _Field):
    """Read a field from the form in its storage form (colours as ints)."""
    widget = getattr(form, field.name)
    kind = field.kind
    if kind == "color":
        return _rgba_int(widget.property("color"))
    if kind == "int":
        return int(widget.value())
    if kind == "float":
        return float(widget.value())
    if kind == "quantity":
        return float(widget.property("rawValue"))
    if kind == "bool":
        return bool(widget.isChecked())
    if kind == "text":
        return str(widget.text())
    if kind == "font":
        return str(widget.currentText())
    if kind == "enum":
        return int(widget.currentIndex())
    if kind == "file":
        return str(widget.fileName())
    raise ValueError(f"unknown field kind {kind!r} for {field.name!r}")


def _write_storage(form, field: _Field, value, *, strict: bool = False) -> None:
    """Write a storage-form value into the form.

    ``strict`` refuses an enum index outside the combo's items (a shell value);
    the seeding path clamps instead, so a stale preference still loads.
    """
    widget = getattr(form, field.name)
    kind = field.kind
    if kind == "color":
        widget.setProperty("color", _rgba_tuple(int(value)))
    elif kind == "quantity":
        widget.setProperty("rawValue", float(value))
    elif kind in ("int", "float"):
        widget.setValue(value)
    elif kind == "bool":
        widget.setChecked(bool(value))
    elif kind == "text":
        widget.setText(str(value))
    elif kind == "font":
        _set_combo_text(widget, str(value))
    elif kind == "enum":
        _set_combo_index(widget, int(value), strict=strict)
    elif kind == "file":
        widget.setFileName(str(value))
    else:  # pragma: no cover - the table above is closed
        raise ValueError(f"unknown field kind {kind!r} for {field.name!r}")


def _set_combo_text(widget, text: str) -> None:
    """Select ``text`` in a font combo, adding it first when necessary.

    The proxy has no font enumeration, so the preferred family must be present
    as an item before ``setCurrentText`` can select it.
    """
    items = [widget.itemText(index) for index in range(widget.count())]
    if text not in items:
        widget.addItem(text)
    widget.setCurrentText(text)


def _set_combo_index(widget, index: int, *, strict: bool) -> None:
    count = widget.count()
    if count == 0:
        if strict and index >= 0:
            raise ValueError(f"{widget.objectName()}: the combo box has no items")
        return
    if index < -1 or index >= count:
        if strict:
            raise ValueError(
                f"{widget.objectName()}: index {index} is outside 0..{count - 1}"
            )
        index = count - 1 if index >= count else -1
    widget.setCurrentIndex(index)


def _combo_text(widget) -> str:
    index = widget.currentIndex()
    if 0 <= index < widget.count():
        return widget.itemText(index)
    return widget.currentText()


def _combo_items(widget) -> list:
    return [widget.itemText(index) for index in range(widget.count())]


def _set_combo_items(widget, items) -> None:
    current = _combo_text(widget)
    widget.clear()
    widget.addItems(list(items))
    if current in items:
        widget.setCurrentText(current)


def _json_value(form, field: _Field):
    """A shell-shaped value: enum as text, colour as ``#rrggbbaa``."""
    value = _read_storage(form, field)
    if field.kind == "color":
        return _hex_colour(value)
    if field.kind == "enum":
        widget = getattr(form, field.name)
        index = int(value)
        if 0 <= index < widget.count():
            return widget.itemText(index)
        return widget.currentText()
    return value


def _from_json(form, field: _Field, value):
    """Convert a shell-shaped value into storage form."""
    kind = field.kind
    if kind == "color":
        return value if isinstance(value, int) else _parse_hex_colour(value)
    if kind == "enum":
        widget = getattr(form, field.name)
        wanted = str(value)
        for index, item in enumerate(_combo_items(widget)):
            if item == wanted:
                return index
        if wanted == "":
            return -1
        raise ValueError(
            f"{field.name}: {wanted!r} is not one of {_combo_items(widget)!r}"
        )
    if kind == "int":
        return int(value)
    if kind in ("float", "quantity"):
        return float(value)
    if kind == "bool":
        if isinstance(value, str):
            return value.strip().lower() in ("1", "true", "yes", "on")
        return bool(value)
    return str(value)


def _field_json(form, field: _Field) -> dict:
    item = {
        "name": field.name,
        "label": field.label,
        "kind": _SHELL_KIND.get(field.kind, field.kind),
        "value": _json_value(form, field),
    }
    if field.kind == "enum":
        item["values"] = _combo_items(getattr(form, field.name))
    elif field.kind in ("int", "float"):
        widget = getattr(form, field.name)
        item["min"] = widget.minimum()
        item["max"] = widget.maximum()
    return item


def _read_parameter(field: _Field):
    group = _draft_group()
    if field.kind == "color":
        return group.GetUnsigned(field.param, int(field.default))
    if field.kind in ("int", "enum"):
        return group.GetInt(field.param, int(field.default))
    if field.kind in ("float", "quantity"):
        return group.GetFloat(field.param, float(field.default))
    if field.kind == "bool":
        return group.GetBool(field.param, bool(field.default))
    return group.GetString(field.param, str(field.default))


def _write_parameter(field: _Field, value) -> None:
    group = _draft_group()
    if field.kind == "color":
        group.SetUnsigned(field.param, int(value))
    elif field.kind in ("int", "enum"):
        group.SetInt(field.param, int(value))
    elif field.kind in ("float", "quantity"):
        group.SetFloat(field.param, float(value))
    elif field.kind == "bool":
        group.SetBool(field.param, bool(value))
    else:
        group.SetString(field.param, str(value))


# ---------------------------------------------------------------------------
# The controller
# ---------------------------------------------------------------------------


class PanelController:
    """A loaded form plus its accept/reject path.

    Subclasses supply ``_field_specs()`` (the fields this panel reads and
    writes), ``_seed()`` (fill the form) and ``_commit()`` (persist the form).
    """

    command: str = ""
    title: str = ""

    def __init__(self, path) -> None:
        self.ui_path = Path(path)
        self.form = FreeCADGui.PySideUic.loadUi(str(self.ui_path))
        self._seed()

    # -- what a shell dialog spec needs ------------------------------------

    def _field_specs(self):
        raise NotImplementedError

    def fields(self) -> list:
        """JSON-ready descriptions of every field this panel drives."""
        return [_field_json(self.form, field) for field in self._field_specs()]

    def values(self) -> dict:
        """The current form values, keyed by field name."""
        return {
            field.name: _json_value(self.form, field)
            for field in self._field_specs()
        }

    def apply(self, values: dict) -> None:
        """Write shell values into the form; unknown names are an error."""
        specs = {field.name: field for field in self._field_specs()}
        for name, value in values.items():
            field = specs.get(name)
            if field is None:
                raise ValueError(
                    f"{self.command}: unknown field {name!r}; "
                    f"expected one of {sorted(specs)}"
                )
            _write_storage(self.form, field, _from_json(self.form, field, value),
                           strict=True)

    # -- the two exits -----------------------------------------------------

    def accept(self, values=None) -> dict:
        """Apply optional shell values, commit, and close the form."""
        if values:
            self.apply(values)
        result = self._commit()
        self._finish(True)
        return result

    def reject(self) -> dict:
        """Discard the form; nothing is written."""
        self._finish(False)
        return {"status": f"{self.command}: cancelled; nothing was written"}

    def _finish(self, accepted: bool) -> None:
        """Mirror QDialog accept/reject when the form root is a dialog."""
        if self.form.findChild("QDialog") is not None:
            if accepted:
                self.form.accept()
            else:
                self.form.reject()

    # -- subclass hooks ----------------------------------------------------

    def _seed(self) -> None:
        raise NotImplementedError

    def _commit(self) -> dict:
        raise NotImplementedError

    def _storage_values(self) -> dict:
        return {
            field.name: _read_storage(self.form, field)
            for field in self._field_specs()
        }

    def _json_values(self) -> dict:
        return {
            field.name: _json_value(self.form, field)
            for field in self._field_specs()
        }


# ---------------------------------------------------------------------------
# Draft_SetStyle
# ---------------------------------------------------------------------------


_STYLE_FIELDS = (
    _Field("ShapeColor", "Shape color", "color", "DefaultShapeColor", 0xCCCCCCFF),
    _Field("AmbientColor", "Ambient shape color", "color",
           "DefaultAmbientColor", 0x333333FF),
    _Field("EmissiveColor", "Emissive shape color", "color",
           "DefaultEmissiveColor", 0x000000FF),
    _Field("SpecularColor", "Specular shape color", "color",
           "DefaultSpecularColor", 0x000000FF),
    _Field("Transparency", "Shape transparency", "int",
           "DefaultShapeTransparency", 0),
    _Field("Shininess", "Shape shininess", "int", "DefaultShapeShininess", 0),
    _Field("LineColor", "Line color", "color", "DefaultShapeLineColor", 0x000000FF),
    _Field("LineWidth", "Line width", "int", "DefaultShapeLineWidth", 2),
    _Field("PointColor", "Point color", "color", "DefaultShapeVertexColor",
           0x000000FF),
    _Field("PointSize", "Point size", "int", "DefaultShapePointSize", 2),
    _Field("DrawStyle", "Draw style", "enum", "DefaultDrawStyle", 0),
    _Field("DisplayMode", "Display mode", "enum", "DefaultDisplayMode", 0),
    _Field("TextColor", "Text color", "color", "DefaultTextColor", 0x000000FF),
    _Field("TextFont", "Font name", "font", "textfont", "sans"),
    _Field("TextSize", "Font size", "quantity", "textheight", 3.5),
    _Field("LineSpacing", "Line spacing factor", "float", "LineSpacing", 1.0),
    _Field("ScaleMultiplier", "Scale multiplier", "float",
           "DefaultAnnoScaleMultiplier", 1.0),
    _Field("AnnoLineColor", "Line and arrow color", "color",
           "DefaultAnnoLineColor", 0x000000FF),
    _Field("AnnoLineWidth", "Line width", "int", "DefaultAnnoLineWidth", 1),
    _Field("ArrowStyleStart", "Start arrow type", "enum", "dimsymbolstart", 2),
    _Field("ArrowSizeStart", "Start arrow size", "quantity", "arrowsizestart", 1.0),
    _Field("ArrowStyleEnd", "End arrow type", "enum", "dimsymbolend", 2),
    _Field("ArrowSizeEnd", "End arrow size", "quantity", "arrowsizeend", 1.0),
    _Field("ShowUnit", "Show unit", "bool", "showUnit", False),
    _Field("UnitOverride", "Unit override", "text", "overrideUnit", ""),
    _Field("DimOvershoot", "Dimension line overshoot", "quantity",
           "dimovershoot", 0.0),
    _Field("ExtLines", "Extension line length", "quantity", "extlines", 0.0),
    _Field("ExtOvershoot", "Extension line overshoot", "quantity",
           "extovershoot", 0.0),
    _Field("TextSpacing", "Text spacing", "quantity", "dimspacing", 1.0),
)


class SetStylePanel(PanelController):
    """``Draft_SetStyle``: the Draft defaults plus named presets.

    Accept writes every field into the Draft parameter group. Presets are a JSON
    object stored in that same group (``StylePresets``), so they survive a
    process restart without the upstream ``StylePresets.json`` file.
    """

    command = "Draft_SetStyle"
    title = "Set Style"

    def _field_specs(self):
        return _STYLE_FIELDS

    def _seed(self) -> None:
        for field in _STYLE_FIELDS:
            _write_storage(self.form, field, _read_parameter(field), strict=False)
        self._sync_preset_combo()

    # -- presets -----------------------------------------------------------

    def presets(self) -> dict:
        """The stored presets, ``{name: {field: value}}``."""
        raw = _draft_group().GetString(PRESET_KEY, "")
        if not raw:
            return {}
        try:
            data = json.loads(raw)
        except json.JSONDecodeError as error:
            raise ValueError(
                f"Draft style presets in {DRAFT_PREFERENCES}: "
                f"{PRESET_KEY!r} is not valid JSON: {error}"
            ) from error
        if not isinstance(data, dict):
            raise ValueError(
                f"Draft style presets in {DRAFT_PREFERENCES}: "
                f"{PRESET_KEY!r} must be a JSON object"
            )
        return data

    def _store_presets(self, presets: dict) -> None:
        _draft_group().SetString(PRESET_KEY, json.dumps(presets, sort_keys=True))
        self._sync_preset_combo()

    def _sync_preset_combo(self) -> None:
        _set_combo_items(self.form.comboPresets, ["Load preset"] + list(self.presets()))

    def save_preset(self, name: str) -> dict:
        """Store the current form values under ``name`` and return them."""
        name = str(name).strip()
        if not name:
            raise ValueError("a preset name must not be empty")
        presets = self.presets()
        presets[name] = self._storage_values()
        self._store_presets(presets)
        return dict(presets[name])

    def delete_preset(self, name: str) -> bool:
        """Remove ``name``; ``False`` when it was not stored."""
        presets = self.presets()
        if name not in presets:
            return False
        del presets[name]
        self._store_presets(presets)
        return True

    def load_preset(self, name: str) -> dict:
        """Fill the form from a stored preset and return its values."""
        presets = self.presets()
        if name not in presets:
            raise KeyError(f"no Draft style preset {name!r}")
        preset = presets[name]
        for field in _STYLE_FIELDS:
            _write_storage(
                self.form,
                field,
                preset.get(field.name, _read_parameter(field)),
                strict=False,
            )
        return {field.name: _read_storage(self.form, field) for field in _STYLE_FIELDS}

    def select_preset(self, name: str) -> dict:
        """Load a preset and select it in the preset combo."""
        values = self.load_preset(name)
        self.form.comboPresets.setCurrentText(name)
        return values

    # -- accept ------------------------------------------------------------

    def _commit(self) -> dict:
        values = self._storage_values()
        for field in _STYLE_FIELDS:
            _write_parameter(field, values[field.name])
        applied = self._apply_to_selection(values)
        if applied:
            suffix = "; applied to " + ", ".join(applied)
        elif FreeCADGui.Selection.getSelection():
            suffix = "; the selected object has no headless view style properties"
        else:
            suffix = "; nothing selected, so the defaults apply to new objects"
        return {
            "status": (
                f"style applied: {len(values)} values written to "
                f"{DRAFT_PREFERENCES}{suffix}"
            ),
            "value": self._json_values(),
        }

    @staticmethod
    def _apply_to_selection(values) -> list:
        """Push the shape-view subset onto the selected view objects.

        Upstream's ``apply_style_to_obj`` gates each assignment on the view
        object's ``PropertiesList``; the headless model now advertises the
        line/point style properties alongside the native appearance ones, so
        the same rule works here. Annotation-only fields are skipped: the
        headless ``ViewObject`` has no font/text properties to receive them.
        """
        applied = []
        for obj in FreeCADGui.Selection.getSelection():
            view = getattr(obj, "ViewObject", None)
            if view is None:
                continue
            properties = set(getattr(view, "PropertiesList", ()) or ())
            if "LineColor" not in properties:
                continue
            if "ShapeColor" in properties:
                view.ShapeColor = _rgba_tuple(int(values["ShapeColor"]))
            if "Transparency" in properties:
                view.Transparency = int(values["Transparency"])
            if "LineColor" in properties:
                view.LineColor = _rgba_tuple(int(values["LineColor"]))
            if "LineWidth" in properties:
                view.LineWidth = int(values["LineWidth"])
            if "PointColor" in properties:
                view.PointColor = _rgba_tuple(int(values["PointColor"]))
            if "PointSize" in properties:
                view.PointSize = int(values["PointSize"])
            if "DrawStyle" in properties:
                index = int(values["DrawStyle"])
                view.DrawStyle = (
                    DRAW_STYLES[index] if 0 <= index < len(DRAW_STYLES) else DRAW_STYLES[0]
                )
            if "DisplayMode" in properties:
                index = int(values["DisplayMode"])
                modes = getattr(view, "listDisplayModes", None)
                modes = list(modes()) if callable(modes) else list(DISPLAY_MODES)
                if 0 <= index < len(modes):
                    view.DisplayMode = modes[index]
            applied.append(obj.Name)
        return applied


# ---------------------------------------------------------------------------
# Draft_Hatch
# ---------------------------------------------------------------------------


_HATCH_FIELDS = (
    _Field("File", "PAT file", "file"),
    _Field("Pattern", "Pattern", "enum"),
    _Field("Scale", "Scale", "float"),
    _Field("Rotation", "Rotation", "float"),
    _Field("Translate", "Align to face", "bool"),
)


def _patterns_in(filename: str) -> list:
    """The ``*NAME, description`` lines of a PAT file, as upstream reads them."""
    if not filename or not os.path.isfile(filename):
        return []
    names = []
    with open(filename, encoding="utf-8", errors="replace") as handle:
        for line in handle:
            if line.startswith("*"):
                name = line[1:].split(",")[0].strip()
                if name:
                    names.append(name)
    return names


class HatchPanel(PanelController):
    """``Draft_Hatch``: hatch parameters, optionally applied to a selection."""

    command = "Draft_Hatch"
    title = "Hatch"

    def _field_specs(self):
        return _HATCH_FIELDS

    def _seed(self) -> None:
        group = _draft_group()
        self.form.Scale.setValue(group.GetFloat("HatchPatternScale", 1000.0))
        self.form.Rotation.setValue(group.GetFloat("HatchPatternRotation", 0.0))
        self.form.Translate.setChecked(
            group.GetBool("HatchPatternTranslate", False)
        )
        self.form.File.setFileName(group.GetString("HatchPatternFile", ""))
        self._refresh_patterns()
        name = group.GetString("HatchPatternName", "")
        if name:
            self.form.Pattern.setCurrentText(name)
        # Connect after seeding: the initial name must not erase the pattern.
        self.form.File.fileNameChanged.connect(self._on_file_changed)

    def _on_file_changed(self, filename) -> None:
        self._refresh_patterns(filename)

    def patterns(self) -> list:
        """The patterns in the form's current PAT file."""
        return _patterns_in(self.form.File.fileName())

    def _refresh_patterns(self, filename=None) -> None:
        filename = self.form.File.fileName() if filename is None else filename
        patterns = _patterns_in(filename)
        current = _combo_text(self.form.Pattern)
        self.form.Pattern.clear()
        self.form.Pattern.addItems(patterns)
        if current in patterns:
            self.form.Pattern.setCurrentText(current)

    def apply(self, values: dict) -> None:
        # The file reloads the pattern list, so it must be applied first.
        remaining = dict(values)
        if "File" in remaining:
            field = next(item for item in _HATCH_FIELDS if item.name == "File")
            _write_storage(
                self.form,
                field,
                _from_json(self.form, field, remaining.pop("File")),
                strict=True,
            )
        super().apply(remaining)

    def _commit(self) -> dict:
        form = self.form
        filename = form.File.fileName()
        pattern = _combo_text(form.Pattern)
        scale = float(form.Scale.value())
        rotation = float(form.Rotation.value())
        translate = bool(form.Translate.isChecked())

        group = _draft_group()
        group.SetString("HatchPatternFile", filename)
        group.SetString("HatchPatternName", pattern)
        group.SetFloat("HatchPatternScale", scale)
        group.SetFloat("HatchPatternRotation", rotation)
        group.SetBool("HatchPatternTranslate", translate)

        applied = []
        for obj in FreeCADGui.Selection.getSelection():
            properties = list(getattr(obj, "PropertiesList", ()) or ())
            # Upstream decides from File/Pattern; the object is a hatch.
            if "File" not in properties or "Pattern" not in properties:
                continue
            obj.File = filename
            obj.Pattern = pattern
            if "Scale" in properties:
                obj.Scale = scale
            if "Rotation" in properties:
                obj.Rotation = rotation
            if "Translate" in properties:
                obj.Translate = translate
            applied.append(obj.Name)

        if applied:
            status = (
                "hatch settings saved and applied to "
                + ", ".join(applied)
            )
        else:
            status = (
                "hatch settings saved; no supported object selected "
                "(an object needs File and Pattern properties)"
            )
        return {
            "status": status,
            "value": {
                "File": filename,
                "Pattern": pattern,
                "Scale": scale,
                "Rotation": rotation,
                "Translate": translate,
                "applied": applied,
            },
        }


# ---------------------------------------------------------------------------
# Draft_AnnotationStyleEditor
# ---------------------------------------------------------------------------


_ANNOTATION_FIELDS = (
    _Field("ArrowSizeStart", "Start arrow size", "quantity", "arrowsizestart", 1.0),
    _Field("ArrowSizeEnd", "End arrow size", "quantity", "arrowsizeend", 1.0),
    _Field("ArrowTypeStart", "Start arrow type", "enum", "dimsymbolstart", 2),
    _Field("ArrowTypeEnd", "End arrow type", "enum", "dimsymbolend", 2),
    _Field("Decimals", "Number of decimals", "int", "dimPrecision", 2),
    _Field("DimOvershoot", "Dimension line overshoot", "quantity",
           "dimovershoot", 0.0),
    _Field("ExtLines", "Extension line length", "quantity", "extlines", 0.0),
    _Field("ExtOvershoot", "Extension line overshoot", "quantity",
           "extovershoot", 0.0),
    _Field("FontName", "Font name", "font", "textfont", "sans"),
    _Field("FontSize", "Font size", "quantity", "textheight", 3.5),
    _Field("LineColor", "Line and arrow color", "color",
           "DefaultAnnoLineColor", 0x000000FF),
    _Field("LineSpacing", "Line spacing factor", "float", "LineSpacing", 1.0),
    _Field("LineWidth", "Line width", "int", "DefaultAnnoLineWidth", 1),
    _Field("ScaleMultiplier", "Scale multiplier", "float",
           "DefaultAnnoScaleMultiplier", 1.0),
    _Field("ShowLine", "Show dimension line", "bool", "DimShowLine", True),
    _Field("ShowUnit", "Show unit", "bool", "showUnit", False),
    _Field("TextColor", "Text color", "color", "DefaultTextColor", 0x000000FF),
    _Field("TextSpacing", "Text spacing", "quantity", "dimspacing", 1.0),
    _Field("UnitOverride", "Unit override", "text", "overrideUnit", ""),
)

_STYLE_PREFIX = "Draft_Style_"


class AnnotationStylePanel(PanelController):
    """``Draft_AnnotationStyleEditor``: annotation styles stored in ``Doc.Meta``.

    Styles live under ``Draft_Style_<name>`` keys, exactly as upstream stores
    them. Accept also writes the edited values into the Draft parameter group
    (the headless ``ViewObject`` has no annotation properties to push them to);
    the returned ``value`` names the objects that were updated, which is empty
    in FerroCAD today.
    """

    command = "Draft_AnnotationStyleEditor"
    title = "Annotation Styles"

    def _field_specs(self):
        return _ANNOTATION_FIELDS

    def _seed(self) -> None:
        combo = self.form.comboBoxStyles
        self._styles = self._read_styles()
        combo.clear()
        combo.addItems(["", "Add new…"] + list(self._styles))
        self.current_style = None
        self._fill_editor(self._default_values())
        combo.currentIndexChanged.connect(self._on_style_changed)
        if self._styles:
            combo.setCurrentIndex(2)
        else:
            self._update_buttons()

    def _default_values(self) -> dict:
        return {field.name: _read_parameter(field) for field in _ANNOTATION_FIELDS}

    def _fill_editor(self, style: dict) -> None:
        for field in _ANNOTATION_FIELDS:
            _write_storage(
                self.form,
                field,
                style.get(field.name, _read_parameter(field)),
                strict=False,
            )
        self._update_buttons()

    def _update_buttons(self) -> None:
        enabled = bool(getattr(self, "current_style", None))
        self.form.pushButtonRename.setEnabled(enabled)
        self.form.pushButtonDelete.setEnabled(enabled)

    # -- the style table ---------------------------------------------------

    def editor_values(self) -> dict:
        """The editor's values in storage form."""
        return self._storage_values()

    def styles(self) -> dict:
        """A shallow copy of the loaded style table."""
        return {name: dict(values) for name, values in self._styles.items()}

    def style_names(self) -> list:
        return list(self._styles)

    def _style_index(self, name: str) -> int:
        for index, item in enumerate(_combo_items(self.form.comboBoxStyles)):
            if item == name:
                return index
        return -1

    def select_style(self, name: str) -> dict:
        """Select ``name`` in the combo (adding it when absent) and fill the form."""
        combo = self.form.comboBoxStyles
        index = self._style_index(name)
        if index < 0:
            self._styles.setdefault(name, self.editor_values())
            combo.addItem(name)
            index = combo.count() - 1
        combo.setCurrentIndex(index)
        return self.editor_values()

    def _on_style_changed(self, index) -> None:
        if getattr(self, "_loading", False):
            return
        if self.current_style:
            self._styles[self.current_style] = self.editor_values()
        if index <= 1:
            self.current_style = None
            self._fill_editor(self._default_values())
        else:
            name = self.form.comboBoxStyles.itemText(index)
            self.current_style = name
            self._fill_editor(self._styles[name])
        self._update_buttons()

    def save_style(self, name: str) -> dict:
        """Store the editor values as ``name`` in the document and return them."""
        name = str(name).strip()
        if not name:
            raise ValueError("a style name must not be empty")
        self._styles[name] = self.editor_values()
        self.current_style = name
        combo = self.form.comboBoxStyles
        index = self._style_index(name)
        if index < 0:
            combo.addItem(name)
            index = combo.count() - 1
        combo.setCurrentIndex(index)
        self._save_styles()
        return dict(self._styles[name])

    def _read_styles(self) -> dict:
        document = FreeCAD.ActiveDocument
        if document is None:
            return {}
        styles = {}
        for key, value in (document.Meta or {}).items():
            if not key.startswith(_STYLE_PREFIX):
                continue
            name = key[len(_STYLE_PREFIX):]
            try:
                data = json.loads(value)
            except (TypeError, json.JSONDecodeError) as error:
                raise ValueError(
                    f"document {document.Name!r}: style {name!r} is not valid "
                    f"JSON: {error}"
                ) from error
            if not isinstance(data, dict):
                raise ValueError(
                    f"document {document.Name!r}: style {name!r} must be a "
                    "JSON object"
                )
            styles[name] = self._repair_style(data)
        return styles

    def _repair_style(self, style: dict) -> dict:
        """Fill missing keys from the current defaults (upstream's repair)."""
        source = dict(style)
        # V1.0 -> V1.1: one ArrowSize/ArrowType became Start/End pairs.
        for old in ("ArrowSize", "ArrowType"):
            if (
                old in source
                and old + "Start" not in source
                and old + "End" not in source
            ):
                source[old + "Start"] = source[old]
                source[old + "End"] = source[old]

        repaired = {}
        for field in _ANNOTATION_FIELDS:
            default = _read_parameter(field)
            raw = source.get(field.name)
            if raw is None:
                repaired[field.name] = default
                continue
            try:
                if field.kind == "bool":
                    repaired[field.name] = bool(raw)
                elif field.kind in ("int", "color"):
                    repaired[field.name] = int(raw)
                elif field.kind == "enum":
                    if isinstance(raw, str):
                        repaired[field.name] = ARROW_TYPES.index(raw)
                    else:
                        repaired[field.name] = int(raw)
                elif field.kind in ("float", "quantity"):
                    repaired[field.name] = float(raw)
                else:
                    repaired[field.name] = str(raw)
            except (TypeError, ValueError):
                repaired[field.name] = default
        return repaired

    def _save_styles(self) -> bool:
        document = FreeCAD.ActiveDocument
        if document is None:
            return False
        meta = dict(document.Meta or {})
        for key in list(meta):
            if key.startswith(_STYLE_PREFIX) and key[len(_STYLE_PREFIX):] not in self._styles:
                del meta[key]
        for name, values in self._styles.items():
            meta[_STYLE_PREFIX + name] = json.dumps(values, sort_keys=True)
        document.Meta = meta
        return True

    # -- accept ------------------------------------------------------------

    def _commit(self) -> dict:
        editor = self.editor_values()
        for field in _ANNOTATION_FIELDS:
            _write_parameter(field, editor[field.name])

        if self.current_style:
            self._styles[self.current_style] = editor
        saved = self._save_styles()

        if self.current_style:
            status = (
                f"annotation style {self.current_style!r} updated in "
                f"{DRAFT_PREFERENCES}"
            )
        else:
            status = (
                "annotation parameters written to "
                f"{DRAFT_PREFERENCES}; no named style was selected"
            )
        if not saved:
            status += "; no active document, so styles were not stored"
        status += "; no annotation view objects exist headlessly"

        return {
            "status": status,
            "value": {
                "style": self.current_style,
                "styles": sorted(self._styles),
                "document": saved,
                "values": self._json_values(),
            },
        }


# ---------------------------------------------------------------------------
# Draft_Scale
# ---------------------------------------------------------------------------


_SCALE_FIELDS = (
    _Field("xValue", "X-factor", "float"),
    _Field("yValue", "Y-factor", "float"),
    _Field("zValue", "Z-factor", "float"),
    _Field("lock", "Uniform scaling", "bool"),
    _Field("isCopy", "Copy", "bool"),
)


class ScalePanel(PanelController):
    """``Draft_Scale``: the scale factors and the copy option."""

    command = "Draft_Scale"
    title = "Scale"

    def _field_specs(self):
        return _SCALE_FIELDS

    def _seed(self) -> None:
        group = _draft_group()
        self.form.lock.setChecked(group.GetBool("ScaleUniform", False))
        self.form.isCopy.setChecked(group.GetBool("ScaleCopy", False))

    def _commit(self) -> dict:
        form = self.form
        x = float(form.xValue.value())
        y = float(form.yValue.value())
        z = float(form.zValue.value())
        uniform = bool(form.lock.isChecked())
        if uniform:
            y = z = x
            form.yValue.setValue(y)
            form.zValue.setValue(z)

        group = _draft_group()
        group.SetBool("ScaleUniform", uniform)
        group.SetBool("ScaleCopy", bool(form.isCopy.isChecked()))

        applied = []
        for obj in FreeCADGui.Selection.getSelection():
            properties = list(getattr(obj, "PropertiesList", ()) or ())
            if "ScaleVector" in properties:
                obj.ScaleVector = FreeCAD.Vector(x, y, z)
                if "Scale" in properties and x == y == z:
                    obj.Scale = x
                applied.append(obj.Name)
            elif "Scale" in properties and x == y == z:
                obj.Scale = x
                applied.append(obj.Name)

        if applied:
            status = "scale applied to " + ", ".join(applied)
        else:
            status = (
                "scale factors saved; no supported object selected "
                "(an object needs a ScaleVector property)"
            )

        return {
            "status": status,
            "value": {
                "x": x,
                "y": y,
                "z": z,
                "uniform": uniform,
                "copy": bool(form.isCopy.isChecked()),
                "applied": applied,
            },
        }


# ---------------------------------------------------------------------------
# open_panel and the commands
# ---------------------------------------------------------------------------


_PANELS = {
    "Draft_SetStyle": SetStylePanel,
    "Draft_Hatch": HatchPanel,
    "Draft_AnnotationStyleEditor": AnnotationStylePanel,
    "Draft_Scale": ScalePanel,
}


def open_panel(name: str, path=None) -> PanelController:
    """Load ``name`` headlessly and return its controller.

    ``name`` is one of :data:`COMMANDS`; ``path`` overrides the form location.
    A missing extension propagates ``FreeCADGui.PySideUic.loadUi``'s actionable
    ``ImportError``; a missing ``.ui`` propagates ``FileNotFoundError``.
    """
    panel_class = _PANELS.get(name)
    if panel_class is None:
        raise KeyError(
            f"unknown Draft form {name!r}; expected one of {sorted(_PANELS)}"
        )
    return panel_class(ui_path(name, path))


def _dialog_spec(panel: PanelController) -> dict:
    """Turn a controller into the dialog spec ``FreeCADGui._payload`` accepts."""
    return {
        "title": panel.title,
        "fields": panel.fields(),
        "accept": "OK",
        "cancel": "Cancel",
        "on_accept": panel.accept,
    }


class _FormCommand:
    """One Draft command, driving its headless form."""

    command = ""
    menu = ""
    tooltip = ""
    accel = ""

    def GetResources(self) -> dict:
        return {
            "MenuText": self.menu or self.command,
            "ToolTip": self.tooltip,
            "Accel": self.accel,
            "Group": "Draft",
        }

    def IsActive(self) -> bool:
        # Every accept path has a parameter-store effect, so the command can
        # run without a document or a selection.
        return True

    def Activated(self) -> dict:
        return _dialog_spec(open_panel(self.command))


class _SetStyleCommand(_FormCommand):
    command = "Draft_SetStyle"
    menu = "Set Style"
    tooltip = "Sets the default Draft style and manages style presets"
    accel = "S, S"


class _HatchCommand(_FormCommand):
    command = "Draft_Hatch"
    menu = "Hatch"
    tooltip = "Creates hatches on the faces of a selected object"
    accel = "H, A"


class _AnnotationStyleCommand(_FormCommand):
    command = "Draft_AnnotationStyleEditor"
    menu = "Annotation Styles"
    tooltip = "Opens an editor to manage or create annotation styles"


class _ScaleCommand(_FormCommand):
    command = "Draft_Scale"
    menu = "Scale"
    tooltip = "Scales the selected objects"


_COMMAND_INSTANCES = (
    _SetStyleCommand(),
    _HatchCommand(),
    _AnnotationStyleCommand(),
    _ScaleCommand(),
)

for _instance in _COMMAND_INSTANCES:
    FreeCADGui.addCommand(_instance.command, _instance)
del _instance


__all__ = [
    "ARROW_TYPES",
    "DRAW_STYLES",
    "DISPLAY_MODES",
    "COMMANDS",
    "DRAFT_PREFERENCES",
    "PRESET_KEY",
    "UPSTREAM_FORMS",
    "AnnotationStylePanel",
    "HatchPanel",
    "PanelController",
    "ScalePanel",
    "SetStylePanel",
    "open_panel",
    "ui_directory",
    "ui_path",
]
