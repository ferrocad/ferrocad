"""Stub for the resource module that ``pyside-rcc`` generates for Draft.

Upstream ``Draft.py`` and ``draftutils/utils.py`` do ``import Draft_rc`` when the
GUI is up purely to pull in the compiled ``Resources/Draft.qrc`` bundle, and use
``Draft_rc.__name__`` as a truthy "resources are loaded" marker. Nothing in the
headless import path uses the resources themselves (the Draft preference
``.ui`` files are read through ``QtCore.QFile``, not through this module), so an
empty module is enough.

Keep this file importable without Qt: it must not import ``PySide``.
"""

__title__ = "Draft resources (FerroCAD stub)"


def qInitResources():
    """No-op: there is no compiled resource bundle to register."""


def qCleanupResources():
    """No-op, mirroring the ``pyside-rcc`` generated module."""
