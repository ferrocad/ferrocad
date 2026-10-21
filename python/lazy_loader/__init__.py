"""Vendored, dependency-free subset of the ``lazy_loader`` package.

FreeCAD workbench scripts import it as::

    import lazy_loader.lazy_loader as lz

Only :class:`~lazy_loader.lazy_loader.LazyLoader` is provided; it is re-exported
here for callers that use ``from lazy_loader import LazyLoader``.
"""

from __future__ import annotations

from .lazy_loader import LazyLoader

__all__ = ("LazyLoader",)
