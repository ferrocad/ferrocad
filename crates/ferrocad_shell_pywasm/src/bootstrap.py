"""The free-standing bottom of the facade `python/` runs against, for the pywasm spike.

**The facade itself is shared.** ``FreeCADGui``, ``FerroCADGui`` and the ``FerroCAD``
package are installed from ``python/`` unchanged (see `lib.rs`); this file only supplies
what a `wasm32-unknown-unknown` RustPython cannot: the host gaps, the document model,
and four small bricks those shared modules sit on. Three things happen here:

1. **The host gaps.** ``wasm32-unknown-unknown`` has no POSIX module, so RustPython's
   frozen ``os``/``_io`` layer is unavailable and ``contextlib``, ``io``, ``pathlib``,
   ``traceback``, ``importlib`` and ``os`` cannot be imported. The shared scripts import
   all six, so this file supplies the handful of names they touch. The console goes
   through ``evaluate()``, which captures stdout with ``io.StringIO``.
2. **The document model.** ``_ferrocad.document()`` returns the property rows the Rust
   side computed with ``ferrocad_core`` (``present::kind`` / ``present::format``),
   read from the host's *active* document each time it is asked. This file turns them
   into the ``FreeCAD.ActiveDocument`` shape the script expects, so the console sees
   the document the shell is showing — including after a file is opened — and reports
   no active document (`None`) when there is none.
3. **The bricks under the shared facade** (B18b): ``FreeCAD.ParamGet`` over a dict,
   ``FreeCAD.Units.Quantity``, and the ``FerroCAD._params`` file controls. The shared
   modules use three parameter groups, seven quantity expressions and nothing else, so
   these are what stand between them and RustPython — not a re-implementation of the
   facade above them.

The spike's honest limit: these modules are **not** the ``*_py`` PyO3 bindings. Those
are CPython extension modules and cannot be loaded by RustPython; §6.3 records that as
the fork's central trade-off. What is shared is the *script* and the *rules* — the
kinds and the display text still come from ``ferrocad_core``, not from here. The
document is also **read-only** from this side: writes, recompute and the `Shape` API
are the document half's, and a write here raises rather than silently missing it.
"""

import json
import sys

import _ferrocad


def _payload():
    """The host's active document rows, or ``None`` when no document is open."""
    return json.loads(_ferrocad.document())


def _module(name, namespace):
    """A module named ``name`` whose attributes are ``namespace``.

    A missing attribute raises :class:`_Absent`, an ``AttributeError``, so
    ``hasattr``/``getattr(..., None)`` probes in the shared code behave as they do for a
    module that simply has no such name, and the message says this build does not
    install it (rather than a typo in this file, which would be a bug here).
    """
    module = type(sys)(name)
    module.__dict__.update(namespace)
    module.__dict__["__name__"] = name
    # Every module built here is a substitute for a native one, so a missing
    # attribute is a capability this half has not grown — never a typo in the
    # substitute, which would be a bug in this file. `_Absent` is what lets a
    # command predicate tell the two apart (`_enabled`).
    module.__dict__["__getattr__"] = _absent_attribute(name)
    sys.modules[name] = module
    return module


class _Absent(AttributeError):
    """A facade name this build does not install.

    An ``AttributeError``, so ``hasattr``/``except AttributeError`` behave exactly as
    they would for a module that simply has no such attribute, and reported as one
    (``_reported_name``) because that is what a script sees.
    """

    _reported_name = "AttributeError"


def _absent_attribute(module_name):
    """The ``__getattr__`` a facade module raises ``_Absent`` from."""

    def __getattr__(attribute):
        raise _Absent(
            "module %r has no attribute %r: this build does not install it"
            % (module_name, attribute)
        )

    return __getattr__


# --- the modules wasm32 cannot import -------------------------------------


class _StringIO:
    """Text buffer, enough for ``ferrocad_shell.evaluate``'s stdout capture."""

    def __init__(self, initial=""):
        self._chunks = [initial]

    def write(self, text):
        self._chunks.append(str(text))
        return len(str(text))

    def writelines(self, lines):
        for line in lines:
            self.write(line)

    def getvalue(self):
        return "".join(self._chunks)

    def flush(self):
        pass

    def close(self):
        pass

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


class _Suppress:
    def __init__(self, *exceptions):
        self._exceptions = exceptions or (Exception,)

    def __enter__(self):
        return self

    def __exit__(self, kind, value, tb):
        return kind is not None and issubclass(kind, self._exceptions)


class _Redirect:
    """``contextlib.redirect_stdout``/``redirect_stderr`` over ``sys``."""

    def __init__(self, attribute, target):
        self._attribute = attribute
        self._target = target

    def __enter__(self):
        self._saved = getattr(sys, self._attribute, None)
        setattr(sys, self._attribute, self._target)
        return self._target

    def __exit__(self, *exc):
        setattr(sys, self._attribute, self._saved)
        return False


class _Path:
    """The slice of ``pathlib`` the shell uses (``Path(...).name``/``is_file``)."""

    def __init__(self, *parts):
        self._text = "/".join(str(part) for part in parts)

    def __str__(self):
        return self._text

    @property
    def name(self):
        return self._text.rsplit("/", 1)[-1]

    @property
    def parent(self):
        head, _, _ = self._text.rpartition("/")
        return _Path(head or ".")

    @property
    def suffix(self):
        name = self.name
        return "" if "." not in name else "." + name.rsplit(".", 1)[-1]

    def with_name(self, name):
        """``Path(__file__).with_name(...)``, the shell's import-time catalogue read."""
        head, _, _ = self._text.rpartition("/")
        return _Path(head + "/" + name if head else name)

    def is_file(self):
        return False

    def exists(self):
        return False

    def __truediv__(self, other):
        return _Path(self._text, other)

    def __eq__(self, other):
        return isinstance(other, _Path) and str(self) == str(other)

    def __hash__(self):
        return hash(self._text)


_module(
    "io",
    {"StringIO": _StringIO, "TextIOBase": object, "BytesIO": _StringIO},
)
_module(
    "contextlib",
    {
        "suppress": _Suppress,
        "redirect_stdout": lambda target: _Redirect("stdout", target),
        "redirect_stderr": lambda target: _Redirect("stderr", target),
    },
)
_module("pathlib", {"Path": _Path, "PurePath": _Path})
_module("traceback", {"format_exc": lambda: ""})


def _import_module(name):
    """`importlib.import_module` over the frozen importer.

    The frozen ``importlib`` cannot load here: ``importlib._bootstrap_external``
    imports ``posix``, which ``host_env``-off builds do not have. The shared modules
    only call ``import_module``, and ``__import__`` is the same operation.
    """
    return __import__(name, fromlist=["*"])


_module("importlib", {"import_module": _import_module})


class _OsPathLike:
    """`os.PathLike`: the marker class `FreeCADGui.open_overlay` checks."""


def _os_fspath(value):
    """`os.fspath`: a path string, or the object's `__fspath__` result."""
    if isinstance(value, str):
        return value
    try:
        return value.__fspath__()
    except AttributeError:
        raise TypeError("expected str or os.PathLike, got %r" % (value,)) from None


def _os_fsdecode(value):
    """`os.fsdecode`: the filesystem path as text (there is one encoding here)."""
    value = _os_fspath(value)
    return value.decode("utf-8") if isinstance(value, bytes) else value


# `os` is only importable off `host_env`, which this build does not have; the shared
# `FreeCADGui` imports it at module level and uses these three names for a path-like
# argument. `path` exists for the workbench loader, which the browser never calls.
_module(
    "os",
    {
        "PathLike": _OsPathLike,
        "fspath": _os_fspath,
        "fsdecode": _os_fsdecode,
        "sep": "/",
        "altsep": None,
        "linesep": "\n",
        "path": _module(
            "os.path",
            {
                "join": lambda *parts: "/".join(str(part) for part in parts),
                "dirname": lambda path: str(path).rpartition("/")[0],
                "basename": lambda path: str(path).rpartition("/")[2],
                "abspath": lambda path: str(path),
                "exists": lambda path: False,
                "isfile": lambda path: False,
                "isdir": lambda path: False,
            },
        ),
    },
)


# --- the document model ---------------------------------------------------


class DocumentObject:
    """One object, as ``ferrocad_shell.properties`` reads it."""

    def __init__(self, row):
        self._row = row

    @property
    def Name(self):
        return self._row["name"]

    @property
    def Label(self):
        return self._row["label"]

    @property
    def TypeId(self):
        return self._row["type_id"]

    @property
    def PropertiesList(self):
        return [property_row["name"] for property_row in self._row["properties"]]

    @property
    def ExpressionEngine(self):
        # Pairs of (property, expression). The spike stores no expressions, but the
        # shell asks for the list on every row, so the shape matters.
        return [(name, "") for name in self._row["driven"]]

    def _row_of(self, prop):
        for property_row in self._row["properties"]:
            if property_row["name"] == prop:
                return property_row
        raise AttributeError(prop)

    def getTypeOfProperty(self, prop):
        # The flag names the payload carries (`ferrocad_core::status_names`), which is
        # what `getTypeOfProperty` reports natively. It used to answer with the type's
        # last segment, which made the script-visible `status` row disagree with
        # CPython's.
        return self._row_of(prop)["status"]

    def getTypeIdOfProperty(self, prop):
        return self._row_of(prop)["type_id"]

    def getEnumerationsOfProperty(self, prop):
        return self._row_of(prop)["choices"]

    def __getattr__(self, prop):
        if prop.startswith("_"):
            raise AttributeError(prop)
        return self._row_of(prop)["value"]

    def __setattr__(self, name, value):
        """Read-only: the document half owns the write path.

        Silently setting an instance attribute would look like an edit and change
        nothing, so a script that tries says so instead.
        """
        if name.startswith("_"):
            object.__setattr__(self, name, value)
            return
        raise RuntimeError(
            "pywasm: the document is read-only from Python; edit it in the shell"
        )

    def __repr__(self):
        return "<DocumentObject {}>".format(self.Name)


class Document:
    def __init__(self, payload):
        self._payload = payload
        self.Name = payload["name"]
        self.Label = payload["name"]
        # FreeCAD's `UndoCount`/`RedoCount`, from the payload's counts (which
        # `ferrocad_doc_api::payload` reads from `ferrocad_core`'s transaction
        # stacks). A payload rendered before those keys existed reports no history.
        # `UndoNames`/`RedoNames` are deliberately absent: undoing is a document
        # write, and this half has no write path, so `App_Undo` refuses at the first
        # name it asks for (`UndoNames`) with an AttributeError naming it.
        self.UndoCount = int(payload.get("undo_count", 0))
        self.RedoCount = int(payload.get("redo_count", 0))
        self.Objects = [DocumentObject(row) for row in payload["objects"]]

    def getObject(self, name):
        for object_row in self.Objects:
            if object_row.Name == name:
                return object_row
        return None

    def getObjectsByLabel(self, label):
        return [row for row in self.Objects if row.Label == label]

    def __repr__(self):
        return "<Document {}>".format(self.Name)


class _LiveDocument:
    """``FreeCAD.ActiveDocument``: the host's document, as it is now.

    The document half owns the document and can replace it (boot, open, close), so
    each attribute read asks the bridge again instead of holding the rows that
    existed at import. A read forwarded here returns an object from *that* read, so a
    caller that follows one attribute to the next sees one consistent snapshot.
    """

    def _document(self):
        payload = _payload()
        if payload is None:
            raise RuntimeError("no active document")
        return Document(payload)

    def __getattr__(self, name):
        if name.startswith("__"):
            raise AttributeError(name)
        return getattr(self._document(), name)

    def __repr__(self):
        try:
            return repr(self._document())
        except RuntimeError:
            return "<no active document>"


_ACTIVE = _LiveDocument()


def _active_document():
    """The live document, or ``None`` when the host has none open."""
    try:
        _ACTIVE.Name
    except RuntimeError:
        return None
    return _ACTIVE


def _get_document(name):
    try:
        return _ACTIVE if name == _ACTIVE.Name else None
    except RuntimeError:
        return None


def _get_object(name):
    try:
        return _ACTIVE.getObject(name)
    except RuntimeError:
        return None


# --- the bricks under the *shared* facade ---------------------------------
#
# `python/FreeCADGui/__init__.py`, `python/FerroCADGui/__init__.py` and the
# `python/FerroCAD` package are the same sources the desktop runs. Between them they
# need four things this build cannot supply natively, and none of them is the facade:
#
# 1. `FreeCAD.ParamGet` (three parameter groups; `FerroCAD/_grid`, `_axis`, `_gizmo`
#    and the provider settings all read and write through it);
# 2. `FreeCAD.Units.Quantity` (seven expressions, all in the settings coercion);
# 3. `os` (imported by `FreeCADGui`; see the host-gap section above);
# 4. the `FerroCAD._params` controls (`_configure_parameter_file` and
#    `_parameter_file_path`).
#
# Providing those is what lets the real modules be installed instead of a
# re-implementation (B18b).

_PARAMETERS = {}
_PARAMETER_FILE = None


class _ParameterGrp:
    """`FreeCAD.ParamGet`: one typed value per name, over `_PARAMETERS`.

    The native half persists this in a versioned JSON file; here it is a dict, so a
    value lasts for the process. The typed-name lists (`GetBools`, …) exist because
    the store uses them to tell "false" from "unset".
    """

    def __init__(self, path):
        self._path = str(path)

    def _values(self):
        return _PARAMETERS.setdefault(self._path, {})

    def GetGroup(self, name):
        return _ParameterGrp("%s/%s" % (self._path.rstrip("/"), name))

    def _get(self, name, kind, default):
        entry = self._values().get(name)
        if isinstance(entry, tuple) and entry[0] == kind:
            return entry[1]
        return default

    def _set(self, name, kind, value):
        self._values()[name] = (kind, value)

    def _names(self, kind):
        return [name for name, entry in self._values().items() if entry[0] == kind]

    def _remove(self, name, kind):
        entry = self._values().get(name)
        if isinstance(entry, tuple) and entry[0] == kind:
            del self._values()[name]

    def GetBool(self, name, default=False):
        return bool(self._get(name, "bool", default))

    def GetInt(self, name, default=0):
        return int(self._get(name, "int", default))

    def GetFloat(self, name, default=0.0):
        return float(self._get(name, "float", default))

    def GetString(self, name, default=""):
        return str(self._get(name, "string", default))

    def SetBool(self, name, value):
        self._set(name, "bool", bool(value))

    def SetInt(self, name, value):
        self._set(name, "int", int(value))

    def SetFloat(self, name, value):
        self._set(name, "float", float(value))

    def SetString(self, name, value):
        self._set(name, "string", str(value))

    def GetBools(self):
        return self._names("bool")

    def GetInts(self):
        return self._names("int")

    def GetFloats(self):
        return self._names("float")

    def GetStrings(self):
        return self._names("string")

    def RemBool(self, name):
        self._remove(name, "bool")

    def RemInt(self, name):
        self._remove(name, "int")

    def RemFloat(self, name):
        self._remove(name, "float")

    def RemString(self, name):
        self._remove(name, "string")

    def Clear(self):
        self._values().clear()


class _Quantity:
    """`FreeCAD.Units.Quantity` for the provider settings' numbers.

    Only what that code uses: a value, its declared unit, `Value`, `getValueAs` and
    the display string. There is no unit table here, so a conversion to a different
    unit is refused rather than mis-scaled.
    """

    def __init__(self, value, unit=None):
        self.Unit = "" if unit is None else str(unit)
        if isinstance(value, str):
            text = value.strip()
            number, _, suffix = text.partition(" ")
            if suffix:
                if self.Unit and suffix.strip() != self.Unit:
                    raise ValueError("cannot convert %r to %r" % (text, self.Unit))
                self.Unit = suffix.strip()
            value = number
        self.Value = float(value)

    def getValueAs(self, unit):
        if unit and self.Unit and str(unit) != self.Unit:
            raise ValueError("cannot convert %s to %s" % (self, unit))
        return _Quantity(self.Value, unit or self.Unit)

    def __float__(self):
        return self.Value

    def __str__(self):
        text = "%g" % self.Value
        return ("%s %s" % (text, self.Unit)).strip()

    def __repr__(self):
        return "Quantity(%r, %r)" % (self.Value, self.Unit)


def _configure_parameter_file(path):
    """`FerroCAD._params.set_parameter_file`: there is no file here."""
    global _PARAMETER_FILE
    _PARAMETER_FILE = path


def _parameter_file_path():
    """`FerroCAD._params.parameter_file`: always `None` in this build."""
    return _PARAMETER_FILE


_UNITS = _module("FreeCAD.Units", {"Quantity": _Quantity})


_module(
    "FreeCAD",
    {
        "ActiveDocument": _ACTIVE,
        "Version": lambda: ["0", "0", "0", "pywasm"],
        "__version__": "0.1.5",
        "backend": "pywasm",
        # The half is installed by a host that has a window (the browser module and
        # `--script-host=pywasm`); the crate's own tests boot it headlessly and read no
        # `GuiUp` branch. It is what makes `View_GridSettings` select the tree card
        # instead of opening the headless modal, which is the surface the shell draws.
        "GuiUp": 1,
        "getDocument": _get_document,
        "getObject": _get_object,
        # Upstream's lower-case accessor: the shared facade reads it (a provider switch
        # records the document it attached to). `None` when nothing is open.
        "activeDocument": _active_document,
        # The three bricks the shared facade needs (see above): a dict store, unit
        # arithmetic, and the FerroCAD-only parameter-file controls.
        "ParamGet": _ParameterGrp,
        "Units": _UNITS,
        "_kernel_id": "occt",
        "_configure_parameter_file": _configure_parameter_file,
        "_parameter_file_path": _parameter_file_path,
    },
)


# --- the engine's property rules, over the snapshot -----------------------


def _property_kind_of(obj, prop):
    return obj._row_of(prop)["kind"]


def _format_property(obj, prop):
    return obj._row_of(prop)["value"]


def _coerce_property_text(obj, prop, text):
    """The spike accepts the text verbatim: coercion is the engine's, and the
    engine is not reachable from RustPython (see the module docstring)."""
    return str(text)


_module(
    "ferrocad",
    {
        "propertyKindOf": _property_kind_of,
        "formatProperty": _format_property,
        "coercePropertyText": _coerce_property_text,
    },
)
