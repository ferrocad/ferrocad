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


if __name__ == "__main__":
    unittest.main()
