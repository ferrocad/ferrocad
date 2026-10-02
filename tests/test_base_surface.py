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


class _RecordingObserver:
    """Minimal document observer recording `(slot, extra)` signals."""

    def __init__(self):
        self.clear()

    def clear(self):
        self.signals = []
        self.args = []

    def _doc(self, tag, doc):
        self.signals.append(tag)
        self.args.append(doc)

    def _obj(self, tag, obj):
        self.signals.append(tag)
        self.args.append(obj)

    def slotCreatedDocument(self, doc):
        self._doc("DocCreated", doc)

    def slotDeletedDocument(self, doc):
        self._doc("DocDeleted", doc)

    def slotRelabelDocument(self, doc):
        self._doc("DocRelabled", doc)

    def slotActivateDocument(self, doc):
        self._doc("DocActivated", doc)

    def slotRecomputedDocument(self, doc):
        self._doc("DocRecomputed", doc)

    def slotUndoDocument(self, doc):
        self._doc("DocUndo", doc)

    def slotRedoDocument(self, doc):
        self._doc("DocRedo", doc)

    def slotBeforeChangeDocument(self, doc, prop):
        self.signals.append(("DocBeforeChange", prop))
        self.args.append(doc)

    def slotChangedDocument(self, doc, prop):
        self.signals.append(("DocChanged", prop))
        self.args.append(doc)

    def slotOpenTransaction(self, doc, name):
        self.signals.append(("DocOpenTransaction", name))
        self.args.append(doc)

    def slotCommitTransaction(self, doc):
        self._doc("DocCommitTransaction", doc)

    def slotAbortTransaction(self, doc):
        self._doc("DocAbortTransaction", doc)

    def slotStartSaveDocument(self, doc, name):
        self.signals.append(("DocStartSave", name))
        self.args.append(doc)

    def slotFinishSaveDocument(self, doc, name):
        self.signals.append(("DocFinishSave", name))
        self.args.append(doc)

    def slotCreatedObject(self, obj):
        self._obj("ObjCreated", obj)

    def slotDeletedObject(self, obj):
        self._obj("ObjDeleted", obj)

    def slotRecomputedObject(self, obj):
        self._obj("ObjRecomputed", obj)

    def slotBeforeChangeObject(self, obj, prop):
        self.signals.append(("ObjBeforeChange", prop))
        self.args.append(obj)

    def slotChangedObject(self, obj, prop):
        self.signals.append(("ObjChanged", prop))
        self.args.append(obj)

    def slotAppendDynamicProperty(self, obj, prop):
        self.signals.append(("ObjAddDynProp", prop))
        self.args.append(obj)

    def slotRemoveDynamicProperty(self, obj, prop):
        self.signals.append(("ObjRemoveDynProp", prop))
        self.args.append(obj)

    def slotChangePropertyEditor(self, obj, prop):
        self.signals.append(("ObjChangePropEdit", prop))
        self.args.append(obj)

    def slotBeforeAddingDynamicExtension(self, obj, ext):
        self.signals.append(("ObjBeforeDynExt", ext))
        self.args.append(obj)

    def slotAddedDynamicExtension(self, obj, ext):
        self.signals.append(("ObjDynExt", ext))
        self.args.append(obj)


class TestDocumentObservers(unittest.TestCase):
    def setUp(self):
        self.obs = _RecordingObserver()
        FreeCAD.addDocumentObserver(self.obs)

    def tearDown(self):
        FreeCAD.removeDocumentObserver(self.obs)

    def test_document_lifecycle(self):
        self.obs.clear()
        doc = FreeCAD.newDocument("Obs")
        self.assertEqual(
            self.obs.signals,
            ["DocCreated", ("DocBeforeChange", "Label"), ("DocChanged", "Label"), "DocRelabled"],
        )
        self.assertIs(self.obs.args[0], doc)

        FreeCAD.setActiveDocument("Obs")  # already active -> no signal
        self.obs.clear()
        FreeCAD.closeDocument("Obs")
        self.assertEqual(self.obs.signals, ["DocDeleted"])
        self.assertIs(self.obs.args[0], doc)

    def test_object_events_and_identity(self):
        doc = FreeCAD.newDocument("ObsObj")
        self.obs.clear()
        obj = doc.addObject("App::DocumentObject", "O")
        self.assertEqual(self.obs.signals, ["ObjCreated"])
        self.assertIs(self.obs.args[0], obj)  # identity, not just equality

        self.obs.clear()
        obj.Label = "renamed"
        self.assertEqual(
            self.obs.signals,
            [("ObjBeforeChange", "Label"), ("ObjChanged", "Label")],
        )
        self.assertIs(self.obs.args[0], obj)

        self.obs.clear()
        doc.removeObject("O")
        self.assertEqual(self.obs.signals, ["ObjDeleted"])
        self.assertIs(self.obs.args[0], obj)
        FreeCAD.closeDocument("ObsObj")

    def test_transactions_and_save(self):
        import os
        import tempfile

        doc = FreeCAD.newDocument("ObsTx")
        self.obs.clear()
        doc.openTransaction("t")
        self.assertEqual(self.obs.signals, [])  # pending until a change
        doc.addObject("App::FeatureTest", "X")
        self.assertEqual(self.obs.signals[0], ("DocOpenTransaction", "t"))

        self.obs.clear()
        doc.commitTransaction()
        self.assertEqual(self.obs.signals, ["DocCommitTransaction"])

        self.obs.clear()
        doc.openTransaction("empty")
        doc.commitTransaction()  # nothing changed -> no signal
        self.assertEqual(self.obs.signals, [])

        self.obs.clear()
        path = os.path.join(tempfile.gettempdir(), "ObsTx.FCStd")
        doc.saveAs(path)
        self.assertEqual([s[0] for s in self.obs.signals], ["DocStartSave", "DocFinishSave"])
        self.assertEqual(self.obs.signals[0][1], doc.FileName)
        FreeCAD.closeDocument("ObsTx")

    def test_recompute_events(self):
        doc = FreeCAD.newDocument("ObsRec")
        obj = doc.addObject("App::FeatureTest", "R")
        self.obs.clear()
        obj.enforceRecompute()
        doc.recompute()
        self.assertEqual(self.obs.signals, ["ObjRecomputed", "DocRecomputed"])
        self.assertIs(self.obs.args[0], obj)
        FreeCAD.closeDocument("ObsRec")

    def test_dynamic_property_and_extension_events(self):
        doc = FreeCAD.newDocument("ObsExt")
        obj = doc.addObject("App::FeaturePython", "P")
        self.obs.clear()
        obj.addProperty("App::PropertyLength", "Prop")
        self.assertEqual(self.obs.signals, [("ObjAddDynProp", "Prop")])
        self.obs.clear()
        obj.setEditorMode("Prop", ["ReadOnly"])
        self.assertEqual(self.obs.signals, [("ObjChangePropEdit", "Prop")])
        self.obs.clear()
        obj.removeProperty("Prop")
        self.assertEqual(self.obs.signals, [("ObjRemoveDynProp", "Prop")])
        self.obs.clear()
        obj.addExtension("App::GroupExtensionPython")
        self.assertEqual(
            self.obs.signals,
            [("ObjBeforeDynExt", "App::GroupExtensionPython"), ("ObjDynExt", "App::GroupExtensionPython")],
        )
        FreeCAD.closeDocument("ObsExt")

    def test_removed_observer_receives_nothing(self):
        FreeCAD.removeDocumentObserver(self.obs)
        self.obs.clear()
        doc = FreeCAD.newDocument("NoObs")
        FreeCAD.closeDocument("NoObs")
        self.assertEqual(self.obs.signals, [])


class TestPersistenceDumpAndRecovery(unittest.TestCase):
    def test_property_object_document_dump_roundtrip(self):
        doc = FreeCAD.newDocument("Dump")
        a = doc.addObject("App::FeatureTest", "A")
        b = doc.addObject("App::FeatureTest", "B")
        c = doc.addObject("App::FeatureTest", "C")
        a.Vector = (1, 2, 3)
        b.restorePropertyContent("Vector", a.dumpPropertyContent("Vector", Compression=9))
        self.assertEqual(a.Vector, b.Vector)

        a.Distance = 12
        a.String = "test"
        c.restoreContent(a.dumpContent())
        self.assertEqual(c.Distance, a.Distance)
        self.assertEqual(c.String, a.String)

        other = FreeCAD.newDocument("DumpRestore")
        other.restoreContent(doc.dumpContent(9))
        self.assertEqual(len(other.Objects), len(doc.Objects))
        self.assertEqual(other.A.Distance, a.Distance)
        self.assertEqual(other.A.Vector, a.Vector)
        FreeCAD.closeDocument("Dump")
        FreeCAD.closeDocument("DumpRestore")

    def test_restore_from_file(self):
        import os
        import tempfile

        doc = FreeCAD.newDocument("RestoreMe")
        doc.addObject("App::FeatureTest", "X")
        doc.saveAs(os.path.join(tempfile.gettempdir(), "RestoreMe.FCStd"))
        doc.addObject("App::FeatureTest", "Y")
        self.assertEqual(len(doc.Objects), 2)
        doc.restore()  # clears current content
        self.assertEqual(len(doc.Objects), 1)
        FreeCAD.closeDocument("RestoreMe")

    def test_recovery_snapshot(self):
        import os
        import xml.etree.ElementTree as ET
        import zipfile

        doc = FreeCAD.newDocument("Recover")
        doc.addObject("App::FeatureTest", "O")
        doc.Label = 'Recovery <Label> & "Name"'
        self.assertTrue(doc.canWriteRecoverySnapshot())
        self.assertTrue(FreeCAD.writeRecoverySnapshotToTransientDir(doc))

        transitive = doc.TransientDir
        meta = os.path.join(transitive, "fc_recovery_file.xml")
        archive = os.path.join(transitive, "fc_recovery_file.fcstd")
        self.assertTrue(os.path.isfile(meta))
        self.assertTrue(os.path.isfile(archive))
        root = ET.parse(meta).getroot()
        self.assertEqual(root.tag, "AutoRecovery")
        self.assertEqual(root.findtext("Label"), doc.Label)
        with zipfile.ZipFile(archive) as recovery:
            self.assertIn("Document.xml", recovery.namelist())

        # Uncompressed variant writes a plain Document.xml.
        self.assertTrue(FreeCAD.writeRecoverySnapshotToTransientDir(doc, compressed=False))
        self.assertTrue(
            os.path.isfile(os.path.join(transitive, "fc_recovery_files", "Document.xml"))
        )

        # Rejected while a transaction is open.
        doc.openTransaction("t")
        self.assertFalse(doc.canWriteRecoverySnapshot())
        with self.assertRaises(RuntimeError):
            FreeCAD.writeRecoverySnapshotToTransientDir(doc)
        doc.abortTransaction()
        FreeCAD.closeDocument("Recover")


class _FeatureProxy:
    """Module-level proxy (importable for restore), FreeCAD dumps/loads protocol."""

    def __init__(self, obj):
        self.Dictionary = {}
        obj.Proxy = self

    def dumps(self):
        return self.Dictionary

    def loads(self, data):
        self.Dictionary = data


class TestPythonObjectPersistence(unittest.TestCase):
    def test_python_object_property_roundtrip(self):
        import os
        import tempfile

        doc = FreeCAD.newDocument("PyObj")
        obj = doc.addObject("App::DocumentObject", "Object")
        obj.addProperty("App::PropertyPythonObject", "Dictionary")
        obj.Dictionary = {"Stored data": [3, 5, 7]}
        self.assertEqual(obj.Dictionary, {"Stored data": [3, 5, 7]})

        path = os.path.join(tempfile.gettempdir(), "PyObj.FCStd")
        doc.saveAs(path)
        FreeCAD.closeDocument("PyObj")
        doc = FreeCAD.open(path)
        self.assertEqual(doc.Object.Dictionary, {"Stored data": [3, 5, 7]})
        FreeCAD.closeDocument("PyObj")

    def test_proxy_persists_via_dumps_loads(self):
        import os
        import tempfile

        doc = FreeCAD.newDocument("ProxyPersist")
        obj = doc.addObject("App::FeaturePython", "Python")
        proxy = _FeatureProxy(obj)
        proxy.Dictionary["Stored data"] = [3, 5, 7]

        path = os.path.join(tempfile.gettempdir(), "ProxyPersist.FCStd")
        doc.saveAs(path)
        FreeCAD.closeDocument("ProxyPersist")
        doc = FreeCAD.open(path)
        self.assertEqual(doc.Python.Proxy.Dictionary, {"Stored data": [3, 5, 7]})
        FreeCAD.closeDocument("ProxyPersist")


if __name__ == "__main__":
    unittest.main()
