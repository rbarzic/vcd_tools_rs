"""Characterize the current pure-Python console wrapper.

These tests deliberately load vcd_tools/_cli.py without importing the compiled
extension. A fake ``vcd_tools`` module supplies deterministic API results.
"""

from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
import importlib.util
from pathlib import Path
import sys
import types
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
CLI_PATH = ROOT / "vcd_tools" / "_cli.py"


def load_cli():
    spec = importlib.util.spec_from_file_location("vcd_tools_test_cli", CLI_PATH)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class PythonConsoleCompatibilityTests(unittest.TestCase):
    def setUp(self):
        self.cli = load_cli()
        self.fake_api = types.ModuleType("vcd_tools")
        self.fake_api.metadata = lambda _path: {
            "signal_count": 2,
            "timescale": "1 ns",
            "start_time": 0,
            "end_time": 10,
        }

    def test_pretty_before_subcommand_is_accepted(self):
        output = StringIO()
        with patch.dict(sys.modules, {"vcd_tools": self.fake_api}), patch.object(
            sys, "argv", ["vcd_tools_rs", "--pretty", "meta", "fake.vcd"]
        ), redirect_stdout(output):
            self.cli.main()

        self.assertTrue(output.getvalue().startswith("Field      | Value\n"))

    def test_pretty_after_subcommand_is_currently_rejected(self):
        error = StringIO()
        with patch.dict(sys.modules, {"vcd_tools": self.fake_api}), patch.object(
            sys, "argv", ["vcd_tools_rs", "meta", "fake.vcd", "--pretty"]
        ), redirect_stderr(error):
            with self.assertRaises(SystemExit) as raised:
                self.cli.main()

        self.assertEqual(raised.exception.code, 2)
        self.assertIn("unrecognized arguments: --pretty", error.getvalue())

    def test_extract_does_not_carry_values_to_later_aligned_rows(self):
        self.fake_api.extract = lambda _path, **_kwargs: [
            {"signal": "top.a", "time": 5, "value": "1"},
            {"signal": "top.b", "time": 5, "value": "1"},
            {"signal": "top.a", "time": 10, "value": "0"},
        ]
        args = types.SimpleNamespace(
            vcd="fake.vcd",
            signal=["top.a", "top.b"],
            signals_file=None,
            start=None,
            end=None,
            pretty=False,
        )
        output = StringIO()
        with patch.dict(sys.modules, {"vcd_tools": self.fake_api}), redirect_stdout(output):
            self.cli.cmd_extract(args)

        self.assertEqual(
            output.getvalue(),
            "time\ttop.a\ttop.b\n5\t1\t1\n10\t0\t\n",
        )


if __name__ == "__main__":
    unittest.main()
