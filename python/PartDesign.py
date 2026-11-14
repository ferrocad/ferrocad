"""The `PartDesign` module: the document types live in the Part kernel.

Upstream `import PartDesign` registers the workbench's object types. FerroCAD
registers `PartDesign::Body`, `PartDesign::AdditiveBox`, `PartDesign::Plane` and
`PartDesign::Draft` when the Part kernel loads, so this module only ensures that
has happened; it defines no functions of its own.

A host that linked its own core already has them registered, and importing the
combined extension there would replace that core (see ``_shim.py``).
"""

import _shim

if not _shim.core_is_builtin():
    try:
        import ferrocad_combined  # noqa: F401  registers the PartDesign types
    except ImportError as error:  # pragma: no cover - depends on the build
        raise ImportError(
            "PartDesign needs the combined ferrocad_combined extension; run ./build.sh"
        ) from error
