"""A drop-in, Rust-backed stand-in for FreeCAD's Python ``App`` module.

The public surface here mirrors the parts of upstream FreeCAD that headless
scripts actually touch:

    import FreeCAD
    doc = FreeCAD.newDocument("HelloWorld")
    obj = doc.addObject("App::FeaturePython", "Box")
    doc.recompute()

Every call is forwarded to the Rust ``freecad-core`` library via
:mod:`FreeCAD._ffi`; the C++ PyCXX bindings are not involved at all.
"""

from __future__ import annotations

from . import _ffi
from ._ffi import enc as _enc
from ._ffi import lib as _lib
from ._ffi import take as _take

__all__ = [
    "Version",
    "newDocument",
    "closeDocument",
    "getDocument",
    "listDocuments",
    "Document",
    "DocumentObject",
    "ActiveDocument",
]

__version__ = _ffi.version()

# Handle -> Document cache, so repeated lookups return the *same* object
# (identity matters to workbenches that compare documents).
_documents: "dict[int, Document]" = {}


def _wrap_document(handle):
    if not handle:
        return None
    key = int(handle)
    doc = _documents.get(key)
    if doc is None:
        doc = Document(handle)
        _documents[key] = doc
    return doc


class DocumentObject:
    """Mirrors ``App::DocumentObject`` (object name, label, dynamic properties)."""

    def __init__(self, handle, document=None):
        object.__setattr__(self, "_handle", handle)
        object.__setattr__(self, "_document", document)

    # -- identity -----------------------------------------------------------
    @property
    def Name(self):
        return _take(_lib.fc_object_name(self._handle))

    @property
    def Label(self):
        return _take(_lib.fc_object_label(self._handle))

    @Label.setter
    def Label(self, value):
        _lib.fc_object_set_label(self._handle, _enc(value))

    @property
    def TypeId(self):
        return _take(_lib.fc_object_type_id(self._handle))

    @property
    def Document(self):
        return self._document

    # -- properties ---------------------------------------------------------
    def addProperty(self, type_id, name, group="", doc="", attr=0):
        if _lib.fc_object_add_property(self._handle, _enc(type_id), _enc(name), _enc("")) != 0:
            raise ValueError("failed to add property '%s'" % name)
        return None

    def getPropertyByName(self, name):
        ptr = _lib.fc_object_get_property(self._handle, _enc(name))
        if not ptr:
            raise AttributeError("'%s' has no property '%s'" % (self.TypeId, name))
        return _take(ptr)

    def setPropertyByName(self, name, value):
        if _lib.fc_object_set_property(self._handle, _enc(name), _enc(value)) != 0:
            raise AttributeError("'%s' has no property '%s'" % (self.TypeId, name))
        return None

    def getTypeIdOfProperty(self, name):
        return _take(_lib.fc_object_property_type(self._handle, _enc(name)))

    @property
    def PropertiesList(self):
        count = _lib.fc_object_property_count(self._handle)
        return [_take(_lib.fc_object_property_name_at(self._handle, i)) for i in range(count)]

    # -- lifecycle ----------------------------------------------------------
    def recompute(self):
        if self._document is not None:
            self._document.recompute()

    # -- dynamic attribute <-> property mapping -----------------------------
    def __getattr__(self, item):
        if item.startswith("_"):
            raise AttributeError(item)
        ptr = _lib.fc_object_get_property(self._handle, _enc(item))
        if not ptr:
            raise AttributeError("'%s' object has no attribute '%s'" % (self.TypeId, item))
        return _take(ptr)

    def __setattr__(self, item, value):
        if item.startswith("_") or isinstance(getattr(type(self), item, None), property):
            object.__setattr__(self, item, value)
            return
        if _lib.fc_object_set_property(self._handle, _enc(item), _enc(value)) != 0:
            raise AttributeError("'%s' object has no attribute '%s'" % (self.TypeId, item))

    def __repr__(self):
        return "<%s object '%s'>" % (self.TypeId, self.Name)


class Document:
    """Mirrors ``App::Document``."""

    def __init__(self, handle):
        object.__setattr__(self, "_handle", handle)

    @property
    def Name(self):
        return _take(_lib.fc_document_name(self._handle))

    @property
    def Label(self):
        return _take(_lib.fc_document_label(self._handle))

    @Label.setter
    def Label(self, value):
        _lib.fc_document_set_label(self._handle, _enc(value))

    def addObject(self, type_id, name=None):
        handle = _lib.fc_document_add_object(self._handle, _enc(type_id), _enc(name))
        if not handle:
            raise RuntimeError("failed to add object of type '%s'" % type_id)
        return DocumentObject(handle, self)

    def removeObject(self, name):
        if _lib.fc_document_remove_object(self._handle, _enc(name)) != 0:
            raise ValueError("no object named '%s' in document '%s'" % (name, self.Name))

    def getObject(self, name):
        handle = _lib.fc_document_get_object(self._handle, _enc(name))
        return DocumentObject(handle, self) if handle else None

    @property
    def Objects(self):
        count = _lib.fc_document_object_count(self._handle)
        return [
            DocumentObject(_lib.fc_document_object_at(self._handle, i), self)
            for i in range(count)
        ]

    @property
    def CountObjects(self):
        return _lib.fc_document_object_count(self._handle)

    def recompute(self):
        _lib.fc_document_recompute(self._handle)
        return True

    def __repr__(self):
        return "<Document object '%s'>" % self.Name


# ---------------------------------------------------------------------------
# Module-level API
# ---------------------------------------------------------------------------
def newDocument(name=None, hidden=False, temp=False):  # noqa: N802 (FreeCAD name)
    return _wrap_document(_lib.fc_new_document(_enc(name)))


def closeDocument(name):  # noqa: N802
    handle = _lib.fc_get_document(_enc(name))
    if not handle:
        raise ValueError("no document named '%s'" % name)
    try:
        _lib.fc_close_document(handle)
    finally:
        _documents.pop(int(handle), None)
    return None


def getDocument(name):  # noqa: N802
    return _wrap_document(_lib.fc_get_document(_enc(name)))


def listDocuments():  # noqa: N802
    count = _lib.fc_document_count()
    return [_wrap_document(_lib.fc_document_at(i)).Name for i in range(count)]


def Version():  # noqa: N802
    """Return the version as a list, matching FreeCAD's ``FreeCAD.Version()``."""
    return __version__.split(".") + ["rust-poc", ""]


def __getattr__(name):
    # PEP 562: resolved lazily because the active document changes over time.
    if name == "ActiveDocument":
        return _wrap_document(_lib.fc_active_document())
    raise AttributeError("module 'FreeCAD' has no attribute '%s'" % name)
