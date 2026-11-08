"""The FreeCAD surface ``python/ferrocad_shell`` runs against, for the pywasm spike.

Two things happen here, both on the Python side of the bridge:

1. **The host gaps.** ``wasm32-unknown-unknown`` has no POSIX module, so RustPython's
   frozen ``os``/``_io`` layer is unavailable and ``contextlib``, ``io``, ``pathlib``
   and ``traceback`` cannot be imported. ``ferrocad_shell`` imports all four, so this
   file supplies the handful of names it touches. They are small but real: the console
   goes through ``evaluate()``, which captures stdout with ``io.StringIO``.
2. **The document model.** ``_ferrocad.document()`` returns the property rows the Rust
   side computed with ``ferrocad_core`` (``present::kind`` / ``present::format``),
   read from the host's *active* document each time it is asked. This file turns them
   into the ``FreeCAD.ActiveDocument`` shape the script expects, so the console sees
   the document the shell is showing — including after a file is opened — and reports
   no active document (`None`) when there is none.

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
    """A module named ``name`` whose attributes are ``namespace``."""
    module = type(sys)(name)
    module.__dict__.update(namespace)
    module.__dict__["__name__"] = name
    sys.modules[name] = module
    return module


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


_module(
    "FreeCAD",
    {
        "ActiveDocument": _ACTIVE,
        "Version": lambda: ["0", "0", "0", "pywasm"],
        "backend": "pywasm",
        "getDocument": _get_document,
        "getObject": _get_object,
    },
)


def _no_providers(*_args, **_kwargs):
    return []


#: Commands the module registers at import (`FreeCADGui.addCommand('App_Move', …)`).
#: The shell's own catalogue is `ferrocad_shell.commands()`, so the spike records them
#: and hands nothing back to the UI: there is no `FreeCADGui` toolbar here.
_REGISTERED_COMMANDS = {}

_module(
    "FreeCADGui",
    {
        "_cancel_all": lambda: None,
        "updateGui": lambda: None,
        "addCommand": lambda name, command: _REGISTERED_COMMANDS.setdefault(
            name, command
        ),
        "removeCommand": lambda name: _REGISTERED_COMMANDS.pop(name, None),
        "listCommands": lambda: list(_REGISTERED_COMMANDS),
        "runCommand": lambda name: None,
        "activeDocument": _active_document,
        "ActiveDocument": _ACTIVE,
        "SendMsgToActiveView": lambda *_args: None,
        "getMainWindow": lambda: None,
    },
)
_module("FerroCAD", {})
_module(
    "FerroCADGui",
    {
        "providers": _no_providers,
        "activeProvider": lambda feature: None,
        "_cancel_all": lambda: None,
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
