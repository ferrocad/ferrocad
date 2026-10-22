"""Tests for the GUI document API (``FreeCADGui.getDocument``).

``Gui.getDocument(name)`` returns a ``GuiDocument`` whose ``ActiveView`` owns a
real, mutable ``pivy.coin`` scene graph. These tests exercise the local slice:
the App registry is the source of truth, the cache follows document close, two
documents get distinct roots, and headless callers never need the shell bridge.

The real ``pivy.coin`` is used when the installed module is usable (as in
conformance); otherwise a tiny complete stub is pinned for each test, mirroring
the other Coin suites.
"""

import contextlib
import io
import sys
import types
import unittest
from unittest import mock

import FreeCAD
import FreeCADGui

# ---------------------------------------------------------------------------
# ``pivy.coin``: the real module when available, a minimal stub otherwise
# ---------------------------------------------------------------------------


class _FakeNode:
    """The structural subset of a Coin group that the view API needs."""

    def __init__(self):
        self._children = []

    def addChild(self, child):
        self._children.append(child)

    def removeChild(self, child):
        for index, candidate in enumerate(self._children):
            if candidate is child:
                del self._children[index]
                return

    def removeAllChildren(self):
        self._children.clear()

    def getNumChildren(self):
        return len(self._children)

    def getChild(self, index):
        return self._children[index]


class _FakeSoSeparator(_FakeNode):
    pass


class _FakeSoCube(_FakeNode):
    pass


def _usable(module) -> bool:
    """Whether ``module`` carries the nodes and group methods the API needs."""
    try:
        root = module.SoSeparator()
    except Exception:  # noqa: BLE001
        return False
    return hasattr(module, "SoCube") and callable(
        getattr(root, "getNumChildren", None)
    )


def _current_coin():
    """The real module when one is importable and usable, else ``None``.

    An earlier Coin suite may have left a narrower stub in ``sys.modules`` (the
    grid suite deliberately leaves its own installed), so a pre-imported module
    is only accepted when it has the nodes and group methods this API needs.
    """
    existing = sys.modules.get("pivy.coin")
    if existing is not None:
        return existing if _usable(existing) else None
    try:
        from pivy import coin
    except Exception:  # noqa: BLE001 - the extension is optional
        return None
    return coin if _usable(coin) else None


COIN = _current_coin()
if COIN is None:
    COIN = types.ModuleType("pivy.coin")
    COIN.SoSeparator = _FakeSoSeparator
    COIN.SoCube = _FakeSoCube
PIVY = types.ModuleType("pivy")
PIVY.coin = COIN


# ---------------------------------------------------------------------------
# Base case: a clean document registry and no GUI bridge
# ---------------------------------------------------------------------------


class GuiDocumentTestCase(unittest.TestCase):
    def setUp(self):
        self._previous_gui = FreeCAD.GuiUp
        FreeCAD.GuiUp = 0
        FreeCADGui._clear_documents()
        FreeCADGui._REPORTED_VIEWS.clear()
        self._names = []
        # Pin this suite's Coin module: the real one when usable, else the stub.
        self._coin_patch = mock.patch.dict(
            sys.modules, {"pivy": PIVY, "pivy.coin": COIN}
        )
        self._coin_patch.start()

    def tearDown(self):
        # Close documents through the App registry first; the cache is then
        # dropped explicitly, so no overlay teardown runs with a fake bridge.
        for name in self._names:
            if FreeCAD.getDocument(name) is not None:
                FreeCAD.closeDocument(name)
        FreeCADGui._clear_documents()
        FreeCADGui._REPORTED_VIEWS.clear()
        FreeCAD.GuiUp = self._previous_gui
        self._coin_patch.stop()

    def make_document(self, name):
        self._names.append(name)
        return FreeCAD.newDocument(name)


# ---------------------------------------------------------------------------
# The scene graph
# ---------------------------------------------------------------------------


class SceneGraphTests(GuiDocumentTestCase):
    def test_get_document_returns_a_mutable_scene_graph(self):
        self.make_document("_GuiDocScene")

        gui_document = FreeCADGui.getDocument("_GuiDocScene")
        self.assertIsNotNone(gui_document)
        root = gui_document.ActiveView.getSceneGraph()

        self.assertIsInstance(root, COIN.SoSeparator)
        self.assertEqual(root.getNumChildren(), 0)

        cube = COIN.SoCube()
        root.addChild(cube)
        self.assertEqual(root.getNumChildren(), 1)
        self.assertIsInstance(root.getChild(0), COIN.SoCube)

        # The view keeps one root; a second call is not a fresh tree.
        self.assertIs(gui_document.ActiveView.getSceneGraph(), root)

    def test_two_documents_have_distinct_scene_graphs(self):
        self.make_document("_GuiDocDistinct1")
        self.make_document("_GuiDocDistinct2")

        first = FreeCADGui.getDocument("_GuiDocDistinct1")
        second = FreeCADGui.getDocument("_GuiDocDistinct2")

        self.assertIsNot(first, second)
        self.assertIsNot(
            first.ActiveView.getSceneGraph(), second.ActiveView.getSceneGraph()
        )

    def test_the_cache_returns_one_view_per_document(self):
        self.make_document("_GuiDocCache")
        self.assertIs(
            FreeCADGui.getDocument("_GuiDocCache"),
            FreeCADGui.getDocument("_GuiDocCache"),
        )

    def test_get_document_accepts_a_document_object(self):
        """Upstream Draft calls ``Gui.getDocument(self.doc)``."""
        document = self.make_document("_GuiDocObject")
        self.assertIs(
            FreeCADGui.getDocument(document),
            FreeCADGui.getDocument("_GuiDocObject"),
        )


# ---------------------------------------------------------------------------
# The App registry is the source of truth
# ---------------------------------------------------------------------------


class DocumentLookupTests(GuiDocumentTestCase):
    def test_missing_documents_return_none(self):
        self.assertIsNone(FreeCADGui.getDocument("_GuiDocNeverOpened"))
        self.assertIsNone(FreeCADGui.activeDocument())
        self.assertIsNone(FreeCADGui.ActiveDocument)

    def test_active_document_follows_the_active_app_document(self):
        self.make_document("_GuiDocActive1")
        self.make_document("_GuiDocActive2")

        FreeCAD.setActiveDocument("_GuiDocActive1")
        self.assertEqual(FreeCADGui.ActiveDocument.Name, "_GuiDocActive1")
        self.assertIs(FreeCADGui.ActiveDocument, FreeCADGui.activeDocument())

        FreeCAD.setActiveDocument("_GuiDocActive2")
        self.assertEqual(FreeCADGui.ActiveDocument.Name, "_GuiDocActive2")
        self.assertIs(FreeCADGui.ActiveDocument, FreeCADGui.activeDocument())

    def test_closing_a_document_drops_the_cache_entry(self):
        self.make_document("_GuiDocClose")
        gui_document = FreeCADGui.getDocument("_GuiDocClose")
        root = gui_document.ActiveView.getSceneGraph()
        root.addChild(COIN.SoCube())
        self.assertIn("_GuiDocClose", FreeCADGui._DOCUMENTS)

        FreeCAD.closeDocument("_GuiDocClose")

        self.assertIsNone(FreeCADGui.getDocument("_GuiDocClose"))
        self.assertNotIn("_GuiDocClose", FreeCADGui._DOCUMENTS)
        self.assertTrue(gui_document._closed)
        self.assertEqual(root.getNumChildren(), 0)

    def test_reopening_a_name_does_not_reuse_the_stale_view(self):
        self.make_document("_GuiDocReopen")
        first = FreeCADGui.getDocument("_GuiDocReopen")
        first_root = first.ActiveView.getSceneGraph()

        FreeCAD.closeDocument("_GuiDocReopen")
        FreeCAD.newDocument("_GuiDocReopen")

        second = FreeCADGui.getDocument("_GuiDocReopen")
        self.assertIsNot(first, second)
        self.assertIsNot(first_root, second.ActiveView.getSceneGraph())

    def test_an_explicit_close_shaped_refresh_drops_the_view(self):
        self.make_document("_GuiDocForget")
        first = FreeCADGui.getDocument("_GuiDocForget")

        FreeCADGui._forget_document("_GuiDocForget")

        second = FreeCADGui.getDocument("_GuiDocForget")
        self.assertIsNot(first, second)
        self.assertTrue(first._closed)

    def test_an_explicit_refresh_accepts_a_document_object(self):
        document = self.make_document("_GuiDocForgetObject")
        first = FreeCADGui.getDocument(document)

        FreeCADGui._forget_document(document)

        self.assertTrue(first._closed)
        self.assertIsNot(first, FreeCADGui.getDocument(document))

    def test_a_closed_view_is_rebuilt_on_the_next_lookup(self):
        self.make_document("_GuiDocRebuild")
        first = FreeCADGui.getDocument("_GuiDocRebuild")
        first.close()

        second = FreeCADGui.getDocument("_GuiDocRebuild")

        self.assertIsNot(first, second)
        self.assertIsNotNone(second.ActiveView.getSceneGraph())


# ---------------------------------------------------------------------------
# Headless and degraded builds
# ---------------------------------------------------------------------------


class HeadlessTests(GuiDocumentTestCase):
    def test_headless_works_without_the_shell_bridge(self):
        self.make_document("_GuiDocHeadless")

        with mock.patch.dict(sys.modules, {"FerroCADGui": None}):
            gui_document = FreeCADGui.getDocument("_GuiDocHeadless")
            root = gui_document.ActiveView.getSceneGraph()

        self.assertIsInstance(root, COIN.SoSeparator)
        self.assertFalse(gui_document._overlay)

    def test_a_build_without_pivy_reports_once_and_returns_none(self):
        self.make_document("_GuiDocNoCoin")
        errors = io.StringIO()

        with mock.patch.dict(
            sys.modules, {"pivy": None, "pivy.coin": None}
        ), contextlib.redirect_stderr(errors):
            gui_document = FreeCADGui.getDocument("_GuiDocNoCoin")
            first = gui_document.ActiveView.getSceneGraph()
            second = gui_document.ActiveView.getSceneGraph()

        self.assertIsNone(first)
        self.assertIsNone(second)
        self.assertEqual(errors.getvalue().count("pivy.coin"), 1)

    def test_a_gui_without_the_shell_bridge_reports_once(self):
        self.make_document("_GuiDocNoBridge")
        errors = io.StringIO()

        FreeCAD.GuiUp = 1
        with mock.patch.dict(
            sys.modules, {"FerroCADGui": None}
        ), contextlib.redirect_stderr(errors):
            gui_document = FreeCADGui.getDocument("_GuiDocNoBridge")
            first = gui_document.ActiveView.getSceneGraph()
            second = gui_document.ActiveView.getSceneGraph()

        self.assertIs(first, second)
        self.assertFalse(gui_document._overlay)
        self.assertEqual(
            errors.getvalue().count("FerroCADGui overlay bridge"), 1
        )


# ---------------------------------------------------------------------------
# The GUI overlay bridge
# ---------------------------------------------------------------------------


class OverlayBridgeTests(GuiDocumentTestCase):
    def test_gui_up_registers_the_root_with_the_bridge(self):
        self.make_document("_GuiDocOverlay")
        registered = []

        FreeCAD.GuiUp = 1
        with mock.patch(
            "FerroCADGui.addOverlay", side_effect=registered.append
        ), mock.patch("FerroCADGui.removeOverlay", return_value=True):
            gui_document = FreeCADGui.getDocument("_GuiDocOverlay")
            root = gui_document.ActiveView.getSceneGraph()
            self.assertTrue(gui_document._overlay)
            self.assertEqual(registered, [root])
            gui_document.close()

        self.assertFalse(gui_document._overlay)

    def test_closing_removes_the_overlay(self):
        self.make_document("_GuiDocOverlayClose")
        removed = []

        FreeCAD.GuiUp = 1
        with mock.patch(
            "FerroCADGui.addOverlay"
        ), mock.patch(
            "FerroCADGui.removeOverlay", side_effect=removed.append
        ):
            gui_document = FreeCADGui.getDocument("_GuiDocOverlayClose")
            root = gui_document.ActiveView.getSceneGraph()
            FreeCADGui._forget_document("_GuiDocOverlayClose")

        self.assertEqual(removed, [root])


if __name__ == "__main__":
    unittest.main()
