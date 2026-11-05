"""Tests for the wasm panic-class audit.

Two kinds of case here, and both matter: the scanner has to flag the calls that panic
on ``wasm32-unknown-unknown``, and it has to stay quiet about everything else — a check
that cannot fail is worthless, and one that always fails is ignored.

The last class runs the audit over the real repository, so a regression in the tree
fails a test instead of reaching a browser.

Run::

    python3 tools/test_wasm_audit.py
"""

import os
import sys
import unittest

TOOLS = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, TOOLS)

import wasm_audit  # noqa: E402


class TestCommentStripping(unittest.TestCase):
    def test_a_whole_line_comment_is_not_source(self):
        self.assertFalse(wasm_audit.strip_comment("    // Instant::now()").strip())

    def test_a_trailing_comment_is_dropped(self):
        self.assertEqual(wasm_audit.strip_comment("x(); // see Instant"), "x(); ")

    def test_a_slash_inside_a_string_survives(self):
        line = 'let url = "http://example";'
        self.assertEqual(wasm_audit.strip_comment(line), line)


class TestFindings(unittest.TestCase):
    def symbols(self, text):
        return [symbol for _, symbol, _ in wasm_audit.scan_source(text)]

    def test_a_std_instant_import_flags_the_file(self):
        text = "use std::time::Instant;\nfn f() -> Instant { Instant::now() }\n"
        self.assertEqual(self.symbols(text), ["std::time"])
        # Reported at the import, which is the line to change.
        self.assertEqual(wasm_audit.scan_source(text)[0][0], 1)

    def test_the_braced_import_flags_the_file(self):
        text = "use std::time::{Duration, Instant};\nfn f() -> Instant { Instant::now() }\n"
        self.assertEqual(self.symbols(text), ["std::time"])

    def test_the_web_clock_is_not_a_finding(self):
        text = "use web_time::Instant;\nfn f() -> Instant { Instant::now() }\n"
        self.assertEqual(wasm_audit.scan_source(text), [])

    def test_a_qualified_std_clock_is_a_finding(self):
        text = "fn f() { let t = std::time::SystemTime::now(); }"
        self.assertEqual(self.symbols(text), ["std::time"])

    def test_threads_are_findings(self):
        for call in (
            "fn f() { std::thread::sleep(d); }",
            "fn f() { thread::spawn(g); }",
            "fn f() { std::thread::Builder::new(); }",
        ):
            with self.subTest(call=call):
                self.assertEqual(self.symbols(call), ["std::thread"])

    def test_duration_is_not_a_finding(self):
        self.assertEqual(wasm_audit.scan_source("use std::time::Duration;\n"), [])

    def test_naming_an_api_in_prose_is_not_a_finding(self):
        text = "//! The old code used `std::time::Instant::now()` and panicked.\n"
        self.assertEqual(wasm_audit.scan_source(text), [])


class TestTestCode(unittest.TestCase):
    def test_a_test_module_ends_the_scan(self):
        text = (
            "use std::time::Duration;\n"
            "#[cfg(test)]\n"
            "mod tests {\n"
            "    fn t() { std::thread::sleep(d); }\n"
            "}\n"
        )
        self.assertEqual(wasm_audit.scan_source(text), [])

    def test_a_cfg_test_helper_does_not_end_the_scan(self):
        # Only a `mod` after the attribute is a test module; a `#[cfg(test)]` static or
        # fn mid-file must not hide the code after it.
        text = "#[cfg(test)]\nstatic LOCK: u8 = 0;\nuse std::time::Instant;\n"
        self.assertEqual([s for _, s, _ in wasm_audit.scan_source(text)], ["std::time"])

    def test_the_opt_out_marker_skips_the_file(self):
        text = (
            "// wasm-audit: native-only - declared behind cfg(not(wasm))\n"
            "use std::time::Instant;\n"
        )
        self.assertEqual(wasm_audit.scan_source(text), [])

    def test_a_tests_directory_is_out_of_scope(self):
        self.assertEqual(wasm_audit.SKIP_COMPONENTS, {"tests", "examples", "benches"})


class TestTheRepository(unittest.TestCase):
    """The audit over the real tree: the gate CI runs, as a test."""

    @classmethod
    def setUpClass(cls):
        cls.members, cls.findings, cls.error = wasm_audit.audit(wasm_audit.ENTRIES)

    def test_the_graph_is_readable(self):
        self.assertEqual(self.error, "")
        self.assertGreater(len(self.members), 10, "the closure looks too small to be real")

    def test_native_hosts_stay_out_of_scope(self):
        names = {package["name"] for package in self.members}
        # The desktop host embeds CPython and never compiles to wasm; if it appears,
        # the closure is following dev-dependencies again.
        self.assertNotIn("ferrocad_shell_python", names)
        self.assertIn("ferrocad_web_gpui", names)

    def test_the_tree_has_no_wasm_panicking_calls(self):
        self.assertEqual(
            [(str(path), line, symbol) for path, line, symbol, _ in self.findings],
            [],
            "use `web_time` (a `std` alias on native), or mark a native-only file with "
            "`// wasm-audit: native-only - <why>`",
        )


if __name__ == "__main__":
    unittest.main()
