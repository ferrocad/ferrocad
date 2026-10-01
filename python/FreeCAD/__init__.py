"""A drop-in, Rust-backed stand-in for FreeCAD's Python ``App`` module.

The public surface mirrors the parts of upstream FreeCAD that headless scripts
touch::

    import FreeCAD
    doc = FreeCAD.newDocument("HelloWorld")
    obj = doc.addObject("App::FeaturePython", "Box")
    doc.recompute()

Two Rust backends provide the implementation; this module just selects one:

* ``_core``           — the PyO3 extension (primary, M1).
* ``_ctypes_backend`` — the M0 C-ABI bridge, used if ``_core`` is unavailable.

The C++ PyCXX bindings are not involved at all.
"""

from __future__ import annotations

try:  # pragma: no cover - trivial branch
    from . import _core as _backend

    backend = "pyo3"
except ImportError:  # pragma: no cover - depends on build artifacts
    from . import _ctypes_backend as _backend

    backend = "ctypes"

Document = _backend.Document
DocumentObject = _backend.DocumentObject
newDocument = _backend.newDocument
closeDocument = _backend.closeDocument
getDocument = _backend.getDocument
listDocuments = _backend.listDocuments
activeDocument = _backend.activeDocument
Version = _backend.Version

__version__ = _backend.__version__

__all__ = [
    "Version",
    "newDocument",
    "closeDocument",
    "getDocument",
    "listDocuments",
    "activeDocument",
    "Document",
    "DocumentObject",
    "ActiveDocument",
    "backend",
]


def __getattr__(name):
    # PEP 562: resolved lazily because the active document changes over time.
    if name == "ActiveDocument":
        return activeDocument()
    raise AttributeError("module 'FreeCAD' has no attribute '%s'" % name)
