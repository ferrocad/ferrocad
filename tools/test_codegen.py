"""Tests for ``tools/codegen.py`` (M3c skeleton codegen).

Hermetic: exercises the generator's mapping/default/signature logic on synthetic
model objects, so it runs without the upstream checkout or a Rust toolchain.

Run::

    python3 tools/test_codegen.py
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(__file__))

import codegen  # noqa: E402
import inventory  # noqa: E402


def _param(name, kind="pos", ann="", default=False, default_value=""):
    return inventory.Param(name, kind, ann, default, default_value)


def _method(name, params, returns="", **flags):
    sig = inventory.Signature(params=params, returns=returns)
    m = inventory.Method(name=name, signatures=[sig])
    for k, v in flags.items():
        setattr(m, k, v)
    return m


class TestSanitizeAndMapping(unittest.TestCase):
    def test_sanitize_rust_keywords(self):
        self.assertEqual(codegen.sanitize("type"), "type_")
        self.assertEqual(codegen.sanitize("match"), "match_")
        self.assertEqual(codegen.sanitize("name"), "name")

    def test_map_param_type(self):
        self.assertEqual(codegen.map_param_type("str"), "&str")
        self.assertEqual(codegen.map_param_type("int"), "i64")
        self.assertEqual(codegen.map_param_type("float"), "f64")
        self.assertEqual(codegen.map_param_type("bool"), "bool")
        self.assertEqual(codegen.map_param_type("Final[str]"), "&str")
        self.assertEqual(codegen.map_param_type("list[DocumentObject]"), "Vec<PyObject>")
        self.assertEqual(codegen.map_param_type("object | None"), "PyObject")

    def test_map_return(self):
        self.assertEqual(codegen.map_return("None"), "()")
        self.assertEqual(codegen.map_return("str"), "String")
        self.assertEqual(codegen.map_return("bool"), "bool")


class TestDefaultLiteral(unittest.TestCase):
    def test_str(self):
        self.assertEqual(codegen.default_literal("str", "'Base'"), '"Base"')
        self.assertEqual(codegen.default_literal("str", "..."), '""')
        self.assertEqual(codegen.default_literal("str", "''"), '""')

    def test_primitives(self):
        self.assertEqual(codegen.default_literal("int", "0"), "0")
        self.assertEqual(codegen.default_literal("int", "..."), "0")
        self.assertEqual(codegen.default_literal("bool", "False"), "false")
        self.assertEqual(codegen.default_literal("bool", "True"), "true")

    def test_unmappable(self):
        self.assertIsNone(codegen.default_literal("object | None", "None"))
        self.assertIsNone(codegen.default_literal("Any", "..."))


class TestRenderSignature(unittest.TestCase):
    def test_with_defaults(self):
        m = _method(
            "getPropertyByName",
            [
                _param("name", "posonly", "str"),
                _param("checkOwner", "posonly", "int", True, "0"),
            ],
        )
        self.assertIn("checkOwner = 0", codegen.render_signature(m))

    def test_unmappable_default_returns_none(self):
        m = _method("f", [_param("obj", "pos", "object | None", True, "None")])
        self.assertIsNone(codegen.render_signature(m))

    def test_kwonly_separator(self):
        m = _method(
            "dump",
            [
                _param("Property", "pos", "str"),
                _param("Compression", "kwonly", "int", True, "3"),
            ],
        )
        self.assertIn("*", codegen.render_signature(m))


class TestRenderClass(unittest.TestCase):
    def test_class_and_attributes(self):
        cls = inventory.Class(
            name="Document",
            attributes=[
                inventory.Attribute("Name", "Final[str]", '""'),
                inventory.Attribute("UndoMode", "int", "0"),
            ],
            methods=[_method("save", [], "None")],
        )
        text = "\n".join(codegen.render_class(cls))
        self.assertIn('#[pyclass(name = "Document", module = "ferrocad_gen")]', text)
        self.assertIn("fn Name(&self) -> String", text)          # Final -> getter only
        self.assertIn("fn UndoMode(&self) -> i64", text)
        self.assertIn("fn set_UndoMode(&mut self, value: i64)", text)  # mutable -> setter
        self.assertIn('todo!("Document.save")', text)


if __name__ == "__main__":
    unittest.main()
