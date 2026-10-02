"""ctypes bridge to the Rust ``freecad-core`` native library.

This is the only place that knows about the C ABI. Everything in
``FreeCAD/__init__.py`` speaks plain Python; everything below this line is the
"Rust bindings" that replace the upstream C++ ``App`` PyCXX bindings.

The library is located in this order:

1. ``$FREECAD_CORE_LIB`` (explicit override),
2. next to this package (the layout produced by ``./build.sh``),
3. the cargo ``target/{release,debug}`` tree (development runs).
"""

from __future__ import annotations

import ctypes as _c
import os as _os

_LIB_NAMES = ("libferrocad_ctypes.so", "libferrocad_ctypes.dylib", "ferrocad_ctypes.dll")


def _candidates():
    here = _os.path.dirname(_os.path.abspath(__file__))
    env = _os.environ.get("FREECAD_CORE_LIB")
    if env:
        yield env
    for name in _LIB_NAMES:
        yield _os.path.join(here, name)
    repo = _os.path.abspath(_os.path.join(here, "..", ".."))
    for profile in ("release", "debug"):
        for name in _LIB_NAMES:
            yield _os.path.join(repo, "rust", "freecad-core", "target", profile, name)


def _load():
    tried = []
    for path in _candidates():
        if _os.path.exists(path):
            return _c.CDLL(path)
        tried.append(path)
    raise ImportError(
        "freecad-core native library not found.\n"
        "Build it with ./build.sh, or point FREECAD_CORE_LIB at the library.\n"
        "Looked in:\n  " + "\n  ".join(tried)
    )


lib = _load()

c_void_p = _c.c_void_p
c_char_p = _c.c_char_p
c_int = _c.c_int

# --- library ---------------------------------------------------------------
lib.fc_version.argtypes = []
lib.fc_version.restype = c_char_p

lib.fc_string_free.argtypes = [c_void_p]
lib.fc_string_free.restype = None

# --- documents -------------------------------------------------------------
lib.fc_new_document.argtypes = [c_char_p]
lib.fc_new_document.restype = c_void_p

lib.fc_close_document.argtypes = [c_void_p]
lib.fc_close_document.restype = c_int

lib.fc_active_document.argtypes = []
lib.fc_active_document.restype = c_void_p

lib.fc_get_document.argtypes = [c_char_p]
lib.fc_get_document.restype = c_void_p

lib.fc_document_count.argtypes = []
lib.fc_document_count.restype = c_int

lib.fc_document_at.argtypes = [c_int]
lib.fc_document_at.restype = c_void_p

lib.fc_document_name.argtypes = [c_void_p]
lib.fc_document_name.restype = c_void_p

lib.fc_document_label.argtypes = [c_void_p]
lib.fc_document_label.restype = c_void_p

lib.fc_document_set_label.argtypes = [c_void_p, c_char_p]
lib.fc_document_set_label.restype = c_int

lib.fc_document_object_count.argtypes = [c_void_p]
lib.fc_document_object_count.restype = c_int

lib.fc_document_object_at.argtypes = [c_void_p, c_int]
lib.fc_document_object_at.restype = c_void_p

lib.fc_document_get_object.argtypes = [c_void_p, c_char_p]
lib.fc_document_get_object.restype = c_void_p

lib.fc_document_add_object.argtypes = [c_void_p, c_char_p, c_char_p]
lib.fc_document_add_object.restype = c_void_p

lib.fc_document_remove_object.argtypes = [c_void_p, c_char_p]
lib.fc_document_remove_object.restype = c_int

lib.fc_document_recompute.argtypes = [c_void_p]
lib.fc_document_recompute.restype = c_int

# --- objects ---------------------------------------------------------------
lib.fc_object_name.argtypes = [c_void_p]
lib.fc_object_name.restype = c_void_p

lib.fc_object_label.argtypes = [c_void_p]
lib.fc_object_label.restype = c_void_p

lib.fc_object_set_label.argtypes = [c_void_p, c_char_p]
lib.fc_object_set_label.restype = c_int

lib.fc_object_type_id.argtypes = [c_void_p]
lib.fc_object_type_id.restype = c_void_p

lib.fc_object_add_property.argtypes = [c_void_p, c_char_p, c_char_p, c_char_p]
lib.fc_object_add_property.restype = c_int

lib.fc_object_get_property.argtypes = [c_void_p, c_char_p]
lib.fc_object_get_property.restype = c_void_p

lib.fc_object_set_property.argtypes = [c_void_p, c_char_p, c_char_p]
lib.fc_object_set_property.restype = c_int

lib.fc_object_property_count.argtypes = [c_void_p]
lib.fc_object_property_count.restype = c_int

lib.fc_object_property_name_at.argtypes = [c_void_p, c_int]
lib.fc_object_property_name_at.restype = c_void_p

lib.fc_object_property_type.argtypes = [c_void_p, c_char_p]
lib.fc_object_property_type.restype = c_void_p


def enc(value):
    """Encode a Python value for a ``const char*`` argument (None -> NULL)."""
    if value is None:
        return None
    if isinstance(value, bytes):
        return value
    return str(value).encode("utf-8")


def take(ptr):
    """Copy an owned C string returned by the core, then free it in Rust."""
    if not ptr:
        return None
    try:
        return _c.string_at(ptr).decode("utf-8")
    finally:
        lib.fc_string_free(ptr)


def version():
    return lib.fc_version().decode("utf-8")
