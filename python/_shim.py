"""Shared guard for the `Part`/`Mesh`/`MeshPart` shims.

Each shim imports ``ferrocad_combined``, which builds (and publishes) ``ferrocad``
as well as the geometry modules. A host that **linked** its own core — an edition
binary, or a test harness that registers the bindings as built-ins — must never have
that built-in replaced: the combined extension would bring a second
``ferrocad_core``, with its own document registry, and the two halves of the shell
would then see different documents.

``ferrocad_combined_py::install`` refuses the same image from the Rust side; this
check is what makes a *stale* ``ferrocad_combined.abi3.so`` on disk harmless.
"""

import importlib.machinery
import importlib.util


def core_is_builtin() -> bool:
    """Whether the interpreter already has a built-in ``ferrocad`` (a linked host)."""
    try:
        spec = importlib.util.find_spec("ferrocad")
    except (ImportError, ValueError):
        return False
    return spec is not None and spec.loader is importlib.machinery.BuiltinImporter
