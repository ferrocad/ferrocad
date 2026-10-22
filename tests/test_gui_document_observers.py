"""Tests for the ``FerroCADGui`` document-observer slot (P0).

The backlog item: a selected provider's ``deactivate`` (the Coin grid overlay
attaches per document) only ran on the *next* activation, because upstream
``FreeCADGui`` has no document-deactivate slot. ``FerroCADGui`` now has one, and
the provider registry subscribes to it: a document closing detaches the active
provider immediately, and the next activation re-attaches the declared one.

The suite is headless (``FreeCAD.GuiUp == 0``), which also proves the lifecycle
path does not require a GUI. The App facade (``FreeCAD.newDocument`` / ``open`` /
``closeDocument`` / ``setActiveDocument``) fires the slot, so upstream-shaped
callers are covered; ``FerroCAD`` exposes the same names as aliases and
``FreeCADGui.addDocumentObserver`` forwards to the slot.
"""

import contextlib
import io
import json
import os
import subprocess
import sys
import unittest
from pathlib import Path

import FreeCAD
import FerroCAD
import FerroCADGui
import ferrocad_shell

ROOT = Path(__file__).resolve().parents[1]
FEATURE = "document-lifecycle"
PROVIDER = "test"


class RecordingObserver:
    """An observer that records every callback it receives."""

    def __init__(self):
        self.events = []

    def onDocumentCreated(self, name):
        self.events.append(("created", name))

    def onDocumentActivated(self, name):
        self.events.append(("activated", name))

    def onDocumentDeactivated(self, name):
        self.events.append(("deactivated", name))

    def onDocumentClosed(self, name):
        self.events.append(("closed", name))


class DocumentObserverTests(unittest.TestCase):
    def setUp(self):
        # The registry is process-wide; save it so this suite leaves the later
        # suites exactly as it found them.
        self._saved_applied = dict(FerroCADGui._PROVIDER_APPLIED)
        self._saved_document = FerroCADGui._PROVIDER_DOCUMENT
        FerroCADGui._PROVIDER_APPLIED.clear()
        FerroCADGui._PROVIDER_DOCUMENT = None
        self.observers = []
        self.activations = 0
        self.deactivations = 0
        FerroCADGui.unregisterProvider(FEATURE, PROVIDER)

    def tearDown(self):
        for observer in self.observers:
            FerroCADGui.removeDocumentListener(observer)
        FerroCADGui.unregisterProvider(FEATURE, PROVIDER)
        for name in list(FreeCAD.listDocuments()):
            FerroCAD.closeDocument(name)
        FerroCADGui._PROVIDER_APPLIED.clear()
        FerroCADGui._PROVIDER_APPLIED.update(self._saved_applied)
        FerroCADGui._PROVIDER_DOCUMENT = self._saved_document

    # -- helpers ---------------------------------------------------------
    def observe(self):
        observer = RecordingObserver()
        FerroCADGui.addDocumentListener(observer)
        self.observers.append(observer)
        return observer

    def activate(self):
        self.activations += 1
        return None

    def deactivate(self):
        self.deactivations += 1
        return None

    def register_provider(self):
        FerroCADGui.registerProvider(
            FEATURE,
            PROVIDER,
            "Test provider",
            priority=0,
            activate=self.activate,
            deactivate=self.deactivate,
        )

    # -- the slot --------------------------------------------------------
    def test_one_registration_per_observer_and_removal_stops_notifications(self):
        observer = RecordingObserver()
        FerroCADGui.addDocumentListener(observer)
        self.observers.append(observer)
        # Identity, not equality: a second add must not register it twice.
        FerroCADGui.addDocumentListener(observer)

        FerroCAD.newDocument("ObserverOnce")
        self.assertEqual(
            observer.events,
            [("created", "ObserverOnce"), ("activated", "ObserverOnce")],
        )

        self.assertTrue(FerroCADGui.removeDocumentListener(observer))
        self.assertFalse(FerroCADGui.removeDocumentListener(observer))
        observer.events.clear()
        FerroCAD.closeDocument("ObserverOnce")
        self.assertEqual(observer.events, [], "a removed observer is not called")

    def test_an_observer_may_implement_only_the_hooks_it_needs(self):
        class ClosedOnly:
            def __init__(self):
                self.closed = []

            def onDocumentClosed(self, name):
                self.closed.append(name)

        observer = ClosedOnly()
        FerroCADGui.addDocumentListener(observer)
        self.observers.append(observer)
        FerroCAD.newDocument("PartialObserver")
        FerroCAD.closeDocument("PartialObserver")
        self.assertEqual(observer.closed, ["PartialObserver"])

    # -- provider attachment follows the document -----------------------
    def test_closing_the_active_document_deactivates_its_provider_immediately(self):
        self.register_provider()
        observer = self.observe()

        FerroCAD.newDocument("LifecycleClose")
        self.assertEqual(self.activations, 1, "activation attaches the provider")

        self.assertTrue(FerroCAD.closeDocument("LifecycleClose"))
        self.assertEqual(
            self.deactivations, 1, "closing detaches before the next document"
        )
        self.assertEqual(
            observer.events[-2:],
            [("deactivated", "LifecycleClose"), ("closed", "LifecycleClose")],
        )

        # Closing twice is a no-op, not a second deactivate.
        self.assertFalse(FerroCAD.closeDocument("LifecycleClose"))
        self.assertEqual(self.deactivations, 1)
        self.assertEqual(len(observer.events), 4, "no further events")

        # And the next document is created after the deactivation, not before.
        FerroCAD.newDocument("LifecycleAfter")
        self.assertEqual(self.deactivations, 1)
        self.assertEqual(self.activations, 2)

    def test_switching_the_active_document_deactivates_the_old_and_activates_the_new(self):
        self.register_provider()
        observer = self.observe()

        first = FerroCAD.newDocument("LifecycleFirst")
        second = FerroCAD.newDocument("LifecycleSecond")
        self.assertEqual(
            observer.events,
            [
                ("created", first.Name),
                ("activated", first.Name),
                ("deactivated", first.Name),
                ("created", second.Name),
                ("activated", second.Name),
            ],
        )
        self.assertEqual((self.activations, self.deactivations), (2, 1))

        FerroCAD.setActiveDocument(first.Name)
        self.assertEqual(
            observer.events[-2:],
            [("deactivated", second.Name), ("activated", first.Name)],
        )
        self.assertEqual((self.activations, self.deactivations), (3, 2))

        # An inactive document closing reports closed but does not touch the
        # provider attached to the active one.
        FerroCAD.closeDocument(second.Name)
        self.assertEqual(self.deactivations, 2)
        self.assertEqual(observer.events[-1], ("closed", second.Name))

        FerroCAD.closeDocument(first.Name)
        self.assertEqual(self.deactivations, 3)

    def test_re_selecting_the_active_document_is_inert(self):
        self.register_provider()
        observer = self.observe()
        FerroCAD.newDocument("LifecycleSame")
        observer.events.clear()

        FerroCAD.setActiveDocument("LifecycleSame")
        self.assertEqual(observer.events, [])
        self.assertEqual((self.activations, self.deactivations), (1, 0))

    # -- isolation -------------------------------------------------------
    def test_a_raising_observer_is_reported_once_and_never_breaks_the_close(self):
        class Broken:
            def onDocumentClosed(self, name):
                raise RuntimeError("observer boom")

        broken = Broken()
        good = self.observe()
        FerroCADGui.addDocumentListener(broken)
        self.observers.append(broken)

        FerroCAD.newDocument("LifecycleBrokenA")
        FerroCAD.newDocument("LifecycleBrokenB")

        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            self.assertTrue(FerroCAD.closeDocument("LifecycleBrokenA"))
            self.assertTrue(FerroCAD.closeDocument("LifecycleBrokenB"))

        report = stderr.getvalue()
        self.assertEqual(
            report.count("onDocumentClosed raised"),
            1,
            "the same observer/hook is reported once: %r" % report,
        )
        self.assertIn("RuntimeError: observer boom", report)
        self.assertIsNone(FreeCAD.getDocument("LifecycleBrokenA"))
        self.assertIsNone(FreeCAD.getDocument("LifecycleBrokenB"))
        self.assertEqual(
            [event for event in good.events if event[0] == "closed"],
            [("closed", "LifecycleBrokenA"), ("closed", "LifecycleBrokenB")],
            "the other observers still ran",
        )

    # -- the shell drives the same slot ----------------------------------
    def test_shell_bootstrap_creates_and_activates_through_the_slot(self):
        observer = self.observe()
        try:
            payload = json.loads(ferrocad_shell.bootstrap("ShellLifecycle"))
            self.assertEqual(payload["document"], "ShellLifecycle")
            self.assertIn(("created", "ShellLifecycle"), observer.events)
            self.assertIn(("activated", "ShellLifecycle"), observer.events)

            # Re-bootstrapping closes the old document first, so the close
            # notification is not deferred either.
            observer.events.clear()
            json.loads(ferrocad_shell.bootstrap("ShellLifecycle"))
            self.assertEqual(
                [event[0] for event in observer.events],
                ["deactivated", "closed", "created", "activated"],
            )
        finally:
            FerroCAD.closeDocument("ShellLifecycle")

    # -- the upstream-shaped entry points --------------------------------
    def test_the_upstream_freecad_entry_points_fire_the_slot(self):
        """A script using ``FreeCAD`` (not the FerroCAD aliases) is notified too."""
        observer = self.observe()

        FreeCAD.newDocument("UpstreamLifecycle")
        self.assertEqual(
            observer.events,
            [("created", "UpstreamLifecycle"), ("activated", "UpstreamLifecycle")],
        )

        observer.events.clear()
        FreeCAD.closeDocument("UpstreamLifecycle")
        self.assertEqual(
            observer.events,
            [("deactivated", "UpstreamLifecycle"), ("closed", "UpstreamLifecycle")],
        )

    def test_freecadgui_forwards_the_document_observer_slot(self):
        import FreeCADGui

        observer = RecordingObserver()
        FreeCADGui.addDocumentObserver(observer)
        self.addCleanup(FreeCADGui.removeDocumentObserver, observer)

        FreeCAD.newDocument("ForwardedLifecycle")
        self.assertIn(("activated", "ForwardedLifecycle"), observer.events)

        self.assertTrue(FreeCADGui.removeDocumentObserver(observer))
        self.assertFalse(
            FerroCADGui.removeDocumentListener(observer),
            "the upstream name and the FerroCAD slot are the same registration",
        )

    def test_closing_a_document_forgets_its_cached_gui_view(self):
        """The view cache is downstream of the same close event."""
        import FreeCADGui

        FerroCAD.newDocument("ViewLifecycle")
        self.assertIsNotNone(FreeCADGui.getDocument("ViewLifecycle"))
        self.assertIn("ViewLifecycle", FreeCADGui._DOCUMENTS)

        FerroCAD.closeDocument("ViewLifecycle")
        self.assertNotIn(
            "ViewLifecycle",
            FreeCADGui._DOCUMENTS,
            "a closed document's view (and its overlay) must not be kept",
        )
        self.assertIsNone(FreeCADGui.getDocument("ViewLifecycle"))

    # -- headless stays GUI-free -----------------------------------------
    def test_headless_app_use_does_not_import_the_gui_modules(self):
        code = (
            "import sys\n"
            "import FreeCAD, FerroCAD\n"
            "assert FreeCAD.GuiUp == 0\n"
            "FerroCAD.newDocument('HeadlessLifecycle')\n"
            "FerroCAD.setActiveDocument('HeadlessLifecycle')\n"
            "FerroCAD.closeDocument('HeadlessLifecycle')\n"
            "assert 'FerroCADGui' not in sys.modules, sorted(sys.modules)\n"
            "assert 'FreeCADGui' not in sys.modules, sorted(sys.modules)\n"
            "print('headless-ok')\n"
        )
        environment = dict(os.environ)
        environment["PYTHONPATH"] = os.pathsep.join(
            [str(ROOT / "python"), environment.get("PYTHONPATH", "")]
        ).rstrip(os.pathsep)
        result = subprocess.run(
            [sys.executable, "-c", code],
            cwd=str(ROOT),
            env=environment,
            capture_output=True,
            text=True,
            timeout=60,
        )
        self.assertEqual(
            result.returncode,
            0,
            "headless subprocess failed:\n%s\n%s"
            % (result.stdout, result.stderr),
        )
        self.assertIn("headless-ok", result.stdout)


if __name__ == "__main__":
    unittest.main()
