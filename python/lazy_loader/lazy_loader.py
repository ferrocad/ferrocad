"""A dependency-free lazy module loader.

Workbenches (Draft in particular) defer ``import Part`` until an attribute is
actually used::

    import lazy_loader.lazy_loader as lz

    Part = lz.LazyLoader("Part", globals(), "Part")

The value bound to the global is this loader. The first attribute access imports
the target module, replaces the global with the module object, and caches it, so
the cost is paid once and subsequent accesses are ordinary module lookups. This
is the subset of the upstream ``lazy_loader`` package that FreeCAD uses; it has
no third-party dependencies.
"""

from __future__ import annotations

import importlib

__all__ = ("LazyLoader",)


class LazyLoader:
    """Stand-in for a module, imported on first attribute access.

    Parameters
    ----------
    name:
        The name the loader is bound to in ``module_globals``. Also the module
        to import when ``module_name`` is not given.
    module_globals:
        The ``globals()`` of the defining module. On first access it is updated
        so ``name`` refers to the imported module instead of this loader.
    module_name:
        The module to import, when it differs from ``name``.
    """

    def __init__(self, name, module_globals, module_name=None):
        self._name = name
        self._module_globals = module_globals
        self._module_name = module_name or name
        self._module = None

    def _load(self):
        """Import the target module once, replacing the global binding."""
        if self._module is None:
            module = importlib.import_module(self._module_name)
            self._module = module
            self._module_globals[self._name] = module
        return self._module

    def __getattr__(self, attr):
        # ``__getattr__`` only runs for attributes the loader itself does not
        # define, so the bookkeeping fields above cannot recurse here.
        return getattr(self._load(), attr)

    def __dir__(self):
        return dir(self._load())

    def __repr__(self):
        if self._module is None:
            return f"<LazyLoader {self._module_name!r} (not imported)>"
        return repr(self._module)
