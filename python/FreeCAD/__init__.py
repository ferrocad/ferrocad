"""A drop-in, Rust-backed stand-in for FreeCAD's Python ``App`` module.

The public surface mirrors the parts of upstream FreeCAD that headless scripts
touch::

    import FreeCAD
    doc = FreeCAD.newDocument("HelloWorld")
    obj = doc.addObject("App::FeaturePython", "Box")
    doc.recompute()

The implementation is the PyO3 extension ``ferrocad``, which binds the Rust
``ferrocad_core`` engine. The C++ PyCXX bindings are not involved at all.

``fc`` exposes the application too: the document registry, the active document and
FreeCAD-style unique naming all live in the Rust core
(``ferrocad_core::Application``). This facade mirrors the ``FreeCAD`` module surface
and keeps no state of its own.
"""

from __future__ import annotations

import builtins as _builtins  # the facade defines `open` (document); keep the builtin.
import importlib.machinery
import importlib.util
import json
import os
import sys
import tempfile
import weakref


def _part_is_builtin() -> bool:
    """Whether `Part` is registered as a built-in (the app's edition build).

    An edition links `ferrocad`, `Part`, `Mesh` and `MeshPart` into the binary, so
    they already share one core and `FreeCAD` must not import a second extension
    from disk. A host interpreter has no built-in `Part`; `importlib` then finds the
    `python/Part.py` shim (or nothing at all), and the combined extension is the
    right source for all four modules.
    """
    try:
        spec = importlib.util.find_spec("Part")
    except (ImportError, ValueError):
        return False
    return spec is not None and spec.loader is importlib.machinery.BuiltinImporter


# Prefer the combined extension when the host has no built-in Part. It publishes
# `ferrocad`, `Part`, `Mesh` and `MeshPart` from one `.so`, so they share a
# document registry; a standalone `ferrocad.abi3.so` would give this facade a
# second core. A kernel-free build has neither, so the import is optional.
if not _part_is_builtin():
    try:
        import ferrocad_combined as _ferrocad_combined  # noqa: F401
        del _ferrocad_combined
    except ImportError:
        pass

import ferrocad as _fc

backend = "ferrocad"

Document = _fc.Document
DocumentObject = _fc.DocumentObject
Quantity = _fc.Quantity
StringHasher = _fc.StringHasher
StringID = _fc.StringID
Vector = _fc.Vector
Matrix = _fc.Matrix
Placement = _fc.Placement
Rotation = _fc.Rotation
TypeId = _fc.TypeId
BoundBox = _fc.BoundBox
Material = _fc.Material
Vector2d = _fc.Vector2d
GuiUp = 0
__version__ = _fc.__version__
_kernel_id = _fc._kernel_id

# Core/utility submodules (the M4 Base/Units/Console surface).
from . import Base, Units, Console

# The native ``ferrocad.ViewObject`` models the App-side appearance state. The
# line/point style properties Draft's ``SetStyle`` writes are installed on the
# class here, so a headless ``obj.ViewObject.LineColor = ...`` sets and reads
# back (see ``_view_style`` for why they are not native yet).
from . import _view_style as _view_style

_view_style.install()

ScaleType = Base.ScaleType

addDocumentObserver = _fc.addDocumentObserver
removeDocumentObserver = _fc.removeDocumentObserver

# The document registry (which documents are open, which is active, FreeCAD-style
# unique naming) lives in the Rust core; `fc` exposes it, so the facade keeps no
# state of its own.


def _notify_document(event, name):
    """Deliver one document event to the GUI slot, when it is loaded.

    The App facade never imports the GUI layer: a headless script or an App-only
    session keeps ``FerroCADGui``/``FreeCADGui`` out of ``sys.modules``. When a
    host (the shell) or a test already imported ``FerroCADGui``, its document
    observers hear the event. That slot is ``FerroCADGui.addDocumentListener``;
    ``FreeCADGui.addDocumentObserver`` forwards to it.

    Deactivation fires while the outgoing document is still open and active, so
    a provider can detach a per-document attachment (the Coin grid) before the
    document disappears.
    """
    gui = sys.modules.get("FerroCADGui")
    dispatch = getattr(gui, "_document_event", None) if gui is not None else None
    if dispatch is not None:
        dispatch(event, name)


def newDocument(name=None, hidden=False, temp=False):
    previous = activeDocument()
    if previous is not None:
        _notify_document("deactivated", previous.Name)
    try:
        document = _fc.newDocument(name)
    except Exception:
        current = activeDocument()
        if previous is not None and current is not None and current.Name == previous.Name:
            _notify_document("activated", previous.Name)
        raise
    _notify_document("created", document.Name)
    _notify_document("activated", document.Name)
    return document

def open(name, hidden=False, temporary=False):
    return openDocument(name, hidden, temporary)

# Upstream alias (`FreeCAD.openDocument(path)`).
def openDocument(path, hidden=False, temporary=False):
    previous = activeDocument()
    if previous is not None:
        _notify_document("deactivated", previous.Name)
    try:
        document = _fc.openDocument(path)
    except Exception:
        current = activeDocument()
        if previous is not None and current is not None and current.Name == previous.Name:
            _notify_document("activated", previous.Name)
        raise
    _notify_document("created", document.Name)
    _notify_document("activated", document.Name)
    return document

def closeDocument(name):
    document = _fc.getDocument(name)
    if document is None:
        raise ValueError("no document named '%s'" % name)
    current = activeDocument()
    if current is not None and current.Name == document.Name:
        _notify_document("deactivated", document.Name)
    _fc.closeDocument(name)
    _view_style.forget_document(document.Name)
    _notify_document("closed", document.Name)

def getDocument(name):
    return _fc.getDocument(name)

def setActiveDocument(name):
    document = _fc.getDocument(name)
    current = activeDocument()
    switching = document is not None and (
        current is None or current.Name != document.Name
    )
    if switching and current is not None:
        _notify_document("deactivated", current.Name)
    _fc.setActiveDocument(name)
    if switching:
        _notify_document("activated", document.Name)

def listDocuments():
    return _fc.listDocuments()

def activeDocument():
    return _fc.activeDocument()

def Version():
    return __version__.split(".") + ["rust-fc", ""]

# App-level build configuration. Upstream fills this from CMake; tests read it as
# ``"BUILD_X" in FreeCAD.__cmake__``. We build no optional module in-tree, so it is
# empty (every optional-module test is skipped).
__cmake__: dict = {}


def getResourceDir():
    """The FerroCAD resource root (upstream's ``<prefix>/share``).

    The result ends with the platform separator: upstream callers concatenate a
    relative path directly (``App.getResourceDir() + "Mod/TechDraw/..."``).
    Workbench data lives under ``Mod/`` here, mirroring FreeCAD's install layout.
    """
    import os

    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    return os.path.join(root, "")


def getHomePath():
    """The FreeCAD installation root (upstream's ``App.getHomePath()``).

    This is the directory that owns the ``Mod`` workbench tree. A workbench
    imported from a source checkout names that root (upstream's
    ``<home>/Mod/Draft/__init__.py``); otherwise this facade's own root is the
    home, the same root :func:`getResourceDir` reports. The result ends with the
    platform separator, as upstream's does.
    """
    import os
    import sys

    for name in ("Draft", "Part", "Import", "Mesh"):
        module = sys.modules.get(name)
        path = getattr(module, "__file__", None)
        if not path:
            continue
        directory = os.path.dirname(os.path.abspath(path))
        # At most `<home>/Mod/<workbench>/<module>.py`; do not wander above it.
        for _ in range(3):
            if os.path.basename(directory) == "Mod":
                return os.path.join(os.path.dirname(directory), "")
            parent = os.path.dirname(directory)
            if parent == directory:
                break
            directory = parent

    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    return os.path.join(root, "")


__all__ = [
    "Version",
    "newDocument",
    "closeDocument",
    "getDocument",
    "listDocuments",
    "openDocument",
    "activeDocument",
    "Document",
    "DocumentObject",
    "ActiveDocument",
    "backend",
    "ParamGet",
    "ParameterGrp",
    "ScaleType",
    "writeRecoverySnapshotToTransientDir",
]


def __getattr__(name):
    # PEP 562: resolved lazily because the active document changes over time.
    if name == "ActiveDocument":
        return activeDocument()
    raise AttributeError("module 'FreeCAD' has no attribute '%s'" % name)


# ---------------------------------------------------------------------------
# Parameters (App-level config store) — typed values and nested groups, backed
# by memory alone until a file is configured.
#
# The file backend is opt-in: an explicit path through
# ``FerroCAD.set_parameter_file``, else ``FERROCAD_PARAM_FILE``, else the
# platform config file in a GUI session only. Headless tests therefore stay in
# memory and never touch a user's config.
# ---------------------------------------------------------------------------

# root path -> {"values": {(group, name, kind): value}, "groups": {group, ...},
#                "handles": {group: [weakref to each live ParameterGrp]}}
_parameter_stores = {}

#: The version written to (and accepted from) the JSON parameter file.
_PARAM_FILE_VERSION = 1

#: Top-level keys the file format defines. Anything else is ignored with a
#: single report, so a newer FerroCAD file still loads what we understand.
_PARAM_FILE_KEYS = frozenset({"version", "values", "groups"})

#: The path set by ``FerroCAD.set_parameter_file``; ``None`` means "resolve
#: through the environment/default" in :func:`_parameter_file_path`.
_PARAM_FILE_EXPLICIT = None

#: Lazy-load and report-once bookkeeping. ``path``/``loaded`` make the first
#: ``ParamGet`` load the configured file exactly once per configured path.
_PARAM_FILE_STATE = {"path": None, "loaded": False, "reported": set()}


def _parameter_file_path():
    """The parameter file in use, or ``None`` while the store is memory-only.

    An explicit path wins, then ``FERROCAD_PARAM_FILE``, then the platform
    config file. The default is enabled only when ``FreeCAD.GuiUp`` is truthy,
    so a headless process never writes to the user's real configuration.
    """
    if _PARAM_FILE_EXPLICIT is not None:
        return _PARAM_FILE_EXPLICIT

    environment = os.environ.get("FERROCAD_PARAM_FILE")
    if environment:
        return os.path.abspath(os.path.expanduser(environment))

    if not GuiUp:
        return None

    base = os.environ.get("XDG_CONFIG_HOME")
    if not base:
        base = os.path.join(os.path.expanduser("~"), ".config")
    return os.path.join(base, "ferrocad", "parameters.json")


def _configure_parameter_file(path):
    """Set the file backend (``None``/``""`` clears the explicit path).

    This is the private half of ``FerroCAD.set_parameter_file``. Loading stays
    lazy: the next ``ParamGet`` reads the file. Calling it again with the same
    path re-arms the load, which is how a restart is simulated in tests.
    """
    if path is None or path == "":
        explicit = None
    else:
        explicit = os.path.abspath(os.path.expanduser(os.fspath(path)))
    global _PARAM_FILE_EXPLICIT
    _PARAM_FILE_EXPLICIT = explicit
    _PARAM_FILE_STATE["path"] = None
    _PARAM_FILE_STATE["loaded"] = False


def _report_parameter_file(path, kind, detail):
    """Report one parameter-file problem per path and kind."""
    key = (path, kind)
    if key in _PARAM_FILE_STATE["reported"]:
        return
    _PARAM_FILE_STATE["reported"].add(key)
    print("FreeCAD: parameter file %s: %s" % (path, detail), file=sys.stderr)


def _raw_param_store(root):
    """The in-memory store for ``root``, without triggering a file load."""
    store = _parameter_stores.setdefault(root, {"values": {}, "groups": set()})
    store.setdefault("handles", {})
    return store


def _param_store(root):
    _load_parameter_file()
    return _raw_param_store(root)


def _load_parameter_file():
    """Merge the configured file into memory, once per configured path.

    A missing file is a normal first run. An unreadable or corrupt file starts
    empty, is reported once, and is overwritten only by the next mutation.
    Unknown top-level keys are ignored with a single report.
    """
    path = _parameter_file_path()
    if path is None:
        return
    state = _PARAM_FILE_STATE
    if state["loaded"] and state["path"] == path:
        return
    state["path"] = path
    state["loaded"] = True

    try:
        with _builtins.open(path, "r", encoding="utf-8") as handle:
            data = json.load(handle)
    except FileNotFoundError:
        return
    except (OSError, ValueError) as exc:
        _report_parameter_file(path, "read", "cannot read it (%s); starting empty" % exc)
        return

    if not isinstance(data, dict):
        _report_parameter_file(path, "format", "the root is not a JSON object; starting empty")
        return

    extra = sorted(set(data) - _PARAM_FILE_KEYS)
    if extra:
        _report_parameter_file(
            path, "keys", "ignoring unknown key(s): %s" % ", ".join(extra)
        )

    if data.get("version") != _PARAM_FILE_VERSION:
        _report_parameter_file(
            path,
            "version",
            "unsupported version %r; starting empty" % (data.get("version"),),
        )
        return

    raw_groups = data.get("groups", [])
    raw_values = data.get("values", [])
    if not isinstance(raw_groups, list) or not isinstance(raw_values, list):
        _report_parameter_file(
            path, "format", "'groups' and 'values' must be lists; starting empty"
        )
        return

    for entry in raw_groups:
        if not isinstance(entry, dict):
            _report_parameter_file(path, "groups", "ignoring malformed group entries")
            continue
        root, group = entry.get("root"), entry.get("path")
        if not isinstance(root, str) or not isinstance(group, str):
            _report_parameter_file(path, "groups", "ignoring malformed group entries")
            continue
        _raw_param_store(root)["groups"].add(_collapse(group))

    for entry in raw_values:
        if not isinstance(entry, dict):
            _report_parameter_file(path, "values", "ignoring malformed value entries")
            continue
        root = entry.get("root")
        group = entry.get("group")
        name = entry.get("name")
        kind = entry.get("kind")
        value = entry.get("value")
        if (
            not isinstance(root, str)
            or not isinstance(group, str)
            or not isinstance(name, str)
            or not isinstance(kind, str)
            or not isinstance(value, (bool, int, float, str))
        ):
            _report_parameter_file(path, "values", "ignoring malformed value entries")
            continue
        group = _collapse(group)
        store = _raw_param_store(root)
        store["values"][(group, name, kind)] = value
        store["groups"].add(group)


def _save_parameter_file():
    """Atomically write the whole store to the configured file.

    A no-op when no file is configured. A write failure is reported once and
    leaves the previous file untouched (the temp file is removed).
    """
    path = _parameter_file_path()
    if path is None:
        return

    groups = []
    values = []
    for root, store in _parameter_stores.items():
        for group in sorted(store["groups"]):
            groups.append({"root": root, "path": group})
        for (group, name, kind), value in store["values"].items():
            values.append(
                {
                    "root": root,
                    "group": group,
                    "name": name,
                    "kind": kind,
                    "value": value,
                }
            )
    data = {"version": _PARAM_FILE_VERSION, "groups": groups, "values": values}

    directory = os.path.dirname(path) or "."
    temporary = None
    try:
        os.makedirs(directory, exist_ok=True)
        descriptor, temporary = tempfile.mkstemp(
            prefix=".parameters-", suffix=".json", dir=directory
        )
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            json.dump(data, handle, ensure_ascii=False, indent=2)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
        temporary = None
    except (OSError, TypeError, ValueError) as exc:
        _report_parameter_file(path, "write", "cannot save it (%s)" % exc)
    finally:
        if temporary is not None:
            try:
                os.unlink(temporary)
            except OSError:
                pass


def _collapse(path):
    return "/".join(part for part in path.split("/") if part)


class ParameterGrp:
    """An in-memory stand-in for ``Base.ParameterGrp`` (typed, nested groups)."""

    def __init__(self, path=""):
        path = path.strip("/")
        if ":" in path:
            root, _, rest = path.partition(":")
        else:
            root, rest = path, ""
        self._init(root, rest.strip("/"), _param_store(root))

    def _init(self, root, group, store):
        self._root = root
        self._group = group
        self._store = store
        store["groups"].add(group)
        # Upstream FreeCAD keeps a group alive while a Python handle references it
        # (`BaseTests.testGroup`), so remember every live handle.
        store.setdefault("handles", {}).setdefault(group, []).append(weakref.ref(self))

    def _full(self, sub):
        return (self._group + "/" + sub) if self._group else sub

    def _get(self, name, kind, default):
        return self._store["values"].get((self._group, name, kind), default)

    def _set(self, name, kind, value):
        # A write re-creates a group removed with `RemGroup`, matching FreeCAD.
        self._store["groups"].add(self._group)
        self._store["values"][(self._group, name, kind)] = value
        _save_parameter_file()

    def _rem(self, name, kind):
        key = (self._group, name, kind)
        if key in self._store["values"]:
            del self._store["values"][key]
            _save_parameter_file()

    def _names(self, kind):
        """The names in this group with a value of `kind`, in insertion order."""
        return [
            name
            for (group, name, entry_kind) in self._store["values"]
            if group == self._group and entry_kind == kind
        ]

    # -- groups -------------------------------------------------------------
    def GetGroup(self, name, _store=None, _group=None):
        sub = _collapse(name)
        if not sub:
            raise ValueError("empty group name")
        child = ParameterGrp.__new__(ParameterGrp)
        child._init(self._root, self._full(sub), self._store)
        return child

    def GetGroupName(self):
        return self._group.split("/")[-1] if self._group else ""

    def GetGroups(self):
        """The immediate sub-group names (FreeCAD `GetGroups`)."""
        prefix = self._group + "/" if self._group else ""
        names = set()
        for group in self._store["groups"]:
            if group == self._group or not group.startswith(prefix):
                continue
            rest = group[len(prefix) :]
            if rest and "/" not in rest:
                names.add(rest)
        return sorted(names)

    def HasGroup(self, name):
        return self._full(_collapse(name)) in self._store["groups"]

    def RemGroup(self, name):
        """Remove a child group, unless a live handle still references it.

        Upstream FreeCAD defers the deletion of a **referenced** group; the
        conformance suite asserts it (`BaseTests.ParameterTestCase.testGroup`).
        A live `ParameterGrp` handle for the group or any descendant therefore
        keeps it alive. With no live handle, its values and descendants go.
        """
        target = self._full(_collapse(name))
        handles = self._store.setdefault("handles", {})
        referenced = False
        for group, refs in list(handles.items()):
            if group != target and not group.startswith(target + "/"):
                continue
            alive = [ref for ref in refs if ref() is not None]
            handles[group] = alive
            referenced = referenced or bool(alive)
        if referenced:
            return

        values = self._store["values"]
        changed = False
        for key in list(values):
            group = key[0]
            if group == target or group.startswith(target + "/"):
                del values[key]
                changed = True
        groups = self._store["groups"]
        for group in list(groups):
            if group == target or group.startswith(target + "/"):
                groups.discard(group)
                changed = True
        if changed:
            _save_parameter_file()

    def Clear(self):
        group = self._group
        changed = False
        for key in list(self._store["values"]):
            if group == "" or key[0] == group or key[0].startswith(group + "/"):
                del self._store["values"][key]
                changed = True
        if changed:
            _save_parameter_file()

    # -- typed accessors ----------------------------------------------------
    def GetInt(self, name, default=0):
        return int(self._get(name, "int", default))

    def SetInt(self, name, value):
        self._set(name, "int", int(value))

    def RemInt(self, name):
        self._rem(name, "int")

    def GetInts(self):
        """Names with an int value in this group (FreeCAD returns names)."""
        return self._names("int")

    def GetUnsigned(self, name, default=0):
        value = self._get(name, "int", None)
        return default if value is None else int(value) & 0xFFFFFFFF

    def GetUnsigneds(self):
        """Names with an unsigned value in this group (FreeCAD returns names)."""
        return self._names("int")

    def SetUnsigned(self, name, value):
        self._set(name, "int", int(value) & 0xFFFFFFFF)

    def RemUnsigned(self, name):
        self._rem(name, "int")

    def GetBool(self, name, default=False):
        return bool(self._get(name, "bool", default))

    def SetBool(self, name, value):
        self._set(name, "bool", bool(value))

    def RemBool(self, name):
        self._rem(name, "bool")

    def GetBools(self):
        """Names with a bool value in this group (FreeCAD returns names)."""
        return self._names("bool")

    def GetFloat(self, name, default=0.0):
        return float(self._get(name, "float", default))

    def SetFloat(self, name, value):
        self._set(name, "float", float(value))

    def RemFloat(self, name):
        self._rem(name, "float")

    def GetFloats(self):
        """Names with a float value in this group (FreeCAD returns names)."""
        return self._names("float")

    def GetString(self, name, default=""):
        return str(self._get(name, "string", default))

    def SetString(self, name, value):
        self._set(name, "string", str(value))

    def RemString(self, name):
        self._rem(name, "string")

    def GetStrings(self):
        """Names with a string value in this group (FreeCAD returns names)."""
        return self._names("string")

    # -- import / export ----------------------------------------------------
    def Export(self, path):
        import json

        data = {}
        group = self._group
        for (g, name, kind), value in self._store["values"].items():
            if g == group or g.startswith(group + "/"):
                rel = g[len(group):].lstrip("/")
                data["%s\t%s\t%s" % (rel, name, kind)] = value
        with _builtins.open(path, "w", encoding="utf-8") as handle:
            json.dump(data, handle)

    def Import(self, path):
        import json

        with _builtins.open(path, "r", encoding="utf-8") as handle:
            data = json.load(handle)
        group = self._group
        for key, value in data.items():
            rel, name, kind = key.split("\t")
            target = (group + "/" + rel) if rel else group
            self._store["values"][(target, name, kind)] = value
            self._store["groups"].add(target)
        if data:
            _save_parameter_file()

    def __repr__(self):
        return "<ParameterGrp '%s:%s'>" % (self._root, self._group)


def ParamGet(path="", create=True):
    """Return the parameter group rooted at ``path``.

    Values live in memory until a file is configured (see
    ``FerroCAD.parameter_file`` / ``FerroCAD.set_parameter_file``); the first
    call after configuration loads that file.
    """
    return ParameterGrp(path)


# ---------------------------------------------------------------------------
# Recovery snapshots (App-level; writes metadata + document to the transient
# directory of a document).
# ---------------------------------------------------------------------------


def _xml_escape(text):
    return (
        str(text)
        .replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
        .replace("'", "&apos;")
    )


def _document_xml(doc):
    """A minimal ``Document.xml`` body for a recovery snapshot."""
    parts = ['<?xml version="1.0" encoding="utf-8"?>', "<Document>"]
    for obj in doc.Objects:
        parts.append(
            '  <Object name="%s" type="%s" />' % (_xml_escape(obj.Name), _xml_escape(obj.TypeId))
        )
    parts.append("</Document>")
    return "\n".join(parts) + "\n"


def writeRecoverySnapshotToTransientDir(doc, compressed=True):
    """Write a recovery snapshot (metadata + document) to ``doc.TransientDir``.

    Raises ``RuntimeError`` while a transaction is open (see
    ``Document.canWriteRecoverySnapshot``).
    """
    if not doc.canWriteRecoverySnapshot():
        raise RuntimeError("cannot write a recovery snapshot while a transaction is open")

    import os
    import zipfile

    transient = doc.TransientDir
    os.makedirs(transient, exist_ok=True)

    metadata = os.path.join(transient, "fc_recovery_file.xml")
    with _builtins.open(metadata, "w", encoding="utf-8") as handle:
        handle.write('<?xml version="1.0" encoding="utf-8"?>\n')
        handle.write("<AutoRecovery>\n")
        handle.write("  <Label>%s</Label>\n" % _xml_escape(doc.Label))
        handle.write("  <FileName>%s</FileName>\n" % _xml_escape(doc.FileName))
        handle.write("</AutoRecovery>\n")

    document_xml = _document_xml(doc)
    if compressed:
        archive = os.path.join(transient, "fc_recovery_file.fcstd")
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as recovery:
            recovery.writestr("Document.xml", document_xml)
    else:
        uncompressed = os.path.join(transient, "fc_recovery_files")
        os.makedirs(uncompressed, exist_ok=True)
        with _builtins.open(os.path.join(uncompressed, "Document.xml"), "w", encoding="utf-8") as handle:
            handle.write(document_xml)

    return True


class PropertyType:
    """Property type bit flags (``FreeCAD.PropertyType.*``)."""

    Prop_None = 0
    Prop_ReadOnly = 1
    Prop_Transient = 2
    Prop_Hidden = 4
    Prop_Output = 8
    Prop_NoRecompute = 16
    Prop_NoPersist = 32
