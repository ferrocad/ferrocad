"""The MVP acceptance demo as a test (``docs/mvp-path.md``, slice D1).

Runs ``examples/mvp_workflow.py`` end to end so the MVP definition cannot regress
silently: typed/dynamic/enumeration properties, an expression dependency,
recompute, grouping, a transaction with undo, and save/reload.

Run: ``PYTHONPATH=python python3 tests/test_mvp_workflow.py``
"""

import os
import runpy
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))


class MvpWorkflowTest(unittest.TestCase):
    def test_mvp_workflow_runs(self):
        example = os.path.join(
            os.path.dirname(__file__), "..", "examples", "mvp_workflow.py"
        )
        # run_name != "__main__" so importing the example does not execute it.
        namespace = runpy.run_path(example, run_name="__mvp_demo__")
        self.assertEqual(namespace["main"](), 0)


if __name__ == "__main__":
    unittest.main()
