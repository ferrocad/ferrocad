"""Behavioural checks for the Rust-backed ``FreeCAD`` module.

These assert the document object model behaves like upstream FreeCAD's for the
subset the POC implements. Run with::

    PYTHONPATH=python python3 -m unittest discover -s tests -v
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

import FreeCAD  # noqa: E402


class TestModule(unittest.TestCase):
    def test_version_shape(self):
        version = FreeCAD.Version()
        self.assertIsInstance(version, list)
        self.assertGreaterEqual(len(version), 3)
        self.assertTrue(all(isinstance(part, str) for part in version))

    def test_active_document_starts_none(self):
        self.assertIsNone(FreeCAD.ActiveDocument)


class TestDocument(unittest.TestCase):
    def tearDown(self):
        for name in FreeCAD.listDocuments():
            FreeCAD.closeDocument(name)

    def test_new_document_becomes_active(self):
        doc = FreeCAD.newDocument("DocA")
        self.assertEqual(doc.Name, "DocA")
        self.assertEqual(doc.Label, "DocA")
        self.assertIs(FreeCAD.ActiveDocument, doc)
        self.assertEqual(sorted(FreeCAD.listDocuments()), ["DocA"])

    def test_duplicate_names_are_made_unique(self):
        FreeCAD.newDocument("Dup")
        second = FreeCAD.newDocument("Dup")
        self.assertEqual(second.Name, "Dup001")

    def test_get_and_close(self):
        doc = FreeCAD.newDocument("DocB")
        self.assertIs(FreeCAD.getDocument("DocB"), doc)
        FreeCAD.closeDocument("DocB")
        self.assertIsNone(FreeCAD.getDocument("DocB"))
        self.assertIsNone(FreeCAD.ActiveDocument)


class TestDocumentObject(unittest.TestCase):
    def setUp(self):
        self.doc = FreeCAD.newDocument("Objects")

    def tearDown(self):
        for name in FreeCAD.listDocuments():
            FreeCAD.closeDocument(name)

    def test_add_object_with_explicit_name(self):
        obj = self.doc.addObject("App::FeaturePython", "Box")
        self.assertEqual(obj.Name, "Box")
        self.assertEqual(obj.Label, "Box")
        self.assertEqual(obj.TypeId, "App::FeaturePython")
        self.assertIs(obj.Document, self.doc)
        self.assertEqual(self.doc.CountObjects, 1)

    def test_default_naming(self):
        first = self.doc.addObject("App::FeaturePython")
        self.assertEqual(first.Name, "FeaturePython")
        second = self.doc.addObject("Part::Box")
        self.assertEqual(second.Name, "Box")
        third = self.doc.addObject("App::FeaturePython")
        self.assertEqual(third.Name, "FeaturePython001")

    def test_label_is_independent_of_name(self):
        obj = self.doc.addObject("App::FeaturePython", "Box")
        obj.Label = "My Label"
        self.assertEqual(obj.Name, "Box")
        self.assertEqual(obj.Label, "My Label")

    def test_duplicate_object_names_get_unique_names_and_labels(self):
        first = self.doc.addObject("App::FeaturePython", "Box")
        second = self.doc.addObject("App::FeaturePython", "Box")
        self.assertEqual(first.Name, "Box")
        self.assertEqual(second.Name, "Box001")
        # Default: the label is made unique too.
        self.assertEqual(second.Label, "Box001")

    def test_requested_name_is_sanitized(self):
        obj = self.doc.addObject("App::FeaturePython", "My Box")
        self.assertEqual(obj.Name, "My_Box")
        self.assertEqual(obj.Label, "My_Box")

    def test_duplicate_labels_preference(self):
        params = FreeCAD.ParamGet("User parameter:BaseApp/Preferences/Document")
        old = params.GetBool("DuplicateLabels", False)
        try:
            params.SetBool("DuplicateLabels", True)
            first = self.doc.addObject("App::FeaturePython", "Lbl")
            second = self.doc.addObject("App::FeaturePython", "Lbl")
            self.assertEqual(first.Label, "Lbl")
            self.assertEqual(second.Label, "Lbl")  # duplicates allowed
            self.assertEqual(second.Name, "Lbl001")
        finally:
            params.SetBool("DuplicateLabels", old)

    def test_enumeration_property(self):
        obj = self.doc.addObject("App::FeaturePython", "Enum")
        obj.addProperty("App::PropertyEnumeration", "Mode")
        with self.assertRaises(ValueError):
            obj.Mode = "Fast"  # no choices registered yet
        obj.Mode = ["Fast", "Slow"]
        self.assertEqual(obj.Mode, "Fast")
        obj.Mode = "Slow"
        self.assertEqual(obj.Mode, "Slow")
        obj.Mode = 0
        self.assertEqual(obj.Mode, "Fast")
        with self.assertRaises(ValueError):
            obj.Mode = "Bogus"
        with self.assertRaises(ValueError):
            obj.Mode = 5
        self.assertEqual(obj.getTypeIdOfProperty("Mode"), "App::PropertyEnumeration")

    def test_invalid_types_raise(self):
        with self.assertRaises(TypeError):
            self.doc.addObject("App::DocumentObjectExtension")
        with self.assertRaises(TypeError):
            self.doc.addObject(type="App::DocumentObjectExtension", attach=True)
        obj = self.doc.addObject("App::FeaturePython", "Obj")
        with self.assertRaises(TypeError):
            obj.addProperty("App::DocumentObjectExtension", "P")
        with self.assertRaises(TypeError):
            self.doc.findObjects(Type="App::DocumentObjectExtension")

    def test_dynamic_properties(self):
        obj = self.doc.addObject("App::FeaturePython", "Box")
        obj.addProperty("App::PropertyString", "Description", "Base", "")
        self.assertIn("Description", obj.PropertiesList)
        self.assertEqual(obj.getTypeIdOfProperty("Description"), "App::PropertyString")
        self.assertEqual(obj.Description, "")
        obj.Description = "hello"
        self.assertEqual(obj.getPropertyByName("Description"), "hello")

    def test_unknown_attribute_raises(self):
        obj = self.doc.addObject("App::FeaturePython", "Box")
        with self.assertRaises(AttributeError):
            _ = obj.NotAThing
        # Upstream FreeCAD allows arbitrary Python attributes on objects (e.g.
        # `obj.Proxy`); they round-trip through the instance `__dict__`.
        obj.Proxy = {"kind": "feature"}
        self.assertEqual(obj.Proxy, {"kind": "feature"})
        with self.assertRaises(AttributeError):
            _ = obj.StillMissing

    def test_get_and_remove_object(self):
        self.doc.addObject("App::FeaturePython", "Box")
        self.assertIsNotNone(self.doc.getObject("Box"))
        self.assertIsNone(self.doc.getObject("Nope"))
        self.doc.removeObject("Box")
        self.assertEqual(self.doc.CountObjects, 0)
        with self.assertRaises(ValueError):
            self.doc.removeObject("Box")


class TestUndoRedo(unittest.TestCase):
    def setUp(self):
        self.doc = FreeCAD.newDocument("UndoTest")

    def tearDown(self):
        for name in FreeCAD.listDocuments():
            FreeCAD.closeDocument(name)

    def test_initially_empty(self):
        self.assertEqual(self.doc.UndoNames, [])
        self.assertEqual(self.doc.UndoCount, 0)
        self.assertEqual(self.doc.RedoNames, [])
        self.assertEqual(self.doc.RedoCount, 0)

    def test_active_transaction_is_visible(self):
        self.doc.openTransaction("T1")
        a = self.doc.addObject("App::FeatureTest", "A")
        a.Integer = 1
        self.assertEqual(self.doc.UndoNames, ["T1"])
        self.assertEqual(self.doc.UndoCount, 1)

        # A second open adds no entry until it records a change…
        self.doc.openTransaction("T2")
        self.assertEqual(self.doc.UndoNames, ["T1"])
        # …which commits T1 and makes T2 the active transaction.
        a.Integer = 2
        self.assertEqual(self.doc.UndoNames, ["T2", "T1"])

    def test_undo_redo_round_trip(self):
        obj = self.doc.addObject("App::FeatureTest", "A")
        obj.Integer = 1
        self.doc.openTransaction("T1")
        obj.Integer = 2
        self.doc.commitTransaction()

        self.assertTrue(self.doc.undo())
        self.assertEqual(obj.Integer, 1)
        self.assertEqual(self.doc.RedoNames, ["T1"])
        self.assertTrue(self.doc.redo())
        self.assertEqual(obj.Integer, 2)
        self.assertEqual(self.doc.UndoNames, ["T1"])

    def test_new_change_clears_redo_and_abort_leaves_no_entry(self):
        obj = self.doc.addObject("App::FeatureTest", "A")
        obj.Integer = 1
        self.doc.openTransaction("T1")
        obj.Integer = 2
        self.doc.commitTransaction()
        self.doc.undo()
        self.assertEqual(self.doc.RedoCount, 1)

        self.doc.openTransaction("T2")
        obj.Integer = 5
        self.assertEqual(self.doc.RedoNames, [])  # new change drops redo
        self.doc.abortTransaction()
        self.assertEqual(obj.Integer, 1)
        self.assertEqual(self.doc.UndoNames, [])

    def test_undo_add_object_clears_active_object(self):
        self.doc.openTransaction("Add")
        self.doc.addObject("App::FeatureTest", "New")
        self.doc.commitTransaction()
        self.doc.undo()
        self.assertIsNone(self.doc.ActiveObject)
        self.assertIsNone(self.doc.getObject("New"))
        self.doc.clearUndos()
        self.assertEqual(self.doc.UndoNames, [])
        self.assertEqual(self.doc.RedoNames, [])

    def test_undo_remove_object_restores_links(self):
        box = self.doc.addObject("App::FeatureTest", "Box")
        cyl = self.doc.addObject("App::FeatureTest", "Cyl")
        fuse = self.doc.addObject("App::FeatureTest", "Fuse")
        fuse.LinkList = [box, cyl]
        self.assertEqual(box.InList, [fuse])
        self.assertEqual(cyl.InList, [fuse])

        self.doc.openTransaction("Remove")
        self.doc.removeObject("Fuse")
        self.doc.commitTransaction()
        self.assertEqual(box.InList, [])

        self.doc.undo()
        restored = self.doc.getObject("Fuse")
        self.assertIsNotNone(restored)
        self.assertEqual(box.InList, [restored])

    def test_expression_creates_backlink(self):
        a = self.doc.addObject("App::FeatureTest", "A")
        b = self.doc.addObject("App::FeatureTest", "B")
        b.setExpression("Float", "A.Float + 1")
        self.assertEqual(b.InList, [])  # b links to a, not the reverse
        self.assertEqual(a.InList, [b])

    def test_booked_transaction_ids_are_distinct(self):
        other = FreeCAD.newDocument("Other")
        self.doc.openTransaction("t1")
        other.openTransaction("t2")
        self.assertNotEqual(
            self.doc.getBookedTransactionID(), other.getBookedTransactionID()
        )

    def test_undo_mode_and_available_steps(self):
        # Upstream reports UndoMode 1 and accepts (ignores) assignment.
        self.assertEqual(self.doc.UndoMode, 1)
        self.doc.UndoMode = 1

        obj = self.doc.addObject("App::FeatureTest", "A")
        obj.Integer = 0
        self.doc.openTransaction("T1")
        booked = self.doc.getBookedTransactionID()
        self.assertNotEqual(booked, 0)
        obj.Integer = 1
        self.assertEqual(self.doc.getAvailableUndos(), 1)
        self.assertEqual(self.doc.getAvailableUndos(booked), 1)

        self.doc.commitTransaction()
        self.assertEqual(self.doc.getBookedTransactionID(), 0)
        self.assertEqual(self.doc.getAvailableUndos(), 1)

        self.doc.undo()
        self.assertEqual(self.doc.getAvailableUndos(), 0)
        self.assertEqual(self.doc.getAvailableRedos(), 1)
        self.assertEqual(self.doc.getAvailableRedos(booked), 1)
        self.assertEqual(self.doc.getAvailableUndos(999999), 0)
        self.assertEqual(self.doc.getAvailableRedos(999999), 0)

    def test_get_object_by_list_raises(self):
        with self.assertRaises(TypeError):
            self.doc.getObject([1])


if __name__ == "__main__":
    unittest.main()
