"""Tests for the M4 `Base`/`Units`/`Console`/`ParamGet`/`StringHasher` surface.

Exercises the new facade modules exposed from ``FreeCAD`` (over the Rust
``fc-core``/``fc-python`` backend).

Run: PYTHONPATH=python python3 -m unittest tests.test_base_surface -v
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

import FreeCAD  # noqa: E402


class TestStringHasherSurface(unittest.TestCase):
    def test_roundtrip_and_type_checks(self):
        h = FreeCAD.StringHasher()
        sid = h.getID("A")
        self.assertGreaterEqual(sid.Value, 1)
        self.assertEqual(sid.Data, "A")
        self.assertTrue(sid.isSame(h.getID("A")))

        with self.assertRaises(TypeError):
            FreeCAD.StringHasher(0)
        with self.assertRaises(ValueError):
            h.getID(0)
        with self.assertRaises(TypeError):
            h.isSame(0)
        with self.assertRaises(TypeError):
            sid.isSame(0)


class TestUnitsAndBase(unittest.TestCase):
    def test_quantity_identity(self):
        self.assertIs(FreeCAD.Base.Quantity, FreeCAD.Units.Quantity)

    def test_quantity_parse(self):
        q = FreeCAD.Base.Quantity("10 mm")
        self.assertEqual(q.value_mm(), 10.0)

    def test_quantity_value_and_unit(self):
        q = FreeCAD.Units.Quantity(1, "m")
        self.assertEqual(q.Value, 1000.0)
        self.assertEqual(q.Unit, FreeCAD.Units.Length)

    def test_quantity_compound(self):
        self.assertEqual(FreeCAD.Units.Quantity("10 m").Value, 10000.0)
        self.assertAlmostEqual(FreeCAD.Units.Quantity("3/8 in").Value, 9.525)
        self.assertEqual(FreeCAD.Units.Quantity("2*pi rad").Value, 360.0)

    def test_get_value_as(self):
        psi = FreeCAD.Units.parseQuantity("1psi")
        self.assertAlmostEqual(psi.getValueAs("MPa").Value, 0.0068947572932, places=8)

    def test_to_number(self):
        self.assertEqual(float(FreeCAD.Units.toNumber(1023, "g", 2)), 1000)
        self.assertEqual(float(FreeCAD.Units.toNumber(1023, "f", 2)), 1023)
        self.assertEqual(float(FreeCAD.Units.toNumber(1023, "e", 2)), 1020)

    def test_schemas(self):
        self.assertEqual(FreeCAD.Units.listSchemas(), ("Standard",))
        psi = FreeCAD.Units.parseQuantity("1psi")
        t = FreeCAD.Units.schemaTranslate(psi, 0)
        self.assertAlmostEqual(
            FreeCAD.Units.parseQuantity(t[0]).getValueAs("psi").Value, 1.0, places=8
        )


class TestConsole(unittest.TestCase):
    def test_print_methods_exist(self):
        for name in ("PrintLog", "PrintMessage", "PrintWarning", "PrintError", "PrintStatus"):
            self.assertTrue(callable(getattr(FreeCAD.Console, name)), name)


class TestParamGet(unittest.TestCase):
    def test_get_and_set(self):
        grp = FreeCAD.ParamGet("User parameter:BaseApp/Preferences/Units")
        self.assertEqual(grp.GetInt("Decimals", 2), 2)
        grp.SetInt("Decimals", 4)
        self.assertEqual(grp.GetInt("Decimals"), 4)
        self.assertEqual(grp.GetBool("Flag", True), True)
        self.assertEqual(grp.GetString("Name", "x"), "x")


class TestGeometry(unittest.TestCase):
    def test_vector(self):
        v = FreeCAD.Vector(1, 2, 3)
        self.assertEqual((v.x, v.y, v.z), (1.0, 2.0, 3.0))
        self.assertEqual(len(v), 3)
        self.assertEqual(v[0], 1.0)
        self.assertEqual(v[-1], 3.0)
        self.assertAlmostEqual(v.Length, (1 + 4 + 9) ** 0.5)
        self.assertEqual(v + FreeCAD.Vector(1, 1, 1), FreeCAD.Vector(2, 3, 4))
        self.assertEqual(v * FreeCAD.Vector(1, 0, 0), 1.0)  # dot product
        self.assertEqual((v * 2.0).x, 2.0)

    def test_placement_identity(self):
        p = FreeCAD.Placement()
        self.assertEqual(p.Base, FreeCAD.Vector(0, 0, 0))

    def test_placement_from_tuple(self):
        p = FreeCAD.Placement()
        p.Base = (1, 2, 3)
        self.assertEqual(p.Base, FreeCAD.Vector(1, 2, 3))
        p.Rotation = (0, 0, 1, 0)

    def test_rotation_from_axis_angle(self):
        r = FreeCAD.Rotation(FreeCAD.Vector(0, 0, 1), 1.0)
        self.assertAlmostEqual(r.Angle, 1.0)
        r.Axis = (1, 0, 0)

    def test_typeid(self):
        t = FreeCAD.Base.TypeId.fromName("App::FeatureTest")
        self.assertEqual(t.Name, "App::FeatureTest")
        self.assertIsNone(t.createInstance())

    def test_base_aliases(self):
        self.assertIs(FreeCAD.Vector, FreeCAD.Base.Vector)
        self.assertIs(FreeCAD.Matrix, FreeCAD.Base.Matrix)
        self.assertIs(FreeCAD.Placement, FreeCAD.Base.Placement)

    def test_feature_test_default_properties(self):
        doc = FreeCAD.newDocument("Ft")
        obj = doc.addObject("App::FeatureTest", "F")
        self.assertEqual(obj.Integer, 0)
        self.assertEqual(obj.Float, 0.0)
        self.assertEqual(obj.String, "")
        self.assertIsInstance(obj.Placement, FreeCAD.Placement)
        FreeCAD.closeDocument("Ft")


class TestPersistence(unittest.TestCase):
    def test_save_and_open_roundtrip(self):
        import os
        import tempfile

        doc = FreeCAD.newDocument("SaveTest")
        obj = doc.addObject("App::FeaturePython", "Box")
        obj.Label = "Persisted label"
        obj.addProperty("App::PropertyString", "Description", "Base", "")
        obj.Description = "hello"
        path = os.path.join(tempfile.gettempdir(), "SaveTest.FCStd")
        doc.saveAs(path)
        FreeCAD.closeDocument("SaveTest")

        doc2 = FreeCAD.open(path)
        self.assertEqual(doc2.Name, "SaveTest")
        o = doc2.getObject("Box")
        self.assertEqual(o.Label, "Persisted label")
        self.assertEqual(o.Description, "hello")
        FreeCAD.closeDocument("SaveTest")

    def test_copy_object(self):
        src = FreeCAD.newDocument("Src")
        o = src.addObject("App::FeaturePython", "Box")
        o.Label = "copy me"
        o.addProperty("App::PropertyInteger", "N", "Base", "")
        o.N = 5

        dst = FreeCAD.newDocument("Dst")
        c = dst.copyObject(o)
        self.assertEqual(c.Name, "Box")
        self.assertEqual(c.Label, "copy me")
        self.assertEqual(c.getPropertyByName("N"), 5)

        FreeCAD.closeDocument("Src")
        FreeCAD.closeDocument("Dst")


class TestDocumentMetadata(unittest.TestCase):
    def test_auto_created(self):
        doc = FreeCAD.newDocument("Auto")
        doc.setAutoCreated(True)
        self.assertTrue(doc.isAutoCreated())
        FreeCAD.closeDocument("Auto")

    def test_active_object(self):
        doc = FreeCAD.newDocument("Act")
        doc.addObject("App::FeaturePython", "A")
        doc.addObject("App::FeaturePython", "B")
        self.assertEqual(doc.ActiveObject.Name, "B")
        FreeCAD.closeDocument("Act")

    def test_find_objects(self):
        doc = FreeCAD.newDocument("Find")
        doc.addObject("App::FeaturePython", "A")
        doc.addObject("App::FeatureTest", "B")
        self.assertEqual(len(doc.findObjects(Type="App::FeatureTest")), 1)
        FreeCAD.closeDocument("Find")

    def test_property_type_constants(self):
        self.assertEqual(FreeCAD.PropertyType.Prop_NoPersist, 32)


class TestExtensionsAndGroups(unittest.TestCase):
    def test_add_and_has_extension(self):
        doc = FreeCAD.newDocument("Ext")
        obj = doc.addObject("App::DocumentObject", "Obj")
        self.assertFalse(obj.hasExtension("App::GroupExtension"))
        obj.addExtension("App::GroupExtensionPython")
        self.assertTrue(obj.hasExtension("App::GroupExtension"))
        self.assertTrue(obj.hasExtension("App::GroupExtensionPython"))
        FreeCAD.closeDocument("Ext")

    def test_group_add_object(self):
        doc = FreeCAD.newDocument("Grp")
        obj = doc.addObject("App::FeaturePython", "Child")
        grp = doc.addObject("App::DocumentObjectGroup", "Group")
        grp.addObject(obj)
        self.assertTrue(grp.hasObject(obj))
        self.assertTrue(obj in grp.Group)
        self.assertEqual(grp.getObject("Child").Name, "Child")
        # a group cannot contain itself
        with self.assertRaises(Exception):
            grp.addObject(grp)
        FreeCAD.closeDocument("Grp")

    def test_duplicate_links(self):
        doc = FreeCAD.newDocument("Dup")
        obj = doc.addObject("App::FeaturePython", "obj")
        grp = doc.addObject("App::DocumentObjectGroup", "group")
        grp.Group = [obj, obj]
        doc.removeObject("obj")
        self.assertEqual(grp.Group, [])
        FreeCAD.closeDocument("Dup")

    def test_parent_group(self):
        doc = FreeCAD.newDocument("Parent")
        obj = doc.addObject("App::FeaturePython", "Child")
        grp = doc.addObject("App::DocumentObjectGroup", "Group")
        grp.addObject(obj)
        self.assertEqual(obj.getParentGroup().Name, "Group")
        FreeCAD.closeDocument("Parent")

    def test_extension_group(self):
        doc = FreeCAD.newDocument("ExtGrp")
        obj = doc.addObject("App::DocumentObject", "Obj")
        grp = doc.addObject("App::FeaturePython", "Extension_2")
        grp.addExtension("App::GroupExtensionPython")
        grp.Group = [obj]
        self.assertTrue(obj in grp.Group)
        FreeCAD.closeDocument("ExtGrp")


class TestGuiStub(unittest.TestCase):
    def test_import_freecadgui_console_mode(self):
        import FreeCADGui

        self.assertIsNotNone(FreeCADGui)
        self.assertFalse(hasattr(FreeCADGui, "getDocument"))
        doc = FreeCAD.newDocument("Gui")
        obj = doc.addObject("App::FeatureTest", "HeadlessViewObject")
        self.assertIsNone(obj.ViewObject)
        FreeCAD.closeDocument("Gui")


class TestOriginSubObject(unittest.TestCase):
    def test_get_sub_object_axes_and_planes(self):
        doc = FreeCAD.newDocument("Orig")
        obj = doc.addObject("App::Origin", "Origin")
        doc.recompute()

        def angle(res, v1, v2):
            return res[1].multVec(v1).getAngle(v2)

        self.assertEqual(angle(obj.getSubObject("X_Axis", retType=2), FreeCAD.Vector(1, 0, 0), FreeCAD.Vector(1, 0, 0)), 0.0)
        self.assertEqual(angle(obj.getSubObject("Y_Axis", retType=2), FreeCAD.Vector(1, 0, 0), FreeCAD.Vector(0, 1, 0)), 0.0)
        self.assertEqual(angle(obj.getSubObject("Z_Axis", retType=2), FreeCAD.Vector(1, 0, 0), FreeCAD.Vector(0, 0, 1)), 0.0)
        self.assertEqual(angle(obj.getSubObject("XY_Plane", retType=2), FreeCAD.Vector(0, 0, 1), FreeCAD.Vector(0, 0, 1)), 0.0)
        self.assertEqual(angle(obj.getSubObject("XZ_Plane", retType=2), FreeCAD.Vector(0, 0, 1), FreeCAD.Vector(0, -1, 0)), 0.0)
        self.assertEqual(angle(obj.getSubObject("YZ_Plane", retType=2), FreeCAD.Vector(0, 0, 1), FreeCAD.Vector(1, 0, 0)), 0.0)

        # retType=3 (Placement) and retType=4 (Matrix) both have multVec
        res = obj.getSubObject("YZ_Plane", retType=3)
        self.assertEqual(res.multVec(FreeCAD.Vector(0, 0, 1)).getAngle(FreeCAD.Vector(1, 0, 0)), 0.0)
        res = obj.getSubObject("YZ_Plane", retType=4)
        self.assertEqual(res.multVec(FreeCAD.Vector(0, 0, 1)).getAngle(FreeCAD.Vector(1, 0, 0)), 0.0)

        # sequence of subnames
        r = obj.getSubObject(("XY_Plane", "YZ_Plane"), retType=4)
        self.assertEqual(r[0], obj.getSubObject("XY_Plane", retType=4))
        self.assertEqual(r[1], obj.getSubObject("YZ_Plane", retType=4))

        # a second origin: OutList children resolve back to themselves
        obj2 = doc.addObject("App::Origin", "Origin2")
        doc.recompute()
        self.assertEqual(len(obj2.OutList), 6)
        for i in obj2.OutList:
            self.assertEqual(obj2.getSubObject(i.Name, retType=1).Name, i.Name)
            self.assertEqual(obj2.getSubObject(i.Name + ".", retType=1).Name, i.Name)

        FreeCAD.closeDocument("Orig")


class TestDocumentSettings(unittest.TestCase):
    def test_meta_and_namespaced_settings(self):
        doc = FreeCAD.newDocument("Settings")
        doc.Meta = {"Draft.GridSpacing": "1 m", "Unrelated": "keep"}

        settings = doc.settings("Draft")
        self.assertEqual(settings.keys(), ["GridSpacing"])
        settings.setString("GridSpacing", "0.1 m")
        self.assertEqual(settings.getString("GridSpacing", ""), "0.1 m")
        self.assertEqual(doc.Meta["Draft.GridSpacing"], "0.1 m")
        self.assertEqual(doc.Meta["Unrelated"], "keep")

        settings.setInt("GridMainlines", 10)
        settings.setFloat("GridSize", 12.5)
        settings.setBool("ShowGrid", True)
        self.assertEqual(doc.Meta["Draft.GridMainlines"], "10")
        self.assertEqual(doc.Meta["Draft.GridSize"], "12.5")
        self.assertEqual(doc.Meta["Draft.ShowGrid"], "true")
        FreeCAD.closeDocument("Settings")

    def test_typed_getters_and_validation(self):
        doc = FreeCAD.newDocument("Settings2")
        doc.Meta = {
            "Draft.GridMainlines": "10",
            "Draft.BadInt": "10 lines",
            "Draft.BadBool": "sometimes",
        }
        settings = doc.settings("Draft")
        self.assertEqual(settings.getInt("GridMainlines", 1), 10)
        self.assertEqual(settings.getInt("BadInt", 7), 7)
        self.assertTrue(settings.getBool("BadBool", True))

        for ns in ("", ".Draft", "Draft.", "Draft..Grid", "Draft-Grid"):
            with self.assertRaises(ValueError):
                doc.settings(ns)
        for key in ("", "Grid.Spacing", "Grid-Spacing"):
            with self.assertRaises(ValueError):
                settings.setString(key, "x")
        with self.assertRaises(TypeError):
            settings.getBool("GridMainlines", "false")
        FreeCAD.closeDocument("Settings2")


class TestDocumentTopology(unittest.TestCase):
    def test_get_object_by_id_and_type(self):
        doc = FreeCAD.newDocument("Topo")
        obj = doc.addObject("App::DocumentObject", "MyName")
        self.assertEqual(doc.getObject(obj.Name), obj)
        self.assertEqual(doc.getObject(obj.ID), obj)
        self.assertIsNone(doc.getObject("Unknown"))
        self.assertIsNone(doc.getObject(obj.ID + 1))
        with self.assertRaises(TypeError):
            doc.getObject([1])
        FreeCAD.closeDocument("Topo")

    def test_root_objects_and_topological_order(self):
        doc = FreeCAD.newDocument("Roots")
        a = doc.addObject("App::FeatureTest", "A")
        b = doc.addObject("App::FeatureTest", "B")
        a.Link = b
        self.assertTrue(b not in doc.RootObjects)
        self.assertTrue(a in doc.RootObjects)
        self.assertEqual(len(doc.Objects), len(doc.TopologicalSortedObjects))
        FreeCAD.closeDocument("Roots")


class TestObjectExtras(unittest.TestCase):
    def test_proxy_attribute_roundtrip(self):
        doc = FreeCAD.newDocument("Proxy")
        obj = doc.addObject("App::FeaturePython", "P")
        marker = {"kind": "feature"}
        obj.Proxy = marker
        self.assertIs(obj.Proxy, marker)
        FreeCAD.closeDocument("Proxy")

    def test_link_and_color_list(self):
        doc = FreeCAD.newDocument("Extras")
        o1 = doc.addObject("App::FeatureTest", "o1")
        o2 = doc.addObject("App::FeatureTest", "o2")
        o1.Link = o2
        self.assertEqual(o1.Link, o2)
        o1.LinkList = [o2]
        self.assertEqual(o1.LinkList, [o2])

        o1.ColourList = [(1.0, 0.5, 0.0), (0.0, 0.5, 1.0)]
        self.assertAlmostEqual(o1.ColourList[0][0], 1.0)
        self.assertAlmostEqual(o1.ColourList[0][3], 1.0)  # alpha defaults to 1
        FreeCAD.closeDocument("Extras")

    def test_link_sub_none_roundtrip(self):
        doc = FreeCAD.newDocument("LinkSub")
        obj = doc.addObject("App::FeaturePython", "Reference")
        obj.addProperty("App::PropertyLinkSub", "Axis")
        self.assertIsNone(obj.Axis)
        obj.Axis = (None, "X_Axis")
        self.assertEqual(obj.Axis, (None, ["X_Axis"]))
        obj.Axis = (None, ["Y_Axis"])
        self.assertEqual(obj.Axis, (None, ["Y_Axis"]))
        obj.Axis = None
        self.assertIsNone(obj.Axis)
        obj.Axis = (None, [])
        self.assertIsNone(obj.Axis)
        with self.assertRaises(TypeError):
            obj.Axis = (None, [1])
        with self.assertRaises(TypeError):
            obj.Axis = (1, ["X_Axis"])
        FreeCAD.closeDocument("LinkSub")


class TestModuleSurface(unittest.TestCase):
    def test_list_documents_is_a_dict(self):
        doc = FreeCAD.newDocument("ListDocs")
        docs = FreeCAD.listDocuments()
        self.assertIsInstance(docs, dict)
        self.assertIn("ListDocs", docs)
        self.assertIs(docs["ListDocs"], doc)
        FreeCAD.closeDocument("ListDocs")


if __name__ == "__main__":
    unittest.main()
